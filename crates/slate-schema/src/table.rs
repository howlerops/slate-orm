//! Table, column and index definitions.
//!
//! Schemas are values, built once and then read-only. The derive macro emits
//! calls to the same builders a hand-written schema uses, so there is one
//! definition path and one set of validation rules.

use crate::constraint::{CheckDef, ForeignKeyBuilder, ForeignKeyDef, Predicate};
use crate::error::{Result, SchemaError};
use crate::row::Row;
use slate_tuple::{Direction, Value, ValueType};
use std::sync::Arc;

/// Identifies a table within a catalog. Also the key prefix for its rows.
///
/// Ids are part of the on-disk format: changing one relocates every row, so
/// treat an assigned id as permanent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TableId(pub u32);

/// Identifies a secondary index within a catalog. Also its key prefix.
///
/// Index ids share a namespace with nothing else, but like [`TableId`] they are
/// part of the on-disk format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IndexId(pub u32);

/// Position of a column within its table. Also its position in the row body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ordinal(pub usize);

/// A column whose value the store writes, not the caller.
///
/// # Why this is not a `DEFAULT`
///
/// A `DEFAULT` is a stored [`Value`], and the value wanted here is "whatever
/// the clock says at the moment of the write". There is no `Value` that means
/// that. Widening `DEFAULT` to hold an expression was the obvious alternative
/// and was rejected: a default that can call a function is a default that has
/// to be *evaluated*, which means a second expression language in the schema
/// layer, evaluated on a path that currently does no evaluation at all, to
/// express two cases.
///
/// # Why the caller's value is overwritten rather than honoured
///
/// Honouring a supplied value — filling in only where the caller left null —
/// is what makes an import of historical rows possible, and it is the wrong
/// default. The whole promise of the column is that it says when the row was
/// written; a client that can set it can break that promise silently, and
/// nothing downstream can tell a real timestamp from a claimed one. Anybody
/// importing rows with their original times declares the column *unmanaged*
/// and writes them, which is one word in a schema against a hazard on every
/// write of every managed column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Managed {
    /// Set when the row is first written, and preserved by every later write.
    ///
    /// Preserved rather than left alone: an update carries a full row, so
    /// "leave it alone" would mean taking the caller's copy, which is a value
    /// they could have edited.
    CreatedAt,
    /// Set when the row is written, and on every write after that.
    UpdatedAt,
}

/// A single column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    name: String,
    ty: ValueType,
    nullable: bool,
    added_in: u32,
    dropped_in: Option<u32>,
    default: Option<Value>,
    previous_names: Vec<String>,
    /// Digits after the decimal point, for a [`ValueType::Decimal`] column.
    ///
    /// Zero for every other type, and meaningless there. See
    /// [`ColumnDef::scale`].
    scale: u8,
    /// What a [`ValueType::Array`] column's elements are.
    ///
    /// `None` for every other type, and meaningless there — the same
    /// arrangement as `scale`, and for the same reason. See
    /// [`ColumnDef::element_type`].
    element: Option<ValueType>,
    /// Whether the store writes this column's value. See [`Managed`].
    managed: Option<Managed>,
}

impl ColumnDef {
    /// The column's name, unique within its table.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The column's type.
    #[must_use]
    pub const fn value_type(&self) -> ValueType {
        self.ty
    }

    /// Whether the store writes this column, and when. See [`Managed`].
    ///
    /// Deliberately **not** part of the schema fingerprint, and the reason is
    /// the rule the fingerprint already follows rather than an exception to
    /// it: a client that disagrees about this still reaches the right column,
    /// and the disagreement is *visible* — it reads back a value it did not
    /// write, on the very first row. That is the test a `CHECK` and a
    /// `DEFAULT` pass and a decimal's scale fails, which is why the scale is
    /// hashed and these three are not.
    #[must_use]
    pub const fn managed(&self) -> Option<Managed> {
        self.managed
    }

    /// Digits after the decimal point, for a decimal column.
    ///
    /// A [`Value::Decimal`] stores a count of the column's smallest unit, and
    /// this is what turns that count back into a number: units `1250` at scale
    /// 2 is `12.50`. It lives here rather than in the value because every row
    /// of a column shares it — which is what lets the encoding be the integer
    /// encoding and `SUM` be exact.
    ///
    /// `None` for a column that is not a decimal, so a caller cannot read a
    /// scale off a type that does not have one.
    #[must_use]
    pub const fn scale(&self) -> Option<u8> {
        match self.ty {
            ValueType::Decimal => Some(self.scale),
            _ => None,
        }
    }

    /// What an array column's elements are.
    ///
    /// An array's element type lives here rather than in
    /// [`ValueType::Array`] for the same reason a decimal's scale does: the
    /// enum is fieldless, `Copy` and matchable in a `const fn`, and an
    /// `Array(Box<ValueType>)` would cost all three to describe one column.
    /// `docs/arrays.md` §1 works the trade through.
    ///
    /// `None` for a column that is not an array, so a caller cannot read an
    /// element type off a type that does not have one. An array column
    /// *without* one does not exist: [`TableBuilder::build`] refuses it.
    #[must_use]
    pub const fn element_type(&self) -> Option<ValueType> {
        match self.ty {
            ValueType::Array => self.element,
            _ => None,
        }
    }

    /// Whether the column accepts nulls.
    #[must_use]
    pub const fn is_nullable(&self) -> bool {
        self.nullable
    }

    /// The schema version this column first appeared in.
    ///
    /// Rows written before this version have no bytes for the column, so it
    /// reads back as its [default](ColumnDef::default_value) — or as null, if
    /// it has none, which is why such a column must be nullable or defaulted.
    #[must_use]
    pub const fn added_in(&self) -> u32 {
        self.added_in
    }

    /// The schema version this column was dropped in, if it has been.
    ///
    /// A dropped column keeps its ordinal for ever. Ordinals are the row body's
    /// field order and are compiled into callers' column constants, so closing
    /// the gap would silently renumber every column after it — reading one
    /// row's `age` out of another column's bytes. The column stays, holds
    /// nothing, and is not written.
    ///
    /// Rows written *before* this version still carry the column's bytes, which
    /// is why the decoder needs the version and not just the flag.
    #[must_use]
    pub const fn dropped_in(&self) -> Option<u32> {
        self.dropped_in
    }

    /// Whether the column has been dropped.
    ///
    /// The builder refuses a drop scheduled after the table's current schema
    /// version, so a column with a drop version is a column already gone.
    #[must_use]
    pub const fn is_dropped(&self) -> bool {
        self.dropped_in.is_some()
    }

    /// The value stored when the column is not supplied.
    ///
    /// Used in two places, which is most of why it earns its keep. A
    /// [`PartialRow`](crate::PartialRow) fills it in for a column the caller
    /// left unset, and the decoder fills it in for a row written before the
    /// column existed — so a column added later can be `NOT NULL`, which
    /// without a default it cannot.
    #[must_use]
    pub const fn default_value(&self) -> Option<&Value> {
        self.default.as_ref()
    }

    /// Names this column used to have, oldest first.
    ///
    /// A rename touches no stored byte — names live only in the schema — so it
    /// is recorded rather than migrated. Keeping the old names resolvable is
    /// the point: code and queries written against the old name keep working,
    /// and the rename is visible in the schema instead of being an unexplained
    /// edit to a string.
    #[must_use]
    pub fn previous_names(&self) -> &[String] {
        &self.previous_names
    }

