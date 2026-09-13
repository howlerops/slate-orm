//! Adversarial probe of RLS across the access paths the matrix does not cover.
//!
//! Written for a security review: every path here asks the same question the
//! matrix asks — run as a user whose policy admits two of five rows, does it
//! return those two, and does an aggregate over them describe only those two?

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Action, Aggregate, CmpOp, Expr, Grant, Grouping, Join, JoinAlgorithm, JoinKey, Policy,
    Principal, Query, RecordStore, Scalar, SecurityCatalog, SecurityContext, SortKey,
    memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const DOCS: TableId = TableId(1);
const BY_KIND: IndexId = IndexId(10);
const BY_OWNER: IndexId = IndexId(11);
const BY_LOWER_TITLE: IndexId = IndexId(12);
const LIVE_BY_KIND: IndexId = IndexId(13);

const TENANT_A: u64 = 10;
const TENANT_B: u64 = 20;
const ALICE: u64 = 100;
const BOB: u64 = 200;

/// `title`, by position: `docs()` names the expression built on it.
const TITLE: Ordinal = Ordinal(4);
/// `retired_at`, by position, for the partial index predicate.
const RETIRED_AT: Ordinal = Ordinal(5);

fn lower_title() -> Scalar {
    Scalar::Lower(Box::new(Scalar::Column(TITLE)))
}

fn docs() -> TableDef {
    TableDef::builder("docs", DOCS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .nullable_column("owner_id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("title", ValueType::Str)
        .nullable_column("retired_at", ValueType::I64)
        .column("score", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(
            IndexDef::builder("by_kind", BY_KIND)
                .column("kind")
                .column("score"),
        )
        .index(IndexDef::builder("by_owner", BY_OWNER).column("owner_id"))
        .index(
            IndexDef::builder("by_lower_title", BY_LOWER_TITLE)
                .expression(lower_title(), ValueType::Str),
        )
        .index(
            IndexDef::builder("live_by_kind", LIVE_BY_KIND)
                .column("kind")
                .only_where(Expr::is_null(RETIRED_AT)),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    docs().ordinal_of(name).expect("column exists")
}

#[test]
fn the_positional_ordinals_are_the_columns_they_claim() {
    assert_eq!(TITLE, col("title"));
    assert_eq!(RETIRED_AT, col("retired_at"));
}

fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("reader", DOCS, [Action::Read, Action::Explain]))
        .grant(Grant::new(
            "reader",
            LOOKUPS,
            [Action::Read, Action::Explain],
        ))
        .policy(Policy::new(
            "own_documents",
            DOCS,
            Action::EVERYTHING,
            |ctx: &SecurityContext| Expr::eq(col("owner_id"), ctx.principal().id.clone()),
        ))
}

fn doc(tenant: u64, id: u64, owner: Option<u64>, kind: &str, title: &str, score: i64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        owner.map_or(Value::Null, Value::U64),
        Value::Str(kind.to_owned()),
        Value::Str(title.to_owned()),
        Value::Null,
        Value::I64(score),
    ])
}

// A second, un-tenanted lookup table, joined to `docs`.
const LOOKUPS: TableId = TableId(2);

fn lookups() -> TableDef {
    TableDef::builder("lookups", LOOKUPS)
        .column("kind", ValueType::Str)
        .column("label", ValueType::Str)
        .primary_key(["kind"])
        .build()
        .expect("valid schema")
}

fn lookup(kind: &str) -> Row {
    Row::new(vec![
        Value::Str(kind.to_owned()),
        Value::Str(format!("label-{kind}")),
    ])
}

fn alice() -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(ALICE))
            .with_tenant(Value::U64(TENANT_A))
            .with_role("reader"),
    )
}

