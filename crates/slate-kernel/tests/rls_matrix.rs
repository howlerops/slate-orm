//! Row-level security, checked once per access path.
//!
//! "Security is in the kernel" is a claim about *every* way a row can leave the
//! store, and it is only as strong as the weakest path. A policy honoured by
//! the table scan and skipped by, say, the top-N heap or the k-NN search is not
//! a partial success — it is a leak, and the paths that skip it are exactly the
//! ones added last and tested least.
//!
//! So this file is a matrix rather than a set of scenarios. Every single-table
//! read path appears in [`paths`] with the same question asked of it: run as a
//! user whose policy admits two of five rows, does it return those two and
//! nothing else? A new access path that is not added to that list is not
//! covered here, and the intent is that the list is the obvious place to look.
//!
//! Joins and chains have their own policy tests, next to the rest of their
//! behaviour, in `join.rs` and `chain.rs`.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Action, Aggregate, CmpOp, Expr, Grant, Metric, Policy, Principal, Projection, Query,
    RecordStore, Scalar, ScanOrder, SecurityCatalog, SecurityContext, SortKey, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use uuid::Uuid;

const DOCUMENTS: TableId = TableId(1);
const TENANT_A: u128 = 10;
const TENANT_B: u128 = 20;
const ALICE: u128 = 100;
const BOB: u128 = 200;

fn documents() -> TableDef {
    TableDef::builder("documents", DOCUMENTS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .nullable_column("owner_id", ValueType::Uuid)
        .column("title", ValueType::Str)
        .column("score", ValueType::I64)
        .nullable_column("embedding", ValueType::Vector)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_owner", IndexId(10)).column("owner_id"))
        .index(IndexDef::builder("by_score", IndexId(11)).column("score"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    documents().ordinal_of(name).expect("column exists")
}

/// Readers see only the documents they own.
fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("reader", DOCUMENTS, [Action::Read]))
        .policy(Policy::new(
            "own_documents",
            DOCUMENTS,
            Action::ALL,
            |ctx: &SecurityContext| Expr::eq(col("owner_id"), ctx.principal().id.clone()),
        ))
}

fn doc(tenant: u128, id: u64, owner: Option<u128>, title: &str, score: i64) -> Row {
    Row::new(vec![
        Value::Uuid(Uuid::from_u128(tenant)),
        Value::U64(id),
        owner.map_or(Value::Null, |o| Value::Uuid(Uuid::from_u128(o))),
        Value::Str(title.to_owned()),
        Value::I64(score),
        // Deliberately close together, so a k-NN search that ignored the policy
        // would rank a forbidden row above a permitted one rather than below.
        Value::Vector(vec![score as f32 * 0.01, 1.0]),
    ])
}

fn alice() -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::Uuid(Uuid::from_u128(ALICE)))
            .with_tenant(Value::Uuid(Uuid::from_u128(TENANT_A)))
            .with_role("reader"),
    )
}

/// Five rows, of which Alice's policy admits exactly two.
///
/// The three she must not see are chosen to break a different shortcut each:
/// another user's row in her tenant, a row with a *null* owner (which a negated
/// or coerced comparison would let through), and her own principal id in a
/// different tenant.
async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([documents()]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security());
    let table = documents();
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for row in [
        doc(TENANT_A, 1, Some(ALICE), "alice one", 10),
        doc(TENANT_A, 2, Some(ALICE), "alice two", 20),
        doc(TENANT_A, 3, Some(BOB), "bob one", 30),
        doc(TENANT_A, 4, None, "unowned", 40),
        doc(TENANT_B, 5, Some(ALICE), "other tenant", 50),
    ] {
        txn.insert(&root, &table, &row).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

/// What Alice is allowed to see, and nothing else.
const PERMITTED: [&str; 2] = ["alice one", "alice two"];

/// Titles that must never appear, whatever the path.
const FORBIDDEN: [&str; 3] = ["bob one", "unowned", "other tenant"];

fn titles(rows: &[Row]) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| match r.get(col("title")) {
            Some(Value::Str(s)) => s.clone(),
            other => panic!("title was {other:?}"),
        })
        .collect();
    out.sort();
    out
}

/// A read path's body: an async block, boxed so a plain `fn` pointer can hold
/// one and the list below can stay a flat table.
type Run = fn(
    &RecordStore<MemoryStore>,
    SecurityContext,
) -> std::pin::Pin<Box<dyn Future<Output = Vec<String>> + Send + '_>>;

/// One named read path, run as `ctx`, yielding the titles it let through.
struct Path {
    name: &'static str,
    /// The titles this path should return, sorted. Every path that reads the
    /// whole table expects [`PERMITTED`]; a path that asks for one specific
    /// hidden row expects nothing, which is its own assertion.
    expected: &'static [&'static str],
    run: Run,
}

