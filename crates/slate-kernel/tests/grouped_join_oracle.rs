//! Grouping over a join, and ordering over groups.
//!
//! Two gaps closed at once, and they fail in opposite ways, so they get
//! different oracles.
//!
//! **A grouped join** consumes the joined row stream rather than a single
//! table's cursor, in the ordinal space [`JoinSchema`] defines. Everything that
//! can go wrong there produces a *plausible* group: a key resolved to the wrong
//! side reads as null and collapses every row into one group; a projection
//! narrowed too far reads as null and does the same; an outer join's missing
//! side is null and must group as null rather than be dropped. None of those is
//! an error, and all of them return a table of numbers that looks like an
//! answer. So the property is a hand-written fold over a join materialised with
//! no projection at all — [`a_grouped_join_agrees_with_grouping_it_by_hand`] —
//! and, separately, that every join algorithm and the planner's own choice give
//! the same groups, which assumes nothing about what a group means.
//!
//! **Ordering over groups** cannot use the row path's bounded top-N heap, and
//! the reason is worth stating because it is not "it did not fit". The heap
//! exists to hold `k` rows instead of `n`; groups are all in memory before the
//! first one can be returned, because nothing can be known about the last group
//! until the last row is read. A heap there would save nothing and be a second
//! sorting implementation. What *is* shared is the comparator, so direction and
//! null placement cannot drift between rows and groups — and the oracle is a
//! comparator written out again here, over the group set sorted in memory.
//!
//! Ties are pinned rather than left arbitrary, the same way every oracle here
//! pins them: the groups arrive in encoded-key order and the sort is stable, so
//! `ORDER BY count(*) DESC LIMIT 3` over three groups that tie has one answer
//! and not six. That one is stated rather than demonstrated, and honestly so —
//! see [`groups_that_tie_keep_the_order_they_arrived_in`], which cannot fail
//! for a reason that has nothing to do with this code.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_kernel::latency::{LatencyProfile, LatencyStore};
use slate_kernel::{
    Action, Aggregate, CmpOp, Expr, Grant, Group, Grouping, Join, JoinAlgorithm, JoinSchema,
    JoinType, JoinedRow, NullsOrder, Policy, Principal, Query, RecordStore, SecurityCatalog,
    SecurityContext, Side, SortKey, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);

/// Small enough that a fold written out in the test is instant, large enough
/// that every join key repeats on both sides and several groups tie on every
/// aggregate.
const AUTHOR_ROWS: u64 = 12;

/// The index on `books.author_id`, which holds the join key and the primary
/// key and nothing else.
const BY_AUTHOR: IndexId = IndexId(20);
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
        .column("shelf", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_author", BY_AUTHOR).column("author_id"))
        .build()
        .expect("valid schema")
}

fn author_col(name: &str) -> Ordinal {
    authors().ordinal_of(name).expect("column exists")
}

fn book_col(name: &str) -> Ordinal {
    books().ordinal_of(name).expect("column exists")
}

fn at() -> JoinSchema {
    JoinSchema::of(&authors(), &books())
}

/// Authors 0..12. `rank` is null on every third, so a group keyed on it has a
/// null key — which is a group, not an omission, and is where a fold that
/// skipped nulls would differ.
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

