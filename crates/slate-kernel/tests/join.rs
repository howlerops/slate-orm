//! Joining two tables.
//!
//! Two things are being tested and they are not the same thing. That the join
//! returns the right rows, and that it cannot return rows either side's policy
//! hides — the second matters more, because a join is exactly where a security
//! model built on single-table filters would be expected to leak.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, CmpOp, Expr, Grant, Join, JoinAlgorithm, JoinKey, JoinSchema, KernelError, Policy,
    Principal, Query, RecordStore, SecurityCatalog, SecurityContext, Side, Statistics, TableStats,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);

fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .column("country", ValueType::Str)
        // Paired with a book's publication year, this makes a comparison only
        // a joined row can answer: was the book published in the author's
        // lifetime?
        .column("died", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn books() -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        // Nullable so the join can be asked what it does with an unset key.
        .nullable_column("author_id", ValueType::U64)
        .column("title", ValueType::Str)
        .column("published", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_author", IndexId(20)).column("author_id"))
        .build()
        .expect("valid schema")
}

fn author_col(name: &str) -> Ordinal {
    authors().ordinal_of(name).expect("column exists")
}

fn book_col(name: &str) -> Ordinal {
    books().ordinal_of(name).expect("column exists")
}

fn author(tenant: u64, id: u64, name: &str, country: &str, died: i64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::Str(name.to_owned()),
        Value::Str(country.to_owned()),
        Value::I64(died),
    ])
}

fn book(tenant: u64, id: u64, author_id: Option<u64>, title: &str, published: i64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        author_id.map_or(Value::Null, Value::U64),
        Value::Str(title.to_owned()),
        Value::I64(published),
    ])
}

/// `authors.id = books.author_id`.
fn on_author() -> Join {
    Join::equating(author_col("id"), book_col("author_id"))
}

fn open() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::ALL))
        .grant(Grant::new("r", BOOKS, Action::ALL))
}

fn reader(tenant: u64) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(tenant))
            .with_role("r"),
    )
}

/// Two authors in tenant 1, one in tenant 2. Books: two for author 1, one for
/// author 2, one orphaned, one with a null author.
///
/// Iain's one book is published after he died, so `published < died` is a
/// cross-side condition that actually splits the data rather than being
/// trivially true.
async fn store(
    security: SecurityCatalog,
) -> (RecordStore<LatencyStore<MemoryStore>>, Arc<IoCounters>) {
    let catalog = Catalog::from_tables([authors(), books()]).expect("catalog");
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let root = SecurityContext::superuser();

    let txn = loader.begin().await.unwrap();
    for row in [
        author(1, 1, "Ursula", "US", 2018),
        author(1, 2, "Iain", "UK", 1980),
        author(2, 1, "Someone Else", "FR", 1950),
    ] {
        txn.insert(&root, &authors(), &row).await.unwrap();
    }
    for row in [
        book(1, 10, Some(1), "A Wizard of Earthsea", 1968),
        book(1, 11, Some(1), "The Dispossessed", 1974),
        book(1, 12, Some(2), "Consider Phlebas", 1987),
        book(1, 13, Some(99), "Orphaned", 2000),
        book(1, 14, None, "Anonymous", 1999),
        book(2, 10, Some(1), "Another Tenant's Book", 1990),
    ] {
        txn.insert(&root, &books(), &row).await.unwrap();
    }
    txn.commit().await.unwrap();

    let slow = LatencyStore::new(backing, LatencyProfile::free());
    let counters = slow.counters();
    (RecordStore::new(slow, catalog, security), counters)
}

fn titles(rows: &[slate_kernel::JoinedRow]) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| match &r.right {
            Some(book) => match book.get(book_col("title")) {
                Some(Value::Str(s)) => s.clone(),
                other => format!("{other:?}"),
            },
            None => "<none>".to_owned(),
        })
        .collect();
    out.sort();
    out
}

