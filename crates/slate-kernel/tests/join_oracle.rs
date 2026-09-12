//! Every join algorithm returns the same rows, and they are the right rows.
//!
//! `oracle.rs` does this for single-table reads. A join has strictly more ways
//! to go wrong: four join types, three algorithm choices (hash building either
//! side, or a nested loop), and outer joins that must invent null rows for
//! whichever side did not match. The interesting failures are asymmetric —
//! building the left side and building the right side are different code, and
//! a right outer join is the one that exercises the path nobody writes by hand.
//!
//! Two properties:
//!
//! 1. **Every algorithm agrees.** Forcing hash-build-left, hash-build-right and
//!    nested-loop over the same join must give the same rows. This assumes
//!    nothing about what a join means, only that the choice is not visible.
//! 2. **The result is a real join.** Compared against a nested loop written out
//!    in the test over the rows themselves, so a bug shared by all three
//!    algorithms is still caught.
//!
//! Rows are compared as multisets. A join has no inherent order, and the three
//! algorithms genuinely produce different ones — requiring a specific order
//! would be testing the implementation rather than the semantics.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_kernel::{
    Action, CmpOp, Expr, Grant, Join, JoinAlgorithm, JoinType, JoinedRow, Query, RecordStore,
    SecurityCatalog, SecurityContext, Side, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);
const REVIEWS: TableId = TableId(3);

/// Small enough that a brute-force join in the test is instant, large enough
/// that keys repeat on both sides — which is where a hash join has to emit a
/// cross product and a nested loop has to not stop at the first match.
const AUTHOR_ROWS: u64 = 12;
const BOOK_ROWS: u64 = 30;

fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("id", ValueType::U64)
        .column("region", ValueType::Str)
        .nullable_column("rank", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_region", IndexId(10)).column("region"))
        .build()
        .expect("valid schema")
}

fn books() -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("id", ValueType::U64)
        .column("author_id", ValueType::U64)
        .column("pages", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_author", IndexId(20)).column("author_id"))
        .build()
        .expect("valid schema")
}

/// A third table, so a chain has somewhere to go. Reviews point at books,
/// which point at authors — the ordinary shape, and the one where a chain step
/// has to join back to the table *before* it rather than to the first.
fn reviews() -> TableDef {
    TableDef::builder("reviews", REVIEWS)
        .column("id", ValueType::U64)
        .column("book_id", ValueType::U64)
        .column("stars", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_book", IndexId(30)).column("book_id"))
        .build()
        .expect("valid schema")
}

const REVIEW_ROWS: u64 = 40;

fn review(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        // Points past BOOK_ROWS for some ids, so a step can match nothing.
        Value::U64(id % 35),
        Value::I64((id % 5) as i64 + 1),
    ])
}

fn review_col(name: &str) -> Ordinal {
    reviews().ordinal_of(name).expect("column exists")
}

fn author_col(name: &str) -> Ordinal {
    authors().ordinal_of(name).expect("column exists")
}

fn book_col(name: &str) -> Ordinal {
    books().ordinal_of(name).expect("column exists")
}

/// Authors 0..12. Some have no books; `rank` is null on every third, so an
/// outer join's invented nulls cannot be told apart from real ones by accident.
fn author(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("r{}", id % 3)),
        if id.is_multiple_of(3) {
            Value::Null
        } else {
            Value::I64((id % 5) as i64)
        },
    ])
}

