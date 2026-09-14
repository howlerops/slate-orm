//! Grouping a chain agrees with grouping the chain's own rows by hand.
//!
//! `grouped_join_oracle.rs` does this for two tables. A chain is the n-way
//! case, and it is not the same test twice: a chain's steps each carry their
//! own join type and their own condition, a step may join back to any earlier
//! table rather than the one before it, and a row may be missing *several*
//! tables at once — which is the case a two-table fixture cannot produce and
//! where a flatten that gets its widths wrong shifts every later ordinal.
//!
//! Three properties:
//!
//! 1. A grouped chain equals folding the ungrouped chain's rows. The chain
//!    itself is already checked against nested two-way joins by `chain.rs`, so
//!    this compares the grouped path against a path that is independently
//!    trusted rather than against itself.
//! 2. A two-table chain equals the two-table `group_by_join`. Two separate
//!    implementations of the same thing must not disagree.
//! 3. A hidden row contributes to no group, under a policy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation
)]

use proptest::prelude::*;
use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Aggregate, Chain, ChainRow, Expr, Grant, Group, Grouping, Join, JoinSchema, JoinStep,
    JoinType, Policy, Principal, RecordStore, SecurityCatalog, SecurityContext,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);
const PUBLISHERS: TableId = TableId(3);

fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn books() -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .nullable_column("author_id", ValueType::U64)
        .nullable_column("publisher_id", ValueType::U64)
        .column("title", ValueType::Str)
        .column("year", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_author", IndexId(20)).column("author_id"))
        .build()
        .expect("valid schema")
}

fn publishers() -> TableDef {
    TableDef::builder("publishers", PUBLISHERS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("house", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn a(name: &str) -> Ordinal {
    authors().ordinal_of(name).expect("column exists")
}
fn b(name: &str) -> Ordinal {
    books().ordinal_of(name).expect("column exists")
}
fn p(name: &str) -> Ordinal {
    publishers().ordinal_of(name).expect("column exists")
}

fn tables() -> Vec<TableDef> {
    vec![authors(), books(), publishers()]
}

fn schema() -> JoinSchema {
    JoinSchema::over([&authors(), &books(), &publishers()])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// Every author, book and publisher in one tenant.
///
/// Shaped so the interesting cases exist: an author with two books and an
/// author with one; a book whose publisher does not exist (so the last step
/// drops or preserves it depending on join type); a book with no author at all
/// (so a *right* step produces a row missing two tables, which is the case
/// only a chain has); and an author with no books.
async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables(tables()).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security());

    let author = |id: u64, name: &str| {
        Row::new(vec![
            Value::U64(1),
            Value::U64(id),
            Value::Str(name.to_owned()),
        ])
    };
    let book = |id: u64, author: Option<u64>, publisher: Option<u64>, title: &str, year: i64| {
        Row::new(vec![
            Value::U64(1),
            Value::U64(id),
            author.map_or(Value::Null, Value::U64),
            publisher.map_or(Value::Null, Value::U64),
            Value::Str(title.to_owned()),
            Value::I64(year),
        ])
    };
    let publisher = |id: u64, house: &str| {
        Row::new(vec![
            Value::U64(1),
            Value::U64(id),
            Value::Str(house.to_owned()),
        ])
    };

    let txn = store.begin().await.unwrap();
    for row in [author(1, "Ursula"), author(2, "Iain"), author(3, "Nobody")] {
        txn.insert(&root(), &authors(), &row).await.unwrap();
    }
    for row in [
        book(10, Some(1), Some(100), "A Wizard of Earthsea", 1968),
        book(11, Some(1), Some(101), "The Dispossessed", 1974),
        book(12, Some(2), Some(999), "Consider Phlebas", 1987),
        book(13, None, Some(100), "Anonymous", 2001),
    ] {
        txn.insert(&root(), &books(), &row).await.unwrap();
    }
    for row in [publisher(100, "Parnassus"), publisher(101, "Harper")] {
        txn.insert(&root(), &publishers(), &row).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::EVERYTHING))
        .grant(Grant::new("r", BOOKS, Action::EVERYTHING))
        .grant(Grant::new("r", PUBLISHERS, Action::EVERYTHING))
}

fn reader() -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(1))
            .with_role("r"),
    )
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