    /// Whether `name` refers to this column, now or before a rename.
    #[must_use]
    pub fn answers_to(&self, name: &str) -> bool {
        self.name == name || self.previous_names.iter().any(|n| n == name)
    }
}

/// One column of an index, with its sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexColumn {
    /// Position of the column in the table.
    pub ordinal: Ordinal,
    /// Direction this column is stored in.
    pub direction: Direction,
}

/// Something that can compute a value from a row.
///
/// The counterpart of [`Predicate`] for an index *key*: `Predicate` says
/// whether an index holds a row, this says what it holds it under. Same seam
/// and the same reason — the expression language is `slate_kernel::Scalar`, and
/// the kernel depends on this crate rather than the other way round, so an
/// [`IndexDef`] cannot name it. The kernel implements this for `Scalar`; there
/// is still one expression language and one evaluator.
pub trait Computed: Send + Sync + 'static {
    /// The value the index keys on for `row`.
    ///
    /// Total, and null where the computation cannot be done — `lower()` of a
    /// number, arithmetic on a null. That is the answer the query evaluator
    /// gives for the same expression, and it has to be: an index whose entries
    /// disagreed with what a query computes would return rows no other access
    /// path returns.
    fn value(&self, row: &Row) -> Value;

    /// The expression itself, for a caller that needs to read it rather than
    /// run it — the planner, matching it against what a query computes. `None`
    /// by default; see [`Predicate::as_any`], which is here for the same
    /// reason and pays the same price.
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        None
    }
}

/// An index key computed from the row rather than read out of it.
///
/// `lower(email)`, `length(url)`: the only way to answer a query about such a
/// value without computing it for every row of a scan.
#[derive(Clone)]
pub struct IndexExpression {
    compute: Arc<dyn Computed>,
    produces: ValueType,
    direction: Direction,
}

impl core::fmt::Debug for IndexExpression {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IndexExpression")
            .field("produces", &self.produces)
            .field("direction", &self.direction)
            .finish_non_exhaustive()
    }
}

impl IndexExpression {
    /// What the expression computes for `row`.
    #[must_use]
    pub fn value(&self, row: &Row) -> Value {
        self.compute.value(row)
    }

    /// The expression itself, for a caller that can read it.
    #[must_use]
    pub fn as_any(&self) -> Option<&dyn core::any::Any> {
        self.compute.as_any()
    }

    /// The type the expression produces, as declared.
    ///
    /// Declared rather than inferred, because nothing here can run the
    /// expression without a row and the decoder needs the type before it has
    /// one. The write path checks each computed value against it rather than
    /// trusting the declaration, so a wrong one is refused at the write that
    /// would have made the entry undecodable, not at the read that finds it.
    #[must_use]
    pub const fn produces(&self) -> ValueType {
        self.produces
    }

    /// The direction the value is stored in.
    #[must_use]
    pub const fn direction(&self) -> Direction {
        self.direction
    }
}

/// A secondary index.
///
/// Index entries live in their own key prefix and are written in the same
/// transaction as the row they describe, so an index can never lag the table.
///
/// # Partial indexes
///
/// An index built with [`IndexBuilder::only_where`] holds an entry only for the
/// rows its predicate admits. That is the one index property the *reader* has
/// to be told about rather than being free to ignore: every other property
/// changes how rows are reached, and this one changes which rows are there to
/// reach. The record store maintains it here — no entry for a row the predicate
/// rejects, and the entry deleted when an update stops matching — and the
/// planner refuses to read it unless it can prove the query lands inside.
#[derive(Clone)]
pub struct IndexDef {
    id: IndexId,
    name: String,
    columns: Vec<IndexColumn>,
    unique: bool,
    /// Rows the index holds. `None` is every row.
    predicate: Option<Arc<dyn Predicate>>,
    /// The computed key, when the index keys on a value the row does not hold.
    /// Mutually exclusive with `columns`, which is then empty.
    expression: Option<IndexExpression>,
    /// Whether this is an inverted index: one entry per *term* of its one
    /// string column, rather than one entry per row.
    ///
    /// A flag rather than a third key shape, because the key *type* is the
    /// column's own — a `Str` — and only the cardinality differs. Everything
    /// that reads an entry back (`decode_index_entry`, `index_key_types`) is
    /// therefore unchanged; what changes is how many entries a row writes,
    /// which is [`IndexDef::key_sets`].
    text: bool,
}

impl core::fmt::Debug for IndexDef {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IndexDef")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("columns", &self.columns)
            .field("unique", &self.unique)
            .field("partial", &self.predicate.is_some())
            .field("expression", &self.expression)
            .field("text", &self.text)
            .finish()
    }
}

/// Two indexes are the same index when everything but the predicate matches,
/// and both are partial or neither is.
///
/// A predicate is a Rust value behind a trait object and does not compare, the
/// same limitation [`CheckDef`] has. Two indexes on one table cannot share a
/// name, so this is a usable identity rather than a convenient fiction — but a
/// schema whose partial index changed *predicate* and nothing else compares
/// equal to the one it replaced, and a migration has to say so itself.
impl PartialEq for IndexDef {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.columns == other.columns
            && self.unique == other.unique
            && self.text == other.text
            && self.predicate.is_some() == other.predicate.is_some()
            && self.expression.as_ref().map(IndexExpression::produces)
                == other.expression.as_ref().map(IndexExpression::produces)
            && self.expression.as_ref().map(IndexExpression::direction)
                == other.expression.as_ref().map(IndexExpression::direction)
    }
}

impl Eq for IndexDef {}

impl IndexDef {
    /// Start defining an index.
    #[must_use]
    pub fn builder(name: impl Into<String>, id: IndexId) -> IndexBuilder {
        IndexBuilder {
            id,
            name: name.into(),
            columns: Vec::new(),
            unique: false,
            predicate: None,
            expression: None,
            text: false,
        }
    }

    /// The predicate restricting which rows the index holds, if it is partial.
    #[must_use]
    pub fn predicate(&self) -> Option<&dyn Predicate> {
        self.predicate.as_deref()
    }

    /// Whether the index holds an entry for `row`.
    ///
    /// `true` for every row of an ordinary index. For a partial one the rule is
    /// a `WHERE`'s and not a `CHECK`'s: **unknown does not admit.** A row whose
    /// `deleted_at` is null is not in an index on `WHERE deleted_at > 0`,
    /// because a scan of that index stands in for a scan filtered by the same
    /// predicate, and that filter would have withheld the row. Getting this
    /// backwards would put rows in the index that reading it must not return.
    #[must_use]
    pub fn admits(&self, row: &Row) -> bool {
        self.predicate
            .as_ref()
            .is_none_or(|predicate| predicate.truth(row) == Some(true))
    }

    /// The computed key, for an expression index.
    #[must_use]
    pub const fn expression(&self) -> Option<&IndexExpression> {
        self.expression.as_ref()
    }

    /// The values this index keys `row` under, in key order.
    ///
    /// The single place that knows whether an index reads its key out of the
    /// row or computes it. Everything that builds or decodes an entry goes
    /// through here and through [`IndexDef::key_directions`], so an expression
    /// index is not a case each of them has to remember.
    #[must_use]
    pub fn key_values(&self, row: &Row) -> Vec<Value> {
        match &self.expression {
            Some(expression) => vec![expression.value(row)],
            None => row.index_values(self),
        }
    }