fn names(rows: &[slate_kernel::JoinedRow]) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(
            |r| match r.left.as_ref().and_then(|l| l.get(author_col("name"))) {
                Some(Value::Str(s)) => s.clone(),
                other => format!("{other:?}"),
            },
        )
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn an_inner_join_pairs_matching_rows() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    let rows = txn
        .join(&reader(1), &authors(), &books(), &on_author())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(
        titles(&rows),
        vec![
            "A Wizard of Earthsea",
            "Consider Phlebas",
            "The Dispossessed"
        ]
    );
    assert_eq!(names(&rows), vec!["Iain", "Ursula", "Ursula"]);
    assert!(rows.iter().all(|r| r.is_matched()));
}

/// A left join keeps every left row. Here both authors have books, so the
/// interesting case is the reverse join: books with no author.
#[tokio::test]
async fn a_left_join_keeps_unmatched_rows() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    // books LEFT JOIN authors: the orphan and the null-author book must both
    // survive, with no right side.
    let join = Join::equating(book_col("author_id"), author_col("id")).left_outer();
    let rows = txn
        .join(&reader(1), &books(), &authors(), &join)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), 5, "every book in the tenant");
    let unmatched: Vec<String> = rows
        .iter()
        .filter(|r| !r.is_matched())
        .map(
            |r| match r.left.as_ref().and_then(|l| l.get(book_col("title"))) {
                Some(Value::Str(s)) => s.clone(),
                other => format!("{other:?}"),
            },
        )
        .collect();
    let mut unmatched = unmatched;
    unmatched.sort();
    assert_eq!(unmatched, vec!["Anonymous", "Orphaned"]);
}

/// Null never equals null. A book with no author must not join to an author
/// whose column is somehow also null, and must not join to anything else
/// either.
#[tokio::test]
async fn a_null_join_value_matches_nothing() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::NestedLoop,
    ] {
        let join = Join::equating(book_col("author_id"), author_col("id")).using(algorithm);
        let rows = txn
            .join(&reader(1), &books(), &authors(), &join)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let anonymous = rows.iter().any(|r| {
            r.left.as_ref().and_then(|l| l.get(book_col("title")))
                == Some(&Value::Str("Anonymous".to_owned()))
        });
        assert!(
            !anonymous,
            "a null author_id matched something under {algorithm:?}"
        );
        assert_eq!(rows.len(), 3, "under {algorithm:?}");
    }
}

/// Every algorithm has to produce the same rows, or the planner choosing
/// between them changes the answer.
#[tokio::test]
async fn every_algorithm_agrees() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    let mut results = Vec::new();
    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::NestedLoop,
    ] {
        let rows = txn
            .join(
                &reader(1),
                &authors(),
                &books(),
                &on_author().using(algorithm),
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        results.push((algorithm, titles(&rows), names(&rows)));
    }

    let (_, first_titles, first_names) = &results[0];
    for (algorithm, t, n) in &results[1..] {
        assert_eq!(t, first_titles, "{algorithm:?} returned different books");
        assert_eq!(n, first_names, "{algorithm:?} returned different authors");
    }

    // And a left outer join agrees between the two shapes that can run one.
    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::NestedLoop,
    ] {
        let join = Join::equating(book_col("author_id"), author_col("id"))
            .left_outer()
            .using(algorithm);
        let rows = txn
            .join(&reader(1), &books(), &authors(), &join)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(rows.len(), 5, "{algorithm:?}");
        assert_eq!(
            rows.iter().filter(|r| !r.is_matched()).count(),
            2,
            "{algorithm:?}"
        );
    }
}