/// Books pointing at authors 0..15 — so some point at authors that do not
/// exist, which is what makes a right outer join return anything.
fn book(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::U64(id % 15),
        Value::I64((id % 7) as i64 * 10),
    ])
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([authors(), books(), reviews()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::ALL))
        .grant(Grant::new("r", BOOKS, Action::ALL))
        .grant(Grant::new("r", REVIEWS, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let root = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    for id in 0..AUTHOR_ROWS {
        txn.insert(&root, &authors(), &author(id)).await.unwrap();
    }
    for id in 0..BOOK_ROWS {
        txn.insert(&root, &books(), &book(id)).await.unwrap();
    }
    for id in 0..REVIEW_ROWS {
        txn.insert(&root, &reviews(), &review(id)).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

// --- generators -----------------------------------------------------------

fn join_type() -> impl Strategy<Value = JoinType> {
    prop_oneof![
        Just(JoinType::Inner),
        Just(JoinType::Left),
        Just(JoinType::Right),
        Just(JoinType::Full),
    ]
}

/// Filters on the left side, chosen to leave a varied number of authors.
fn left_filter() -> impl Strategy<Value = Expr> {
    prop_oneof![
        Just(Expr::True),
        (0..3u64).prop_map(|r| Expr::eq(author_col("region"), Value::Str(format!("r{r}")))),
        (0..12u64).prop_map(|n| Expr::compare(author_col("id"), CmpOp::Lt, Value::U64(n))),
        Just(Expr::is_null(author_col("rank"))),
        Just(Expr::Not(Box::new(Expr::is_null(author_col("rank"))))),
        (0..5i64).prop_map(|n| Expr::compare(author_col("rank"), CmpOp::Ge, Value::I64(n))),
    ]
}

fn right_filter() -> impl Strategy<Value = Expr> {
    prop_oneof![
        Just(Expr::True),
        (0..70i64).prop_map(|n| Expr::compare(book_col("pages"), CmpOp::Gt, Value::I64(n))),
        (0..30u64).prop_map(|n| Expr::compare(book_col("id"), CmpOp::Lt, Value::U64(n))),
        (0..15u64).prop_map(|n| Expr::eq(book_col("author_id"), Value::U64(n))),
    ]
}

/// A whole join: type, a filter per side, and an optional window.
fn any_join() -> impl Strategy<Value = (JoinType, Expr, Expr, Option<usize>, usize)> {
    (
        join_type(),
        left_filter(),
        right_filter(),
        prop_oneof![Just(None), (0..20usize).prop_map(Some)],
        0..3usize,
    )
}

fn build(kind: JoinType, left: &Expr, right: &Expr, limit: Option<usize>, offset: usize) -> Join {
    let mut join = Join::equating(author_col("id"), book_col("author_id"))
        .left(Query::all().filter(left.clone()))
        .right(Query::all().filter(right.clone()))
        .offset(offset);
    join = match kind {
        JoinType::Inner => join,
        JoinType::Left => join.left_outer(),
        JoinType::Right => join.right_outer(),
        JoinType::Full => join.full_outer(),
    };
    if let Some(limit) = limit {
        join = join.limit(limit);
    }
    join
}

/// The algorithms that can serve this join type, hash-build-left first.
///
/// A nested loop streams the left side and probes the right, so it never sees
/// a right row that matched nothing — it *cannot* preserve unmatched right
/// rows. The engine refuses rather than returning a quietly wrong answer, which
/// [`a_nested_loop_refuses_a_right_outer_join`] pins.
fn algorithms(kind: JoinType) -> Vec<JoinAlgorithm> {
    let mut out = vec![
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::Hash { build: Side::Right },
    ];
    if !kind.preserves(Side::Right) {
        out.push(JoinAlgorithm::NestedLoop);
    }
    out
}

// --- helpers --------------------------------------------------------------

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// A joined row reduced to the two primary keys, which identify it uniquely.
/// `None` on a side means the join invented a null row there.
fn keys(rows: &[JoinedRow]) -> Vec<(Option<u64>, Option<u64>)> {
    let mut out: Vec<(Option<u64>, Option<u64>)> = rows
        .iter()
        .map(|r| {
            let left = r.left.as_ref().and_then(|row| match row.values()[0] {
                Value::U64(id) => Some(id),
                _ => None,
            });
            let right = r.right.as_ref().and_then(|row| match row.values()[0] {
                Value::U64(id) => Some(id),
                _ => None,
            });
            (left, right)
        })
        .collect();
    // A multiset: a join has no inherent order and the algorithms differ.
    out.sort_unstable();
    out
}

async fn run(store: &RecordStore<MemoryStore>, join: &Join) -> Vec<JoinedRow> {
    let txn = store.begin().await.unwrap();
    txn.join(&root(), &authors(), &books(), join)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
}

/// The join, written out over the rows themselves.
///
/// Deliberately the slowest possible implementation: every left row against
/// every right row. A bug shared by all three algorithms is only visible
/// against something that shares no code with them.
fn brute_force(
    kind: JoinType,
    left_filter: &Expr,
    right_filter: &Expr,
) -> Vec<(Option<u64>, Option<u64>)> {
    let lefts: Vec<Row> = (0..AUTHOR_ROWS)
        .map(author)
        .filter(|r| left_filter.admits(r))
        .collect();
    let rights: Vec<Row> = (0..BOOK_ROWS)
        .map(book)
        .filter(|r| right_filter.admits(r))
        .collect();

    let id = |row: &Row| match row.values()[0] {
        Value::U64(n) => n,
        ref other => panic!("id was {other:?}"),
    };
    let matches =
        |l: &Row, r: &Row| l.values()[author_col("id").0] == r.values()[book_col("author_id").0];

    let mut out = Vec::new();
    let mut right_matched = vec![false; rights.len()];

    for l in &lefts {
        let mut any = false;
        for (i, r) in rights.iter().enumerate() {
            if matches(l, r) {
                out.push((Some(id(l)), Some(id(r))));
                right_matched[i] = true;
                any = true;
            }
        }
        // A left row that matched nothing survives only on a left or full join.
        if !any && matches!(kind, JoinType::Left | JoinType::Full) {
            out.push((Some(id(l)), None));
        }
    }
    // And the mirror, for the right side.
    if matches!(kind, JoinType::Right | JoinType::Full) {
        for (i, r) in rights.iter().enumerate() {
            if !right_matched[i] {
                out.push((None, Some(id(r))));
            }
        }
    }
    out.sort_unstable();
    out
}

// --- the properties -------------------------------------------------------

/// Hash-build-left, hash-build-right and nested-loop all return the same rows.
///
/// No window is applied here: `LIMIT` over an unordered join legitimately
/// returns different rows per algorithm, so mixing it in would test nothing.
#[test]
fn every_join_algorithm_returns_the_same_rows() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|((kind, left, right, _limit, _offset) in any_join())| {
        let base = build(kind, &left, &right, None, 0);

        let expected = keys(&rt.block_on(run(&store, &base.clone().using(JoinAlgorithm::Hash {
            build: Side::Left,
        }))));

        for algorithm in algorithms(kind).into_iter().skip(1) {
            let got = keys(&rt.block_on(run(&store, &base.clone().using(algorithm))));
            prop_assert_eq!(
                &got, &expected,
                "{:?} disagreed with hash-build-left on a {:?} join, left={:?} right={:?}",
                algorithm, kind, left, right
            );
        }

        // And the planner's own choice, with nothing forced.
        let chosen = keys(&rt.block_on(run(&store, &base)));
        prop_assert_eq!(
            &chosen, &expected,
            "the planner's choice disagreed on a {:?} join",
            kind
        );
    });
}

/// The join is the join: every algorithm matches a brute-force nested loop
/// written over the rows themselves.
#[test]
fn a_join_returns_what_a_brute_force_join_returns() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|((kind, left, right, _limit, _offset) in any_join())| {
        let expected = brute_force(kind, &left, &right);
        let base = build(kind, &left, &right, None, 0);

        for algorithm in algorithms(kind) {
            let got = keys(&rt.block_on(run(&store, &base.clone().using(algorithm))));
            prop_assert_eq!(
                &got, &expected,
                "{:?} on a {:?} join returned the wrong rows for left={:?} right={:?}",
                algorithm, kind, left, right
            );
        }
    });
}

