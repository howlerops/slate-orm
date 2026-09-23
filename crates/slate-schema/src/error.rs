//! Errors raised when defining or using a schema.

use slate_tuple::{TupleError, ValueType};

/// One `CHECK` a row failed.
///
/// Separate from the error so the error can hold several. `column` and
/// `message` are `None` unless the schema said otherwise: a check about two
/// columns has no single field to blame, and most checks have no sentence
/// written for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckFailure {
    /// The constraint that refused the row.
    pub check: String,
    /// The column the constraint is about, when it is about one.
    pub column: Option<String>,
    /// The sentence to show a person, when the schema wrote one.
    pub message: Option<String>,
}

/// The text a [`SchemaError::CheckViolation`] renders to.
///
/// One failure reads exactly as it did when only one could be reported, so a
/// log line and a test that matched the old wording still do. Several read as
/// a list, because the alternative — reporting the first and dropping the rest
/// into a count — throws away the thing that made collecting them worthwhile.
fn render_violations(table: &str, violations: &[CheckFailure]) -> String {
    let one = |failure: &CheckFailure| match &failure.message {
        Some(message) => format!("`{}`: {message}", failure.check),
        None => format!("`{}`", failure.check),
    };
    match violations {
        [only] => format!(
            "row violates check `{}` on table `{table}`{}",
            only.check,
            only.message
                .as_deref()
                .map(|m| format!(": {m}"))
                .unwrap_or_default()
        ),
        many => format!(
            "row violates {} checks on table `{table}`: {}",
            many.len(),
            many.iter().map(one).collect::<Vec<_>>().join("; ")
        ),
    }
}