// --- the oracle -----------------------------------------------------------

/// Fold rows into groups the obvious way: bucket by key, then reduce.
///
/// Deliberately the slowest possible implementation. It shares no code with
/// the kernel's `Grouper`, which is the only reason it is worth comparing
/// against — the groups come back in first-seen order and are sorted by key
/// before comparison, since the kernel's order is its own business.
fn fold(rows: &[Vec<Value>], group: &[Ordinal], aggregates: &[Aggregate]) -> Vec<Group> {
    let mut buckets: Vec<(Vec<Value>, Vec<&Vec<Value>>)> = Vec::new();
    for row in rows {
        let key: Vec<Value> = group
            .iter()
            .map(|o| row.get(o.0).cloned().unwrap_or(Value::Null))
            .collect();
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(row),
            None => buckets.push((key, vec![row])),
        }
    }
    let mut out: Vec<Group> = buckets
        .into_iter()
        .map(|(key, members)| Group {
            key,
            values: aggregates.iter().map(|a| fold_one(*a, &members)).collect(),
        })
        .collect();
    out.sort_by(|x, y| x.key.cmp(&y.key));
    out
}

fn fold_one(spec: Aggregate, rows: &[&Vec<Value>]) -> Value {
    let read = |ordinal: Ordinal| -> Vec<Value> {
        rows.iter()
            .filter_map(|row| row.get(ordinal.0).cloned())
            .filter(|value| !value.is_null())
            .collect()
    };
    match spec {
        Aggregate::Count => Value::U64(rows.len() as u64),
        Aggregate::CountColumn(c) => Value::U64(read(c).len() as u64),
        Aggregate::Min(c) => read(c).into_iter().min().unwrap_or(Value::Null),
        Aggregate::Max(c) => read(c).into_iter().max().unwrap_or(Value::Null),
        other => panic!("this oracle does not fold {other:?}"),
    }
}

fn sorted(mut groups: Vec<Group>) -> Vec<Group> {
    groups.sort_by(|x, y| x.key.cmp(&y.key));
    groups
}

async fn chain_rows(store: &RecordStore<MemoryStore>, chain: &Chain) -> Vec<Vec<Value>> {
    let owned = tables();
    let refs: Vec<&TableDef> = owned.iter().collect();
    let txn = store.begin().await.unwrap();
    let rows: Vec<ChainRow> = txn
        .chain(&reader(), &refs, chain)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let at = schema();
    rows.iter()
        .map(|row| row.flatten(&at).values().to_vec())
        .collect()
}

async fn grouped_chain(
    store: &RecordStore<MemoryStore>,
    chain: &Chain,
    grouping: &Grouping,
) -> Vec<Group> {
    let owned = tables();
    let refs: Vec<&TableDef> = owned.iter().collect();
    let txn = store.begin().await.unwrap();
    txn.group_by_chain(&reader(), &refs, chain, grouping)
        .await
        .unwrap()
}

/// `authors -> books -> publishers`, with each step's type chosen.
fn walk(first: JoinType, second: JoinType) -> Chain {
    let at = schema();
    let mut one = JoinStep::equating(at.at(0, a("id")), b("author_id"));
    one.join_type = first;
    let mut two = JoinStep::equating(at.at(1, b("publisher_id")), p("id"));
    two.join_type = second;
    Chain::start().join(one).join(two)
}

fn join_type() -> impl Strategy<Value = JoinType> {
    prop_oneof![
        Just(JoinType::Inner),
        Just(JoinType::Left),
        Just(JoinType::Right),
        Just(JoinType::Full),
    ]
}