    /// Whether this is an inverted index: one entry per term, not per row.
    #[must_use]
    pub const fn is_text(&self) -> bool {
        self.text
    }

    /// The key values of every entry this index holds for `row`.
    ///
    /// **One list for an ordinary index and one per term for a text one**, and
    /// this is the only place that difference lives. Every caller that used to
    /// build a single entry now iterates this, which is the whole of the
    /// cardinality change: `entry_for` in the record store became
    /// `entries_for`, and the write path compares two sets of keys where it
    /// used to compare two keys.
    ///
    /// A text index over a value that is not a string — a null, or a column
    /// whose type the builder somehow let through — holds *no* entry, the same
    /// way a partial index holds none for a row its predicate rejects. A row
    /// with no text is a row no term can find, which is the answer a search
    /// wants; writing an entry for the empty term would put every such row
    /// under one key and make it a hot spot for nothing.
    #[must_use]
    pub fn key_sets(&self, row: &Row) -> Vec<Vec<Value>> {
        if !self.text {
            return vec![self.key_values(row)];
        }
        let Some(IndexColumn { ordinal, .. }) = self.columns.first() else {
            return Vec::new();
        };
        let Some(Value::Str(text)) = row.get(*ordinal) else {
            return Vec::new();
        };
        crate::text::tokenize(text)
            .into_iter()
            .map(|term| vec![Value::Str(term)])
            .collect()
    }

    /// The sort direction of each key term, in key order.
    #[must_use]
    pub fn key_directions(&self) -> Vec<Direction> {
        match &self.expression {
            Some(expression) => vec![expression.direction()],
            None => self.columns.iter().map(|c| c.direction).collect(),
        }
    }

    /// The index's id, which is also its key prefix.
    #[must_use]
    pub const fn id(&self) -> IndexId {
        self.id
    }

    /// The index's name, unique within its table.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The indexed columns, in key order.
    #[must_use]
    pub fn columns(&self) -> &[IndexColumn] {
        &self.columns
    }

    /// Whether the indexed columns must be unique across the table.
    #[must_use]
    pub const fn is_unique(&self) -> bool {
        self.unique
    }

    /// The sort direction of each indexed column, in key order.
    ///
    /// Empty for an expression index, which keys on no column; use
    /// [`IndexDef::key_directions`] for the directions of the key itself.
    #[must_use]
    pub fn directions(&self) -> Vec<Direction> {
        self.columns.iter().map(|c| c.direction).collect()
    }
}

/// Builder for [`IndexDef`]. Column names are resolved when the table is built.
#[derive(Clone)]
pub struct IndexBuilder {
    id: IndexId,
    name: String,
    columns: Vec<(String, Direction)>,
    unique: bool,
    predicate: Option<Arc<dyn Predicate>>,
    expression: Option<IndexExpression>,
    text: bool,
}

impl core::fmt::Debug for IndexBuilder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IndexBuilder")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("columns", &self.columns)
            .field("unique", &self.unique)
            .field("partial", &self.predicate.is_some())
            .field("expression", &self.expression)
            .field("text", &self.text)
            .finish()
    }
}

impl IndexBuilder {
    /// Append an ascending column.
    #[must_use]
    pub fn column(self, name: impl Into<String>) -> Self {
        self.column_with(name, Direction::Asc)
    }

    /// Append a column with an explicit direction.
    #[must_use]
    pub fn column_with(mut self, name: impl Into<String>, direction: Direction) -> Self {
        self.columns.push((name.into(), direction));
        self
    }

    /// Mark the index unique.
    ///
    /// On a partial index this is uniqueness *among the rows it holds*, which
    /// falls out of holding no entry for the others rather than being a second
    /// rule: `only_where(active).unique()` is one active row per value and any
    /// number of inactive ones. That is what a partial unique index is for.
    #[must_use]
    pub const fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Hold an entry only for the rows `predicate` admits.
    ///
    /// The predicate is evaluated against the whole row, so it may name columns
    /// the index does not key on — `WHERE deleted_at IS NULL` on an index over
    /// `author` is the common case, and the one worth having: a table that is
    /// mostly deleted rows gets an index that is not.
    ///
    /// Named for what it does to the *index* rather than to a query. It is not
    /// a filter the reader gets for free: the planner uses a partial index only
    /// for queries it can prove land inside the predicate, and falls back to a
    /// scan otherwise.
    ///
    /// # Naming columns in the predicate
    ///
    /// By [`Ordinal`], and the ordinals are positions in the table being built.
    /// A helper that resolves a name by building the table — the obvious thing
    /// to reach for, and what a test here did — recurses forever, because
    /// building the table now evaluates this argument. Write the position, or
    /// resolve names against a table built without the index.
    #[must_use]
    pub fn only_where<P: Predicate>(mut self, predicate: P) -> Self {
        self.predicate = Some(Arc::new(predicate));
        self
    }

    /// Key on a value computed from the row rather than on a column.
    ///
    /// `produces` is the type the expression yields, declared because nothing
    /// here can run it without a row and the decoder needs the type before it
    /// has one. The write path checks every computed value against it, so a
    /// wrong declaration is refused at the write rather than found at the read.
    ///
    /// An expression index keys on exactly this one value: mixing it with
    /// [`IndexBuilder::column`] is refused when the table is built, rather than
    /// one of them silently winning.
    #[must_use]
    pub fn expression<C: Computed>(self, compute: C, produces: ValueType) -> Self {
        self.expression_with(compute, produces, Direction::Asc)
    }

    /// Hold one entry per *term* of the column, rather than one per row.
    ///
    /// An inverted index, which is what makes `contains` a lookup rather than
    /// a scan. The column is still named with [`IndexBuilder::column`] — the
    /// key type is the column's own `Str` — and exactly one is allowed:
    /// two columns would need a cross product of their terms, which is a
    /// different structure and a much larger one.
    ///
    /// Refused when the table is built, rather than half-working: on a column
    /// that is not a string, beside a second column, beside an expression, or
    /// with [`IndexBuilder::unique`]. That last one is worth naming: a term
    /// appears in many rows by construction, so a unique inverted index is a
    /// constraint no realistic text can satisfy, and accepting it would turn
    /// the second row containing "the" into a write failure nobody could read.
    ///
    /// [`IndexBuilder::only_where`] composes with it and is free: a partial
    /// text index holds terms for the rows its predicate admits.
    #[must_use]
    pub const fn text(mut self) -> Self {
        self.text = true;
        self
    }

    /// [`IndexBuilder::expression`] with an explicit direction.
    #[must_use]
    pub fn expression_with<C: Computed>(
        mut self,
        compute: C,
        produces: ValueType,
        direction: Direction,
    ) -> Self {
        self.expression = Some(IndexExpression {
            compute: Arc::new(compute),
            produces,
            direction,
        });
        self
    }
}

