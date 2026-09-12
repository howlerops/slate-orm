//! Table, column and index definitions.
//!
//! Schemas are values, built once and then read-only. The derive macro emits
//! calls to the same builders a hand-written schema uses, so there is one
//! definition path and one set of validation rules.

use crate::error::{Result, SchemaError};
use slate_tuple::{Direction, ValueType};

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
    /// reads back as null — which is why such a column must be nullable.
    #[must_use]
    pub const fn added_in(&self) -> u32 {
        self.added_in
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

/// A secondary index.
///
/// Index entries live in their own key prefix and are written in the same
/// transaction as the row they describe, so an index can never lag the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDef {
    id: IndexId,
    name: String,
    columns: Vec<IndexColumn>,
    unique: bool,
}

impl IndexDef {
    /// Start defining an index.
    #[must_use]
    pub fn builder(name: impl Into<String>, id: IndexId) -> IndexBuilder {
        IndexBuilder {
            id,
            name: name.into(),
            columns: Vec::new(),
            unique: false,
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
    #[must_use]
    pub fn directions(&self) -> Vec<Direction> {
        self.columns.iter().map(|c| c.direction).collect()
    }
}

/// Builder for [`IndexDef`]. Column names are resolved when the table is built.
#[derive(Debug, Clone)]
pub struct IndexBuilder {
    id: IndexId,
    name: String,
    columns: Vec<(String, Direction)>,
    unique: bool,
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
    #[must_use]
    pub const fn unique(mut self) -> Self {
        self.unique = true;
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
    /// Columns stored in the row body, precomputed.
    ///
    /// Decoding a row walks this list, so building it per decode meant an
    /// allocation on every row of every scan.
    body_columns: Vec<Ordinal>,
    indexes: Vec<IndexDef>,
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
            tenant_column: None,
            schema_version: 0,
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
    #[must_use]
    pub fn ordinal_of(&self, name: &str) -> Option<Ordinal> {
        self.columns
            .iter()
            .position(|c| c.name == name)
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
        self.key_types(&index.columns.iter().map(|c| c.ordinal).collect::<Vec<_>>())
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

    /// Columns stored in the row body, in ordinal order.
    ///
    /// Primary key columns are omitted: they are already in the key, and
    /// reconstructing them from it costs less than storing them twice.
    #[must_use]
    pub fn body_columns(&self) -> &[Ordinal] {
        &self.body_columns
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
    tenant_column: Option<String>,
    schema_version: u32,
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
    /// as null and must be nullable.
    #[must_use]
    pub fn added_column(self, name: impl Into<String>, ty: ValueType, added_in: u32) -> Self {
        self.push_column(name, ty, true, added_in)
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
    pub fn build(self) -> Result<TableDef> {
        let table = &self.name;

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
            if spec.columns.is_empty() {
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
            });
        }

        let body_columns = (0..self.columns.len())
            .map(Ordinal)
            .filter(|o| !primary_key.contains(o))
            .collect();

        Ok(TableDef {
            id: self.id,
            name: self.name,
            columns: self.columns,
            primary_key,
            body_columns,
            indexes,
            tenant_column,
            schema_version: self.schema_version,
        })
    }
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