/// Books pointing at authors 0..15, so some point at authors that do not
/// exist — which is what makes a right or full outer join produce rows whose
/// left side is missing, and therefore groups keyed on null.
fn book(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::U64(id % 15),
        Value::I64((id % 7) as i64 * 10),
        Value::Str(format!("s{}", id % 4)),
    ])
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([authors(), books()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::ALL))
        .grant(Grant::new("r", BOOKS, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let root = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    for id in 0..AUTHOR_ROWS {
        txn.insert(&root, &authors(), &author(id)).await.unwrap();
    }
    for id in 0..BOOK_ROWS {
        txn.insert(&root, &books(), &book(id)).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
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

fn left_filter() -> impl Strategy<Value = Expr> {
    prop_oneof![
        Just(Expr::True),
        (0..3u64).prop_map(|r| Expr::eq(author_col("region"), Value::Str(format!("r{r}")))),
        (0..12u64).prop_map(|n| Expr::compare(author_col("id"), CmpOp::Lt, Value::U64(n))),
        Just(Expr::Not(Box::new(Expr::is_null(author_col("rank"))))),
    ]
}

fn right_filter() -> impl Strategy<Value = Expr> {
    prop_oneof![
        Just(Expr::True),
        (0..70i64).prop_map(|n| Expr::compare(book_col("pages"), CmpOp::Gt, Value::I64(n))),
        (0..4u64).prop_map(|s| Expr::eq(book_col("shelf"), Value::Str(format!("s{s}")))),
    ]
}

/// Grouping keys, drawn from both sides and from both a nullable column and a
/// non-nullable one. The empty list is in there deliberately: no `GROUP BY` is
/// one group over everything, and it is the case a hash map keyed on an empty
/// byte string gets wrong by returning nothing.
fn group_columns() -> impl Strategy<Value = Vec<Ordinal>> {
    let schema = at();
    prop_oneof![
        Just(Vec::new()),
        Just(vec![schema.left(author_col("region"))]),
        // Null a third of the time on the left, and null for every row a right
        // or full outer join preserved.
        Just(vec![schema.left(author_col("rank"))]),
        Just(vec![schema.right(book_col("shelf"))]),
        // Across both sides, which is the thing a caller could not previously
        // express at all.
        Just(vec![
            schema.left(author_col("region")),
            schema.right(book_col("shelf")),
        ]),
        Just(vec![schema.right(book_col("author_id"))]),
        // Thirty one-row groups, so a sort by `count(*)` has thirty ties. Small
        // tie sets are sorted by insertion sort whichever algorithm is asked
        // for, and insertion sort is stable — a fixture that never gets past
        // that size cannot tell a stable sort from an unstable one.
        Just(vec![schema.right(book_col("id"))]),
    ]
}

fn aggregates() -> impl Strategy<Value = Vec<Aggregate>> {
    let schema = at();
    let pages = schema.right(book_col("pages"));
    let rank = schema.left(author_col("rank"));
    let author_id = schema.right(book_col("author_id"));
    prop_oneof![
        Just(vec![Aggregate::Count]),
        Just(vec![Aggregate::Count, Aggregate::CountColumn(rank)]),
        Just(vec![Aggregate::Min(pages), Aggregate::Max(pages)]),
        Just(vec![Aggregate::Sum(pages), Aggregate::Avg(pages)]),
        Just(vec![
            Aggregate::CountDistinct(author_id),
            Aggregate::Count,
            Aggregate::Max(rank),
        ]),
    ]
}

/// The algorithms that can serve this join type, hash-build-left first.
///
/// A nested loop streams the left side and never sees a right row that matched
/// nothing, so it cannot preserve the right side. `join_oracle.rs` pins the
/// refusal; here it is simply not offered.
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

/// A condition only a formed pair can answer, over columns neither side's key
/// includes.
///
/// It is here because it is a *third* thing the narrowed projections have to
/// account for, beyond the grouping and the join keys, and it is the one that
/// nothing else would notice: a key column is read whatever the projection says
/// and a primary key arrives decoded on every path, so dropping either from the
/// projection is caught by accident. `rank` and `pages` are neither, and `rank`
/// is null a third of the time, so the pair is rejected under three-valued
/// logic rather than by a comparison.
fn cross_condition() -> impl Strategy<Value = Expr> {
    let schema = at();
    prop_oneof![
        2 => Just(Expr::True),
        1 => Just(Expr::compare_columns(
            schema.left(author_col("rank")),
            CmpOp::Gt,
            schema.right(book_col("pages")),
        )),
    ]
}

fn build(kind: JoinType, left: &Expr, right: &Expr, having: &Expr) -> Join {
    let join = Join::equating(author_col("id"), book_col("author_id"))
        .left(Query::all().filter(left.clone()))
        .right(Query::all().filter(right.clone()))
        .having(having.clone());
    match kind {
        JoinType::Inner => join,
        JoinType::Left => join.left_outer(),
        JoinType::Right => join.right_outer(),
        JoinType::Full => join.full_outer(),
    }
}

// --- the model ------------------------------------------------------------

/// A joined pair as one row, written out here rather than through
/// `JoinedRow::flatten`.
///
/// The rule is a sentence long and the implementation is the thing under test,
/// so restating it costs four lines and buys an independent opinion — including
/// on the part that matters, which is that a missing side is that side's width
/// in nulls and not zero columns.
fn flatten(pair: &JoinedRow) -> Vec<Value> {
    let widths = [authors().columns().len(), books().columns().len()];
    let mut out = Vec::new();
    for (side, width) in [&pair.left, &pair.right].into_iter().zip(widths) {
        match side {
            Some(row) => out.extend(row.values().iter().cloned()),
            None => out.extend(core::iter::repeat_n(Value::Null, width)),
        }
    }
    out
}

/// One aggregate, folded over the rows of one group by hand.
///
/// The `match` is exhaustive on purpose: adding an [`Aggregate`] variant fails
/// to compile here rather than going untested.
fn fold_one(spec: Aggregate, rows: &[&Vec<Value>]) -> Value {
    let read = |ordinal: Ordinal| -> Vec<Value> {
        rows.iter()
            .filter_map(|row| row.get(ordinal.0).cloned())
            .filter(|value| !value.is_null())
            .collect()
    };
    let total = |values: &[Value]| -> Option<(f64, bool, i64)> {
        if values.is_empty() {
            return None;
        }
        let mut real = 0.0f64;
        let mut integer = 0i64;
        let mut is_real = false;
        for value in values {
            match value {
                Value::I64(v) => integer += v,
                Value::U64(v) => integer += *v as i64,
                Value::F64(v) => {
                    is_real = true;
                    real += v;
                }
                other => panic!("not summable: {other:?}"),
            }
        }
        Some((real + integer as f64, is_real, integer))
    };
    match spec {
        Aggregate::Count => Value::U64(rows.len() as u64),
        Aggregate::CountColumn(c) => Value::U64(read(c).len() as u64),
        Aggregate::Min(c) => read(c).into_iter().min().unwrap_or(Value::Null),
        Aggregate::Max(c) => read(c).into_iter().max().unwrap_or(Value::Null),
        Aggregate::Sum(c) => match total(&read(c)) {
            None => Value::Null,
            Some((real, true, _)) => Value::F64(real),
            Some((_, false, integer)) => Value::I64(integer),
        },
        Aggregate::Avg(c) => {
            let values = read(c);
            match total(&values) {
                None => Value::Null,
                Some((sum, _, _)) => Value::F64(sum / values.len() as f64),
            }
        }
        Aggregate::CountDistinct(c) => {
            let mut values = read(c);
            values.sort();
            values.dedup();
            Value::U64(values.len() as u64)
        }
    }
}

/// Group the rows by hand: a linear scan rather than a hash map, and sorted by
/// the key values rather than by their encoding.
///
/// Both differences are the point. The implementation hashes an encoded key and
/// sorts on the bytes; this compares `Value`s and sorts on them. That the two
/// agree is the codec's central property — byte order equals value order — so
/// using it here is a second statement of the answer rather than the same one.
fn fold(rows: &[Vec<Value>], group: &[Ordinal], aggregates: &[Aggregate]) -> Vec<Group> {
    let mut buckets: Vec<(Vec<Value>, Vec<&Vec<Value>>)> = Vec::new();
    for row in rows {
        let key: Vec<Value> = group
            .iter()
            .map(|c| row.get(c.0).cloned().unwrap_or(Value::Null))
            .collect();
        match buckets.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, members)) => members.push(row),
            None => buckets.push((key, vec![row])),
        }
    }
    buckets.sort_by(|a, b| a.0.cmp(&b.0));
    buckets
        .into_iter()
        .map(|(key, members)| Group {
            key,
            values: aggregates
                .iter()
                .map(|spec| fold_one(*spec, &members))
                .collect(),
        })
        .collect()
}

