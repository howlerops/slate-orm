//! A set of tables, checked for id and name collisions.

use crate::constraint::ForeignKeyDef;
use crate::error::{Result, SchemaError};
use crate::table::{TableDef, TableId};

/// The tables a store knows about.
///
/// The catalog is also the RBAC enforcement point's view of the world: access
/// checks name tables, so they resolve through here before a plan runs.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    tables: Vec<TableDef>,
}

impl Catalog {
    /// An empty catalog.
    #[must_use]
    pub const fn new() -> Self {
        Self { tables: Vec::new() }
    }

    /// Build a catalog from tables, rejecting duplicate ids or names and
    /// checking every foreign key against the table it points at.
    pub fn from_tables<I: IntoIterator<Item = TableDef>>(tables: I) -> Result<Self> {
        let mut catalog = Self::new();
        for table in tables {
            catalog.insert(table)?;
        }
        catalog.validate_foreign_keys()?;
        Ok(catalog)
    }

    /// Add a table.
    ///
    /// Ids and names must both be unique: an id collision would overlay two
    /// tables in the same keyspace, and a name collision would make access
    /// rules ambiguous.
    pub fn insert(&mut self, table: TableDef) -> Result<()> {
        if let Some(existing) = self
            .tables
            .iter()
            .find(|t| t.id() == table.id() || t.name() == table.name())
        {
            return Err(SchemaError::DuplicateTable {
                table: existing.name().to_owned(),
            });
        }
        self.tables.push(table);
        Ok(())
    }

    /// Look up a table by id.
    #[must_use]
    pub fn table(&self, id: TableId) -> Option<&TableDef> {
        self.tables.iter().find(|t| t.id() == id)
    }

    /// Look up a table by name.
    #[must_use]
    pub fn table_by_name(&self, name: &str) -> Option<&TableDef> {
        self.tables.iter().find(|t| t.name() == name)
    }

    /// Every table in the catalog.
    #[must_use]
    pub fn tables(&self) -> &[TableDef] {
        &self.tables
    }

    /// Every foreign key pointing at `parent`, with the table that declares it.
    ///
    /// A delete has to ask this of the whole catalog: a table does not know who
    /// references it, and a reference nobody looked for is a dangling row.
    #[must_use]
    pub fn referencing(&self, parent: TableId) -> Vec<(&TableDef, &ForeignKeyDef)> {
        self.tables
            .iter()
            .flat_map(|table| {
                table
                    .foreign_keys()
                    .iter()
                    .filter(move |key| key.parent() == parent)
                    .map(move |key| (table, key))
            })
            .collect()
    }

    /// Check every foreign key against the primary key it references.
    ///
    /// Not done by [`TableBuilder::build`](crate::TableBuilder::build), which
    /// cannot: the parent may not exist yet when the child is defined, and
    /// insisting it did would mean no table could reference itself or take part
    /// in a cycle. [`Catalog::from_tables`] runs this once every table is in;
    /// call it yourself after building a catalog with [`Catalog::insert`].
    pub fn validate_foreign_keys(&self) -> Result<()> {
        for table in &self.tables {
            for key in table.foreign_keys() {
                let parent = self.table(key.parent()).ok_or_else(|| {
                    SchemaError::UnknownForeignKeyParent {
                        table: table.name().to_owned(),
                        foreign_key: key.name().to_owned(),
                        parent: key.parent(),
                    }
                })?;

                // The reference is to the parent's whole primary key, because
                // that is what makes the check a point read. A partial one
                // would have to invent the rest of the key.
                let parent_key = parent.primary_key();
                if key.columns().len() != parent_key.len() {
                    return Err(SchemaError::ForeignKeyWidthMismatch {
                        table: table.name().to_owned(),
                        foreign_key: key.name().to_owned(),
                        parent: parent.name().to_owned(),
                        expected: parent_key.len(),
                        actual: key.columns().len(),
                    });
                }

                for (child_ordinal, parent_ordinal) in key.columns().iter().zip(parent_key) {
                    let (Some(child_column), Some(parent_column)) =
                        (table.column(*child_ordinal), parent.column(*parent_ordinal))
                    else {
                        continue;
                    };
                    // The child's values are encoded into a parent row key, so
                    // a type mismatch would look up a key no row can have —
                    // the constraint would refuse every write rather than
                    // enforce anything.
                    if child_column.value_type() != parent_column.value_type() {
                        return Err(SchemaError::ForeignKeyTypeMismatch {
                            table: table.name().to_owned(),
                            foreign_key: key.name().to_owned(),
                            parent: parent.name().to_owned(),
                            column: child_column.name().to_owned(),
                            expected: parent_column.value_type(),
                            actual: child_column.value_type(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}