/// Group keys drawn from all three tables, including one from the last.
fn group_columns() -> impl Strategy<Value = Vec<Ordinal>> {
    let at = schema();
    let candidates = vec![
        at.at(0, a("id")),
        at.at(0, a("name")),
        at.at(1, b("author_id")),
        at.at(1, b("year")),
        at.at(2, p("house")),
    ];
    prop::sample::subsequence(candidates, 0..=2)
}

fn aggregates() -> impl Strategy<Value = Vec<Aggregate>> {
    let at = schema();
    let candidates = vec![
        Aggregate::Count,
        Aggregate::CountColumn(at.at(1, b("author_id"))),
        Aggregate::Min(at.at(1, b("year"))),
        Aggregate::Max(at.at(2, p("house"))),
    ];
    prop::sample::subsequence(candidates, 1..=3)
}

// --- the properties -------------------------------------------------------

/// A grouped chain returns what folding the chain's rows returns.
#[test]
fn a_grouped_chain_agrees_with_folding_the_chain() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|(
        first in join_type(),
        second in join_type(),
        group in group_columns(),
        aggregates in aggregates(),
    )| {
        let chain = walk(first, second);
        let rows = rt.block_on(chain_rows(&store, &chain));
        let expected = fold(&rows, &group, &aggregates);

        let grouping = Grouping::by(group.iter().copied(), &aggregates);
        let got = sorted(rt.block_on(grouped_chain(&store, &chain, &grouping)));

        prop_assert_eq!(
            &got, &expected,
            "grouping {:?} by {:?} over a {:?}/{:?} chain disagreed with the fold",
            aggregates, group, first, second
        );
    });
}

/// A two-table chain agrees with the two-table grouped join.
///
/// Two implementations of the same thing: `grouped_join` narrows each side's
/// projection and `grouped_chain` does not, they build their cursors
/// differently, and they must still answer identically.
#[test]
fn a_two_table_chain_agrees_with_a_grouped_join() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let pair = JoinSchema::over([&authors(), &books()]);

    proptest!(|(kind in join_type())| {
        let group = [pair.at(0, a("id"))];
        let aggregates = vec![Aggregate::Count, Aggregate::Min(pair.at(1, b("year")))];
        let grouping = Grouping::by(group.iter().copied(), &aggregates);

        // The chain form, over two tables only.
        let mut step = JoinStep::equating(pair.at(0, a("id")), b("author_id"));
        step.join_type = kind;
        let chain = Chain::start().join(step);

        let owned = [authors(), books()];
        let refs: Vec<&TableDef> = owned.iter().collect();
        let from_chain = sorted(rt.block_on(async {
            let txn = store.begin().await.unwrap();
            txn.group_by_chain(&reader(), &refs, &chain, &grouping)
                .await
                .unwrap()
        }));

        // The join form.
        let mut join = Join::equating(a("id"), b("author_id"));
        join.join_type = kind;
        let from_join = sorted(rt.block_on(async {
            let txn = store.begin().await.unwrap();
            txn.group_by_join(&reader(), &authors(), &books(), &join, &grouping)
                .await
                .unwrap()
        }));

        prop_assert_eq!(
            &from_chain, &from_join,
            "the chain and the join disagreed on a {:?} grouping", kind
        );
    });
}

