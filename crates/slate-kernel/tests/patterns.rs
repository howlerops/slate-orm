//! `ILIKE` and regular expressions.
//!
//! Both are matching over text, and both stay residual filters: a
//! case-insensitive pattern and a regular expression each match values that do
//! not sort next to each other, so neither can become a key range the way an
//! anchored `LIKE` can.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::scalar::backreferences;
use slate_kernel::{
    Access, AccessHint, Action, Expr, Grant, Projection, RecordStore, Scalar, ScanOrder,
    SecurityCatalog, SecurityContext, TableStats, Truth, plan_hinted,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const T: TableId = TableId(1);

fn table() -> TableDef {
    TableDef::builder("pages", T)
        .column("id", ValueType::U64)
        .column("url", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_url", IndexId(10)).column("url"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("column exists")
}

const URLS: &[&str] = &[
    "http://google.com/a",
    "https://www.Google.com/b",
    "HTTP://GOOGLE.COM/C",
    "http://example.com/d",
    "https://sub.example.org/e",
    "not a url",
    "",
];

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let rows: Vec<Row> = URLS
        .iter()
        .enumerate()
        .map(|(i, url)| Row::new(vec![Value::U64(i as u64), Value::Str((*url).to_owned())]))
        .collect();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &rows).await.unwrap();
    txn.commit().await.unwrap();
    store
}