/// A table: its columns, primary key, indexes and tenant scoping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDef {
    id: TableId,
    name: String,
    columns: Vec<ColumnDef>,
    primary_key: Vec<Ordinal>,
    /// Columns the row body has ever carried, precomputed.
    ///
    /// Decoding a row walks this list, so building it per decode meant an
    /// allocation on every row of every scan.
    body_columns: Vec<Ordinal>,
    /// Columns the row body carries *now*: `body_columns` less the dropped
    /// ones. Precomputed for the same reason, and separate because a row
    /// written before a drop still carries the column the encoder now omits.
    stored_columns: Vec<Ordinal>,
    indexes: Vec<IndexDef>,
    checks: Vec<CheckDef>,
    foreign_keys: Vec<ForeignKeyDef>,
    tenant_column: Option<Ordinal>,
    /// The column a soft delete stamps, if this table soft-deletes.
    ///
    /// Its presence changes two things: `delete` stamps this column instead of
    /// removing the row, and every read conjoins `<column> IS NULL`. See
    /// [`TableBuilder::soft_delete`].
    soft_delete: Option<Ordinal>,
    schema_version: u32,
}

impl TableDef {
    /// Start defining a table.
    #[must_use]
    pub fn builder(name: impl Into<String>, id: TableId) -> TableBuilder {
        TableBuilder {
            id,
            name: name.into(),
            columns: Vec::new(),
            primary_key: Vec::new(),
            indexes: Vec::new(),
            checks: Vec::new(),
            foreign_keys: Vec::new(),
            tenant_column: None,
            soft_delete: None,
            schema_version: 0,
            changes: Vec::new(),
        }
    }

    /// The table's id, which is also its row key prefix.
    #[must_use]
    pub const fn id(&self) -> TableId {
        self.id
    }

    /// The table's name, unique within its catalog.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Every column, in ordinal order.
    #[must_use]
    pub fn columns(&self) -> &[ColumnDef] {
        &self.columns
    }

    /// The column at `ordinal`, if it exists.
    #[must_use]
    pub fn column(&self, ordinal: Ordinal) -> Option<&ColumnDef> {
        self.columns.get(ordinal.0)
    }

    /// Resolve a column name to its ordinal.
    ///
    /// Current names win over names retired by a rename, so reusing a freed
    /// name for a new column resolves to the new column. The builder refuses
    /// the schema where that would be ambiguous, so this cannot quietly pick
    /// one of two candidates.
    #[must_use]
    pub fn ordinal_of(&self, name: &str) -> Option<Ordinal> {
        self.columns
            .iter()
            .position(|c| c.name == name)
            .or_else(|| self.columns.iter().position(|c| c.answers_to(name)))
            .map(Ordinal)
    }

    /// The primary key columns, in key order.
    #[must_use]
    pub fn primary_key(&self) -> &[Ordinal] {
        &self.primary_key
    }

    /// Whether `ordinal` is part of the primary key.
    #[must_use]
    pub fn is_primary_key_column(&self, ordinal: Ordinal) -> bool {
        self.primary_key.contains(&ordinal)
    }

    /// The types of the primary key columns, for decoding a row key.
    #[must_use]
    pub fn primary_key_types(&self) -> Vec<ValueType> {
        self.key_types(&self.primary_key)
    }

    /// The types of an index's columns, for decoding an index key.
    #[must_use]
    pub fn index_key_types(&self, index: &IndexDef) -> Vec<ValueType> {
        match index.expression() {
            Some(expression) => vec![expression.produces()],
            None => self.key_types(&index.columns.iter().map(|c| c.ordinal).collect::<Vec<_>>()),
        }
    }

    fn key_types(&self, ordinals: &[Ordinal]) -> Vec<ValueType> {
        ordinals
            .iter()
            .map(|o| {
                self.columns
                    .get(o.0)
                    // Ordinals are validated at build time, so this is
                    // unreachable; the fallback keeps the accessor total.
                    .map_or(ValueType::Bytes, ColumnDef::value_type)
            })
            .collect()
    }

    /// Columns the row body has ever carried, in ordinal order.
    ///
    /// Primary key columns are omitted: they are already in the key, and
    /// reconstructing them from it costs less than storing them twice. Dropped
    /// columns are *not* omitted, because a row written before the drop still
    /// carries their bytes and a decoder that skipped the list entry would read
    /// the following column out of them.
    ///
    /// This is the decoder's walk. [`TableDef::stored_columns`] is the
    /// encoder's.
    #[must_use]
    pub fn body_columns(&self) -> &[Ordinal] {
        &self.body_columns
    }

    /// Columns a row body written now carries, in ordinal order.
    ///
    /// [`TableDef::body_columns`] less the dropped ones.
    #[must_use]
    pub fn stored_columns(&self) -> &[Ordinal] {
        &self.stored_columns
    }

    /// Every secondary index on the table.
    #[must_use]
    pub fn indexes(&self) -> &[IndexDef] {
        &self.indexes
    }

    /// Look up an index by id.
    #[must_use]
    pub fn index(&self, id: IndexId) -> Option<&IndexDef> {
        self.indexes.iter().find(|i| i.id == id)
    }

    /// Look up an index by name.
    #[must_use]
    pub fn index_by_name(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.iter().find(|i| i.name == name)
    }

    /// Every `CHECK` constraint on the table.
    #[must_use]
    pub fn checks(&self) -> &[CheckDef] {
        &self.checks
    }

    /// Every foreign key this table declares.
    #[must_use]
    pub fn foreign_keys(&self) -> &[ForeignKeyDef] {
        &self.foreign_keys
    }

    /// The tenant column, when the table is tenant-scoped.
    ///
    /// A tenant-scoped table is guaranteed to have this as its first primary
    /// key column, which is what lets the kernel turn a tenant restriction into
    /// a key prefix instead of a filter.
    #[must_use]
    pub const fn tenant_column(&self) -> Option<Ordinal> {
        self.tenant_column
    }

    /// The column a soft delete stamps, if this table soft-deletes.
    ///
    /// `None` is an ordinary table, where `delete` removes the row.
    #[must_use]
    pub const fn soft_delete(&self) -> Option<Ordinal> {
        self.soft_delete
    }

    /// The current schema version, stamped onto every row written.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

/// Builder for [`TableDef`]. Validation happens in [`TableBuilder::build`].
#[derive(Debug, Clone)]
pub struct TableBuilder {
    id: TableId,
    name: String,
    columns: Vec<ColumnDef>,
    primary_key: Vec<String>,
    indexes: Vec<IndexBuilder>,
    checks: Vec<CheckDef>,
    foreign_keys: Vec<ForeignKeyBuilder>,
    tenant_column: Option<String>,
    soft_delete: Option<String>,
    schema_version: u32,
    /// Column changes applied after the columns are declared, so that a drop, a
    /// rename or a default can be written next to the version it happened in
    /// rather than folded into the original declaration.
    changes: Vec<ColumnChange>,
}

/// A change to a column declared earlier in the builder.
#[derive(Debug, Clone)]
enum ColumnChange {
    Default { column: String, value: Value },
    Dropped { column: String, version: u32 },
    Renamed { column: String, previous: String },
}

impl TableBuilder {
    /// Append a non-nullable column.
    #[must_use]
    pub fn column(self, name: impl Into<String>, ty: ValueType) -> Self {
        self.push_column(name, ty, false, 0)
    }

    /// Append a nullable column.
    #[must_use]
    pub fn nullable_column(self, name: impl Into<String>, ty: ValueType) -> Self {
        self.push_column(name, ty, true, 0)
    }

    /// Append a nullable column introduced in a later schema version.
    ///
    /// Rows written before `added_in` carry no bytes for it, so it reads back
    /// as null and must be nullable. Give it a default — with
    /// [`TableBuilder::added_column_with_default`] or
    /// [`TableBuilder::default_for`] — and it need not be.
    #[must_use]
    pub fn added_column(self, name: impl Into<String>, ty: ValueType, added_in: u32) -> Self {
        self.push_column(name, ty, true, added_in)
    }

