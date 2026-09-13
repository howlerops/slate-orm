//! Fixture rows, and the statistics gathered after them.
//!
//! # Why this is a command-line flag and not a configuration section
//!
//! Seeding writes rows with [`SecurityContext::superuser`], which bypasses
//! RBAC, the tenant restriction and every policy. There is no way around that:
//! the rows have to land before the grants they will be read under exist, and
//! a fixture that could only write what some configured principal may write
//! would be a fixture that cannot set up the interesting cases.
//!
//! So it is deliberately *not* reachable from the configuration file. A file
//! is a thing that gets copied between environments, and a copied file that
//! quietly re-seeds a database — as a superuser — is a failure mode worth
//! designing out. `--seed <file>` is an act somebody performs, on a command
//! line, once.
//!
//! It is also the only reason the word `superuser` appears in this crate. The
//! kernel's own argument for the named constructor is that its value is being
//! one grep away; that grep finds exactly two call sites here, both in this
//! file, both at startup, neither reachable from the wire.
//!
//! # Rows are written by column name
//!
//! ```toml
//! [[seed]]
//! table = "docs"
//! rows = [
//!   { id = 1, kind = "kind-a", size = 5, note = "first" },
//!   { id = 2, kind = "kind-a", size = 15 },          # note is null
//! ]
//! ```
//!
//! Positional rows were the obvious alternative — the kernel's [`Row`] is
//! positional, and it would be less code — and were rejected for the reason
//! the wire protocol gives for `ColumnRef`: a column added to the middle of a
//! table silently re-points every value after it, and the result is a fixture
//! that loads without error and means something else.
//!
//! Order is insert order, so a table with a foreign key comes after the table
//! it references. Sorting by dependency was considered and left out: the
//! dependency graph can have a cycle that the referential actions make
//! perfectly legal, and a loader that reordered would then have to explain
//! itself.

use crate::error::{Fault, Started};
use crate::value;
use serde::Deserialize;
use slate_kernel::{KvStore, RecordStore, SecurityCatalog, SecurityContext, Statistics};
use slate_schema::{Catalog, Row, TableDef};
use std::path::Path;
use std::sync::Arc;

/// A seed file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fixture {
    /// Tables to fill, in insert order.
    #[serde(default)]
    pub(crate) seed: Vec<Batch>,
}

/// Rows for one table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Batch {
    /// The table's name.
    pub(crate) table: String,
    /// Rows, each a map from column name to value.
    pub(crate) rows: Vec<toml::value::Table>,
}

impl Fixture {
    /// Read a seed file.
    pub(crate) fn read(path: &Path) -> Started<Self> {
        let text = std::fs::read_to_string(path).map_err(|why| {
            Fault::new(format!(
                "cannot read the seed file `{}`: {why}",
                path.display()
            ))
        })?;
        toml::from_str(&text).map_err(|why| {
            Fault::new(format!(
                "`{}` is not a valid seed file:\n{why}",
                path.display()
            ))
        })
    }

    /// How many rows this fixture holds, for the banner.
    pub(crate) fn row_count(&self) -> usize {
        self.seed.iter().map(|batch| batch.rows.len()).sum()
    }
}

/// Insert every row, as a superuser, in one transaction.
///
/// One transaction rather than one per table, so a fixture with a foreign key
/// violation leaves nothing behind: a half-loaded fixture is worse than none,
/// because the tests that run against it pass some of the time.
pub(crate) async fn load<S: KvStore>(
    store: &RecordStore<Arc<S>>,
    catalog: &Catalog,
    fixture: &Fixture,
) -> Started<()> {
    let root = SecurityContext::superuser();
    let transaction = store
        .begin()
        .await
        .map_err(|why| Fault::new(format!("cannot begin the seeding transaction: {why}")))?;

    for batch in &fixture.seed {
        let table = catalog.table_by_name(&batch.table).ok_or_else(|| {
            Fault::new(format!(
                "the seed file names table `{}`, which this configuration does not serve",
                batch.table
            ))
        })?;
        let rows: Vec<Row> = batch
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                build(table, row)
                    .map_err(|fault| fault.within(format!("table `{}`, row {index}", batch.table)))
            })
            .collect::<Started<_>>()?;

        transaction
            .insert_many(&root, table, &rows)
            .await
            .map_err(|why| Fault::new(format!("cannot seed table `{}`: {why}", batch.table)))?;
    }

    transaction
        .commit()
        .await
        .map_err(|why| Fault::new(format!("cannot commit the seeded rows: {why}")))?;
    Ok(())
}