/// The sort rule, stated independently of the executor's comparator.
///
/// The same shape as `oracle.rs`'s, and written out again for the same reason:
/// two independent statements of one rule, including the part people get wrong,
/// which is that null placement is absolute and is not flipped by a descending
/// sort.
fn compare(left: &Group, right: &Group, keys: &[SortKey]) -> core::cmp::Ordering {
    use core::cmp::Ordering;
    let (left, right) = (left.as_row(), right.as_row());
    for key in keys {
        let (Some(a), Some(b)) = (left.get(key.column), right.get(key.column)) else {
            continue;
        };
        let ordering = match (a.is_null(), b.is_null()) {
            (true, true) => Ordering::Equal,
            (true, false) | (false, true) => {
                let nulls_low = matches!(key.nulls, NullsOrder::First);
                return if a.is_null() == nulls_low {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (false, false) => match key.direction {
                Direction::Asc => a.cmp(b),
                Direction::Desc => b.cmp(a),
            },
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

// --- helpers --------------------------------------------------------------

async fn joined_rows(store: &RecordStore<MemoryStore>, join: &Join) -> Vec<Vec<Value>> {
    let txn = store.begin().await.unwrap();
    txn.join(&root(), &authors(), &books(), join)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .iter()
        .map(flatten)
        .collect()
}

async fn grouped(store: &RecordStore<MemoryStore>, join: &Join, grouping: &Grouping) -> Vec<Group> {
    let txn = store.begin().await.unwrap();
    txn.group_by_join(&root(), &authors(), &books(), join, grouping)
        .await
        .unwrap()
}

// --- the properties -------------------------------------------------------

/// A grouped join returns what grouping the join by hand returns.
///
/// The join it is compared against is materialised with no projection, so a
/// narrowing that dropped a column the grouping needed shows up as a group
/// keyed on null rather than as an error.
#[test]
fn a_grouped_join_agrees_with_grouping_it_by_hand() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|(
        kind in join_type(),
        left in left_filter(),
        right in right_filter(),
        having in cross_condition(),
        group in group_columns(),
        aggregates in aggregates(),
    )| {
        let join = build(kind, &left, &right, &having);
        let rows = rt.block_on(joined_rows(&store, &join));
        let expected = fold(&rows, &group, &aggregates);

        let grouping = Grouping::by(group.iter().copied(), &aggregates);
        let got = rt.block_on(grouped(&store, &join, &grouping));
        prop_assert_eq!(
            &got, &expected,
            "grouping {:?} by {:?} over a {:?} join disagreed with the fold",
            aggregates, group, kind
        );
    });
}

/// Every join algorithm produces the same groups.
///
/// Nothing is assumed about what a group is: only that which side the planner
/// chose to build cannot be visible in the answer. A hash build flags its
/// buckets and drains the unprobed ones at the end, a nested loop streams —
/// they see the rows in genuinely different orders, and grouping must not care.
#[test]
fn every_join_algorithm_produces_the_same_groups() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|(
        kind in join_type(),
        left in left_filter(),
        right in right_filter(),
        having in cross_condition(),
        group in group_columns(),
        aggregates in aggregates(),
    )| {
        let grouping = Grouping::by(group.iter().copied(), &aggregates);
        let mut expected: Option<Vec<Group>> = None;
        for algorithm in algorithms(kind) {
            let mut join = build(kind, &left, &right, &having);
            join.force = Some(algorithm);
            let got = rt.block_on(grouped(&store, &join, &grouping));
            match &expected {
                None => expected = Some(got),
                Some(first) => prop_assert_eq!(
                    &got, first,
                    "{:?} disagreed with the first algorithm on a {:?} join",
                    algorithm, kind
                ),
            }
        }
        // The unhinted plan is a fourth answer that has to agree with the rest.
        let got = rt.block_on(grouped(&store, &build(kind, &left, &right, &having), &grouping));
        prop_assert_eq!(Some(got), expected);
    });
}

/// `HAVING`, `ORDER BY`, `LIMIT` and `OFFSET` over groups agree with filtering,
/// sorting and slicing the group set in memory.
///
/// The order of the four is SQL's and is the thing most easily got wrong:
/// windowing before ordering returns a different set of groups rather than the
/// same set differently arranged.
#[test]
fn ordering_and_paging_groups_agrees_with_doing_it_in_memory() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|(
        kind in join_type(),
        left in left_filter(),
        right in right_filter(),
        having in cross_condition(),
        group in group_columns(),
        aggregates in aggregates(),
        // Whether each sort key names a group key or an aggregate, drawn
        // independently rather than by position. Keying on the group key first
        // would decide every comparison outright — the keys are distinct by
        // construction — and a sort whose first key never ties says nothing
        // about how ties are broken.
        keys in proptest::collection::vec(any::<bool>(), 0..3),
        descending in any::<bool>(),
        nulls_last in any::<bool>(),
        limit in prop_oneof![Just(None), (0..6usize).prop_map(Some)],
        offset in 0..3usize,
        floor in 0..4u64,
    )| {
        let join = build(kind, &left, &right, &having);
        let base = Grouping::by(group.iter().copied(), &aggregates);

        // `HAVING count(*) >= floor`, which is the shape everybody writes and
        // the one that needs the aggregate's position rather than a column's.
        let counted = aggregates.iter().position(|a| matches!(a, Aggregate::Count));
        let having = counted.map_or(Expr::True, |n| {
            Expr::compare(
                Group::aggregate(group.len(), n),
                CmpOp::Ge,
                Value::U64(floor),
            )
        });

        // Sort keys over the group: a key position, or an aggregate position.
        let sort: Vec<SortKey> = keys
            .iter()
            .enumerate()
            .map(|(i, over_aggregate)| {
                let column = if *over_aggregate || group.is_empty() {
                    Group::aggregate(group.len(), i % aggregates.len())
                } else {
                    Ordinal(i % group.len())
                };
                let key = if descending {
                    SortKey::desc(column)
                } else {
                    SortKey::asc(column)
                };
                if nulls_last { key.nulls_last() } else { key.nulls_first() }
            })
            .collect();

        // The group set as it comes out unordered and *unfiltered*, which is
        // the encoded-key order. Filtering, sorting and slicing are all done
        // here rather than asked for, so the order of the three is a statement
        // this test makes and not one it inherits: taking a window before
        // ordering, or filtering after it, returns a different set of groups
        // rather than the same set differently arranged.
        //
        // A stable sort on top of the encoded-key order is what pins the ties,
        // so the model has to start from the same list.
        let unordered = rt.block_on(grouped(&store, &join, &base));
        let mut expected: Vec<Group> = unordered
            .into_iter()
            .filter(|group| having.admits(&group.as_row()))
            .collect();
        expected.sort_by(|a, b| compare(a, b, &sort));
        let expected: Vec<Group> = expected
            .into_iter()
            .skip(offset)
            .take(limit.unwrap_or(usize::MAX))
            .collect();

        let mut grouping = base.having(having).sort_by(sort.clone()).offset(offset);
        grouping.limit = limit;
        let got = rt.block_on(grouped(&store, &join, &grouping));
        prop_assert_eq!(
            &got, &expected,
            "ordering or paging differed for sort={:?} limit={:?} offset={}",
            sort, limit, offset
        );
    });
}