    /// Append a **non-nullable** column introduced in a later schema version,
    /// with the value older rows read back as.
    ///
    /// This is the migration a default exists for. Nothing is rewritten: rows
    /// written before `added_in` carry no bytes for the column, and the decoder
    /// supplies `default` in their place. Adding a `NOT NULL` column is
    /// therefore as cheap as adding a nullable one, and neither touches a
    /// stored byte.
    #[must_use]
    pub fn added_column_with_default(
        self,
        name: impl Into<String>,
        ty: ValueType,
        added_in: u32,
        default: Value,
    ) -> Self {
        let name = name.into();
        self.push_column(name.clone(), ty, false, added_in)
            .default_for(name, default)
    }

    /// Give a column a default: the value stored when a
    /// [`PartialRow`](crate::PartialRow) leaves it unset, and the value a row
    /// written before the column existed reads back as.
    ///
    /// Separate from the column declarations rather than multiplied through
    /// them, so that "nullable", "added in version n" and "has a default" stay
    /// three independent facts instead of eight constructors.
    #[must_use]
    pub fn default_for(mut self, column: impl Into<String>, value: Value) -> Self {
        self.changes.push(ColumnChange::Default {
            column: column.into(),
            value,
        });
        self
    }

    /// Drop a column as of `dropped_in`.
    ///
    /// The column stays in the schema, keeps its ordinal for ever and holds
    /// nothing. Ordinals are the row body's field order and are compiled into
    /// callers' column constants, so closing the gap would renumber every
    /// column after it and start reading one column's bytes as another's.
    ///
    /// Rows written before `dropped_in` still carry the column's bytes; the
    /// decoder skips them. Rows written after do not carry it at all. Nothing
    /// is rewritten, and the space is reclaimed only as rows are.
    ///
    /// A dropped column may not be in the primary key, an index or a foreign
    /// key — drop those first.
    #[must_use]
    pub fn drop_column(mut self, column: impl Into<String>, dropped_in: u32) -> Self {
        self.changes.push(ColumnChange::Dropped {
            column: column.into(),
            version: dropped_in,
        });
        self
    }

    /// Record that `column` used to be called `previous`.
    ///
    /// A rename moves no bytes — a name appears nowhere on disk — so this is a
    /// declaration rather than a migration. What it buys is that the old name
    /// still resolves through [`TableDef::ordinal_of`], so queries and code
    /// written against it keep working, and that the rename is legible in the
    /// schema rather than being an unexplained edit to a string literal.
    #[must_use]
    pub fn renamed_column(
        mut self,
        column: impl Into<String>,
        previous: impl Into<String>,
    ) -> Self {
        self.changes.push(ColumnChange::Renamed {
            column: column.into(),
            previous: previous.into(),
        });
        self
    }

    fn push_column(
        mut self,
        name: impl Into<String>,
        ty: ValueType,
        nullable: bool,
        added_in: u32,
    ) -> Self {
        self.columns.push(ColumnDef {
            name: name.into(),
            ty,
            nullable,
            added_in,
            dropped_in: None,
            default: None,
            previous_names: Vec::new(),
            scale: 0,
            element: None,
            managed: None,
        });
        self
    }

    /// Append a decimal column with `scale` digits after the point.
    ///
    /// The scale is fixed for the column and every value in it is a count of
    /// the smallest unit: at scale 2, `1250` is `12.50`. That is what makes
    /// comparison and `SUM` exact integer operations, and it is why the scale
    /// is declared once here rather than carried by each value.
    ///
    /// A scale above 18 is refused at build time: `i64` holds about 9.2 × 10¹⁸
    /// units, so beyond that the integral part has no room left and every
    /// value in the column would be a fraction.
    #[must_use]
    pub fn decimal_column(self, name: impl Into<String>, scale: u8) -> Self {
        self.push_decimal(name, scale, false, 0)
    }

    /// [`TableBuilder::decimal_column`], accepting nulls.
    #[must_use]
    pub fn nullable_decimal_column(self, name: impl Into<String>, scale: u8) -> Self {
        self.push_decimal(name, scale, true, 0)
    }

    /// Append an array column whose elements are `element`.
    ///
    /// The element type is declared once here and every value in the column
    /// obeys it, which is what keeps [`ValueType`] fieldless. An array of
    /// arrays is refused at build time: `ValueType::Array` cannot name an
    /// inner element type, so the inner array would be a value the schema
    /// cannot describe.
    ///
    /// An array cannot be a primary key or an index column. The order is
    /// meaningful — unlike a vector's — but the question an indexed array is
    /// asked is *containment*, which needs one index entry per element and is
    /// a different index cardinality from the one this store has. See
    /// `docs/arrays.md` §4.
    #[must_use]
    pub fn array_column(self, name: impl Into<String>, element: ValueType) -> Self {
        self.push_array(name, element, false, 0)
    }

    /// [`TableBuilder::array_column`], accepting nulls.
    ///
    /// Nullable in the column's sense: the whole value may be absent. It says
    /// nothing about an *element* being null, which is refused either way —
    /// see [`crate::row::Row::validate`].
    #[must_use]
    pub fn nullable_array_column(self, name: impl Into<String>, element: ValueType) -> Self {
        self.push_array(name, element, true, 0)
    }

    /// Set the element type of an array column already appended.
    ///
    /// [`TableBuilder::array_column`] is the way to declare one; this is for a
    /// caller that cannot use it, exactly as [`TableBuilder::scale_for`] is.
    /// A name that is not a column here is ignored rather than refused, for
    /// the reason given there.
    #[must_use]
    pub fn element_for(mut self, column: &str, element: ValueType) -> Self {
        if let Some(found) = self.columns.iter_mut().find(|c| c.name == column) {
            found.element = Some(element);
        }
        self
    }

    fn push_array(
        mut self,
        name: impl Into<String>,
        element: ValueType,
        nullable: bool,
        added_in: u32,
    ) -> Self {
        self = self.push_column(name, ValueType::Array, nullable, added_in);
        if let Some(column) = self.columns.last_mut() {
            column.element = Some(element);
        }
        self
    }

    /// Make a column already appended one the store writes. See [`Managed`].
    ///
    /// On an already-appended column for the reason [`TableBuilder::scale_for`]
    /// is: the four `column` entry points already cover a
    /// nullable/`added_in`/`default` matrix, and a managed variant of each
    /// would double it to express one property.
    ///
    /// A name that is not a column here is ignored rather than refused, again
    /// as `scale_for` does — the builder reports nothing until
    /// [`TableBuilder::build`], and the column this would have named is
    /// refused there under its own error, which is a better message.
    #[must_use]
    pub fn managed_for(mut self, column: &str, managed: Managed) -> Self {
        if let Some(found) = self.columns.iter_mut().find(|c| c.name == column) {
            found.managed = Some(managed);
        }
        self
    }