/// A window over a join returns *some* window of the join, not rows from
/// nowhere.
///
/// `LIMIT` without an order is genuinely ambiguous, so the assertion is the
/// strongest one that is actually true: the rows returned are a subset of the
/// full join, and there are as many as there should be.
#[test]
fn a_windowed_join_returns_a_subset_of_the_whole() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|((kind, left, right, limit, offset) in any_join())| {
        let whole = brute_force(kind, &left, &right);
        let windowed = keys(&rt.block_on(run(
            &store,
            &build(kind, &left, &right, limit, offset),
        )));

        let expected_len = whole
            .len()
            .saturating_sub(offset)
            .min(limit.unwrap_or(usize::MAX));
        prop_assert_eq!(
            windowed.len(),
            expected_len,
            "a {:?} join of {} rows with limit={:?} offset={} returned {}",
            kind,
            whole.len(),
            limit,
            offset,
            windowed.len()
        );

        // Every row returned is a row the join actually contains.
        for row in &windowed {
            prop_assert!(
                whole.contains(row),
                "a windowed {:?} join returned {row:?}, which is not in the join",
                kind
            );
        }
    });
}

/// Asking for a nested loop on a right or full outer join is refused.
///
/// The alternative to refusing would be to return the inner-join rows and
/// silently drop the unmatched right ones — an answer that looks right and is
/// short. This is the engine declining to guess, and it is worth pinning
/// because a later change that "supports" the combination by quietly falling
/// back to a hash join would also pass every other test in this file.
#[tokio::test]
async fn a_nested_loop_refuses_a_right_outer_join() {
    let store = seeded().await;
    let (left, right) = (authors(), books());

    for kind in [JoinType::Right, JoinType::Full] {
        let join = build(kind, &Expr::True, &Expr::True, None, 0).using(JoinAlgorithm::NestedLoop);
        let txn = store.begin().await.unwrap();
        let outcome = txn.join(&root(), &left, &right, &join).await;
        assert!(
            outcome.is_err(),
            "a nested loop claimed to serve a {kind:?} outer join"
        );
    }

    // And the combinations that *are* legal still work.
    for kind in [JoinType::Inner, JoinType::Left] {
        let join = build(kind, &Expr::True, &Expr::True, None, 0).using(JoinAlgorithm::NestedLoop);
        let txn = store.begin().await.unwrap();
        txn.join(&root(), &left, &right, &join)
            .await
            .unwrap_or_else(|e| panic!("a nested loop refused a {kind:?} join: {e}"));
    }
}