/// The same, for a single table: `ORDER BY` over groups is not a join feature.
#[test]
fn ordering_groups_of_a_single_table_agrees_too() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let table = books();

    proptest!(|(
        descending in any::<bool>(),
        limit in prop_oneof![Just(None), (0..4usize).prop_map(Some)],
        offset in 0..3usize,
    )| {
        let group = [book_col("shelf")];
        let aggregates = vec![Aggregate::Count, Aggregate::Sum(book_col("pages"))];
        let column = Group::aggregate(group.len(), 1);
        let sort = vec![if descending {
            SortKey::desc(column)
        } else {
            SortKey::asc(column)
        }];

        let base = Grouping::by(group.iter().copied(), &aggregates);
        let txn = rt.block_on(store.begin()).unwrap();
        let unordered = rt
            .block_on(txn.grouped(&root(), &table, &Query::all(), &base))
            .unwrap();
        let mut expected = unordered.clone();
        expected.sort_by(|a, b| compare(a, b, &sort));
        let expected: Vec<Group> = expected
            .into_iter()
            .skip(offset)
            .take(limit.unwrap_or(usize::MAX))
            .collect();

        let mut grouping = base.sort_by(sort.clone()).offset(offset);
        grouping.limit = limit;
        let got = rt
            .block_on(txn.grouped(&root(), &table, &Query::all(), &grouping))
            .unwrap();
        prop_assert_eq!(
            &got, &expected,
            "sort={:?} limit={:?} offset={}",
            sort, limit, offset
        );
    });
}