/// Five rows, of which Alice's policy admits exactly two.
async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([docs(), lookups()]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security());
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for row in [
        doc(TENANT_A, 1, Some(ALICE), "alpha", "Alice One", 10),
        doc(TENANT_A, 2, Some(ALICE), "beta", "Alice Two", 20),
        doc(TENANT_A, 3, Some(BOB), "alpha", "Bob One", 30),
        doc(TENANT_A, 4, None, "gamma", "Unowned", 40),
        doc(TENANT_B, 5, Some(ALICE), "alpha", "Other Tenant", 50),
    ] {
        txn.insert(&root, &docs(), &row).await.unwrap();
    }
    for kind in ["alpha", "beta", "gamma"] {
        txn.insert(&root, &lookups(), &lookup(kind)).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

const PERMITTED: [&str; 2] = ["Alice One", "Alice Two"];
const FORBIDDEN: [&str; 3] = ["Bob One", "Unowned", "Other Tenant"];

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

#[track_caller]
fn assert_only_permitted(what: &str, got: &[String]) {
    let leaked: Vec<&&str> = FORBIDDEN
        .iter()
        .filter(|h| got.iter().any(|t| t == *h))
        .collect();
    assert!(
        leaked.is_empty(),
        "{what}: leaked {leaked:?} (returned {got:?})"
    );
    assert_eq!(got, PERMITTED, "{what}: returned {got:?}");
}

// --- 1. `IN` over an indexed non-key column: several disjoint ranges --------

#[tokio::test]
async fn an_in_over_a_secondary_index_applies_the_policy() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let query = Query::all().filter(Expr::In {
        column: col("kind"),
        values: ["alpha", "beta", "gamma"]
            .into_iter()
            .map(|k| Value::Str(k.to_owned()))
            .collect(),
    });
    // Confirm the shape first, so a planner change that stops producing a
    // union does not silently turn this into a re-test of the table scan.
    let plan = txn.explain(&alice(), &docs(), &query).unwrap();
    let rows = txn
        .execute(&alice(), &docs(), &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_only_permitted(&format!("IN over by_kind ({plan})"), &titles(&rows));
}

#[tokio::test]
async fn an_in_over_a_secondary_index_descending_applies_the_policy() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let query = Query::all().descending().filter(Expr::In {
        column: col("kind"),
        values: ["alpha", "beta", "gamma"]
            .into_iter()
            .map(|k| Value::Str(k.to_owned()))
            .collect(),
    });
    let rows = txn
        .execute(&alice(), &docs(), &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_only_permitted("IN over by_kind, descending", &titles(&rows));
}

// --- 2. covering scan over an expression index ------------------------------

/// The policy reads `owner_id`, which an expression index on `lower(title)`
/// does not hold — so the covering scan must not be claimed, and the rows must
/// still be filtered.
#[tokio::test]
async fn a_covering_scan_over_an_expression_index_applies_the_policy() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let computed = Query::computed(&docs(), 0);
    let query = Query::all()
        .computing([lower_title()])
        .select([col("id"), computed])
        .using_index(BY_LOWER_TITLE)
        .filter(Expr::compare(
            computed,
            CmpOp::Ge,
            Value::Str(String::new()),
        ));
    let plan = txn.explain(&alice(), &docs(), &query).unwrap();
    let rows = txn
        .execute(&alice(), &docs(), &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let mut got: Vec<String> = rows
        .iter()
        .map(|r| match r.get(computed) {
            Some(Value::Str(s)) => s.clone(),
            other => panic!("computed was {other:?} in {r:?}"),
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec!["alice one".to_owned(), "alice two".to_owned()],
        "expression-index scan returned {got:?}; plan was {plan}"
    );
}

// --- 3. partial index -------------------------------------------------------

#[tokio::test]
async fn a_partial_index_scan_applies_the_policy() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let query = Query::all()
        .filter(
            Expr::is_null(RETIRED_AT).and(Expr::eq(col("kind"), Value::Str("alpha".to_owned()))),
        )
        .using_index(LIVE_BY_KIND);
    let plan = txn.explain(&alice(), &docs(), &query).unwrap();
    let rows = txn
        .execute(&alice(), &docs(), &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let got = titles(&rows);
    let leaked: Vec<&&str> = FORBIDDEN
        .iter()
        .filter(|h| got.iter().any(|t| t == *h))
        .collect();
    assert!(
        leaked.is_empty(),
        "partial index leaked {leaked:?} ({got:?}); plan {plan}"
    );
    assert_eq!(got, vec!["Alice One".to_owned()], "plan {plan}");
}

// --- 4. aggregates that an index can answer without reading a row -----------

#[tokio::test]
async fn a_count_that_an_index_could_cover_counts_only_permitted_rows() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let n = txn.count(&alice(), &docs(), &Query::all()).await.unwrap();
    assert_eq!(n, 2, "count(*) counted rows the policy hides");

    // The same count, forced onto each index in turn.
    for index in [BY_KIND, BY_OWNER, LIVE_BY_KIND] {
        let n = txn
            .count(
                &alice(),
                &docs(),
                &Query::all()
                    .using_index(index)
                    .filter(Expr::is_null(RETIRED_AT)),
            )
            .await
            .unwrap();
        assert_eq!(n, 2, "count(*) through index {index:?} counted hidden rows");
    }
}

#[tokio::test]
async fn min_and_max_describe_only_permitted_rows() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let values = txn
        .aggregate(
            &alice(),
            &docs(),
            &Query::all(),
            &[
                Aggregate::Min(col("score")),
                Aggregate::Max(col("score")),
                Aggregate::CountDistinct(col("kind")),
                Aggregate::Sum(col("score")),
                Aggregate::Avg(col("score")),
            ],
        )
        .await
        .unwrap();
    assert_eq!(values[0], Value::I64(10), "min saw a hidden row");
    assert_eq!(
        values[1],
        Value::I64(20),
        "max saw a hidden row: {values:?}"
    );
    assert_eq!(values[2], Value::U64(2), "count(distinct) saw a hidden row");
    assert_eq!(values[3], Value::I64(30), "sum saw a hidden row");
    assert_eq!(values[4], Value::F64(15.0), "avg saw a hidden row");
}