/// The whole security argument. A join is two secured reads, so a policy on
/// either side applies — and one side cannot be used to see the other's
/// hidden rows.
#[tokio::test]
async fn a_policy_on_either_side_still_applies() {
    // Readers may only see UK authors, and only books whose id is under 12.
    let security = open()
        .policy(Policy::new(
            "uk_only",
            AUTHORS,
            Action::ALL,
            |_: &SecurityContext| Expr::eq(author_col("country"), Value::Str("UK".into())),
        ))
        .policy(Policy::new(
            "early_books",
            BOOKS,
            Action::ALL,
            |_: &SecurityContext| Expr::compare(book_col("id"), CmpOp::Lt, Value::U64(12)),
        ));
    let (store, _) = store(security).await;
    let txn = store.begin().await.unwrap();

    let rows = txn
        .join(&reader(1), &authors(), &books(), &on_author())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // Iain (UK) is visible, but his only book has id 12 and is hidden; Ursula
    // is hidden even though her books are visible. Nothing survives.
    assert!(
        rows.is_empty(),
        "a join returned rows a policy hides: {rows:?}"
    );

    // The same join under a superuser returns rows, so the emptiness above is
    // the policy and not a broken fixture.
    //
    // Seven, not three: the join condition is `authors.id = books.author_id`
    // and says nothing about the tenant, so for a caller with no tenant filter
    // author 1 of each tenant pairs with every book naming author 1. A tenant
    // boundary is a scan prefix, and a superuser has no prefix — which is what
    // bypassing the policy means. An ordinary caller is confined to their own
    // tenant on both sides; see `a_join_does_not_cross_tenants`.
    let all = txn
        .join(
            &SecurityContext::superuser(),
            &authors(),
            &books(),
            &on_author(),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(all.len(), 7);
}

/// A tenant boundary is a key prefix, so it applies to a join for the same
/// reason it applies to a scan — but it has to be checked, because a join is
/// where "the other side was already filtered" stops being obvious.
#[tokio::test]
async fn a_join_does_not_cross_tenants() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::NestedLoop,
    ] {
        let rows = txn
            .join(
                &reader(2),
                &authors(),
                &books(),
                &on_author().using(algorithm),
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "under {algorithm:?}");
        assert_eq!(
            rows[0]
                .left
                .as_ref()
                .and_then(|l| l.get(author_col("name"))),
            Some(&Value::Str("Someone Else".to_owned())),
            "under {algorithm:?}"
        );
    }
}

/// Reading with no grant is refused before anything is read, on either side.
#[tokio::test]
async fn a_join_needs_a_grant_on_both_tables() {
    let only_authors = SecurityCatalog::new().grant(Grant::new("r", AUTHORS, Action::ALL));
    let (store, _) = store(only_authors).await;
    let txn = store.begin().await.unwrap();

    let err = txn
        .join(&reader(1), &authors(), &books(), &on_author())
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::AccessDenied { .. }),
        "got {err:?}"
    );
}

/// Each side's own filter narrows it before the join, which is the difference
/// between filtering and joining-then-filtering.
#[tokio::test]
async fn each_side_carries_its_own_filter() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    let join = on_author()
        .left(Query::all().filter(Expr::eq(author_col("country"), Value::Str("UK".into()))));
    let rows = txn
        .join(&reader(1), &authors(), &books(), &join)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(titles(&rows), vec!["Consider Phlebas"]);
}

/// A cross join is not offered, and asking for one says so rather than
/// returning the product.
#[tokio::test]
async fn a_join_with_no_condition_is_refused() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    let err = txn
        .join(&reader(1), &authors(), &books(), &Join::on([]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::JoinNotSupported { .. }),
        "got {err:?}"
    );

    // And a column that is not on the table it was named for.
    let wrong = Join::on([JoinKey::new(Ordinal(99), book_col("author_id"))]);
    let err = txn
        .join(&reader(1), &authors(), &books(), &wrong)
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::JoinNotSupported { .. }),
        "got {err:?}"
    );
}

/// A build side that will not fit reports rather than being killed by the
/// allocator.
#[tokio::test]
async fn a_hash_build_side_is_bounded() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    let join = on_author()
        .using(JoinAlgorithm::Hash { build: Side::Right })
        .build_limit(2);
    let err = txn
        .join(&reader(1), &authors(), &books(), &join)
        .await
        .unwrap_err();
    match err {
        KernelError::JoinBuildTooLarge { table, limit } => {
            assert_eq!(table, "books");
            assert_eq!(limit, 2);
        }
        other => panic!("expected a build-size refusal, got {other:?}"),
    }
}

/// A limit and offset apply to the joined result, not to either side.
#[tokio::test]
async fn a_limit_applies_to_the_join() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    let all = txn
        .join(&reader(1), &authors(), &books(), &on_author())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(all.len(), 3);

    let limited = txn
        .join(&reader(1), &authors(), &books(), &on_author().limit(2))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(limited.len(), 2);

    let offset = txn
        .join(
            &reader(1),
            &authors(),
            &books(),
            &on_author().offset(1).limit(5),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(offset.len(), 2);
}