// --- what the generators have to reach ------------------------------------

/// The corpus has to contain the cases the properties are about.
///
/// A grouped join over rows that all fall in one group, or a `HAVING` that
/// keeps everything, would pass every check above and prove nothing.
#[test]
fn the_generated_groupings_are_not_degenerate() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let counts = std::cell::RefCell::new(Vec::new());

    proptest!(ProptestConfig::with_cases(300), |(
        kind in join_type(),
        left in left_filter(),
        right in right_filter(),
        having in cross_condition(),
        group in group_columns(),
        aggregates in aggregates(),
    )| {
        let join = build(kind, &left, &right, &having);
        let grouping = Grouping::by(group.iter().copied(), &aggregates);
        counts.borrow_mut().push(rt.block_on(grouped(&store, &join, &grouping)).len());
    });

    let counts = counts.into_inner();
    let several = counts.iter().filter(|n| **n > 1).count();
    // Measured around 75%; the bar is 40%. One of the six grouping choices is
    // the empty list, which is one group by definition, so the ceiling is 5/6.
    assert!(
        several * 5 > counts.len() * 2,
        "only {several} of {} groupings produced more than one group",
        counts.len()
    );
}

/// A group keyed on a column of the side an outer join did not match is a
/// group keyed on null, not a group that vanishes.
///
/// Pinned by name because it is the case a fold that skipped nulls, or a
/// flatten that produced a short row, would get wrong — and it is the one the
/// generator reaches only through the outer join types.
#[tokio::test]
async fn an_unmatched_side_groups_as_null() {
    let store = seeded().await;
    let schema = at();
    let join = build(JoinType::Right, &Expr::True, &Expr::True, &Expr::True);
    let grouping = Grouping::by([schema.left(author_col("region"))], &[Aggregate::Count]);
    let groups = store
        .begin()
        .await
        .unwrap()
        .group_by_join(&root(), &authors(), &books(), &join, &grouping)
        .await
        .unwrap();

    let null_group = groups.iter().find(|g| g.key == vec![Value::Null]);
    assert!(
        null_group.is_some(),
        "books pointing at authors 12..15 have no left side, so they group \
         under null: {groups:?}"
    );
    // And the other groups are still there, so the null group is an addition
    // rather than everything collapsing into one.
    assert!(
        groups.len() > 1,
        "the matched rows should still be grouped by region: {groups:?}"
    );
}