/// The same aggregates over each index in turn, in case one path skips the row.
#[tokio::test]
async fn min_and_max_through_a_forced_index_describe_only_permitted_rows() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    for index in [BY_KIND, BY_OWNER] {
        let values = txn
            .aggregate(
                &alice(),
                &docs(),
                &Query::all().using_index(index),
                &[Aggregate::Min(col("score")), Aggregate::Max(col("score"))],
            )
            .await
            .unwrap();
        assert_eq!(
            values[0],
            Value::I64(10),
            "min through {index:?}: {values:?}"
        );
        assert_eq!(
            values[1],
            Value::I64(20),
            "max through {index:?}: {values:?}"
        );
    }
}

/// Grouping by a column an index holds, where the index alone could answer.
#[tokio::test]
async fn a_grouped_index_only_read_does_not_disclose_a_hidden_group() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let groups = txn
        .group_by(
            &alice(),
            &docs(),
            &Query::all().using_index(BY_KIND),
            &[col("kind")],
            &[Aggregate::Count, Aggregate::Max(col("score"))],
        )
        .await
        .unwrap();
    let mut seen: Vec<(String, Value, Value)> = groups
        .iter()
        .map(|g| {
            let row = g.as_row();
            (
                match &row.values()[0] {
                    Value::Str(s) => s.clone(),
                    other => panic!("group key {other:?}"),
                },
                row.values()[1].clone(),
                row.values()[2].clone(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("alpha".to_owned(), Value::U64(1), Value::I64(10)),
            ("beta".to_owned(), Value::U64(1), Value::I64(20)),
        ],
        "a grouped index-only read disclosed a hidden group: {seen:?}"
    );
}

/// A group key taken out of an expression index's entry.
#[tokio::test]
async fn grouping_by_an_expression_index_key_does_not_disclose_a_hidden_group() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let computed = Query::computed(&docs(), 0);
    let groups = txn
        .group_by(
            &alice(),
            &docs(),
            &Query::all()
                .computing([lower_title()])
                .using_index(BY_LOWER_TITLE),
            &[computed],
            &[Aggregate::Count],
        )
        .await
        .unwrap();
    let mut keys: Vec<String> = groups
        .iter()
        .map(|g| match &g.as_row().values()[0] {
            Value::Str(s) => s.clone(),
            other => panic!("group key {other:?}"),
        })
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["alice one".to_owned(), "alice two".to_owned()],
        "an expression-index grouping disclosed hidden rows"
    );
}