    /// Set the scale of a decimal column already appended.
    ///
    /// [`TableBuilder::decimal_column`] is the way to declare one, and this is
    /// for a caller that cannot use it: `slate-serverd` builds a column from a
    /// TOML table whose `nullable`, `added_in` and `default` combination is
    /// already a five-armed match over the four `column` entry points, and a
    /// scale-carrying variant of each would double it for one type.
    ///
    /// A name that is not a column here is ignored rather than refused. The
    /// builder reports nothing until [`TableBuilder::build`], and the column
    /// this would have named is refused there under its own error — which is a
    /// better message than "no such column" from a scale.
    #[must_use]
    pub fn scale_for(mut self, column: &str, scale: u8) -> Self {
        if let Some(found) = self.columns.iter_mut().find(|c| c.name == column) {
            found.scale = scale;
        }
        self
    }

    fn push_decimal(
        mut self,
        name: impl Into<String>,
        scale: u8,
        nullable: bool,
        added_in: u32,
    ) -> Self {
        self = self.push_column(name, ValueType::Decimal, nullable, added_in);
        if let Some(column) = self.columns.last_mut() {
            column.scale = scale;
        }
        self
    }

    /// Set the primary key, in key order.
    #[must_use]
    pub fn primary_key<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.primary_key = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Mark a column as the tenant discriminator.
    ///
    /// It must be the first primary key column; see
    /// [`SchemaError::TenantColumnNotKeyPrefix`].
    #[must_use]
    pub fn tenant_column(mut self, name: impl Into<String>) -> Self {
        self.tenant_column = Some(name.into());
        self
    }

    /// Soft-delete this table, stamping `name` instead of removing a row.
    ///
    /// `delete` writes the current time into `name` and leaves the row where
    /// it is, and every read conjoins `name IS NULL` so the row stops being
    /// visible. The column is nullable `i64` seconds, null meaning "not
    /// deleted" — the one representation that needs no sentinel time and no
    /// second boolean to disagree with.
    ///
    /// The read filter goes through [`SecurityCatalog::row_filter`], the same
    /// choke point row-level security uses, for one reason: that path is
    /// already proven to cover every access path, and a second filtering
    /// mechanism would have to be proven again — on the joins, the aggregates,
    /// the chains and the index-only scans. It also inherits the property that
    /// makes it safe, which is that the filter is conjoined *before* planning,
    /// so an index that does not carry this column cannot be chosen for an
    /// index-only scan and answer from keys alone.
    ///
    /// It is not a policy, though a policy could express it, because a deleted
    /// row is not hidden for a security reason: a superuser is not exempt, and
    /// it applies whether or not row-level security is enabled for the table.
    ///
    /// [`SecurityCatalog::row_filter`]: ../slate_kernel/struct.SecurityCatalog.html
    #[must_use]
    pub fn soft_delete(mut self, name: impl Into<String>) -> Self {
        self.soft_delete = Some(name.into());
        self
    }

    /// Attach a secondary index.
    #[must_use]
    pub fn index(mut self, index: IndexBuilder) -> Self {
        self.indexes.push(index);
        self
    }

    /// Attach a `CHECK` constraint, refused by the record store at write time.
    #[must_use]
    pub fn check(mut self, check: CheckDef) -> Self {
        self.checks.push(check);
        self
    }

    /// Attach a foreign key into another table's primary key.
    #[must_use]
    pub fn foreign_key(mut self, foreign_key: ForeignKeyBuilder) -> Self {
        self.foreign_keys.push(foreign_key);
        self
    }

    /// Set the current schema version, stamped onto rows as they are written.
    #[must_use]
    pub const fn schema_version(mut self, version: u32) -> Self {
        self.schema_version = version;
        self
    }