/// Groups that tie under the requested order keep the order they already had.
///
/// Pinned by name because the property above can only catch it when the
/// generator happens to draw a tie set large enough to matter, and because the
/// requirement is easy to lose: `sort_unstable_by` is the obvious call and is
/// wrong here. Ties are common in exactly the query people write — `ORDER BY
/// count(*) DESC LIMIT 10` over a long tail of groups that all count one — and
/// a `LIMIT` over an arbitrary tie order returns a different ten each time the
/// implementation of the sort changes underneath it.
///
/// Thirty one-row groups sorted on a value with seven distinct levels, so the
/// ties are partial rather than total: a total tie is left alone by every sort
/// there is and would prove nothing.
///
/// And it still does not fail when the stable sort is swapped for an unstable
/// one, which is worth writing down rather than leaving as a test that looks
/// like it has teeth. Measured: on this toolchain `sort_unstable_by` reorders
/// exactly this pattern for a `Vec<usize>` and does *not* reorder it for a
/// struct the size of a [`Group`]. So the mutation is not observable at any
/// size this fixture can reach — a fact about the standard library's sort
/// rather than a gap here — and the stable sort stays because the guarantee
/// wanted is that the tie order is a property of this code and not of whichever
/// sort `std` ships next.
#[tokio::test]
async fn groups_that_tie_keep_the_order_they_arrived_in() {
    let store = seeded().await;
    let schema = at();
    let join = build(JoinType::Full, &Expr::True, &Expr::True, &Expr::True);
    let group = [schema.right(book_col("id"))];
    let aggregates = vec![Aggregate::Sum(schema.right(book_col("pages")))];
    let base = Grouping::by(group.iter().copied(), &aggregates);
    let sort = vec![SortKey::asc(Group::aggregate(group.len(), 0))];

    let unordered = grouped(&store, &join, &base).await;
    assert!(
        unordered.len() > 20,
        "premise: a tie set small enough for insertion sort is left alone by \
         every sort there is, so this needs to be bigger: {}",
        unordered.len()
    );
    let levels: std::collections::BTreeSet<&Value> =
        unordered.iter().filter_map(|g| g.values.first()).collect();
    assert!(
        levels.len() > 1 && levels.len() < unordered.len(),
        "premise: the sort key must tie some groups and separate others, \
         got {} levels over {} groups",
        levels.len(),
        unordered.len()
    );

    let mut expected = unordered;
    expected.sort_by(|a, b| compare(a, b, &sort));
    let got = grouped(&store, &join, &base.sort_by(sort)).await;
    assert_eq!(got, expected, "ties were not broken by the arrival order");
}