/// The cost model's whole claim: a hash join for two full scans, a loop when
/// the outer side is a handful of rows against a large inner one.
#[tokio::test]
async fn the_planner_picks_a_loop_only_for_a_small_outer_side() {
    let statistics = Statistics::new()
        .with(
            AUTHORS,
            TableStats::with_row_count(10_000).with_column(
                author_col("id"),
                slate_kernel::ColumnStats {
                    distinct: 10_000,
                    null_fraction: 0.0,
                },
            ),
        )
        .with(
            BOOKS,
            // A hundred million books over ten million authors: ten each, and
            // an inner table large enough that scanning it is genuinely worse
            // than ten point reads. At a hundred thousand books it is not —
            // see the note on the crossover in `join.rs`.
            TableStats::with_row_count(100_000_000).with_column(
                book_col("author_id"),
                slate_kernel::ColumnStats {
                    distinct: 10_000_000,
                    null_fraction: 0.0,
                },
            ),
        );
    let (store, _) = store(open()).await;
    let store = store.with_statistics(statistics);
    let txn = store.begin().await.unwrap();

    // Both sides whole: two scans beat ten thousand probes.
    let plan = txn
        .explain_join(&reader(1), &authors(), &books(), &on_author())
        .unwrap();
    assert!(
        !plan.is_nested_loop(),
        "chose a loop for two full scans: {plan}"
    );

    // One author against a hundred thousand books: one probe beats a scan of
    // the whole other table. This is the ORM's own query.
    let one = on_author().left(Query::all().filter(Expr::eq(author_col("id"), Value::U64(1))));
    let plan = txn
        .explain_join(&reader(1), &authors(), &books(), &one)
        .unwrap();
    assert!(
        plan.is_nested_loop(),
        "chose a hash join to fetch one author's books: {plan}"
    );
}

