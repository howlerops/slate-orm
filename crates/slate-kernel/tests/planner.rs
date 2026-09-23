//! Statistics, cost, and being able to see what the planner decided.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    AccessSummary, Action, CmpOp, ColumnStats, Expr, Grant, Query, RecordStore, ScanOrder,
    SecurityCatalog, SecurityContext, Statistics, TableStats, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const T: TableId = TableId(1);
const ROWS: u64 = 400;

fn table() -> TableDef {
    TableDef::builder("events", T)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("at", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_kind", IndexId(10)).column("kind"))
        .index(IndexDef::builder("by_at", IndexId(11)).column("at"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("column exists")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        // Eight kinds, so an equality keeps an eighth of the table.
        Value::Str(format!("kind-{}", id % 8)),
        Value::I64(id as i64),
        // A quarter of the rows have no note.
        if id.is_multiple_of(4) {
            Value::Null
        } else {
            Value::Str("note".to_owned())
        },
    ])
}

async fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);

    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root, &table(), &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// Statistics have to describe the data, or they are worse than none.
#[tokio::test]
async fn analyze_describes_the_table() {
    let store = store().await;
    let txn = store.begin().await.unwrap();
    let stats = txn.analyze(&root(), &table()).await.unwrap();

    assert_eq!(stats.row_count, ROWS);
    assert_eq!(stats.column(col("kind")).distinct, 8);
    // Every id is distinct.
    assert_eq!(stats.column(col("id")).distinct, ROWS);
    // A quarter of the notes are null.
    let note = stats.column(col("note"));
    assert!((note.null_fraction - 0.25).abs() < 0.001, "got {note:?}");
    // And a null is not counted as a distinct value.
    assert_eq!(note.distinct, 1);

    // Selectivity follows from that: an eighth for `kind`.
    assert!((stats.equality_selectivity(col("kind")) - 0.125).abs() < 0.001);
}

/// The estimate should land near the truth on a table it has actually seen.
#[tokio::test]
async fn estimates_track_reality_after_analyzing() {
    let mut store = store().await;
    let txn = store.begin().await.unwrap();
    let stats = txn.analyze(&root(), &table()).await.unwrap();
    drop(txn);
    store.set_statistics(Statistics::new().with(T, stats));

    let query = Query::all().filter(Expr::eq(col("kind"), Value::Str("kind-3".into())));
    let txn = store.begin().await.unwrap();
    let explained = txn.explain(&root(), &table(), &query).unwrap();
    let actual = txn
        .execute(&root(), &table(), &query)
        .await
        .unwrap()
        .count()
        .await
        .unwrap();

    assert_eq!(actual, (ROWS / 8) as usize);
    let error = (explained.estimated_rows - actual as f64).abs() / actual as f64;
    assert!(
        error < 0.2,
        "estimate {} was far from the actual {actual}",
        explained.estimated_rows
    );
}

/// The whole reason for the cost model: an index that would cost more than a
/// scan must not be chosen.
#[tokio::test]
async fn an_index_that_costs_more_than_a_scan_is_not_chosen() {
    let mut store = store().await;
    let txn = store.begin().await.unwrap();
    let stats = txn.analyze(&root(), &table()).await.unwrap();
    drop(txn);
    store.set_statistics(Statistics::new().with(T, stats));

    // An eighth of 400 rows is 50 point reads, against scanning 400 — a bad
    // trade when a read costs a round trip and a scanned row does not.
    let query = Query::all().filter(Expr::eq(col("kind"), Value::Str("kind-3".into())));
    let txn = store.begin().await.unwrap();
    let explained = txn.explain(&root(), &table(), &query).unwrap();
    assert_eq!(
        explained.access,
        AccessSummary::TableScan,
        "expected a scan, got {explained}"
    );

    // Asking only for columns the index holds removes the point reads, and with
    // them the reason to avoid the index.
    let covered = query.clone().select([col("id"), col("kind")]);
    let explained = txn.explain(&root(), &table(), &covered).unwrap();
    assert!(
        explained.is_index_only(),
        "a covered query should use the index: {explained}"
    );
}