/// Wrap an async block so a plain `fn` pointer can hold it.
macro_rules! path {
    ($name:literal, $expected:expr, |$store:ident, $ctx:ident| $body:expr) => {
        Path {
            name: $name,
            expected: $expected,
            run: |$store, $ctx| Box::pin(async move { $body }),
        }
    };
}

/// Every single-table read path. A new one belongs here.
fn paths() -> Vec<Path> {
    vec![
        path!("table scan", &PERMITTED, |store, ctx| {
            let table = documents();
            let txn = store.begin().await.unwrap();
            let rows = txn
                .query(&ctx, &table, Expr::True, ScanOrder::Ascending)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            titles(&rows)
        }),
        path!(
            "index scan on a secondary index",
            &PERMITTED,
            |store, ctx| {
                let table = documents();
                let txn = store.begin().await.unwrap();
                let rows = txn
                    .query(
                        &ctx,
                        &table,
                        Expr::compare(col("score"), CmpOp::Ge, Value::I64(0)),
                        ScanOrder::Ascending,
                    )
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap();
                titles(&rows)
            }
        ),
        path!("descending scan", &PERMITTED, |store, ctx| {
            let table = documents();
            let txn = store.begin().await.unwrap();
            let rows = txn
                .query(&ctx, &table, Expr::True, ScanOrder::Descending)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            titles(&rows)
        }),
        path!("point get on the full primary key", &[], |store, ctx| {
            // Asks for exactly one row Alice may not see. It must come back
            // empty, not forbidden: see `security.rs` for why probing matters.
            let table = documents();
            let txn = store.begin().await.unwrap();
            let rows = txn
                .execute(
                    &ctx,
                    &table,
                    &Query::all().filter(
                        Expr::eq(col("tenant_id"), Value::Uuid(Uuid::from_u128(TENANT_A)))
                            .and(Expr::eq(col("id"), Value::U64(3))),
                    ),
                )
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            titles(&rows)
        }),
        path!(
            "point gets from an IN over the key",
            &PERMITTED,
            |store, ctx| {
                let table = documents();
                let txn = store.begin().await.unwrap();
                let rows = txn
                    .execute(
                        &ctx,
                        &table,
                        &Query::all().filter(Expr::In {
                            column: col("id"),
                            values: (1..=5).map(Value::U64).collect(),
                        }),
                    )
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap();
                titles(&rows)
            }
        ),
        path!(
            "ORDER BY with a LIMIT, through the bounded heap",
            &PERMITTED,
            |store, ctx| {
                // A limit large enough to hold every row in the table: if the heap
                // filled before the policy ran, the forbidden rows would be here.
                let table = documents();
                let txn = store.begin().await.unwrap();
                let rows = txn
                    .execute(
                        &ctx,
                        &table,
                        &Query::all().sort_by([SortKey::desc(col("score"))]).limit(5),
                    )
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap();
                titles(&rows)
            }
        ),
        path!("a computed column, sorted by", &PERMITTED, |store, ctx| {
            let table = documents();
            let doubled = Query::computed(&table, 0);
            let txn = store.begin().await.unwrap();
            let rows = txn
                .execute(
                    &ctx,
                    &table,
                    &Query::all()
                        .computing([Scalar::column(col("score")) * 2i64])
                        .sort_by([SortKey::asc(doubled)]),
                )
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            titles(&rows)
        }),
        path!("LIKE", &PERMITTED, |store, ctx| {
            let table = documents();
            let txn = store.begin().await.unwrap();
            let rows = txn
                .execute(
                    &ctx,
                    &table,
                    &Query::all().filter(Expr::like(col("title"), "%o%")),
                )
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            titles(&rows)
        }),
        path!("ILIKE", &PERMITTED, |store, ctx| {
            let table = documents();
            let txn = store.begin().await.unwrap();
            let rows = txn
                .execute(
                    &ctx,
                    &table,
                    &Query::all().filter(Expr::ilike(col("title"), "%O%")),
                )
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            titles(&rows)
        }),
        path!("a regular expression", &PERMITTED, |store, ctx| {
            let table = documents();
            let txn = store.begin().await.unwrap();
            let rows = txn
                .execute(
                    &ctx,
                    &table,
                    &Query::all().filter(Expr::matches(col("title"), ".")),
                )
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            titles(&rows)
        }),
        path!("nearest-neighbour search", &PERMITTED, |store, ctx| {
            // The target sits nearest the rows Alice may *not* see, so a search
            // that ranked before filtering would return them first.
            let table = documents();
            let distance = Query::computed(&table, 0);
            let txn = store.begin().await.unwrap();
            let rows = txn
                .execute(
                    &ctx,
                    &table,
                    &Query::all()
                        .computing([Scalar::column(col("embedding"))
                            .distance(vec![0.5_f32, 1.0], Metric::L2)])
                        .sort_by([SortKey::asc(distance)])
                        .limit(3),
                )
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            titles(&rows)
        }),
        path!(
            "a projection that an index covers",
            &PERMITTED,
            |store, ctx| {
                // Projects only indexed columns, which is what lets the row lookup
                // be skipped — and skipping it must not skip the policy.
                let table = documents();
                let txn = store.begin().await.unwrap();
                let rows = txn
                    .query_projected(
                        &ctx,
                        &table,
                        Expr::compare(col("score"), CmpOp::Ge, Value::I64(0)),
                        ScanOrder::Ascending,
                        &Projection::Columns(vec![col("title"), col("score")]),
                    )
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap();
                titles(&rows)
            }
        ),
    ]
}