async fn ids(store: &RecordStore<MemoryStore>, filter: Expr) -> Vec<u64> {
    let txn = store.begin().await.unwrap();
    let mut out: Vec<u64> = txn
        .query(&root(), &table(), filter, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| match r.get(col("id")) {
            Some(Value::U64(n)) => Some(*n),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out
}

/// `ILIKE` is `LIKE` with case folded on both sides.
#[tokio::test]
async fn ilike_ignores_case() {
    let store = seeded().await;

    // Case-sensitive finds only the lower-case one.
    assert_eq!(
        ids(&store, Expr::like(col("url"), "%google%")).await,
        vec![0]
    );
    // Case-insensitive finds all three.
    assert_eq!(
        ids(&store, Expr::ilike(col("url"), "%google%")).await,
        vec![0, 1, 2]
    );
    // Anchored, and still case-insensitive.
    assert_eq!(
        ids(&store, Expr::ilike(col("url"), "http://%")).await,
        vec![0, 2, 3],
        "two lower-case and one upper"
    );
    // Negation is the complement, over the rows where the answer is known.
    let matched = ids(&store, Expr::ilike(col("url"), "%google%")).await;
    let unmatched = ids(&store, Expr::not_ilike(col("url"), "%google%")).await;
    assert_eq!(matched.len() + unmatched.len(), URLS.len());
    assert!(matched.iter().all(|id| !unmatched.contains(id)));
}

/// An anchored `LIKE` is a key range; an anchored `ILIKE` is not, because the
/// values it also matches do not sort there.
///
/// Checked on the range the planner derives rather than on its row estimate:
/// without statistics both estimates fall back to the same flat guess, so an
/// estimate would prove nothing.
#[test]
fn ilike_does_not_become_a_key_range() {
    let table = table();
    let through_index = |filter: Expr| {
        let plan = plan_hinted(
            &table,
            std::sync::Arc::new(filter),
            ScanOrder::Ascending,
            &Projection::All,
            &TableStats::assumed(),
            None,
            &[],
            Some(AccessHint::Index(IndexId(10))),
            &[],
        );
        match plan.access {
            Access::IndexScan { range, .. } => format!("{range:?}"),
            other => panic!("expected an index scan, got {other:?}"),
        }
    };

    let unbounded = through_index(Expr::True);
    let sensitive = through_index(Expr::like(col("url"), "http://%"));
    let insensitive = through_index(Expr::ilike(col("url"), "http://%"));

    assert_ne!(
        sensitive, unbounded,
        "an anchored LIKE should narrow the index range"
    );
    assert_eq!(
        insensitive, unbounded,
        "an anchored ILIKE cannot narrow it, and must not pretend to"
    );
}

/// A regular expression, and that it stays a filter.
#[tokio::test]
async fn a_regular_expression_matches_rows() {
    let store = seeded().await;

    assert_eq!(
        ids(&store, Expr::matches(col("url"), r"^https?://")).await,
        vec![0, 1, 3, 4],
        "everything that looks like a url — case-sensitively, so not the          upper-case HTTP:// one"
    );
    // Row 2 is HTTP:// in upper case, so the case-sensitive anchor misses it.
    let sensitive = ids(&store, Expr::matches(col("url"), r"^http://")).await;
    assert_eq!(sensitive, vec![0, 3]);
    assert_eq!(
        ids(&store, Expr::matches_insensitive(col("url"), r"^http://")).await,
        vec![0, 2, 3]
    );
    // Alternation, which `LIKE` cannot express at all.
    assert_eq!(
        ids(&store, Expr::matches(col("url"), r"\.(org|net)/")).await,
        vec![4]
    );
}

/// A pattern that does not compile matches nothing, rather than failing a
/// query part-way through a scan. It can be found out about first.
#[tokio::test]
async fn a_bad_pattern_matches_nothing_and_can_be_reported() {
    let store = seeded().await;
    let broken = Expr::matches(col("url"), "((unclosed");

    assert!(broken.regex_error().is_some(), "should report the problem");
    assert!(Expr::matches(col("url"), "^ok$").regex_error().is_none());
    assert!(ids(&store, broken).await.is_empty());
}

/// A null is unknown for both, not false — the same rule every other
/// comparison follows, and the one a deny-style policy depends on.
#[test]
fn a_null_is_unknown_for_both() {
    let row = Row::new(vec![Value::U64(1), Value::Null]);
    for filter in [
        Expr::like(col("url"), "%x%"),
        Expr::ilike(col("url"), "%x%"),
        Expr::matches(col("url"), "x"),
        Expr::not_ilike(col("url"), "%x%"),
    ] {
        assert_eq!(filter.evaluate(&row), Truth::Unknown, "{filter:?}");
        assert!(!filter.admits(&row));
        assert!(!Expr::Not(Box::new(filter)).admits(&row));
    }
}

/// `REGEXP_REPLACE`, including ClickBench Q29's pattern — the one query that
/// needed a regex engine at all.
#[test]
fn regexp_replace_rewrites_with_capture_groups() {
    let host = |url: &str| {
        Scalar::column(Ordinal(0))
            .regexp_replace(r"^https?://(?:www\.)?([^/]+)/.*$", r"\1")
            .evaluate(&Row::new(vec![Value::Str(url.to_owned())]))
    };

    assert_eq!(
        host("http://google.com/a"),
        Value::Str("google.com".to_owned())
    );
    assert_eq!(
        host("https://www.example.org/x/y"),
        Value::Str("example.org".to_owned()),
        "the optional www. group is dropped"
    );
    // No match leaves the text alone, which is what REGEXP_REPLACE does.
    assert_eq!(host("not a url"), Value::Str("not a url".to_owned()));

    // A pattern that will not compile is null, not a panic.
    assert_eq!(
        Scalar::column(Ordinal(0))
            .regexp_replace("((", "x")
            .evaluate(&Row::new(vec![Value::Str("a".to_owned())])),
        Value::Null
    );
}

/// SQL writes capture references as `\1`; the regex crate wants `$1`. Both
/// conventions cannot be honoured at once, so a literal `$` is escaped and
/// `\1` is what refers to a group.
#[test]
fn capture_references_follow_sql() {
    assert_eq!(backreferences(r"\1"), "$1");
    assert_eq!(backreferences(r"\1-\2"), "$1-$2");
    assert_eq!(backreferences("plain"), "plain");
    // A literal dollar survives as one rather than becoming a group.
    assert_eq!(backreferences("$1"), "$$1");
    assert_eq!(backreferences("costs $5"), "costs $$5");
    // A doubled backslash is an escaped backslash.
    assert_eq!(backreferences(r"\\1"), r"\1");
    // A trailing backslash is itself.
    assert_eq!(backreferences(r"a\"), r"a\");

    // And the whole thing round-trips through an actual replacement.
    let value = Scalar::column(Ordinal(0))
        .regexp_replace(r"(\d+)", r"[\1]")
        .evaluate(&Row::new(vec![Value::Str("a1b22".to_owned())]));
    assert_eq!(value, Value::Str("a[1]b[22]".to_owned()));
}