/// The loop's advantage is that it does not read the whole inner table. Count
/// the rows read rather than trusting the estimate.
///
/// This is about the *mechanics* of the two algorithms, not about which one the
/// planner picks. It used to assert both, and could no longer: at a thousand
/// inner rows the planner now correctly prefers a hash join, because scanning a
/// thousand rows is one object-store request and two point reads are about six.
/// Which algorithm wins where is
/// [`the_planner_picks_a_loop_only_for_a_small_outer_side`]'s question, and it
/// needs a corpus far larger than one a test can seed. What is still true at
/// any size, and is what this checks, is that a loop reads less.
#[tokio::test]
async fn a_loop_reads_less_than_a_scan_of_the_inner_side() {
    const AUTHOR_COUNT: u64 = 500;
    const BOOK_COUNT: u64 = 1_000;

    let catalog = Catalog::from_tables([authors(), books()]).expect("catalog");
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let root = SecurityContext::superuser();

    let txn = loader.begin().await.unwrap();
    for id in 1..=AUTHOR_COUNT {
        txn.insert(
            &root,
            &authors(),
            &author(1, id, &format!("a{id}"), "US", 2000),
        )
        .await
        .unwrap();
    }
    for id in 1..=BOOK_COUNT {
        let by = (id % AUTHOR_COUNT) + 1;
        txn.insert(
            &root,
            &books(),
            &book(1, 1_000 + id, Some(by), &format!("b{id}"), 1990),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let slow = LatencyStore::new(backing, LatencyProfile::free());
    let counters = slow.counters();
    let store = RecordStore::new(slow, catalog, open());

    // Real statistics, so the probe is planned against what is there rather
    // than against the defaults.
    let (author_stats, book_stats) = {
        let txn = store.begin().await.unwrap();
        (
            txn.analyze(&root, &authors()).await.unwrap(),
            txn.analyze(&root, &books()).await.unwrap(),
        )
    };
    let store = store.with_statistics(
        Statistics::new()
            .with(AUTHORS, author_stats)
            .with(BOOKS, book_stats),
    );

    let txn = store.begin().await.unwrap();
    // The probe is pinned to `by_author`. Left to itself it would scan books
    // instead — a thousand rows is one object-store request and two point reads
    // are about six — and then the loop would read exactly as much as the hash
    // join, which is the outcome this test kept getting before the hint was
    // added. Pinning it makes the comparison the intended one: a real index
    // probe against a full build.
    let mut probe = Query::all();
    probe.hint = Some(slate_kernel::AccessHint::Index(IndexId(20)));
    let one = on_author()
        .left(Query::all().filter(Expr::eq(author_col("id"), Value::U64(7))))
        .right(probe);

    counters.reset();
    let loops = txn
        .join(
            &reader(1),
            &authors(),
            &books(),
            &one.clone().using(JoinAlgorithm::NestedLoop),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let loop_rows = counters.scan_rows();

    counters.reset();
    let hashed = txn
        .join(
            &reader(1),
            &authors(),
            &books(),
            &one.using(JoinAlgorithm::Hash { build: Side::Right }),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let hash_rows = counters.scan_rows();

    assert_eq!(titles(&loops), titles(&hashed), "the two disagree");
    assert_eq!(loops.len(), 2, "author 7 wrote two books");
    assert!(
        loop_rows * 10 < hash_rows,
        "the loop scanned {loop_rows} rows and the hash join {hash_rows}; \
         the loop is supposed to touch a small fraction of the inner table"
    );
}

/// A right outer join keeps every right row. The hash join learns which built
/// rows nothing matched only once the probe side is exhausted, so this is the
/// case that exercises the drain.
#[tokio::test]
async fn a_right_join_keeps_unmatched_right_rows() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    // authors RIGHT JOIN books: every book survives, including the orphan and
    // the one with no author.
    let join = on_author().right_outer();
    let rows = txn
        .join(&reader(1), &authors(), &books(), &join)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), 5, "every book in the tenant");
    assert!(
        rows.iter().all(|r| r.right.is_some()),
        "a right join must never drop the right side"
    );
    let mut orphaned: Vec<String> = rows
        .iter()
        .filter(|r| r.left.is_none())
        .map(
            |r| match r.right.as_ref().and_then(|b| b.get(book_col("title"))) {
                Some(Value::Str(s)) => s.clone(),
                other => format!("{other:?}"),
            },
        )
        .collect();
    orphaned.sort();
    assert_eq!(orphaned, vec!["Anonymous", "Orphaned"]);
}

/// A full outer join keeps everything on both sides: the pairs, the left rows
/// with no match, and the right rows with no match.
#[tokio::test]
async fn a_full_join_keeps_both_sides() {
    let (store, _) = store(open()).await;

    // Give one author no books at all, so there is a left row to preserve.
    let write = store.begin().await.unwrap();
    write
        .insert(
            &SecurityContext::superuser(),
            &authors(),
            &author(1, 3, "Nobody", "IE", 1990),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let rows = txn
        .join(&reader(1), &authors(), &books(), &on_author().full_outer())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // 3 matched pairs + Nobody with no book + 2 books with no author.
    assert_eq!(rows.len(), 6, "{rows:?}");
    assert_eq!(rows.iter().filter(|r| r.is_matched()).count(), 3);
    assert_eq!(
        rows.iter().filter(|r| r.right.is_none()).count(),
        1,
        "the author with no books"
    );
    assert_eq!(
        rows.iter().filter(|r| r.left.is_none()).count(),
        2,
        "the orphan and the null-author book"
    );

    // Every row has at least one side. A row with neither would be a bug that
    // the counts above would not notice.
    assert!(rows.iter().all(|r| r.left.is_some() || r.right.is_some()));
}

/// Building either side must give the same answer, for every join type. This
/// is the property the planner's freedom to pick a build side rests on.
#[tokio::test]
async fn the_build_side_does_not_change_the_answer() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    for join_type in ["inner", "left", "right", "full"] {
        let base = match join_type {
            "left" => on_author().left_outer(),
            "right" => on_author().right_outer(),
            "full" => on_author().full_outer(),
            _ => on_author(),
        };
        let mut shapes = Vec::new();
        for build in [Side::Right, Side::Left] {
            let rows = txn
                .join(
                    &reader(1),
                    &authors(),
                    &books(),
                    &base.clone().using(JoinAlgorithm::Hash { build }),
                )
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            // Compared as a sorted multiset: a join promises no order, and
            // which side is built is exactly what changes it.
            let mut shape: Vec<String> = rows
                .iter()
                .map(|r| {
                    format!(
                        "{:?}|{:?}",
                        r.left.as_ref().and_then(|l| l.get(author_col("name"))),
                        r.right.as_ref().and_then(|b| b.get(book_col("title")))
                    )
                })
                .collect();
            shape.sort();
            shapes.push(shape);
        }
        assert_eq!(
            shapes[0], shapes[1],
            "{join_type} join disagreed with itself when the build side changed"
        );
    }
}

/// A nested loop cannot know which right rows nothing matched, so it must not
/// be used for a join that has to return them — and saying so beats returning
/// a quietly incomplete answer.
#[tokio::test]
async fn a_loop_is_refused_for_a_join_it_cannot_serve() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    for base in [on_author().right_outer(), on_author().full_outer()] {
        let err = txn
            .join(
                &reader(1),
                &authors(),
                &books(),
                &base.clone().using(JoinAlgorithm::NestedLoop),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, KernelError::JoinNotSupported { .. }),
            "got {err:?}"
        );

        // And the planner does not choose one on its own, even where the cost
        // model would otherwise prefer it.
        let narrow = base.left(Query::all().filter(Expr::eq(author_col("id"), Value::U64(1))));
        let plan = txn
            .explain_join(&reader(1), &authors(), &books(), &narrow)
            .unwrap();
        assert!(!plan.is_nested_loop(), "planner chose a loop: {plan}");
    }
}