/// Every read path returns exactly the rows the policy admits.
#[tokio::test]
async fn every_access_path_applies_the_policy() {
    let store = seeded().await;

    // Every path is checked before anything fails. A matrix that stops at the
    // first bad cell hides how far the problem spreads, and when a change
    // breaks the policy it usually breaks it on more than one path.
    let mut failures: Vec<String> = Vec::new();
    for path in paths() {
        let got = (path.run)(&store, alice()).await;

        let leaked: Vec<&str> = FORBIDDEN
            .into_iter()
            .filter(|hidden| got.iter().any(|t| t == hidden))
            .collect();
        if !leaked.is_empty() {
            failures.push(format!(
                "{}: leaked {leaked:?} (returned {got:?})",
                path.name
            ));
            continue;
        }
        let expected: Vec<String> = path.expected.iter().map(|s| (*s).to_owned()).collect();
        if got != expected {
            failures.push(format!(
                "{}: returned {got:?}, expected {expected:?}",
                path.name
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of the access paths did not apply the policy:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// Grouping happens after the policy, so the counts describe the caller's slice
/// and not the table.
///
/// An aggregate is the easiest place to leak without returning a single
/// forbidden row: `COUNT(*)` over rows the caller cannot read still tells them
/// how many there are.
#[tokio::test]
async fn an_aggregate_counts_only_permitted_rows() {
    let store = seeded().await;
    let table = documents();
    let txn = store.begin().await.unwrap();

    let groups = txn
        .group_by(
            &alice(),
            &table,
            &Query::all(),
            &[],
            &[Aggregate::Count, Aggregate::Sum(col("score"))],
        )
        .await
        .unwrap();

    assert_eq!(groups.len(), 1, "one group when grouping by nothing");
    let row = groups[0].as_row();
    assert_eq!(
        row.values()[0],
        Value::U64(2),
        "counted rows the policy hides"
    );
    // 10 + 20, not 10 + 20 + 30 + 40 + 50.
    assert_eq!(
        row.values()[1],
        Value::I64(30),
        "summed rows the policy hides"
    );
}

/// Grouping by a column must not turn a hidden row into a visible group key.
///
/// The group key is itself data: a group for Bob's `owner_id` would disclose
/// that Bob has documents even with the rows themselves withheld.
#[tokio::test]
async fn grouping_does_not_disclose_a_hidden_key() {
    let store = seeded().await;
    let table = documents();
    let txn = store.begin().await.unwrap();

    let groups = txn
        .group_by(
            &alice(),
            &table,
            &Query::all(),
            &[col("owner_id")],
            &[Aggregate::Count],
        )
        .await
        .unwrap();

    assert_eq!(groups.len(), 1, "only Alice's own owner_id is a group");
    let row = groups[0].as_row();
    assert_eq!(row.values()[0], Value::Uuid(Uuid::from_u128(ALICE)));
    assert_eq!(row.values()[1], Value::U64(2));
}

/// `HAVING` is evaluated over groups, which are built from permitted rows only.
#[tokio::test]
async fn having_filters_groups_built_from_permitted_rows() {
    let store = seeded().await;
    let table = documents();
    let txn = store.begin().await.unwrap();

    // Over the whole table this group would count 5 and survive; over Alice's
    // slice it counts 2 and must not.
    let groups = txn
        .group_by_having(
            &alice(),
            &table,
            &Query::all(),
            &[],
            &[Aggregate::Count],
            &Expr::compare(
                slate_kernel::Group::aggregate(0, 0),
                CmpOp::Gt,
                Value::U64(2),
            ),
        )
        .await
        .unwrap();

    assert!(
        groups.is_empty(),
        "HAVING saw a count that included hidden rows: {groups:?}"
    );
}

/// Statistics are an ordinary read, so they describe what the caller can see.
///
/// This is deliberate rather than incidental — but it means statistics gathered
/// under a policy must not be mistaken for statistics about the table, and the
/// row count is the place that would show it.
#[tokio::test]
async fn statistics_describe_only_the_permitted_slice() {
    let store = seeded().await;
    let table = documents();
    let txn = store.begin().await.unwrap();

    let stats = txn.analyze(&alice(), &table).await.unwrap();
    assert_eq!(
        stats.row_count, 2,
        "analyze counted rows the caller cannot read"
    );
}