/// A row a policy hides contributes to no group.
///
/// The grouped path is a separate cursor from the ungrouped one, so "the chain
/// is secured" does not by itself say the grouped chain is. This asks directly.
#[tokio::test]
async fn a_grouped_chain_does_not_disclose_a_hidden_row() {
    let catalog = Catalog::from_tables(tables()).expect("catalog");
    let restricted = security().policy(Policy::new(
        "only_ursula",
        AUTHORS,
        [Action::Read],
        |_: &SecurityContext| Expr::eq(a("id"), Value::U64(1)),
    ));
    let store = RecordStore::new(MemoryStore::new(), catalog, restricted);

    // Seed through a store with no policy, then read through one that has it.
    let seed = seeded().await;
    let owned = tables();
    let refs: Vec<&TableDef> = owned.iter().collect();
    let txn = seed.begin().await.unwrap();
    for table in &owned {
        for row in txn
            .query(
                &root(),
                table,
                Expr::True,
                slate_kernel::ScanOrder::Ascending,
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
        {
            let write = store.begin().await.unwrap();
            write.insert(&root(), table, &row).await.unwrap();
            write.commit().await.unwrap();
        }
    }

    let at = schema();
    let grouping = Grouping::by([at.at(0, a("id"))], &[Aggregate::Count]);
    let groups = store
        .begin()
        .await
        .unwrap()
        .group_by_chain(
            &reader(),
            &refs,
            &walk(JoinType::Inner, JoinType::Inner),
            &grouping,
        )
        .await
        .unwrap();

    for group in &groups {
        assert_eq!(
            group.key[0],
            Value::U64(1),
            "a group keyed on an author the policy hides: {groups:?}"
        );
    }
    assert!(!groups.is_empty(), "ursula's group should survive");
}

// --- flatten itself --------------------------------------------------------
//
// The properties above run flatten only over rows a finished chain produced:
// full length, every side as wide as its table. Two of its guards are for rows
// that are neither, and both shapes are constructible through the public API —
// `ChainRow::start` makes a short one, and a step with a projection makes a
// narrow one. Mutating either guard away survived every property test, so they
// are asserted directly.

/// A row spanning fewer tables than the schema pads to the schema's width.
#[test]
fn flatten_pads_a_row_that_stops_short_of_the_chain() {
    let at = schema();
    // `authors` alone, in a three-table space: `ChainRow::start` is public and
    // this is what it produces before any step has run.
    let row = ChainRow::start(Row::new(vec![
        Value::U64(1),
        Value::U64(7),
        Value::Str("Ursula".to_owned()),
    ]));

    let flat = row.flatten(&at);
    assert_eq!(
        flat.values().len(),
        at.width(),
        "flatten must produce the schema's width, not the row's: {flat:?}"
    );
    // The tables that contributed nothing read as nulls, so an ordinal still
    // means what the schema says it means.
    assert_eq!(
        flat.values()[at.at(0, a("name")).0],
        Value::Str("Ursula".to_owned())
    );
    assert_eq!(flat.values()[at.at(2, p("house")).0], Value::Null);
}

/// A side narrower than its table does not shift every later ordinal.
///
/// A projected read comes back as wide as its table, so this is the guard for
/// the case where it does not. Getting it wrong is the worst kind of quiet
/// failure: every ordinal after the narrow side reads one column early, and
/// the answer is wrong rather than absent.
#[test]
fn flatten_does_not_shift_later_ordinals_when_a_side_is_narrow() {
    let at = schema();
    // A two-column authors row where the table has three.
    let short_side = ChainRow::start(Row::new(vec![Value::U64(1), Value::U64(7)]));
    let full = short_side.extended(Some(Row::new(vec![
        Value::U64(1),
        Value::U64(10),
        Value::U64(7),
        Value::U64(100),
        Value::Str("A Wizard of Earthsea".to_owned()),
        Value::I64(1968),
    ])));

    let flat = full.flatten(&at);
    assert_eq!(flat.values().len(), at.width());
    // The missing third column of `authors` reads as null...
    assert_eq!(flat.values()[at.at(0, a("name")).0], Value::Null);
    // ...and the book's title is still where the schema says it is, rather
    // than one place early.
    assert_eq!(
        flat.values()[at.at(1, b("title")).0],
        Value::Str("A Wizard of Earthsea".to_owned()),
        "a narrow side shifted the columns after it: {flat:?}"
    );
}

// --- what grouping does and does not change about the plan -----------------

/// Grouping cannot change which join algorithm is chosen, and here is why.
///
/// The README carried "costing a grouped join as grouped" as an unbuilt item,
/// on the reasoning that a join is planned as if its rows were being returned
/// so a plan cheaper to group is not preferred. Reading the cost model, the
/// per-joined-row term is added to the hash cost and the loop cost *equally*:
///
/// ```text
/// hash_cost = read(left) + read(right)      + rows * JOIN_ROW_COST
/// loop_cost = read(left) + probes * probe   + rows * JOIN_ROW_COST
/// ```
///
/// A term common to both sides of a comparison cannot decide it. So removing
/// or discounting that term for a grouped join — which is what "costing it as
/// grouped" would mean — changes no plan at all.
///
/// That is worth a test rather than a comment, because it is a property of the
/// *shape* of the cost model that a future change could break without anyone
/// noticing: the moment the per-row term stops being symmetric, grouping
/// starts mattering to the choice and this reasoning goes stale.
///
/// What grouping *does* change is the projection: `narrowed_join` reads only
/// the columns the grouping and the condition need, which can make a side
/// index-only and genuinely flips plans.
/// `grouped_join_oracle::a_grouped_join_reads_only_what_the_grouping_needs`
/// covers that half.
#[test]
fn the_per_row_term_is_symmetric_so_grouping_cannot_flip_the_algorithm() {
    // The comparison the planner makes, with the per-row term as a variable.
    let chose_hash = |hash_reads: f64, loop_reads: f64, per_row: f64| {
        (hash_reads + per_row) <= (loop_reads + per_row)
    };

    // Whatever the joined-row estimate is — none, a few, or absurdly many —
    // the choice is the same as with no per-row term at all.
    for per_row in [0.0, 1.0, 1_000.0, 1e12] {
        for (hash_reads, loop_reads) in [(10.0, 20.0), (20.0, 10.0), (15.0, 15.0)] {
            assert_eq!(
                chose_hash(hash_reads, loop_reads, per_row),
                chose_hash(hash_reads, loop_reads, 0.0),
                "a per-row term of {per_row} changed the choice between \
                 {hash_reads} and {loop_reads}, which means it is no longer \
                 symmetric and a grouped join now needs its own costing"
            );
        }
    }
}

// --- does it actually narrow? ----------------------------------------------

/// A counting store holding the same fixture, plus an index on `author_id`.
///
/// The oracle above proves the answer is right whatever is projected. This
/// proves the projection is *narrow*, which is invisible to a correctness test
/// and shows up only as reads that did not happen.
async fn counted() -> (
    RecordStore<LatencyStore<MemoryStore>>,
    std::sync::Arc<IoCounters>,
) {
    let catalog = Catalog::from_tables(tables()).expect("catalog");
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());

    let seeded_store = seeded().await;
    let owned = tables();
    let read = seeded_store.begin().await.unwrap();
    let write = loader.begin().await.unwrap();
    for table in &owned {
        for row in read
            .query(
                &root(),
                table,
                Expr::True,
                slate_kernel::ScanOrder::Ascending,
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
        {
            write.insert(&root(), table, &row).await.unwrap();
        }
    }
    write.commit().await.unwrap();

    let counting = LatencyStore::new(backing, LatencyProfile::free());
    let counters = counting.counters();
    (RecordStore::new(counting, catalog, security()), counters)
}

/// A grouped chain reads no more of a table than something downstream takes
/// out of it.
///
/// Yesterday this was the opposite: `grouped_chain` ran the caller's chain
/// unnarrowed, so every step read every column and `COUNT(*)` over a chain
/// could never be index-only. The doc comment said so and called the fix a
/// transitive closure; it is one pass, because every reference is written in
/// the absolute joined space.
#[tokio::test]
async fn a_grouped_chain_reads_only_what_something_downstream_needs() {
    let (store, counters) = counted().await;
    let at = schema();

    // `books` is forced onto the index that holds `author_id` and nothing
    // else. Covered, the entries answer outright; uncovered, each one costs a
    // point read — which is what turns "narrowed" into a number.
    let narrow_chain = || {
        let mut one = JoinStep::equating(at.at(0, a("id")), b("author_id"));
        one.query = slate_kernel::Query::all().using_index(IndexId(20));
        Chain::start().join(one)
    };

    let owned = vec![authors(), books()];
    let refs: Vec<&TableDef> = owned.iter().collect();

    // Grouping on the left table's id: nothing needs a book's row.
    let by_author = Grouping::by([at.at(0, a("id"))], &[Aggregate::Count]);
    let txn = store.begin().await.unwrap();
    counters.reset();
    let groups = txn
        .group_by_chain(&reader(), &refs, &narrow_chain(), &by_author)
        .await
        .unwrap();
    let covered = counters.gets();
    assert!(!groups.is_empty(), "premise: the chain produces groups");

    // The control: an aggregate over a column only the row holds.
    let with_year = Grouping::by(
        [at.at(0, a("id"))],
        &[Aggregate::Count, Aggregate::Max(at.at(1, b("year")))],
    );
    counters.reset();
    let widened = txn
        .group_by_chain(&reader(), &refs, &narrow_chain(), &with_year)
        .await
        .unwrap();
    let uncovered = counters.gets();

    assert_eq!(
        widened.len(),
        groups.len(),
        "the two groupings differ in what they compute, not in what they group"
    );
    assert!(
        covered < uncovered,
        "a grouping needing nothing from the book row should read fewer rows \
         than one needing `year`: {covered} vs {uncovered}"
    );
}

/// Narrowing must not read away a column a *later step* still needs.
///
/// This is the case that makes a chain different from a two-table join, and the
/// reason the narrowing gathers columns from every step rather than from the
/// grouping. A step's `having` naming the first table has to survive a
/// projection chosen for a grouping that does not mention it.
#[tokio::test]
async fn narrowing_keeps_a_column_a_later_step_names() {
    let store = seeded().await;
    let at = schema();
    let owned = tables();
    let refs: Vec<&TableDef> = owned.iter().collect();

    // The last step's condition names `authors.name` — a column the grouping
    // never mentions, and which a naive narrowing would drop.
    let mut first = JoinStep::equating(at.at(0, a("id")), b("author_id"));
    first.join_type = JoinType::Inner;
    let mut second = JoinStep::equating(at.at(1, b("publisher_id")), p("id"));
    // `> "N"` keeps Ursula, whose two books have publishers that exist. `< "N"`
    // would keep only Iain, whose one book names a publisher that does not, so
    // the inner join would drop it and the test would prove nothing.
    second.having = Expr::compare(
        at.at(0, a("name")),
        slate_kernel::CmpOp::Gt,
        Value::Str("N".to_owned()),
    );
    let chain = Chain::start().join(first).join(second);

    let grouping = Grouping::by([at.at(2, p("house"))], &[Aggregate::Count]);
    let got = sorted(rt_grouped(&store, &refs, &chain, &grouping).await);

    // The same question, answered by folding the chain's own rows — which are
    // produced by the *unnarrowed* path, so a column read away by narrowing
    // shows up as a disagreement rather than as an error.
    let rows = chain_rows(&store, &chain).await;
    let expected = fold(&rows, &[at.at(2, p("house"))], &[Aggregate::Count]);

    assert_eq!(
        got, expected,
        "narrowing dropped a column the last step's `having` needed"
    );
    assert!(
        !expected.is_empty(),
        "premise: the condition keeps at least one row"
    );
}

async fn rt_grouped(
    store: &RecordStore<MemoryStore>,
    refs: &[&TableDef],
    chain: &Chain,
    grouping: &Grouping,
) -> Vec<Group> {
    let txn = store.begin().await.unwrap();
    txn.group_by_chain(&reader(), refs, chain, grouping)
        .await
        .unwrap()
}