/// A schema definition was rejected, or a row did not match its schema.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SchemaError {
    /// A managed column is not the type a timestamp is here.
    #[error(
        "column `{column}` of table `{table}` is managed, so the store writes a time into it, \
         but it is declared {found:?}; a managed column is `i64` seconds since the epoch, \
         which is what every other time in this schema is"
    )]
    ManagedColumnNotTimestamp {
        /// The table being defined.
        table: String,
        /// The column.
        column: String,
        /// The type it was declared as.
        found: slate_tuple::ValueType,
    },

    /// A soft-delete column that is not an `i64`.
    #[error(
        "table `{table}` soft-deletes into column `{column}`, which is declared {found:?}; \
         a soft-delete column is nullable `i64` seconds since the epoch, null meaning \
         the row is not deleted"
    )]
    SoftDeleteNotTimestamp {
        /// The table being defined.
        table: String,
        /// The column.
        column: String,
        /// The type it was declared as.
        found: slate_tuple::ValueType,
    },

    /// A soft-delete column that cannot hold "not deleted".
    #[error(
        "table `{table}` soft-deletes into column `{column}`, which is not nullable; \
         null is what \"not deleted\" means, so without it every row reads as deleted \
         and the table answers nothing"
    )]
    SoftDeleteNotNullable {
        /// The table being defined.
        table: String,
        /// The column.
        column: String,
    },

    /// A soft-delete column that is also managed.
    #[error(
        "table `{table}` soft-deletes into column `{column}`, which is also a managed \
         timestamp; the delete stamp and the managed stamp would both own one column"
    )]
    SoftDeleteManaged {
        /// The table being defined.
        table: String,
        /// The column.
        column: String,
    },

    /// A managed column is declared nullable, which it can never be.
    #[error(
        "column `{column}` of table `{table}` is managed and nullable; the store writes a \
         value on every write, so the null can never happen and the declaration would send a \
         reader down a branch that cannot run"
    )]
    ManagedColumnNullable {
        /// The table being defined.
        table: String,
        /// The column.
        column: String,
    },

    /// A managed column is in the primary key.
    #[error(
        "column `{column}` of table `{table}` is managed and in the primary key; the store \
         writes it, so the key would either move on every update or be unknown until after \
         the insert"
    )]
    ManagedColumnInKey {
        /// The table being defined.
        table: String,
        /// The column.
        column: String,
    },

    /// A decimal column's scale leaves no room for an integral part.
    #[error(
        "column `{column}` of table `{table}` declares scale {scale}; an i64 holds about \
         9.2 x 10^18 units, so a scale above {max} leaves no integral part at all"
    )]
    ScaleTooLarge {
        /// The table being defined.
        table: String,
        /// The column.
        column: String,
        /// What it asked for.
        scale: u8,
        /// The largest scale that leaves room.
        max: u8,
    },

    /// A column name referenced in a key or index does not exist on the table.
    #[error("table `{table}` has no column named `{column}`")]
    UnknownColumn {
        /// The table being defined.
        table: String,
        /// The name that could not be resolved.
        column: String,
    },

    /// Two columns on the same table share a name.
    #[error("table `{table}` declares column `{column}` more than once")]
    DuplicateColumn {
        /// The table being defined.
        table: String,
        /// The repeated name.
        column: String,
    },

    /// A vector column was used in a key or an index.
    ///
    /// A vector has a total order so it can be stored, grouped and
    /// deduplicated, but that order says nothing about similarity: two nearby
    /// embeddings need not sort near each other. An index on one would
    /// therefore answer no question worth asking, and a range over one would
    /// be meaningless. Nearest-neighbour search is `ORDER BY` a distance with
    /// a `LIMIT`, not a key range.
    #[error(
        "`{key}` on table `{table}` uses vector column `{column}`: a vector's \
         order is not its similarity, so it cannot be a key"
    )]
    VectorInKey {
        /// The table being defined.
        table: String,
        /// Which key or index.
        key: String,
        /// The offending column.
        column: String,
    },

    /// An array column was used in a primary key or an index.
    ///
    /// Not for the vector's reason — an array's order *is* meaningful. The
    /// question asked of an indexed array is containment, and one index entry
    /// per element is a cardinality this store does not have.
    #[error(
        "`{key}` on table `{table}` uses array column `{column}`: an array \
         sorts meaningfully but the question asked of one is containment, \
         which needs one index entry per element and is not built"
    )]
    ArrayInKey {
        /// The table being defined.
        table: String,
        /// Which key or index.
        key: String,
        /// The offending column.
        column: String,
    },

    /// An array column was declared without saying what its elements are.
    #[error(
        "column `{column}` on table `{table}` is an array and declares no \
         element type; use `array_column(name, element)`"
    )]
    ArrayWithoutElementType {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
    },

    /// An array column declared an array as its element type.
    ///
    /// `ValueType::Array` carries no element type of its own, so the inner
    /// array would be a value the schema cannot describe.
    #[error(
        "column `{column}` on table `{table}` declares an array of arrays; \
         the inner array could not say what *its* elements are, so nesting \
         is refused rather than half-described"
    )]
    NestedArrayColumn {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
    },

    /// An element of an array value did not match the column's element type.
    #[error(
        "column `{column}` on table `{table}` is an array of {expected}, and \
         element {index} is {actual}"
    )]
    ArrayElementTypeMismatch {
        /// The table the row belongs to.
        table: String,
        /// The offending column.
        column: String,
        /// Which element, zero-based.
        index: usize,
        /// The declared element type.
        expected: ValueType,
        /// What the element actually was.
        actual: &'static str,
    },

    /// A table was defined without a primary key.
    #[error("table `{table}` has no primary key")]
    MissingPrimaryKey {
        /// The table being defined.
        table: String,
    },

    /// A column appears twice in the same key or index.
    #[error("`{key}` on table `{table}` lists column `{column}` more than once")]
    DuplicateKeyColumn {
        /// The table being defined.
        table: String,
        /// The primary key or index at fault.
        key: String,
        /// The repeated column.
        column: String,
    },

    /// A primary key column was declared nullable.
    ///
    /// A null in a key position would make two rows share an identity, so the
    /// record layer forbids it rather than picking a tie-break rule.
    #[error("primary key column `{column}` on table `{table}` may not be nullable")]
    NullablePrimaryKeyColumn {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
    },

    /// Two indexes on the same table share an id or a name.
    #[error("table `{table}` declares index `{index}` more than once")]
    DuplicateIndex {
        /// The table being defined.
        table: String,
        /// The repeated index id or name.
        index: String,
    },

    /// An expression index computed a value of a type it did not declare.
    #[error(
        "index `{index}` on table `{table}` declares it produces {expected:?}, but computed {actual}"
    )]
    IndexValueTypeMismatch {
        /// Table name.
        table: String,
        /// Index name.
        index: String,
        /// The type the index declared.
        expected: slate_tuple::ValueType,
        /// The type the expression actually produced.
        actual: &'static str,
    },

    /// An index keys on neither columns nor an expression, or on both.
    ///
    /// One variant for two opposite mistakes, because the rule is one rule: an
    /// index's key comes from exactly one place. The message says both, since
    /// the older wording — "has no columns" — was read by an author who had
    /// given it columns *and* an expression and explained nothing.
    #[error(
        "index `{index}` on table `{table}` must key on columns or on an expression, and has \
         either neither or both"
    )]
    EmptyIndex {
        /// The table being defined.
        table: String,
        /// The offending index.
        index: String,
    },

    /// A full-text index was declared with something it cannot hold terms of.
    ///
    /// One error rather than four, carrying the reason, because all four are
    /// the same mistake at the declaration and a caller fixing one wants to
    /// read what an inverted index *is*: one entry per term of one string
    /// column.
    #[error(
        "index `{index}` on table `{table}` is a full-text index and {reason}. It holds one \
         entry per term of one string column, which is what makes `contains` a lookup rather \
         than a scan"
    )]
    UnindexableText {
        /// The table being defined.
        table: String,
        /// The offending index.
        index: String,
        /// What is wrong with it.
        reason: &'static str,
    },

    /// The tenant column is not the first primary key column.
    ///
    /// Tenant scoping is enforced by key prefix, so the tenant must be the
    /// leading component of every key on the table.
    #[error(
        "tenant column `{column}` on table `{table}` must be the first primary key column, \
         so that tenant scoping is a key prefix"
    )]
    TenantColumnNotKeyPrefix {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
    },

    /// Two tables in a catalog share an id or a name.
    #[error("catalog declares table `{table}` more than once")]
    DuplicateTable {
        /// The repeated table id or name.
        table: String,
    },

    /// Two tables declared the same index id.
    ///
    /// An index entry is keyed on the index id and not the table's, so the
    /// index keyspace is global: two tables sharing an id share one key range,
    /// and a scan of either walks both.
    #[error(
        "index `{index}` on table `{table}` uses id {id}, which index          `{existing}` on table `{existing_table}` already has; index ids are          global because an index entry's key does not name its table"
    )]
    DuplicateIndexId {
        /// The repeated id.
        id: u32,
        /// The index being added.
        index: String,
        /// The table being added.
        table: String,
        /// The index already holding the id.
        existing: String,
        /// The table it belongs to.
        existing_table: String,
    },

    /// A row had the wrong number of columns for its table.
    #[error("table `{table}` expects {expected} column(s), row has {actual}")]
    ColumnCountMismatch {
        /// The table the row belongs to.
        table: String,
        /// Column count from the schema.
        expected: usize,
        /// Column count in the row.
        actual: usize,
    },

    /// A row value had the wrong type for its column.
    #[error("column `{column}` on table `{table}` expects {expected}, got {actual}")]
    ValueTypeMismatch {
        /// The table the row belongs to.
        table: String,
        /// The offending column.
        column: String,
        /// The declared column type.
        expected: ValueType,
        /// The type actually supplied.
        actual: &'static str,
    },

    /// A null was supplied for a non-nullable column.
    #[error("column `{column}` on table `{table}` is not nullable")]
    UnexpectedNull {
        /// The table the row belongs to.
        table: String,
        /// The offending column.
        column: String,
    },

    /// A stored row body could not be decoded.
    #[error("decoding a row of table `{table}`: {source}")]
    RowDecode {
        /// The table the row belongs to.
        table: String,
        /// The underlying codec failure.
        #[source]
        source: TupleError,
    },

    /// A stored row was written by a newer schema than this binary knows.
    #[error(
        "row of table `{table}` was written at schema version {found}, \
         but this build only understands up to {known}"
    )]
    RowFromFutureSchema {
        /// The table the row belongs to.
        table: String,
        /// Version stamped on the stored row.
        found: u32,
        /// Version this build declares.
        known: u32,
    },

    /// A stored row body used an unrecognised container format.
    #[error("row of table `{table}` has unsupported storage format {format}")]
    UnsupportedRowFormat {
        /// The table the row belongs to.
        table: String,
        /// The format byte found.
        format: u8,
    },

    /// Two names in one table — current or retired by a rename — refer to
    /// different columns, so neither can be resolved unambiguously.
    #[error("`{name}` names more than one column of table `{table}`")]
    AmbiguousColumnName {
        /// The table being defined.
        table: String,
        /// The name that resolves two ways.
        name: String,
    },

    /// A column was dropped in a version at or before the one that added it.
    #[error(
        "column `{column}` on table `{table}` is dropped in version {dropped_in} \
         but only added in {added_in}"
    )]
    ColumnDroppedBeforeAdded {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
        /// The version the column was added in.
        added_in: u32,
        /// The version the drop claims.
        dropped_in: u32,
    },

    /// A column was dropped in a version the table has not reached.
    ///
    /// The encoder writes at the table's current version, so a drop ahead of it
    /// would leave the column still being written while the schema says it is
    /// gone.
    #[error(
        "column `{column}` on table `{table}` is dropped in version {dropped_in}, \
         but the table is at schema version {schema_version}"
    )]
    ColumnDroppedInFutureVersion {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
        /// The version the drop claims.
        dropped_in: u32,
        /// The table's declared schema version.
        schema_version: u32,
    },

    /// A dropped column was used in a key, an index or a foreign key.
    ///
    /// A dropped column holds nothing, so a key over one would index a column
    /// of nulls — and in a primary key, give every row the same identity.
    #[error("`{key}` on table `{table}` uses dropped column `{column}`")]
    DroppedColumnInKey {
        /// The table being defined.
        table: String,
        /// Which key, index or foreign key.
        key: String,
        /// The offending column.
        column: String,
    },

    /// A default was declared on a dropped column, where it can never apply.
    #[error("dropped column `{column}` on table `{table}` cannot have a default")]
    DroppedColumnHasDefault {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
    },

    /// A column's default was declared as null.
    ///
    /// "No default" already means "null when unset", so a null default is that
    /// written out at length and is refused to keep one meaning per state.
    #[error("column `{column}` on table `{table}` cannot have a null default")]
    NullDefault {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
    },

    /// A row carried a value for a column that has been dropped.
    ///
    /// The value would not be stored. Refused rather than discarded, because a
    /// silently dropped value looks exactly like one that was written.
    #[error("column `{column}` on table `{table}` is dropped and must be null in a row")]
    DroppedColumnValue {
        /// The table the row belongs to.
        table: String,
        /// The offending column.
        column: String,
    },

    /// Two `CHECK` constraints on the same table share a name.
    #[error("table `{table}` declares check `{check}` more than once")]
    DuplicateCheck {
        /// The table being defined.
        table: String,
        /// The repeated name.
        check: String,
    },

    /// A `CHECK` carries a message that is empty or only whitespace.
    ///
    /// Refused rather than tolerated. The message is not decoration: it is
    /// published to every client, generated into three languages, and rendered
    /// beside a form field — so an empty one is a blank error message shown to
    /// somebody trying to fix their input. It also renders the refusal as
    /// ``check `year_is_positive`: `` with nothing after the colon.
    ///
    /// A check with *no* message is fine and common; this is only for one that
    /// was given a message and given an empty one, which is never what the
    /// author meant.
    #[error(
        "table `{table}` gives check `{check}` an empty message; \
         omit the message rather than setting it to nothing"
    )]
    EmptyCheckMessage {
        /// The table being defined.
        table: String,
        /// The check whose message is empty.
        check: String,
    },

    /// Two foreign keys on the same table share a name.
    #[error("table `{table}` declares foreign key `{foreign_key}` more than once")]
    DuplicateForeignKey {
        /// The table being defined.
        table: String,
        /// The repeated name.
        foreign_key: String,
    },

    /// A foreign key was defined with no columns.
    #[error("foreign key `{foreign_key}` on table `{table}` has no columns")]
    EmptyForeignKey {
        /// The table being defined.
        table: String,
        /// The offending constraint.
        foreign_key: String,
    },

    /// A foreign key points at a table the catalog does not contain.
    #[error("foreign key `{foreign_key}` on table `{table}` references unknown table {parent:?}")]
    UnknownForeignKeyParent {
        /// The referencing table.
        table: String,
        /// The offending constraint.
        foreign_key: String,
        /// The table id that could not be resolved.
        parent: crate::table::TableId,
    },

    /// A foreign key names a different number of columns from the parent's
    /// primary key.
    ///
    /// The referenced columns are always the parent's whole primary key, so a
    /// partial reference would have to invent the rest of the key.
    #[error(
        "foreign key `{foreign_key}` on table `{table}` names {actual} column(s), \
         but the primary key of `{parent}` has {expected}"
    )]
    ForeignKeyWidthMismatch {
        /// The referencing table.
        table: String,
        /// The offending constraint.
        foreign_key: String,
        /// The referenced table.
        parent: String,
        /// Length of the parent's primary key.
        expected: usize,
        /// Number of columns the constraint names.
        actual: usize,
    },

    /// A foreign key column's type differs from the parent key column it
    /// references.
    ///
    /// The child's values are encoded into a parent row key, so a type
    /// mismatch would look up a key that no row can ever have.
    #[error(
        "foreign key `{foreign_key}` on table `{table}` column `{column}` holds {actual}, \
         but the matching primary key column of `{parent}` holds {expected}"
    )]
    ForeignKeyTypeMismatch {
        /// The referencing table.
        table: String,
        /// The offending constraint.
        foreign_key: String,
        /// The referenced table.
        parent: String,
        /// The referencing column.
        column: String,
        /// The parent key column's type.
        expected: ValueType,
        /// The referencing column's type.
        actual: ValueType,
    },

    /// A row failed one or more `CHECK` constraints.
    ///
    /// **Every** failing check, not the first. A form with four bad fields
    /// should cost one round trip, which was the headline feature of
    /// ActiveRecord validations and the reason this is a list.
    ///
    /// `violations` is never empty — an empty one would render as "row
    /// violates 0 checks", which is not a thing that can happen and would mean
    /// a bug here rather than in the caller's data.
    #[error("{}", render_violations(table, violations))]
    CheckViolation {
        /// The table written to.
        table: String,
        /// Every constraint that refused the row, in declaration order.
        violations: Vec<CheckFailure>,
    },

    /// A write referenced a parent row that is not there.
    ///
    /// A parent row the caller's policy hides is not there *for them*, and
    /// reports identically to one that does not exist — otherwise the
    /// constraint would answer "does this row exist?" for rows they cannot
    /// read.
    #[error(
        "foreign key `{foreign_key}` on table `{table}`: \
         table `{parent}` has no such row, or none this caller can read"
    )]
    ForeignKeyViolation {
        /// The referencing table.
        table: String,
        /// The constraint that refused the row.
        foreign_key: String,
        /// The referenced table.
        parent: String,
    },

    /// A tenant-scoped table references a parent that is not tenant-scoped.
    ///
    /// The referential action would have to reach children in every tenant:
    /// `Cascade` deletes them and `Restrict` reads them, and both run as a
    /// superuser because referential integrity cannot depend on who is asking.
    /// So one tenant's delete would destroy or disclose another's rows.
    #[error(
        "table `{table}`'s foreign key `{foreign_key}` is tenant-scoped but its parent \
         `{parent}` is not, so ON DELETE {action:?} would reach rows in every tenant; \
         give the parent a tenant column, or drop the foreign key and check the \
         reference in the application"
    )]
    CrossTenantForeignKey {
        /// The tenant-scoped child.
        table: String,
        /// The foreign key.
        foreign_key: String,
        /// The shared parent.
        parent: String,
        /// The action that would cross the boundary.
        action: crate::ReferentialAction,
    },

    /// A delete was refused because rows still reference the row deleted.
    ///
    /// `retired` is the difference between a refusal an operator can act on and
    /// one that reads as the database lying. A `RESTRICT` edge blocks on a
    /// soft-deleted child — it is still a child, and its reference is still
    /// there — but an ordinary read of that table returns nothing, so somebody
    /// told only "rows still reference it" looks, finds an empty result, and
    /// concludes the constraint is wrong. The message has to say which case it
    /// is, because the two need opposite responses: delete the children, or
    /// *purge* them.
    #[error(
        "cannot delete from `{table}`: {}rows in `{child}` still reference it \
         through foreign key `{foreign_key}`{}",
        if *retired { "soft-deleted " } else { "" },
        if *retired {
            ". A retired row still holds its reference and an ordinary read \
             will not show it; purge it, or restore it and deal with it"
        } else {
            ""
        }
    )]
    ForeignKeyRestricted {
        /// The table being deleted from.
        table: String,
        /// The table still referencing it.
        child: String,
        /// The constraint that refused the delete.
        foreign_key: String,
        /// Whether the rows that blocked it are ones a soft delete retired.
        ///
        /// True only when *every* blocker found was retired. A mixture reports
        /// as the ordinary case, because the live ones are what the caller
        /// should deal with first and they are the ones they can see.
        retired: bool,
    },

    /// A cascading delete would remove more rows than the limit allows.
    ///
    /// Reported rather than absorbed: the whole cascade is held in memory and
    /// committed atomically, so an unbounded one is bounded by the allocator
    /// instead.
    #[error("deleting from `{table}` would cascade to more than {limit} rows")]
    CascadeTooLarge {
        /// The table the delete started at.
        table: String,
        /// The limit that was passed.
        limit: usize,
    },

    /// A column was added in a later schema version without being nullable, so
    /// rows written before it cannot be read back.
    #[error(
        "column `{column}` was added to table `{table}` after schema version {written}, \
         so it must be nullable to read rows written then"
    )]
    IncompatibleSchemaEvolution {
        /// The table the row belongs to.
        table: String,
        /// The column missing from the stored row.
        column: String,
        /// Schema version the stored row was written at.
        written: u32,
    },
}

/// Convenience alias for schema results.
pub type Result<T> = core::result::Result<T, SchemaError>;