    /// Validate the definition and freeze it.
    ///
    /// Everything the kernel assumes about a table is checked exactly once,
    /// here, so downstream code can index and encode without re-validating.
    ///
    /// A foreign key's *parent* is the exception: its shape cannot be checked
    /// without the table it points at, so [`Catalog`](crate::Catalog) checks it
    /// once every table is present.
    pub fn build(mut self) -> Result<TableDef> {
        // Cloned rather than borrowed: every error below names the table, and
        // `apply_changes` needs `&mut self` in between.
        let table = &self.name.clone();

        // Column names must be unique; ordinals are how everything else refers
        // to a column, so an ambiguous name would silently pick one.
        for (i, col) in self.columns.iter().enumerate() {
            if self.columns.iter().take(i).any(|c| c.name == col.name) {
                return Err(SchemaError::DuplicateColumn {
                    table: table.clone(),
                    column: col.name.clone(),
                });
            }
        }

        // A decimal's scale has to leave room for a number. `i64` holds about
        // 9.2 x 10^18 units, so at scale 19 every value is a fraction and at
        // scale 18 there is exactly one integral digit — which is the last
        // scale that can represent anything above one.
        const MAX_SCALE: u8 = 18;
        for col in &self.columns {
            // An array column must say what it holds, and must not say
            // "another array". Both are build-time refusals rather than
            // silent defaults: an unstated element type has no sensible
            // stand-in, and a nested one is a value `Row::validate` could
            // never accept, so accepting the *declaration* would only move
            // the failure to the first write.
            if col.ty == ValueType::Array {
                match col.element {
                    None => {
                        return Err(SchemaError::ArrayWithoutElementType {
                            table: table.clone(),
                            column: col.name.clone(),
                        });
                    }
                    Some(ValueType::Array) => {
                        return Err(SchemaError::NestedArrayColumn {
                            table: table.clone(),
                            column: col.name.clone(),
                        });
                    }
                    Some(_) => {}
                }
            }
            if col.ty == ValueType::Decimal && col.scale > MAX_SCALE {
                return Err(SchemaError::ScaleTooLarge {
                    table: table.clone(),
                    column: col.name.clone(),
                    scale: col.scale,
                    max: MAX_SCALE,
                });
            }
        }

        // A managed column is a timestamp the store writes, so three things
        // have to hold and none of them is obvious from the type alone.
        for (at, col) in self.columns.iter().enumerate() {
            let Some(managed) = col.managed else {
                continue;
            };
            // `I64` because that is what a time is everywhere else here —
            // seconds since the epoch, which is what `date_trunc`,
            // `CalendarPart` and every seeded timestamp in this repository
            // already mean. A managed column of another type would be a
            // second, silently different, representation of time.
            if col.ty != ValueType::I64 {
                return Err(SchemaError::ManagedColumnNotTimestamp {
                    table: table.clone(),
                    column: col.name.clone(),
                    found: col.ty,
                });
            }
            // Nullable is refused rather than tolerated. It would be harmless
            // — the store always writes a value — and it would be a lie in the
            // schema: a reader seeing `nullable` reasonably writes code that
            // handles the null, and that branch can never run.
            if col.nullable {
                return Err(SchemaError::ManagedColumnNullable {
                    table: table.clone(),
                    column: col.name.clone(),
                });
            }
            // A key column addresses the row. `UpdatedAt` in a key would move
            // the row on every write, and `CreatedAt` would make the key
            // unknowable until after the insert — so neither is a key, and the
            // two failures are different enough that saying "managed" once is
            // clearer than two messages.
            if self.primary_key.iter().any(|name| name == &col.name) {
                return Err(SchemaError::ManagedColumnInKey {
                    table: table.clone(),
                    column: col.name.clone(),
                });
            }
            let _ = (at, managed);
        }

        self.apply_changes(table)?;
        self.check_name_resolution(table)?;
        self.check_evolution(table)?;

        let resolve = |name: &str| -> Result<Ordinal> {
            self.columns
                .iter()
                .position(|c| c.name == name)
                .map(Ordinal)
                .ok_or_else(|| SchemaError::UnknownColumn {
                    table: table.clone(),
                    column: name.to_owned(),
                })
        };

        if self.primary_key.is_empty() {
            return Err(SchemaError::MissingPrimaryKey {
                table: table.clone(),
            });
        }

        let mut primary_key = Vec::with_capacity(self.primary_key.len());
        for name in &self.primary_key {
            let ordinal = resolve(name)?;
            if primary_key.contains(&ordinal) {
                return Err(SchemaError::DuplicateKeyColumn {
                    table: table.clone(),
                    key: "primary key".to_owned(),
                    column: name.clone(),
                });
            }
            // A null key component would let two rows share an identity.
            if self
                .columns
                .get(ordinal.0)
                .is_some_and(ColumnDef::is_nullable)
            {
                return Err(SchemaError::NullablePrimaryKeyColumn {
                    table: table.clone(),
                    column: name.clone(),
                });
            }
            reject_unkeyable(&self.columns, ordinal, table.as_str(), "primary key", name)?;
            reject_dropped(&self.columns, ordinal, table.as_str(), "primary key", name)?;
            primary_key.push(ordinal);
        }

        let tenant_column = match &self.tenant_column {
            None => None,
            Some(name) => {
                let ordinal = resolve(name)?;
                // Tenant scoping is a key prefix, not a filter. That only holds
                // if the tenant leads the key.
                if primary_key.first() != Some(&ordinal) {
                    return Err(SchemaError::TenantColumnNotKeyPrefix {
                        table: table.clone(),
                        column: name.clone(),
                    });
                }
                Some(ordinal)
            }
        };

        // A soft-delete column is the inverse of a managed one in the place
        // that matters: it *must* be nullable, because null is what "not
        // deleted" means. The rest of the rules are the same, and for the same
        // reasons.
        let soft_delete = match &self.soft_delete {
            None => None,
            Some(name) => {
                let ordinal = resolve(name)?;
                // `resolve` already refused an unknown name, so this is the
                // same lookup rather than a second chance to fail.
                let col =
                    self.columns
                        .get(ordinal.0)
                        .ok_or_else(|| SchemaError::UnknownColumn {
                            table: table.clone(),
                            column: name.clone(),
                        })?;
                if col.ty != ValueType::I64 {
                    return Err(SchemaError::SoftDeleteNotTimestamp {
                        table: table.clone(),
                        column: name.clone(),
                        found: col.ty,
                    });
                }
                // Refused rather than tolerated, and this is the rule that
                // catches the likely mistake: a non-nullable column has no
                // value meaning "not deleted", so every row would read as
                // deleted the moment the table was declared and the table
                // would go silently empty.
                if !col.nullable {
                    return Err(SchemaError::SoftDeleteNotNullable {
                        table: table.clone(),
                        column: name.clone(),
                    });
                }
                // No check for "in the primary key" here, though the case is
                // real: a soft-delete column must be nullable and a primary
                // key column may not be, so `NullablePrimaryKey` already
                // refuses the combination from the other side. A check was
                // written, and a mutation showed no input could reach it.
                // `updated_at` moves on every write including the delete,
                // which is right; `created_at` and the delete stamp would
                // fight over one column, which is not.
                if col.managed.is_some() {
                    return Err(SchemaError::SoftDeleteManaged {
                        table: table.clone(),
                        column: name.clone(),
                    });
                }
                Some(ordinal)
            }
        };

        let mut indexes: Vec<IndexDef> = Vec::with_capacity(self.indexes.len());
        for spec in &self.indexes {
            // Before the gate below, which would report this as "no columns":
            // a text index with an expression has columns *and* an expression,
            // so it lands in the same XNOR and comes back with a reason that
            // is not its reason.
            if spec.text && spec.expression.is_some() {
                return Err(SchemaError::UnindexableText {
                    table: table.clone(),
                    index: spec.name.clone(),
                    reason: "also keys on an expression, and a term is not a computed value",
                });
            }
            // An index keys on columns or on an expression. Neither is nothing
            // to look up by; both would be two answers to what its key holds.
            if spec.columns.is_empty() == spec.expression.is_none() {
                return Err(SchemaError::EmptyIndex {
                    table: table.clone(),
                    index: spec.name.clone(),
                });
            }
            if indexes
                .iter()
                .any(|i| i.id == spec.id || i.name == spec.name)
            {
                return Err(SchemaError::DuplicateIndex {
                    table: table.clone(),
                    index: spec.name.clone(),
                });
            }
            let mut columns = Vec::with_capacity(spec.columns.len());
            for (name, direction) in &spec.columns {
                let ordinal = resolve(name)?;
                if columns.iter().any(|c: &IndexColumn| c.ordinal == ordinal) {
                    return Err(SchemaError::DuplicateKeyColumn {
                        table: table.clone(),
                        key: spec.name.clone(),
                        column: name.clone(),
                    });
                }
                reject_unkeyable(&self.columns, ordinal, table.as_str(), &spec.name, name)?;
                reject_dropped(&self.columns, ordinal, table.as_str(), &spec.name, name)?;
                columns.push(IndexColumn {
                    ordinal,
                    direction: *direction,
                });
            }
            if spec.text {
                // Four ways to declare something an inverted index cannot
                // hold, refused here rather than half-working. The types are
                // read out of the columns being built, not out of the table,
                // because the table does not exist yet.
                let reason = if spec.unique {
                    Some(
                        "is also unique, which no realistic text can satisfy: a term appears \
                         in many rows by construction",
                    )
                } else if columns.len() != 1 {
                    Some(
                        "names more than one column, and a cross product of two columns' \
                          terms is a different and much larger structure",
                    )
                } else if columns
                    .first()
                    .and_then(|c| self.columns.get(c.ordinal.0))
                    .map(ColumnDef::value_type)
                    != Some(ValueType::Str)
                {
                    Some("keys on a column that is not a string")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    return Err(SchemaError::UnindexableText {
                        table: table.clone(),
                        index: spec.name.clone(),
                        reason,
                    });
                }
            }
            indexes.push(IndexDef {
                id: spec.id,
                name: spec.name.clone(),
                columns,
                unique: spec.unique,
                predicate: spec.predicate.clone(),
                expression: spec.expression.clone(),
                text: spec.text,
            });
        }

        // Check names are the constraint's identity in an error message and in
        // `TableDef` equality, so two of them would make one unnameable.
        for (i, check) in self.checks.iter().enumerate() {
            if self.checks.iter().take(i).any(|c| c.name() == check.name()) {
                return Err(SchemaError::DuplicateCheck {
                    table: table.clone(),
                    check: check.name().to_owned(),
                });
            }
            // A message that is present and blank. `None` is the way to have no
            // message; `Some("")` is an author who meant to write one, and it
            // reaches a form as an empty error beside the field it is supposed
            // to explain.
            if check.message().is_some_and(|m| m.trim().is_empty()) {
                return Err(SchemaError::EmptyCheckMessage {
                    table: table.clone(),
                    check: check.name().to_owned(),
                });
            }
        }

        let mut foreign_keys: Vec<ForeignKeyDef> = Vec::with_capacity(self.foreign_keys.len());
        for spec in &self.foreign_keys {
            if spec.columns.is_empty() {
                return Err(SchemaError::EmptyForeignKey {
                    table: table.clone(),
                    foreign_key: spec.name.clone(),
                });
            }
            if foreign_keys.iter().any(|f| f.name() == spec.name) {
                return Err(SchemaError::DuplicateForeignKey {
                    table: table.clone(),
                    foreign_key: spec.name.clone(),
                });
            }
            let mut columns: Vec<Ordinal> = Vec::with_capacity(spec.columns.len());
            for name in &spec.columns {
                let ordinal = resolve(name)?;
                if columns.contains(&ordinal) {
                    return Err(SchemaError::DuplicateKeyColumn {
                        table: table.clone(),
                        key: spec.name.clone(),
                        column: name.clone(),
                    });
                }
                reject_dropped(&self.columns, ordinal, table.as_str(), &spec.name, name)?;
                columns.push(ordinal);
            }
            foreign_keys.push(spec.finish(columns));
        }

        let body_columns: Vec<Ordinal> = (0..self.columns.len())
            .map(Ordinal)
            .filter(|o| !primary_key.contains(o))
            .collect();
        let stored_columns = body_columns
            .iter()
            .copied()
            .filter(|o| !self.columns.get(o.0).is_some_and(ColumnDef::is_dropped))
            .collect();

        Ok(TableDef {
            id: self.id,
            name: self.name,
            columns: self.columns,
            primary_key,
            body_columns,
            stored_columns,
            indexes,
            checks: self.checks,
            foreign_keys,
            tenant_column,
            soft_delete,
            schema_version: self.schema_version,
        })
    }

