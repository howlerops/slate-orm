//! A set of tables, checked for id and name collisions.

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

    /// Build a catalog from tables, rejecting duplicate ids or names.
    pub fn from_tables<I: IntoIterator<Item = TableDef>>(tables: I) -> Result<Self> {
        let mut catalog = Self::new();
        for table in tables {
            catalog.insert(table)?;
        }
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
}
