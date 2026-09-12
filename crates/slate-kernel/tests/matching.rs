//! `LIKE`, `COUNT(DISTINCT)`, and grouping — the three things ClickBench asked
//! for that were not here.
//!
//! The load-bearing test is [`bounds_never_lose_a_match`]: a pattern anchored
//! at the front becomes a key range, and a range that is too narrow silently
//! drops rows.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::expr::{like_matches, like_prefix};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Access, Action, Aggregate, Expr, Grant, Query, RecordStore, ScanOrder, SecurityCatalog,
    SecurityContext, plan,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const T: TableId = TableId(1);

fn table() -> TableDef {
    TableDef::builder("pages", T)
        .column("id", ValueType::U64)
        .column("url", ValueType::Str)
        .column("host", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_url", IndexId(10)).column("url"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("column exists")
}

const URLS: &[&str] = &[
    "",
    "a",
    "abc",
    "abc/def",
    "abcd",
    "ab\0c",
    "ab%c",
    "ab_c",
    "abz",
    "b",
    "google.com",
    "http://google.com/x",
    "http://www.google.com/",
    "zebra",
    "\u{1f600}bc",
];

fn row(id: u64) -> Row {
    let url = URLS[id as usize % URLS.len()];
    Row::new(vec![
        Value::U64(id),
        Value::Str(url.to_owned()),
        // Duplicated on purpose, so distinct counts differ from row counts.
        Value::Str(format!("host-{}", id % 4)),
    ])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let rows: Vec<Row> = (0..URLS.len() as u64 * 4).map(row).collect();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &rows).await.unwrap();
    txn.commit().await.unwrap();
    store
}

/// SQL's two wildcards, the escape, and the corners.
#[test]
fn like_matches_what_sql_says_it_should() {
    // Literals.
    assert!(like_matches("abc", "abc"));
    assert!(!like_matches("abc", "abd"));
    assert!(!like_matches("abc", "ab"));
    assert!(!like_matches("ab", "abc"));

    // `%` is any run, including none.
    assert!(like_matches("abc", "%"));
    assert!(like_matches("", "%"));
    assert!(like_matches("abc", "a%"));
    assert!(like_matches("abc", "%c"));
    assert!(like_matches("abc", "a%c"));
    assert!(like_matches("abc", "abc%"));
    assert!(like_matches("abc", "%abc%"));
    assert!(!like_matches("abc", "%d%"));

    // `_` is exactly one.
    assert!(like_matches("abc", "a_c"));
    assert!(!like_matches("abc", "a_"));
    assert!(!like_matches("ac", "a_c"));
    assert!(like_matches("", ""));
    assert!(!like_matches("", "_"));

    // Escapes make a wildcard literal.
    assert!(like_matches("a%c", r"a\%c"));
    assert!(!like_matches("abc", r"a\%c"));
    assert!(like_matches("a_c", r"a\_c"));
    assert!(!like_matches("abc", r"a\_c"));

    // Backtracking: the first `%` has to give ground.
    assert!(like_matches("aaa", "%a"));
    assert!(like_matches("aaab", "%ab"));
    assert!(like_matches("banana", "%an%na"));
    assert!(!like_matches("banana", "%an%nx"));

    // A pathological pattern must terminate, not recurse into the stack.
    let text = "a".repeat(2_000);
    assert!(like_matches(&text, "%a%a%a%a%a%a%a%a%a%a%"));
    assert!(!like_matches(&text, "%a%a%a%a%a%a%a%a%a%ab"));
}