/// Turn one `{ column = value }` map into a row.
fn build(table: &TableDef, given: &toml::value::Table) -> Started<Row> {
    // Names first, so a typo is caught by name rather than showing up as a
    // missing value in a column the writer never mentioned.
    for name in given.keys() {
        if table.ordinal_of(name).is_none() {
            return Err(Fault::new(format!(
                "there is no column `{name}` on this table"
            )));
        }
    }

    let mut values = Vec::with_capacity(table.columns().len());
    for column in table.columns() {
        let supplied = given
            .iter()
            .find(|(name, _)| column.answers_to(name))
            .map(|(_, value)| value);

        let value = match supplied {
            Some(value) => value::from_toml(
                value,
                column.value_type(),
                &format!("column `{}`", column.name()),
            )?,
            // A dropped column holds nothing and a nullable one holds null; a
            // column with a default gets it. Anything else is a row that
            // cannot be written, said now rather than by the record store.
            None if column.is_dropped() || column.is_nullable() => slate_tuple::Value::Null,
            None => match column.default_value() {
                Some(default) => default.clone(),
                None => {
                    return Err(Fault::new(format!(
                        "column `{}` is not nullable and has no default, and this row does not give it a value",
                        column.name()
                    )));
                }
            },
        };
        values.push(value);
    }
    Ok(Row::new(values))
}

/// Read every table and record what the planner should believe.
///
/// A superuser read, because statistics gathered under a restrictive policy
/// describe that slice of the table rather than the table — which the kernel's
/// own `analyze` documentation points out, and which would make the planner's
/// row counts depend on whose statistics happened to be gathered last.
pub(crate) async fn analyze<S: KvStore>(
    store: &RecordStore<Arc<S>>,
    catalog: &Catalog,
) -> Started<Statistics> {
    let root = SecurityContext::superuser();
    let transaction = store
        .begin()
        .await
        .map_err(|why| Fault::new(format!("cannot begin the analysis transaction: {why}")))?;
    let mut statistics = Statistics::new();
    for table in catalog.tables() {
        let stats = transaction
            .analyze(&root, table)
            .await
            .map_err(|why| Fault::new(format!("cannot analyze `{}`: {why}", table.name())))?;
        statistics.set(table.id(), stats);
    }
    // Rolled back rather than committed: `analyze` reads, and a transaction
    // left open would hold a snapshot for the life of the process.
    transaction.rollback();
    Ok(statistics)
}