/// An outer join still cannot show a row a policy hides. The row is not
/// "preserved as unmatched" either — it is simply not there, which is what
/// keeps a full outer join from being a way to enumerate hidden rows.
#[tokio::test]
async fn an_outer_join_does_not_preserve_hidden_rows() {
    let security = open().policy(Policy::new(
        "uk_only",
        AUTHORS,
        Action::ALL,
        |_: &SecurityContext| Expr::eq(author_col("country"), Value::Str("UK".into())),
    ));
    let (store, _) = store(security).await;
    let txn = store.begin().await.unwrap();

    let rows = txn
        .join(&reader(1), &authors(), &books(), &on_author().full_outer())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // Only Iain is visible. His book pairs; every other book is preserved with
    // no left side; Ursula appears nowhere at all — not even as an unmatched
    // left row.
    let names: Vec<String> = rows
        .iter()
        .filter_map(
            |r| match r.left.as_ref().and_then(|l| l.get(author_col("name"))) {
                Some(Value::Str(s)) => Some(s.clone()),
                _ => None,
            },
        )
        .collect();
    assert_eq!(names, vec!["Iain"], "a hidden author surfaced: {names:?}");
    assert_eq!(
        rows.len(),
        5,
        "one pair plus four books with no visible author"
    );
}

/// A condition spanning both sides: was the book published in the author's
/// lifetime? Neither table can answer that alone, which is the whole point.
#[tokio::test]
async fn a_condition_can_span_both_sides() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();
    let at = JoinSchema::of(&authors(), &books());

    let in_lifetime = || {
        Expr::compare_columns(
            at.right(book_col("published")),
            CmpOp::Lt,
            at.left(author_col("died")),
        )
    };

    let rows = txn
        .join(
            &reader(1),
            &authors(),
            &books(),
            &on_author().having(in_lifetime()),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        titles(&rows),
        vec!["A Wizard of Earthsea", "The Dispossessed"],
        "Consider Phlebas was published after its author died"
    );

    // Reversed, it selects exactly the complement.
    let posthumous = Expr::compare_columns(
        at.right(book_col("published")),
        CmpOp::Gt,
        at.left(author_col("died")),
    );
    let rows = txn
        .join(
            &reader(1),
            &authors(),
            &books(),
            &on_author().having(posthumous),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(titles(&rows), vec!["Consider Phlebas"]);
}