    /// Fold the declared defaults, drops and renames into the columns.
    ///
    /// Applied after the columns rather than during, so that a column's
    /// declaration stays the shape it was first written in and the migrations
    /// read as a list of what happened to it since.
    fn apply_changes(&mut self, table: &str) -> Result<()> {
        let schema_version = self.schema_version;
        for change in core::mem::take(&mut self.changes) {
            let name = match &change {
                ColumnChange::Default { column, .. }
                | ColumnChange::Dropped { column, .. }
                | ColumnChange::Renamed { column, .. } => column.clone(),
            };
            let ordinal = self
                .columns
                .iter()
                .position(|c| c.name == name)
                .ok_or_else(|| SchemaError::UnknownColumn {
                    table: table.to_owned(),
                    column: name.clone(),
                })?;
            let Some(column) = self.columns.get_mut(ordinal) else {
                continue;
            };
            match change {
                ColumnChange::Default { value, .. } => match value.value_type() {
                    // A null default is the absence of a default written out at
                    // length; refused so that "unset" has one meaning.
                    None => {
                        return Err(SchemaError::NullDefault {
                            table: table.to_owned(),
                            column: name,
                        });
                    }
                    Some(actual) if actual != column.ty => {
                        return Err(SchemaError::ValueTypeMismatch {
                            table: table.to_owned(),
                            column: name,
                            expected: column.ty,
                            actual: value.type_name(),
                        });
                    }
                    Some(_) => column.default = Some(value),
                },
                ColumnChange::Dropped { version, .. } => {
                    if version <= column.added_in {
                        return Err(SchemaError::ColumnDroppedBeforeAdded {
                            table: table.to_owned(),
                            column: name,
                            added_in: column.added_in,
                            dropped_in: version,
                        });
                    }
                    // A drop the current schema has not reached yet would leave
                    // the encoder still writing the column while the schema
                    // says it is gone. Refused rather than half-applied.
                    if version > schema_version {
                        return Err(SchemaError::ColumnDroppedInFutureVersion {
                            table: table.to_owned(),
                            column: name,
                            dropped_in: version,
                            schema_version,
                        });
                    }
                    column.dropped_in = Some(version);
                }
                ColumnChange::Renamed { previous, .. } => column.previous_names.push(previous),
            }
        }
        Ok(())
    }

    /// Every name — current or retired — must name exactly one column.
    ///
    /// Without this, renaming `a` to `b` and later reusing `a` makes
    /// `ordinal_of("a")` a coin toss decided by declaration order, and the
    /// caller who gets the wrong one reads the wrong column rather than an
    /// error.
    fn check_name_resolution(&self, table: &str) -> Result<()> {
        let mut seen: Vec<&str> = Vec::new();
        for column in &self.columns {
            for name in core::iter::once(&column.name).chain(&column.previous_names) {
                if seen.contains(&name.as_str()) {
                    return Err(SchemaError::AmbiguousColumnName {
                        table: table.to_owned(),
                        name: name.clone(),
                    });
                }
                seen.push(name);
            }
        }
        Ok(())
    }

    /// The rules that make a stored row from an older schema still readable.
    ///
    /// The one that matters most is not here: a column added after version 0
    /// must be nullable or defaulted, or a row written before it has nothing to
    /// read back. That is unrepresentable rather than checked —
    /// [`TableBuilder::added_column`] makes the column nullable and
    /// [`TableBuilder::added_column_with_default`] gives it a default, and
    /// there is no third way to declare one. The decoder re-checks it anyway,
    /// as [`SchemaError::IncompatibleSchemaEvolution`], because a stored row is
    /// the one input the builder never saw.
    fn check_evolution(&self, table: &str) -> Result<()> {
        for column in &self.columns {
            // A dropped column is never read back, so a default on one is a
            // declaration with no effect. Refused rather than ignored.
            if column.is_dropped() && column.default.is_some() {
                return Err(SchemaError::DroppedColumnHasDefault {
                    table: table.to_owned(),
                    column: column.name.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Refuse a dropped column in a key, an index or a foreign key.
///
/// A dropped column holds nothing and is not written, so a key over one would
/// index a column of nulls — and for a primary key, give every row the same
/// identity. Drop the index or the key first.
fn reject_dropped(
    columns: &[ColumnDef],
    ordinal: Ordinal,
    table: &str,
    key: &str,
    column: &str,
) -> Result<()> {
    if columns.get(ordinal.0).is_some_and(ColumnDef::is_dropped) {
        return Err(SchemaError::DroppedColumnInKey {
            table: table.to_owned(),
            key: key.to_owned(),
            column: column.to_owned(),
        });
    }
    Ok(())
}

/// Refuse a vector or an array column in a key or an index.
///
/// Two types, two different reasons, and keeping them apart is the point of
/// the two errors.
///
/// A vector orders totally, so it can be stored and grouped, but that order is
/// not its similarity — two nearby embeddings need not sort near each other.
/// An index on one would answer no question worth asking and a range over one
/// would mean nothing, so this is a schema error rather than a slow query.
///
/// An array's order *is* meaningful, so that argument does not transfer. It is
/// refused for a different reason: what a user wants from an indexed array is
/// almost never "rows whose array sorts near this one" but *which rows contain
/// this element*, and that is one index entry per element per row against a
/// write path that produces exactly one. The message says so, rather than
/// leaving a caller to infer that arrays sort badly — they do not.
fn reject_unkeyable(
    columns: &[ColumnDef],
    ordinal: Ordinal,
    table: &str,
    key: &str,
    column: &str,
) -> Result<()> {
    match columns.get(ordinal.0).map(ColumnDef::value_type) {
        Some(ValueType::Vector) => Err(SchemaError::VectorInKey {
            table: table.to_owned(),
            key: key.to_owned(),
            column: column.to_owned(),
        }),
        Some(ValueType::Array) => Err(SchemaError::ArrayInKey {
            table: table.to_owned(),
            key: key.to_owned(),
            column: column.to_owned(),
        }),
        _ => Ok(()),
    }
}