/// A grouped join reads only the columns the grouping needs.
///
/// The reason grouping belongs in the kernel rather than in a caller's loop is
/// the projection: an aggregate reads a handful of columns, so an index that
/// holds them answers without touching a row. That was already true of a
/// grouped table scan and is the thing most easily lost when grouping is put
/// over a join, because the ordinals arrive in the joined space and have to be
/// mapped back to a side before either side's planner can see them.
///
/// Measured in reads rather than asserted in prose, and with the control
/// measured in the same run: `count(*)` per author needs nothing off a book but
/// the join key, which `by_author` holds, so no book row is read; asking for
/// `sum(pages)` in the same shape reads every one of them. A zero next to a
/// number is a measurement, a zero on its own is a constant.
#[tokio::test]
async fn a_grouped_join_reads_only_what_the_grouping_needs() {
    let catalog = Catalog::from_tables([authors(), books()]).expect("catalog");
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let txn = loader.begin().await.unwrap();
    for id in 0..AUTHOR_ROWS {
        txn.insert(&root(), &authors(), &author(id)).await.unwrap();
    }
    for id in 0..BOOK_ROWS {
        txn.insert(&root(), &books(), &book(id)).await.unwrap();
    }
    txn.commit().await.unwrap();

    let counting = LatencyStore::new(backing, LatencyProfile::free());
    let counters = counting.counters();
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::ALL))
        .grant(Grant::new("r", BOOKS, Action::ALL));
    let store = RecordStore::new(counting, catalog, security);

    let schema = at();
    // Hash-built on the left so the right side is walked as a stream rather
    // than probed a row at a time; a nested loop reads a row per probe whatever
    // the projection says, which would measure the algorithm and not the
    // projection.
    let mut join = build(JoinType::Inner, &Expr::True, &Expr::True, &Expr::True);
    join.force = Some(JoinAlgorithm::Hash { build: Side::Left });
    // The right side is forced onto `by_author`, which holds the join key and
    // nothing else. Left to itself the planner reads the whole small table,
    // where a covering scan and a full one cost the same and there is nothing
    // to count. Forcing the index is what turns "the projection was narrowed"
    // into a number: covered, the entries answer outright; uncovered, every
    // entry costs a point read.
    join.right = Query::all().using_index(BY_AUTHOR);

    let by_author = Grouping::by([schema.left(author_col("id"))], &[Aggregate::Count]);
    let txn = store.begin().await.unwrap();
    counters.reset();
    let groups = txn
        .group_by_join(&root(), &authors(), &books(), &join, &by_author)
        .await
        .unwrap();
    let counted = counters.gets();
    assert!(!groups.is_empty(), "premise: the join produces groups");

    // The same grouping, plus an aggregate over a column no index holds.
    let with_pages = Grouping::by(
        [schema.left(author_col("id"))],
        &[
            Aggregate::Count,
            Aggregate::Sum(schema.right(book_col("pages"))),
        ],
    );
    counters.reset();
    let widened = txn
        .group_by_join(&root(), &authors(), &books(), &join, &with_pages)
        .await
        .unwrap();
    let read = counters.gets();
    assert_eq!(
        widened.len(),
        groups.len(),
        "the two groupings differ in what they compute, not in what they group"
    );
    assert_eq!(
        counted, 0,
        "the entries hold the join key, so a grouping that needs nothing else \
         must read no book rows at all"
    );
    assert_eq!(
        read, BOOK_ROWS,
        "the control: `pages` can only come from the row"
    );
}

