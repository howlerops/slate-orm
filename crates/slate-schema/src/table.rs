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
        });
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
            reject_vector(&self.columns, ordinal, table.as_str(), "primary key", name)?;
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

        let mut indexes: Vec<IndexDef> = Vec::with_capacity(self.indexes.len());
        for spec in &self.indexes {
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
                reject_vector(&self.columns, ordinal, table.as_str(), &spec.name, name)?;
                reject_dropped(&self.columns, ordinal, table.as_str(), &spec.name, name)?;
                columns.push(IndexColumn {
                    ordinal,
                    direction: *direction,
                });
            }
            indexes.push(IndexDef {
                id: spec.id,
                name: spec.name.clone(),
                columns,
                unique: spec.unique,
                predicate: spec.predicate.clone(),
                expression: spec.expression.clone(),
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

/// Refuse a vector column in a key or an index.
///
/// A vector orders totally, so it can be stored and grouped, but that order is
/// not its similarity — two nearby embeddings need not sort near each other.
/// An index on one would answer no question worth asking and a range over one
/// would mean nothing, so this is a schema error rather than a slow query.
fn reject_vector(
    columns: &[ColumnDef],
    ordinal: Ordinal,
    table: &str,
    key: &str,
    column: &str,
) -> Result<()> {
    if columns
        .get(ordinal.0)
        .is_some_and(|c| c.value_type() == ValueType::Vector)
    {
        return Err(SchemaError::VectorInKey {
            table: table.to_owned(),
            key: key.to_owned(),
            column: column.to_owned(),
        });
    }
    Ok(())
}
