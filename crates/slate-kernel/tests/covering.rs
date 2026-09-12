//! Index-only scans: when the row lookup can be skipped, and when it must not
//! be.
//!
//! The win is measured in point reads rather than in time, because that is the
//! quantity the optimisation actually changes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Access, Action, CmpOp, Expr, Grant, Policy, Principal, Projection, Query, RecordStore, Scalar,
    ScanOrder, SecurityCatalog, SecurityContext, plan_projected,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;

const NOTES: TableId = TableId(1);
const ROWS: u64 = 200;

fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .column("body", ValueType::Str)
        .nullable_column("owner", ValueType::U64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        // Holds kind and size, so a query over just those plus the key needs no
        // row read.
        .index(
            IndexDef::builder("by_kind_size", IndexId(10))
                .column("kind")
                .column("size"),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    notes().ordinal_of(name).expect("column exists")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(1),
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 4)),
        Value::I64(id as i64),
        Value::Str("a body that would be a waste to fetch".to_owned()),
        Value::U64(id % 3),
    ])
}

async fn store(
    security: SecurityCatalog,
) -> (RecordStore<LatencyStore<MemoryStore>>, Arc<IoCounters>) {
    let catalog = Catalog::from_tables([notes()]).expect("catalog");
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let table = notes();
    let root = SecurityContext::superuser();

    let txn = loader.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root, &table, &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();

    let slow = LatencyStore::new(backing, LatencyProfile::free());
    let counters = slow.counters();
    (RecordStore::new(slow, catalog, security), counters)
}

fn open() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("r", NOTES, Action::ALL))
}

fn reader() -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(1))
            .with_role("r"),
    )
}

/// The point of the whole thing: a query an index can answer does no row reads.
#[tokio::test]
async fn a_covered_query_reads_no_rows() {
    let (store, counters) = store(open()).await;
    let table = notes();

    counters.reset();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query_projected(
            &reader(),
            &table,
            Expr::eq(col("kind"), Value::Str("kind-1".into())),
            ScanOrder::Ascending,
            &Projection::Columns(vec![col("id"), col("size")]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), (ROWS / 4) as usize);
    assert_eq!(
        counters.gets(),
        0,
        "a covered query should read no rows at all"
    );

    // The projected columns are populated from the index entry.
    for row in &rows {
        assert!(matches!(row.values()[col("id").0], Value::U64(_)));
        assert!(matches!(row.values()[col("size").0], Value::I64(_)));
        // And the ones not asked for are absent rather than wrong.
        assert_eq!(row.values()[col("body").0], Value::Null);
    }
}

/// Asking for a column the index lacks means the rows must be read after all —
/// whether by a table scan or by an index scan plus a lookup, but never by
/// pretending the index holds it.
#[tokio::test]
async fn an_uncovered_projection_is_not_answered_from_the_index() {
    let (store, _) = store(open()).await;
    let table = notes();
    let filter = Expr::eq(col("kind"), Value::Str("kind-1".into()));
    let projection = Projection::Columns(vec![col("body")]);

    let chosen = plan_projected(&table, &filter, ScanOrder::Ascending, &projection);
    assert!(
        !matches!(chosen.access, Access::IndexScan { covering: true, .. }),
        "the body is not in the index, so the plan must read rows: {:?}",
        chosen.access
    );

    let txn = store.begin().await.unwrap();
    let rows = txn
        .query_projected(&reader(), &table, filter, ScanOrder::Ascending, &projection)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), (ROWS / 4) as usize);
    for row in &rows {
        // The value is real, not the null a covering scan would have produced.
        assert!(matches!(row.values()[col("body").0], Value::Str(_)));
    }
}