/// The record store the two startup jobs above run through.
pub(crate) fn store_for<S: KvStore>(
    writer: &Arc<S>,
    catalog: &Catalog,
    security: &SecurityCatalog,
) -> RecordStore<Arc<S>> {
    RecordStore::new(Arc::clone(writer), catalog.clone(), security.clone())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use slate_kernel::memory::MemoryStore;
    use slate_kernel::{Action, Grant, Query};
    use slate_schema::{Ordinal, TableId};
    use slate_tuple::{Value, ValueType};

    fn catalog() -> Catalog {
        Catalog::from_tables([TableDef::builder("docs", TableId(1))
            .column("id", ValueType::U64)
            .column("kind", ValueType::Str)
            .nullable_column("note", ValueType::Str)
            .column("region", ValueType::Str)
            .default_for("region", Value::Str("eu".into()))
            .primary_key(["id"])
            .build()
            .unwrap()])
        .unwrap()
    }

    fn fixture(text: &str) -> Fixture {
        toml::from_str(text).unwrap_or_else(|e| panic!("{e}"))
    }

    async fn loaded(text: &str) -> Started<Vec<Row>> {
        let catalog = catalog();
        let security = SecurityCatalog::new().grant(Grant::new("app", TableId(1), Action::ALL));
        let backing = Arc::new(MemoryStore::new());
        let store = store_for(&backing, &catalog, &security);
        load(&store, &catalog, &fixture(text)).await?;

        let transaction = store.begin().await.unwrap_or_else(|e| panic!("{e}"));
        let table = catalog.table_by_name("docs").unwrap();
        let mut cursor = transaction
            .execute(&SecurityContext::superuser(), table, &Query::all())
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().await.unwrap_or_else(|e| panic!("{e}")) {
            rows.push(row);
        }
        Ok(rows)
    }

    #[tokio::test]
    async fn rows_are_written_by_name_and_read_back() {
        let rows = loaded(
            r#"
[[seed]]
table = "docs"
rows = [
  { id = 1, kind = "a", note = "first", region = "uk" },
  { kind = "b", id = 2 },
]
"#,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get(Ordinal(1)), Some(&Value::Str("a".into())));
        // Order in the file does not matter, because the key is the name.
        assert_eq!(rows[1].get(Ordinal(0)), Some(&Value::U64(2)));
        // Omitted nullable column is null; omitted column with a default is
        // the default.
        assert_eq!(rows[1].get(Ordinal(2)), Some(&Value::Null));
        assert_eq!(rows[1].get(Ordinal(3)), Some(&Value::Str("eu".into())));
    }

    #[tokio::test]
    async fn a_misspelled_column_is_refused_by_name() {
        let error = loaded("[[seed]]\ntable = \"docs\"\nrows = [{ id = 1, knid = \"a\" }]\n")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("no column `knid`"), "{error}");
        assert!(error.contains("row 0"), "{error}");
    }

    #[tokio::test]
    async fn a_missing_non_nullable_column_with_no_default_is_refused() {
        let error = loaded("[[seed]]\ntable = \"docs\"\nrows = [{ id = 1 }]\n")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not nullable and has no default"), "{error}");
    }

    #[tokio::test]
    async fn a_value_of_the_wrong_type_is_refused() {
        let error = loaded("[[seed]]\ntable = \"docs\"\nrows = [{ id = \"one\", kind = \"a\" }]\n")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("a string"), "{error}");
    }

    #[tokio::test]
    async fn a_table_that_is_not_served_is_refused() {
        let error = loaded("[[seed]]\ntable = \"nope\"\nrows = []\n")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not serve"), "{error}");
    }

    #[tokio::test]
    async fn a_failed_batch_leaves_nothing_behind() {
        // The second table's row is bad, and the first table's rows must not
        // survive: a half-loaded fixture is worse than an empty one.
        let catalog = catalog();
        let security = SecurityCatalog::new();
        let backing = Arc::new(MemoryStore::new());
        let store = store_for(&backing, &catalog, &security);
        let bad = fixture(
            "[[seed]]\ntable = \"docs\"\nrows = [{ id = 1, kind = \"a\" }]\n\
             [[seed]]\ntable = \"docs\"\nrows = [{ id = 2 }]\n",
        );
        assert!(load(&store, &catalog, &bad).await.is_err());

        let transaction = store.begin().await.unwrap_or_else(|e| panic!("{e}"));
        let count = transaction
            .count(
                &SecurityContext::superuser(),
                catalog.table_by_name("docs").unwrap(),
                &Query::all(),
            )
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn analyze_measures_what_was_seeded() {
        let catalog = catalog();
        let security = SecurityCatalog::new();
        let backing = Arc::new(MemoryStore::new());
        let store = store_for(&backing, &catalog, &security);
        load(
            &store,
            &catalog,
            &fixture(
                "[[seed]]\ntable = \"docs\"\nrows = [{ id = 1, kind = \"a\" }, { id = 2, kind = \"b\" }]\n",
            ),
        )
        .await
        .unwrap();
        let statistics = analyze(&store, &catalog).await.unwrap();
        assert!(statistics.has(TableId(1)));
        let table = catalog.table_by_name("docs").unwrap();
        assert_eq!(statistics.table(table).row_count, 2);
    }
}