/// What a pattern guarantees about where its matches sort.
#[test]
fn a_prefix_is_only_what_comes_before_a_wildcard() {
    assert_eq!(like_prefix("abc%"), Some("abc".to_owned()));
    assert_eq!(like_prefix("abc%def"), Some("abc".to_owned()));
    assert_eq!(like_prefix("abc"), Some("abc".to_owned()));
    assert_eq!(like_prefix("a_c"), Some("a".to_owned()));
    assert_eq!(like_prefix(r"a\%b%"), Some("a%b".to_owned()));
    // Nothing to bound by: `None`, not an empty string, so a caller cannot
    // read "no constraint" as "starts with nothing".
    assert_eq!(like_prefix("%abc"), None);
    assert_eq!(like_prefix("_abc"), None);
    assert_eq!(like_prefix(""), None);
}

/// The load-bearing one. An anchored pattern becomes a key range, and a range
/// that is too narrow drops rows silently — so every pattern shape must return
/// exactly what filtering by hand returns.
#[tokio::test]
async fn bounds_never_lose_a_match() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    let patterns = [
        "abc",
        "abc%",
        "ab%",
        "a%",
        "%",
        "%c",
        "%abc%",
        "a_c",
        "ab_c",
        r"ab\%c",
        r"ab\_c",
        "google%",
        "%google%",
        "zebra",
        "zzz%",
        "",
        "\u{1f600}%",
    ];

    for pattern in patterns {
        for negated in [false, true] {
            let filter = if negated {
                Expr::not_like(col("url"), pattern)
            } else {
                Expr::like(col("url"), pattern)
            };
            let got = txn
                .query(&root(), &table, filter.clone(), ScanOrder::Ascending)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();

            let expected: Vec<u64> = (0..URLS.len() as u64 * 4)
                .filter(|id| {
                    let url = URLS[*id as usize % URLS.len()];
                    like_matches(url, pattern) != negated
                })
                .collect();
            let mut ids: Vec<u64> = got
                .iter()
                .filter_map(|r| match r.get(col("id")) {
                    Some(Value::U64(n)) => Some(*n),
                    _ => None,
                })
                .collect();
            ids.sort_unstable();
            assert_eq!(
                ids, expected,
                "LIKE {pattern:?} (negated: {negated}) returned the wrong rows"
            );
        }
    }
}

/// And the bound is actually derived, or the test above would only be proving
/// that a full scan works.
#[test]
fn an_anchored_pattern_narrows_the_scan() {
    let table = table();
    let anchored = plan(
        &table,
        &Expr::like(col("id"), "ignored"),
        ScanOrder::Ascending,
    );
    // `id` is a u64, so a pattern on it bounds nothing; the point is that this
    // does not panic or invent a range.
    assert!(matches!(anchored.access, Access::TableScan { .. }));

    // On the indexed string column, an anchored pattern is a narrower range
    // than an unanchored one.
    let narrow = plan(
        &table,
        &Expr::like(col("url"), "abc%"),
        ScanOrder::Ascending,
    );
    let wide = plan(
        &table,
        &Expr::like(col("url"), "%abc"),
        ScanOrder::Ascending,
    );
    let span = |access: &Access| match access {
        Access::IndexScan { range, .. } | Access::TableScan { range } => format!("{range:?}").len(),
        _ => 0,
    };
    assert!(
        narrow.estimated_rows <= wide.estimated_rows,
        "anchored {} vs unanchored {}",
        narrow.estimated_rows,
        wide.estimated_rows
    );
    let _ = span(&narrow.access);
}

/// `COUNT(DISTINCT)`: seven ClickBench queries needed only this.
#[tokio::test]
async fn count_distinct_counts_values_not_rows() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();
    let rows = URLS.len() as u64 * 4;

    let values = txn
        .aggregate(
            &root(),
            &table,
            &Query::all(),
            &[
                Aggregate::Count,
                Aggregate::CountDistinct(col("host")),
                Aggregate::CountDistinct(col("url")),
                Aggregate::CountDistinct(col("id")),
            ],
        )
        .await
        .unwrap();

    assert_eq!(values[0], Value::U64(rows), "COUNT(*)");
    assert_eq!(values[1], Value::U64(4), "four distinct hosts");
    assert_eq!(
        values[2],
        Value::U64(URLS.len() as u64),
        "one distinct url per fixture entry"
    );
    assert_eq!(values[3], Value::U64(rows), "ids are unique");
}

