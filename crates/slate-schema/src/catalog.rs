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
    ///
    /// So must *index* ids, and across the whole catalog rather than within a
    /// table. An index entry is keyed on the index id alone — see
    /// `slate_kernel::keys::index_entry`, which writes the index space and the
    /// id and no table id — so the index keyspace is global while the only
    /// check that existed was `TableBuilder`'s, which is per table. Two tables
    /// declaring the same `IndexId` therefore shared one key range, and a scan
    /// of either walked both: measured, before this check existed, as an index
    /// scan of a three-row table returning five rows belonging to another one.
    ///
    /// Checked here rather than fixed by putting the table id in the key. That
    /// would be the deeper fix and it is a storage format change — every index
    /// entry ever written moves — where this is a startup refusal that costs
    /// nothing and makes the collision unrepresentable. The key layout is
    /// documented as it is, and a schema that cannot state the broken thing
    /// does not need the format to defend against it.
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
        for index in table.indexes() {
            if let Some((owner, clashing)) = self.tables.iter().find_map(|t| {
                t.indexes()
                    .iter()
                    .find(|i| i.id() == index.id())
                    .map(|i| (t, i))
            }) {
                return Err(SchemaError::DuplicateIndexId {
                    id: index.id().0,
                    index: index.name().to_owned(),
                    table: table.name().to_owned(),
                    existing: clashing.name().to_owned(),
                    existing_table: owner.name().to_owned(),
                });
            }
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

                // A referential action from a *shared* parent to a
                // tenant-scoped child cannot be honoured by one tenant.
                //
                // `delete`'s doc comment bounds the cascade search with "when
                // the child is tenant-scoped its foreign key carries the
                // tenant, so the search is confined to the caller's own tenant
                // by the key encoding". That is true only when the *parent* is
                // tenant-scoped too, because then the child's key begins with
                // the tenant it inherited. A parent with no tenant column is
                // shared, its children live in every tenant, and the closure
                // walk — which runs as a superuser, correctly, since
                // referential integrity cannot depend on who is asking —
                // reaches all of them. Measured: tenant A deleting a shared
                // parent row destroyed tenant B's children, and `Restrict`
                // refused the delete in a way that disclosed that B's row
                // exists.
                //
                // Refused at build rather than confined at the scan, which was
                // the other candidate. Confining the walk to the caller's
                // tenant stops the destruction and leaves the other tenants'
                // children pointing at a parent that is gone — trading a
                // security hole for a correctness one. The edge is not
                // expressible safely by either action: `Cascade` writes rows
                // the caller cannot name and `Restrict` reads them. Saying so
                // at startup, naming both tables, is the honest answer.
                if table.tenant_column().is_some() && parent.tenant_column().is_none() {
                    return Err(SchemaError::CrossTenantForeignKey {
                        table: table.name().to_owned(),
                        foreign_key: key.name().to_owned(),
                        parent: parent.name().to_owned(),
                        action: key.on_delete(),
                    });
                }
            }
        }
        Ok(())
    }
}