/// The generators have to produce joins that actually join.
///
/// A suite where every generated join is empty passes everything and proves
/// nothing, and with four join types and two filters that is easy to do by
/// accident.
#[test]
fn the_generated_joins_produce_a_range_of_sizes() {
    let collected = std::cell::RefCell::new(Vec::new());
    proptest!(ProptestConfig::with_cases(96), |((kind, left, right, _l, _o) in any_join())| {
        collected
            .borrow_mut()
            .push(brute_force(kind, &left, &right).len());
    });
    let sizes = collected.into_inner();

    let empty = sizes.iter().filter(|n| **n == 0).count();
    let big = sizes.iter().filter(|n| **n > 5).count();
    assert!(
        empty * 4 < sizes.len(),
        "{empty} of {} generated joins were empty",
        sizes.len()
    );
    assert!(
        big * 2 > sizes.len(),
        "only {big} of {} generated joins had more than five rows",
        sizes.len()
    );
}

// --- chains ---------------------------------------------------------------
//
// A chain of three tables is not three two-way joins glued together: it keeps
// one accumulated row and joins each table onto it, with its own plan per step.
// So it is separate code from `join`, and the property that ties it down is
// that it must nonetheless agree with what two-way joins would give.

/// One chain step, inner or left-outer.
fn step(accumulated: Ordinal, own: Ordinal, query: Query, outer: bool) -> slate_kernel::JoinStep {
    let step = slate_kernel::JoinStep::equating(accumulated, own).query(query);
    if outer { step.left_outer() } else { step }
}

/// A three-table chain returns what a hand-written triple loop returns.
///
/// Authors → books → reviews, which is the shape an ORM produces for "load
/// these records and their children and their children". Inner and left steps
/// are covered; a right or full step on a chain preserves rows of the new table
/// with *every* earlier table absent, which is a different question and has its
/// own test in `chain.rs`.
#[test]
fn a_three_table_chain_agrees_with_a_triple_loop() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let schema = slate_kernel::JoinSchema::over([&authors(), &books(), &reviews()]);

    proptest!(|(
        left in left_filter(),
        right in right_filter(),
        outer in any::<bool>(),
    )| {
        let step_kind = if outer { JoinType::Left } else { JoinType::Inner };

        let chain = slate_kernel::Chain::from(Query::all().filter(left.clone()))
            .join(step(
                schema.at(0, author_col("id")),
                book_col("author_id"),
                Query::all().filter(right.clone()),
                outer,
            ))
            .join(step(
                schema.at(1, book_col("id")),
                review_col("book_id"),
                Query::all(),
                outer,
            ));

        let got = rt.block_on(async {
            let txn = store.begin().await.unwrap();
            let rows = txn
                .chain(&root(), &[&authors(), &books(), &reviews()], &chain)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            let mut keys: Vec<(Option<u64>, Option<u64>, Option<u64>)> = rows
                .iter()
                .map(|r| {
                    let id = |p: usize| {
                        r.at(p).and_then(|row| match row.values()[0] {
                            Value::U64(n) => Some(n),
                            _ => None,
                        })
                    };
                    (id(0), id(1), id(2))
                })
                .collect();
            keys.sort_unstable();
            keys
        });

        prop_assert_eq!(
            got,
            triple_loop(step_kind, &left, &right),
            "a {:?} chain disagreed for left={:?} right={:?}",
            step_kind,
            left,
            right
        );
    });
}