/// Every algorithm applies the condition the same way. A condition that one
/// algorithm honoured and another ignored would make the planner's freedom to
/// choose a correctness bug.
#[tokio::test]
async fn every_algorithm_applies_the_condition() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();
    let at = JoinSchema::of(&authors(), &books());
    let condition = Expr::compare_columns(
        at.right(book_col("published")),
        CmpOp::Lt,
        at.left(author_col("died")),
    );

    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::NestedLoop,
    ] {
        let rows = txn
            .join(
                &reader(1),
                &authors(),
                &books(),
                &on_author().having(condition.clone()).using(algorithm),
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(
            titles(&rows),
            vec!["A Wizard of Earthsea", "The Dispossessed"],
            "under {algorithm:?}"
        );
    }
}

/// The condition behaves like SQL's `ON`, not `WHERE`. A left row whose only
/// candidates are all rejected is *unmatched*, not gone — the difference that
/// trips people up in real SQL, so it had better be the difference here.
#[tokio::test]
async fn a_rejected_pair_leaves_an_unmatched_row_not_a_missing_one() {
    let (store, _) = store(open()).await;
    let at = JoinSchema::of(&authors(), &books());
    // Iain's only book is posthumous, so he pairs with nothing.
    let condition = Expr::compare_columns(
        at.right(book_col("published")),
        CmpOp::Lt,
        at.left(author_col("died")),
    );

    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::NestedLoop,
    ] {
        let txn = store.begin().await.unwrap();
        let rows = txn
            .join(
                &reader(1),
                &authors(),
                &books(),
                &on_author()
                    .left_outer()
                    .having(condition.clone())
                    .using(algorithm),
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();

        assert_eq!(rows.len(), 3, "under {algorithm:?}: {rows:?}");
        let unmatched: Vec<String> = rows
            .iter()
            .filter(|r| !r.is_matched())
            .filter_map(
                |r| match r.left.as_ref().and_then(|l| l.get(author_col("name"))) {
                    Some(Value::Str(s)) => Some(s.clone()),
                    _ => None,
                },
            )
            .collect();
        assert_eq!(
            unmatched,
            vec!["Iain"],
            "an author whose every candidate was rejected must come back \
             unmatched, not vanish, under {algorithm:?}"
        );
    }
}

/// An ordinal outside the joined space names no column. Reading it as null
/// would turn a typo into a condition nobody wrote, so it is refused.
#[tokio::test]
async fn a_condition_outside_the_joined_space_is_refused() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    let width = JoinSchema::of(&authors(), &books()).width();
    let join = on_author().having(Expr::eq(Ordinal(width), Value::U64(1)));
    let err = txn
        .join(&reader(1), &authors(), &books(), &join)
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::JoinNotSupported { .. }),
        "got {err:?}"
    );
}

/// The planner accounts for the condition when estimating how many rows come
/// back, and `EXPLAIN` shows the condition it is costing.
#[tokio::test]
async fn the_condition_shows_up_in_the_plan() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();
    let at = JoinSchema::of(&authors(), &books());

    let plain = txn
        .explain_join(&reader(1), &authors(), &books(), &on_author())
        .unwrap();
    assert!(plain.having.is_none(), "{plain}");

    let filtered = txn
        .explain_join(
            &reader(1),
            &authors(),
            &books(),
            &on_author().having(Expr::compare_columns(
                at.right(book_col("published")),
                CmpOp::Lt,
                at.left(author_col("died")),
            )),
        )
        .unwrap();
    assert!(filtered.having.is_some(), "{filtered}");
    assert!(
        filtered.estimated_rows < plain.estimated_rows,
        "a condition that rejects rows should lower the estimate: {} vs {}",
        filtered.estimated_rows,
        plain.estimated_rows
    );
}

/// The same type rule holds for a join's condition, where a mismatch is easier
/// to write because the two columns come from different schemas.
#[tokio::test]
async fn a_cross_side_comparison_between_types_is_refused() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();
    let at = JoinSchema::of(&authors(), &books());

    // authors.name is Str; books.published is I64.
    let join = on_author().having(Expr::compare_columns(
        at.left(author_col("name")),
        CmpOp::Lt,
        at.right(book_col("published")),
    ));
    let err = txn
        .join(&reader(1), &authors(), &books(), &join)
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::ComparisonTypeMismatch { .. }),
        "got {err:?}"
    );
}