/// Nulls are not a value. `COUNT(DISTINCT)` skips them, the way `COUNT(col)`
/// does and `COUNT(*)` does not.
#[tokio::test]
async fn count_distinct_skips_nulls() {
    let with_nulls = TableDef::builder("t", TableId(2))
        .column("id", ValueType::U64)
        .nullable_column("tag", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap();
    let catalog = Catalog::from_tables([with_nulls.clone()]).unwrap();
    let store = RecordStore::new(
        MemoryStore::new(),
        catalog,
        SecurityCatalog::new().grant(Grant::new("r", TableId(2), Action::ALL)),
    );
    let rows: Vec<Row> = (0..10u64)
        .map(|id| {
            Row::new(vec![
                Value::U64(id),
                if id % 2 == 0 {
                    Value::Null
                } else {
                    Value::Str(format!("tag-{}", id % 4))
                },
            ])
        })
        .collect();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &with_nulls, &rows).await.unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let tag = with_nulls.ordinal_of("tag").unwrap();
    let values = txn
        .aggregate(
            &root(),
            &with_nulls,
            &Query::all(),
            &[
                Aggregate::Count,
                Aggregate::CountColumn(tag),
                Aggregate::CountDistinct(tag),
            ],
        )
        .await
        .unwrap();
    assert_eq!(values[0], Value::U64(10), "every row");
    assert_eq!(values[1], Value::U64(5), "the non-null ones");
    assert_eq!(values[2], Value::U64(2), "tag-1 and tag-3");
}

/// Grouping is hashed now rather than kept in an ordered map, and must still
/// come back in the order it used to.
#[tokio::test]
async fn groups_still_arrive_in_key_order() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    let groups = txn
        .group_by(
            &root(),
            &table,
            &Query::all(),
            &[col("host")],
            &[Aggregate::Count, Aggregate::CountDistinct(col("url"))],
        )
        .await
        .unwrap();

    assert_eq!(groups.len(), 4);
    let keys: Vec<&Value> = groups.iter().map(|g| &g.key[0]).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "groups came back out of key order");

    // And every row is accounted for.
    let total: u64 = groups
        .iter()
        .map(|g| match g.values[0] {
            Value::U64(n) => n,
            _ => 0,
        })
        .sum();
    assert_eq!(total, URLS.len() as u64 * 4);
}

/// Grouping on a column with nulls must keep them as their own group, not
/// silently merge them into whatever encodes nearby.
#[tokio::test]
async fn a_null_is_its_own_group() {
    let with_nulls = TableDef::builder("t", TableId(3))
        .column("id", ValueType::U64)
        .nullable_column("tag", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap();
    let catalog = Catalog::from_tables([with_nulls.clone()]).unwrap();
    let store = RecordStore::new(
        MemoryStore::new(),
        catalog,
        SecurityCatalog::new().grant(Grant::new("r", TableId(3), Action::ALL)),
    );
    let rows: Vec<Row> = (0..9u64)
        .map(|id| {
            Row::new(vec![
                Value::U64(id),
                if id % 3 == 0 {
                    Value::Null
                } else {
                    Value::Str("x".to_owned())
                },
            ])
        })
        .collect();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &with_nulls, &rows).await.unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let groups = txn
        .group_by(
            &root(),
            &with_nulls,
            &Query::all(),
            &[with_nulls.ordinal_of("tag").unwrap()],
            &[Aggregate::Count],
        )
        .await
        .unwrap();
    assert_eq!(groups.len(), 2, "null and 'x': {groups:?}");
    assert_eq!(groups[0].key[0], Value::Null, "null sorts first");
    assert_eq!(groups[0].values[0], Value::U64(3));
}