/// Counting is the case with nothing to project at all.
#[tokio::test]
async fn counting_through_an_index_reads_nothing() {
    let (store, counters) = store(open()).await;
    let table = notes();

    counters.reset();
    let txn = store.begin().await.unwrap();
    let matched = txn
        .query_projected(
            &reader(),
            &table,
            Expr::eq(col("kind"), Value::Str("kind-2".into())),
            ScanOrder::Ascending,
            &Projection::none(),
        )
        .await
        .unwrap()
        .count()
        .await
        .unwrap();

    assert_eq!(matched, (ROWS / 4) as usize);
    assert_eq!(counters.gets(), 0);
}

/// The security filter is part of the predicate, so a policy on a column the
/// index lacks has to prevent the optimisation rather than be skipped by it.
#[tokio::test]
async fn a_policy_on_an_uncovered_column_prevents_the_optimisation() {
    let owner = notes().ordinal_of("owner").expect("column");
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", NOTES, Action::ALL))
        .policy(Policy::new(
            "own_notes",
            NOTES,
            [Action::Read],
            move |ctx: &SecurityContext| {
                // `owner` is not in the index.
                Expr::eq(owner, ctx.principal().id.clone())
            },
        ));
    let (store, _) = store(security).await;
    let table = notes();

    // The caller asks only for covered columns, but the policy reads `owner`,
    // so the plan may not answer from the index alone.
    let caller_filter = Expr::eq(col("kind"), Value::Str("kind-1".into()));
    let secured = caller_filter.clone().and(Expr::eq(owner, Value::U64(1)));
    let projection = Projection::Columns(vec![col("id"), col("size")]);
    let chosen = plan_projected(&table, &secured, ScanOrder::Ascending, &projection);
    assert!(
        !matches!(chosen.access, Access::IndexScan { covering: true, .. }),
        "a policy column outside the index must block the optimisation: {:?}",
        chosen.access
    );

    let txn = store.begin().await.unwrap();
    let rows = txn
        .query_projected(
            &reader(),
            &table,
            caller_filter,
            ScanOrder::Ascending,
            &projection,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // And the policy actually filtered, which it could not have done from the
    // index entry alone.
    assert!(!rows.is_empty());
    assert!(
        rows.len() < (ROWS / 4) as usize,
        "the policy filtered nothing"
    );
}

/// A predicate column outside the index also prevents it.
#[tokio::test]
async fn a_filter_on_an_uncovered_column_prevents_the_optimisation() {
    let table = notes();
    let filter = Expr::eq(col("kind"), Value::Str("kind-1".into())).and(Expr::compare(
        col("body"),
        CmpOp::Ne,
        Value::Str("x".into()),
    ));

    let chosen = plan_projected(
        &table,
        &filter,
        ScanOrder::Ascending,
        &Projection::Columns(vec![col("id")]),
    );
    match chosen.access {
        Access::IndexScan { covering, .. } => assert!(
            !covering,
            "the predicate reads `body`, which the index does not hold"
        ),
        // A table scan is also a correct answer here; what must not happen is a
        // covering index scan.
        other => assert!(matches!(other, Access::TableScan { .. }), "got {other:?}"),
    }
}

#[tokio::test]
async fn asking_for_everything_is_not_covered_by_a_partial_index() {
    let table = notes();
    let chosen = plan_projected(
        &table,
        &Expr::eq(col("kind"), Value::Str("kind-1".into())),
        ScanOrder::Ascending,
        &Projection::All,
    );
    match chosen.access {
        Access::IndexScan { covering, .. } => assert!(!covering),
        other => assert!(matches!(other, Access::TableScan { .. }), "got {other:?}"),
    }
}

/// Whatever the projection, the rows returned must be the rows a full read
/// would have returned.
#[tokio::test]
async fn a_projection_never_changes_which_rows_match() {
    let (store, _) = store(open()).await;
    let table = notes();
    let filter = Expr::eq(col("kind"), Value::Str("kind-3".into())).and(Expr::compare(
        col("size"),
        CmpOp::Ge,
        Value::I64(100),
    ));

    let ids = |rows: Vec<Row>| -> Vec<u64> {
        rows.into_iter()
            .map(|r| match r.values()[col("id").0] {
                Value::U64(v) => v,
                ref other => panic!("id was {other:?}"),
            })
            .collect()
    };

    let txn = store.begin().await.unwrap();
    let full = txn
        .query(&reader(), &table, filter.clone(), ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let projected = txn
        .query_projected(
            &reader(),
            &table,
            filter,
            ScanOrder::Ascending,
            &Projection::Columns(vec![col("id")]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert!(!full.is_empty());
    assert_eq!(ids(full), ids(projected));
}

/// A covering scan that also has to *compute* something evaluates it from the
/// columns the entry carries, and the column it read is not part of the answer.
///
/// The ordinary-index half of what teaching the executor to cover an expression
/// index changed. The planner used to expand `lower(kind)` into `kind` before
/// it knew which index it was asking about *and* insist on the computed
/// ordinal, which no index holds, so no index ever covered a query that
/// computed anything. Now `by_kind_size` holds `kind` and can.
///
/// A differential against a forced table scan, comparing whole rows, because
/// the two ways to get this wrong both produce a plausible row: computing from
/// a `kind` the entry never filled in gives null, and returning the `kind` the
/// entry did fill in gives a column the projection did not ask for — and a
/// table scan would return the other answer in each case.
#[tokio::test]
async fn a_covering_scan_computes_from_the_entry_it_holds() {
    let (store, counters) = store(open()).await;
    let table = notes();
    let filter = Expr::eq(col("kind"), Value::Str("kind-1".into()));
    let query = Query::all().filter(filter).select([col("id")]).computing([
        Scalar::Upper(Box::new(Scalar::Column(col("kind")))),
        // Over a column the predicate does *not* read, so it is in the
        // answer for no reason but the computation — and must therefore
        // not be in the answer at all.
        Scalar::Add(
            Box::new(Scalar::Column(col("size"))),
            Box::new(Scalar::Literal(Value::I64(1))),
        ),
    ]);

    counters.reset();
    let txn = store.begin().await.unwrap();
    // Explained rather than planned directly, because the plan that runs is
    // the one with the security filter folded in — and on a tenant-scoped
    // table it is the policy's own tenant term that makes the index usable at
    // all.
    let explained = txn.explain(&reader(), &table, &query).unwrap();
    assert!(
        explained.access.to_string().contains("Index Only"),
        "`by_kind_size` holds `kind`, so it can compute `upper(kind)`: {explained}"
    );
    let covered: Vec<Vec<Value>> = txn
        .execute(&reader(), &table, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.values().to_vec())
        .collect();
    assert_eq!(
        counters.gets(),
        0,
        "the entry holds everything this query reads"
    );

    let scanned: Vec<Vec<Value>> = txn
        .execute(&reader(), &table, &query.clone().using_table_scan())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.values().to_vec())
        .collect();

    assert_eq!(covered.len(), (ROWS / 4) as usize);
    assert_eq!(covered, scanned, "the two paths must return the same rows");
    let width = table.columns().len();
    for row in &covered {
        assert_eq!(
            row[width],
            Value::Str("KIND-1".to_owned()),
            "`upper(kind)` computed from the value the entry carries"
        );
        let Value::I64(bumped) = row[width + 1] else {
            panic!("`size + 1` should be an integer, got {:?}", row[width + 1]);
        };
        // `kind` comes back because the predicate reads it — the documented
        // rule for a projection — while `size` was read only to compute
        // `size + 1` and is not part of the answer. The distinction is the
        // whole reason an expression index can cover anything: an entry keyed
        // on a computed value cannot produce the column underneath it, so
        // nothing else may either.
        assert_eq!(row[col("kind").0], Value::Str("kind-1".to_owned()));
        assert_eq!(row[col("size").0], Value::Null, "read only to compute");
        let Value::U64(id) = row[col("id").0] else {
            panic!("id was {:?}", row[col("id").0]);
        };
        assert_eq!(bumped, id as i64 + 1);
    }
}