/// The chain, written out as three nested loops.
fn triple_loop(
    step_kind: JoinType,
    left_filter: &Expr,
    right_filter: &Expr,
) -> Vec<(Option<u64>, Option<u64>, Option<u64>)> {
    let id = |row: &Row| match row.values()[0] {
        Value::U64(n) => n,
        ref other => panic!("id was {other:?}"),
    };
    let authors_kept: Vec<Row> = (0..AUTHOR_ROWS)
        .map(author)
        .filter(|r| left_filter.admits(r))
        .collect();
    let books_kept: Vec<Row> = (0..BOOK_ROWS)
        .map(book)
        .filter(|r| right_filter.admits(r))
        .collect();
    let reviews_all: Vec<Row> = (0..REVIEW_ROWS).map(review).collect();
    let keep_unmatched = matches!(step_kind, JoinType::Left);

    let mut out = Vec::new();
    for a in &authors_kept {
        // Step one: books whose author_id equals this author's id.
        let matched: Vec<&Row> = books_kept
            .iter()
            .filter(|b| b.values()[book_col("author_id").0] == a.values()[author_col("id").0])
            .collect();
        if matched.is_empty() {
            if keep_unmatched {
                out.push((Some(id(a)), None, None));
            }
            continue;
        }
        for b in matched {
            // Step two: reviews of that book.
            let reviewed: Vec<&Row> = reviews_all
                .iter()
                .filter(|v| v.values()[review_col("book_id").0] == b.values()[book_col("id").0])
                .collect();
            if reviewed.is_empty() {
                if keep_unmatched {
                    out.push((Some(id(a)), Some(id(b)), None));
                }
                continue;
            }
            for v in reviewed {
                out.push((Some(id(a)), Some(id(b)), Some(id(v))));
            }
        }
    }
    out.sort_unstable();
    out
}

/// A chain of two is a join of two.
///
/// The two code paths exist for different reasons and must not have drifted:
/// anything `Chain` does with one step, `Join` must do the same way.
#[test]
fn a_one_step_chain_matches_the_equivalent_join() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let schema = slate_kernel::JoinSchema::of(&authors(), &books());

    proptest!(|(left in left_filter(), right in right_filter(), outer in any::<bool>())| {
        let kind = if outer { JoinType::Left } else { JoinType::Inner };

        let by_join = keys(&rt.block_on(run(&store, &build(kind, &left, &right, None, 0))));

        let chain = slate_kernel::Chain::from(Query::all().filter(left.clone())).join(step(
            schema.at(0, author_col("id")),
            book_col("author_id"),
            Query::all().filter(right.clone()),
            outer,
        ));
        let by_chain = rt.block_on(async {
            let txn = store.begin().await.unwrap();
            let rows = txn
                .chain(&root(), &[&authors(), &books()], &chain)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            let mut keys: Vec<(Option<u64>, Option<u64>)> = rows
                .iter()
                .map(|r| {
                    let id = |p: usize| {
                        r.at(p).and_then(|row| match row.values()[0] {
                            Value::U64(n) => Some(n),
                            _ => None,
                        })
                    };
                    (id(0), id(1))
                })
                .collect();
            keys.sort_unstable();
            keys
        });

        prop_assert_eq!(
            by_chain, by_join,
            "a one-step {:?} chain disagreed with the same {:?} join",
            kind, kind
        );
    });
}