#[tokio::test]
async fn explain_names_the_path_and_shows_its_estimates() {
    let store = store().await;
    let txn = store.begin().await.unwrap();

    let point = txn
        .explain(
            &root(),
            &table(),
            &Query::all().filter(Expr::eq(col("id"), Value::U64(7))),
        )
        .unwrap();
    assert_eq!(point.access, AccessSummary::PointGet);
    assert!(point.to_string().starts_with("Point Get on events"));

    let scan = txn.explain(&root(), &table(), &Query::all()).unwrap();
    assert_eq!(scan.access, AccessSummary::TableScan);
    assert!(scan.estimated_cost > 0.0);

    let counted = txn
        .explain(
            &root(),
            &table(),
            &Query::all()
                .filter(Expr::eq(col("at"), Value::I64(3)))
                .count_only(),
        )
        .unwrap();
    assert!(
        matches!(counted.access, AccessSummary::IndexOnlyScan { ref index } if index == "by_at"),
        "got {counted}"
    );

    let windowed = txn
        .explain(
            &root(),
            &table(),
            &Query::all().limit(5).offset(10).descending(),
        )
        .unwrap();
    let rendered = windowed.to_string();
    assert!(rendered.contains("backwards"), "{rendered}");
    assert!(rendered.contains("limit=5"), "{rendered}");
    assert!(rendered.contains("offset=10"), "{rendered}");
}

#[tokio::test]
async fn limit_and_offset_window_the_results() {
    let store = store().await;
    let txn = store.begin().await.unwrap();

    let ids = |rows: Vec<Row>| -> Vec<u64> {
        rows.into_iter()
            .map(|r| match r.values()[0] {
                Value::U64(v) => v,
                ref other => panic!("id was {other:?}"),
            })
            .collect()
    };

    let page = txn
        .execute(&root(), &table(), &Query::all().limit(3).offset(2))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(ids(page), vec![2, 3, 4]);

    // Descending, the same window comes from the other end.
    let page = txn
        .execute(
            &root(),
            &table(),
            &Query::all().descending().limit(3).offset(2),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(ids(page), vec![ROWS - 3, ROWS - 4, ROWS - 5]);

    // An offset past the end yields nothing rather than failing.
    let empty = txn
        .execute(&root(), &table(), &Query::all().offset(10_000))
        .await
        .unwrap()
        .count()
        .await
        .unwrap();
    assert_eq!(empty, 0);
}

/// A query object should compose the same way the individual arguments did.
#[tokio::test]
async fn query_conditions_compose() {
    let store = store().await;
    let txn = store.begin().await.unwrap();

    let query = Query::all()
        .filter(Expr::compare(col("at"), CmpOp::Ge, Value::I64(100)))
        .and(Expr::compare(col("at"), CmpOp::Lt, Value::I64(110)))
        .order(ScanOrder::Ascending);

    let matched = txn
        .execute(&root(), &table(), &query)
        .await
        .unwrap()
        .count()
        .await
        .unwrap();
    assert_eq!(matched, 10);
}

/// Statistics gathered under a policy describe what the policy shows, which is
/// why analysing as a restricted principal is documented as the wrong thing.
#[tokio::test]
async fn analyze_sees_only_what_the_caller_can() {
    use slate_kernel::{Policy, Principal};

    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", T, Action::ALL))
        .policy(Policy::new("half", T, [Action::Read], |_: &_| {
            Expr::compare(
                table().ordinal_of("at").expect("column"),
                CmpOp::Lt,
                Value::I64(100),
            )
        }));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);

    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root, &table(), &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();

    let restricted = SecurityContext::new(Principal::new(Value::U64(1)).with_role("r"));
    let txn = store.begin().await.unwrap();
    assert_eq!(txn.analyze(&root, &table()).await.unwrap().row_count, ROWS);
    assert_eq!(
        txn.analyze(&restricted, &table()).await.unwrap().row_count,
        100
    );
}

#[test]
fn assumed_statistics_are_used_when_nothing_is_known() {
    let stats = TableStats::assumed();
    assert_eq!(stats.row_count, 1_000);
    // A column nobody has looked at still gets a usable guess.
    assert!((stats.equality_selectivity(Ordinal(0)) - 0.009).abs() < 0.001);
}