/// A policy on one side of the join applies to the groups.
///
/// A grouped join is two secured reads with a fold on top; it cannot be a way
/// around a policy. The check is a count, because a count is the easiest way to
/// leak without returning a single forbidden row — `COUNT(*)` over rows the
/// caller cannot read still says how many there are.
#[tokio::test]
async fn a_policy_on_one_side_narrows_the_groups() {
    let catalog = Catalog::from_tables([authors(), books()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("reader", AUTHORS, Action::ALL))
        .grant(Grant::new("reader", BOOKS, Action::ALL))
        .policy(Policy::new(
            "one_region",
            AUTHORS,
            Action::ALL,
            |_: &SecurityContext| Expr::eq(author_col("region"), Value::Str("r0".into())),
        ));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();
    for id in 0..AUTHOR_ROWS {
        txn.insert(&SecurityContext::superuser(), &authors(), &author(id))
            .await
            .unwrap();
    }
    for id in 0..BOOK_ROWS {
        txn.insert(&SecurityContext::superuser(), &books(), &book(id))
            .await
            .unwrap();
    }
    txn.commit().await.unwrap();

    let reader = SecurityContext::new(Principal::new(Value::U64(1)).with_role("reader"));
    let schema = at();
    let join = build(JoinType::Inner, &Expr::True, &Expr::True, &Expr::True);
    let grouping = Grouping::by([schema.left(author_col("region"))], &[Aggregate::Count]);

    let txn = store.begin().await.unwrap();
    let restricted = txn
        .group_by_join(&reader, &authors(), &books(), &join, &grouping)
        .await
        .unwrap();
    assert_eq!(
        restricted.iter().map(|g| g.key.clone()).collect::<Vec<_>>(),
        vec![vec![Value::Str("r0".into())]],
        "the policy admits one region, so there is one group: {restricted:?}"
    );

    // The control: as a superuser the other regions are there, so the check
    // above is a restriction rather than an empty join.
    let all = txn
        .group_by_join(&root(), &authors(), &books(), &join, &grouping)
        .await
        .unwrap();
    assert!(
        all.len() > 1,
        "premise: without the policy there is more than one region: {all:?}"
    );
}