// --- 5. joins ---------------------------------------------------------------

async fn join_titles(force: Option<JoinAlgorithm>) -> Vec<String> {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let mut join = Join::on([JoinKey::new(col("kind"), Ordinal(0))]);
    if let Some(force) = force {
        join = join.using(force);
    }
    let rows = txn
        .join(&alice(), &docs(), &lookups(), &join)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let schema = slate_kernel::JoinSchema::of(&docs(), &lookups());
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| match r.flatten(&schema).get(col("title")) {
            Some(Value::Str(s)) => s.clone(),
            other => panic!("title {other:?}"),
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn a_hash_join_applies_the_policy() {
    let got = join_titles(Some(JoinAlgorithm::Hash {
        build: slate_kernel::Side::Left,
    }))
    .await;
    assert_only_permitted("hash join, build left", &got);
    let got = join_titles(Some(JoinAlgorithm::Hash {
        build: slate_kernel::Side::Right,
    }))
    .await;
    assert_only_permitted("hash join, build right", &got);
}

#[tokio::test]
async fn a_nested_loop_join_applies_the_policy() {
    let got = join_titles(Some(JoinAlgorithm::NestedLoop)).await;
    assert_only_permitted("nested loop join", &got);
}

#[tokio::test]
async fn a_grouped_join_does_not_disclose_a_hidden_row() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let join = Join::on([JoinKey::new(col("kind"), Ordinal(0))]);
    let groups = txn
        .group_by_join(
            &alice(),
            &docs(),
            &lookups(),
            &join,
            &Grouping::by([col("kind")], &[Aggregate::Count]),
        )
        .await
        .unwrap();
    let total: u64 = groups
        .iter()
        .map(|g| match g.as_row().values()[1] {
            Value::U64(n) => n,
            _ => panic!("count"),
        })
        .sum();
    assert_eq!(total, 2, "a grouped join counted hidden rows: {groups:?}");
}

// --- 6. chains --------------------------------------------------------------

#[tokio::test]
async fn a_three_table_chain_applies_the_policy() {
    use slate_kernel::{Chain, JoinStep};
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let tables = [&docs(), &lookups(), &lookups()];
    let chain = Chain::from(Query::all())
        .join(JoinStep::on([JoinKey::new(col("kind"), Ordinal(0))]))
        .join(JoinStep::on([JoinKey::new(
            Ordinal(docs().columns().len()),
            Ordinal(0),
        )]));
    let rows = txn
        .chain(&alice(), &tables, &chain)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let mut got: Vec<String> = rows
        .iter()
        .map(|r| match r.at(0).and_then(|row| row.get(col("title"))) {
            Some(Value::Str(s)) => s.clone(),
            other => panic!("title {other:?}"),
        })
        .collect();
    got.sort();
    got.dedup();
    assert_only_permitted("three-table chain", &got);
}

// --- 7. statistics ----------------------------------------------------------

#[tokio::test]
async fn analyze_does_not_describe_hidden_rows() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let stats = txn.analyze(&alice(), &docs()).await.unwrap();
    assert_eq!(stats.row_count, 2, "analyze counted hidden rows");
    let histogram = stats.histogram(col("score"));
    if let Some(histogram) = histogram {
        let bounds = format!("{histogram:?}");
        for hidden in [30i64, 40, 50] {
            assert!(
                !bounds.contains(&hidden.to_string()),
                "a histogram bound came from a hidden row: {bounds}"
            );
        }
    }
}

// --- 8. offsets, limits and sorts ------------------------------------------