/// Overlapping reads costs the same as not overlapping them.
///
/// This test asserted the opposite until the model was measured against object
/// storage. The old rule was `ceil(n / depth)`: reads issued together land
/// together, so sixteen at depth sixteen cost one round trip. That is true of
/// *latency* and false of *work*, and the cost model's unit is work — the
/// requests the provider serves and bills. Concurrency does not make them
/// fewer.
///
/// Believing otherwise made index lookups look sixteen times cheaper than they
/// are, which, together with scans being overcharged eighty times, had the
/// planner preferring a plan that did 58x the object-store requests and took
/// 9x as long. See `slate-slatedb`'s `cost_calibration` example.
#[test]
fn overlapped_reads_cost_the_same_as_serial_ones() {
    use slate_kernel::stats::{POINT_READ_COST, pipelined_read_cost};

    assert_eq!(pipelined_read_cost(0.0, 16), 0.0, "nothing to read");
    for reads in [1.0, 16.0, 17.0, 100.0] {
        assert_eq!(
            pipelined_read_cost(reads, 16),
            reads * POINT_READ_COST,
            "{reads} reads at depth 16"
        );
        // Depth does not enter into it any more.
        assert_eq!(
            pipelined_read_cost(reads, 16),
            pipelined_read_cost(reads, 1),
            "depth changed the cost of {reads} reads"
        );
    }
}

/// Where a non-covering index scan stops being worth its lookups.
///
/// Both ends of this test now say "scan", which is the measured answer for a
/// ten-thousand-row table: see the note inside. The boundary has moved twice —
/// first from 1% to 6% when pipelining was added to the model, then off the
/// percentage scale entirely when the model was measured against object
/// storage rather than a latency fixture.
#[tokio::test]
async fn an_index_is_not_worth_its_lookups_on_a_small_table() {
    let filter = || Expr::eq(col("kind"), Value::Str("kind-1".into()));
    let with = |distinct: u64| {
        TableStats::with_row_count(10_000).with_column(
            col("kind"),
            ColumnStats {
                distinct,
                null_fraction: 0.0,
            },
        )
    };

    // Selective *as a percentage* is not the test any more. 500 distinct values
    // over ten thousand rows keeps 0.2% of them — twenty rows — and fetching
    // those twenty costs about twenty object-store requests, one apiece, where
    // the whole table is one or two. So the scan wins, and it really is
    // faster.
    //
    // The rule that replaced "a few percent" is absolute, not proportional:
    // fetching `k` rows beats scanning `n` only when `n > 8000k`, because a
    // scan returns ~8000 rows per request and a point read costs 1. A
    // percentage can never satisfy that, however small — `k = 0.002n` needs
    // `n > 16n`. (This said 24000 and 48 while `POINT_READ_COST` was 3.0;
    // #269 re-measured it at 1.0 and the ratio moved with it.) Indexes earn their keep on large tables and small absolute
    // result sets, which is a narrower claim than this test used to make.
    let selective = store()
        .await
        .with_statistics(Statistics::new().with(T, with(500)));
    let txn = selective.begin().await.unwrap();
    let plan = txn
        .explain(&root(), &table(), &Query::all().filter(filter()))
        .unwrap();
    assert!(
        matches!(plan.access, AccessSummary::TableScan),
        "twenty rows out of ten thousand is cheaper to scan, got {plan}"
    );

    // Broad: two distinct values means half the table, far past the point
    // where a lookup per row pays for itself.
    let broad = store()
        .await
        .with_statistics(Statistics::new().with(T, with(2)));
    let txn = broad.begin().await.unwrap();
    let plan = txn
        .explain(&root(), &table(), &Query::all().filter(filter()))
        .unwrap();
    assert!(
        matches!(plan.access, AccessSummary::TableScan),
        "half the table should be scanned, got {plan}"
    );
}

/// The latency fixture and the cost model must state the same rows per block.
///
/// They are different units — the fixture models elapsed time, the cost model
/// counts object-store requests — but they share one physical fact: how many
/// rows a single block fetch returns. When they disagree, every measurement of
/// a scan against an estimate compares two different beliefs, and the
/// disagreement looks like a planner bug.
///
/// `docs/performance.md` records this happening twice before. It happened a
/// third time the moment `SCAN_ROW_COST` was recalibrated against object
/// storage and the fixture was left alone, because the only thing tying them
/// together was a comment saying they had to match. This is that comment as an
/// assertion.
#[test]
fn the_fixture_and_the_cost_model_agree_on_rows_per_block() {
    use slate_kernel::latency::LatencyProfile;
    use slate_kernel::stats::SCAN_ROW_COST;

    let implied = 1.0 / SCAN_ROW_COST;
    let fixture = LatencyProfile::object_storage().rows_per_block as f64;
    assert!(
        (implied - fixture).abs() < 1.0,
        "the cost model implies {implied:.0} rows per block and the latency \
         fixture charges one every {fixture:.0}. Change both together, or a \
         scan measured against the fixture will not match its estimate for \
         reasons that have nothing to do with the planner."
    );
}