#[tokio::test]
async fn a_deep_offset_does_not_page_into_hidden_rows() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    for offset in 0..5usize {
        let rows = txn
            .execute(&alice(), &docs(), &Query::all().offset(offset).limit(10))
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let got = titles(&rows);
        let leaked: Vec<&&str> = FORBIDDEN
            .iter()
            .filter(|h| got.iter().any(|t| t == *h))
            .collect();
        assert!(leaked.is_empty(), "offset {offset} leaked {leaked:?}");
    }
}

#[tokio::test]
async fn a_sort_on_an_unprojected_column_does_not_leak() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(
            &alice(),
            &docs(),
            &Query::all()
                .select([col("title")])
                .sort_by([SortKey::desc(col("score"))])
                .limit(5),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_only_permitted("sort on an unprojected column", &titles(&rows));
}

// --- 9. projections that name nothing --------------------------------------

#[tokio::test]
async fn a_projection_of_nothing_still_counts_only_permitted_rows() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let n = txn
        .execute(&alice(), &docs(), &Query::all().count_only())
        .await
        .unwrap()
        .count()
        .await
        .unwrap();
    assert_eq!(n, 2, "an empty projection returned hidden rows");
}

// --- 10. a query whose bounds reach outside the tenant ----------------------

#[tokio::test]
async fn a_filter_naming_another_tenant_returns_nothing_rather_than_their_rows() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    for path in [None, Some(BY_KIND), Some(BY_OWNER)] {
        let mut query = Query::all().filter(Expr::eq(col("tenant_id"), Value::U64(TENANT_B)));
        if let Some(index) = path {
            query = query.using_index(index);
        }
        let rows = txn
            .execute(&alice(), &docs(), &query)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert!(rows.is_empty(), "a cross-tenant filter returned {rows:?}");
    }
    // The same through an `IN` on the tenant column, which becomes point gets.
    let rows = txn
        .execute(
            &alice(),
            &docs(),
            &Query::all().filter(Expr::In {
                column: col("tenant_id"),
                values: vec![Value::U64(TENANT_A), Value::U64(TENANT_B)],
            }),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_only_permitted("IN over the tenant column", &titles(&rows));
}

// --- 11. a covering scan over an expression index, tenant scoping only ------
//
// The case the review brief singles out: the row handed to the residual is
// rebuilt from an index entry that holds one computed value and a primary key,
// and nothing else. With no RLS policy in play, the *only* thing standing
// between tenant A and tenant B is the tenant equality being evaluated against
// the tenant value decoded out of that key.

#[tokio::test]
async fn a_genuinely_covering_expression_scan_still_confines_the_tenant() {
    let catalog = Catalog::from_tables([docs(), lookups()]).expect("catalog");
    // Tenant scoping and nothing else: no policy, so the expression index can
    // actually cover the query and the covering path is exercised.
    let store = RecordStore::new(
        MemoryStore::new(),
        catalog,
        SecurityCatalog::new().grant(Grant::new("reader", DOCS, [Action::Read, Action::Explain])),
    );
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for row in [
        doc(TENANT_A, 1, Some(ALICE), "alpha", "Alice One", 10),
        doc(TENANT_B, 5, Some(ALICE), "alpha", "Other Tenant", 50),
    ] {
        txn.insert(&root, &docs(), &row).await.unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let computed = Query::computed(&docs(), 0);
    let query = Query::all()
        .computing([lower_title()])
        .select([computed])
        .using_index(BY_LOWER_TITLE)
        .filter(Expr::compare(
            computed,
            CmpOp::Ge,
            Value::Str(String::new()),
        ));
    let plan = txn.explain(&alice(), &docs(), &query).unwrap();
    assert!(
        plan.to_string().contains("Index Only Scan"),
        "this test is only meaningful over a covering scan; got {plan}"
    );
    let rows = txn
        .execute(&alice(), &docs(), &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let got: Vec<String> = rows
        .iter()
        .map(|r| match r.get(computed) {
            Some(Value::Str(s)) => s.clone(),
            other => panic!("computed was {other:?}"),
        })
        .collect();
    assert_eq!(
        got,
        vec!["alice one".to_owned()],
        "a covering expression scan crossed the tenant boundary: {got:?} ({plan})"
    );
}
