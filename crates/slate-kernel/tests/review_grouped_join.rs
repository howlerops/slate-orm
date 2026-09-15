//! Adversarial probes on grouping over a join, and on grouping by a value the
//! query computes.
//!
//! `grouped_join_oracle.rs` already requires the three join algorithms to agree
//! and the groups to match a hand-written fold. It builds every join it tests
//! through one helper that never sets `Join::limit` or `Join::offset`, so the
//! interaction between the join's own window and the grouping above it is
//! outside its corpus. That interaction is what the first test here is about.
//!
//! The second pair is the other thing the grouped oracles never draw: a group
//! key that is not a column but a value the query computes. It is in the
//! ordinal space every other computed value lives in, so the grouped read has
//! to narrow a projection around an ordinal the table does not have.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Aggregate, Grant, Group, Grouping, Join, JoinAlgorithm, Query, RecordStore, Scalar,
    SecurityCatalog, SecurityContext, Side,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);
const BY_AUTHOR: IndexId = IndexId(20);
const BY_SHELF: IndexId = IndexId(21);

fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("id", ValueType::U64)
        .column("region", ValueType::Str)
        .primary_key(["id"])
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
        .index(IndexDef::builder("by_shelf", BY_SHELF).column("shelf"))
        .build()
        .expect("valid schema")
}

fn author_col(name: &str) -> Ordinal {
    authors().ordinal_of(name).expect("column exists")
}

fn book_col(name: &str) -> Ordinal {
    books().ordinal_of(name).expect("column exists")
}

fn author(id: u64) -> Row {
    Row::new(vec![Value::U64(id), Value::Str(format!("r{}", id % 3))])
}

fn book(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::U64(id % 6),
        Value::I64((id % 7) as i64 * 10),
        Value::Str(format!("s{}", id % 4)),
    ])
}

const AUTHOR_ROWS: u64 = 6;
const BOOK_ROWS: u64 = 24;

async fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([authors(), books()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::ALL))
        .grant(Grant::new("r", BOOKS, Action::ALL));
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let root = SecurityContext::superuser();
    let txn = loader.begin().await.unwrap();
    for id in 0..AUTHOR_ROWS {
        txn.insert(&root, &authors(), &author(id)).await.unwrap();
    }
    for id in 0..BOOK_ROWS {
        txn.insert(&root, &books(), &book(id)).await.unwrap();
    }
    txn.commit().await.unwrap();
    RecordStore::new(backing, catalog, security)
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// A grouped read of a join whose own `limit` narrows the rows going in.
///
/// The single-table grouped read drops a query's `limit` and `offset` before it
/// runs — `read::narrowed` says so in as many words, because "aggregating a
/// windowed subset of an unordered result is not a meaningful request".
/// `read::narrowed_join` used to clone the join and replace only the two
/// projections, so a join's `limit` survived into a grouped join and windowed
/// the row stream before the grouper ever saw it.
///
/// A join has no order. Which rows the window kept was therefore whichever ones
/// the chosen algorithm happened to produce first, and the answer was a table
/// of plausible numbers either way: this fixture came back as counts of
/// 3/3/3/3 building the left side and 4/2/4/2 building the right.
#[tokio::test]
async fn a_grouped_join_does_not_depend_on_which_algorithm_served_it() {
    let store = store().await;
    // `shelf` in the joined ordinal space: the right table's columns sit past
    // the left table's width.
    let shelf = Ordinal(authors().columns().len() + book_col("shelf").0);
    let grouping = Grouping::by([shelf], &[Aggregate::Count]);

    let mut answers: Vec<(JoinAlgorithm, Vec<Group>)> = Vec::new();
    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::NestedLoop,
    ] {
        let mut join = Join::equating(author_col("id"), book_col("author_id"))
            .left(Query::all())
            .right(Query::all())
            // Half the joined rows. Any window at all is enough; this is the
            // shape a caller writes when they want "the first page".
            .limit(12);
        join.force = Some(algorithm);
        let txn = store.begin().await.unwrap();
        let groups = txn
            .group_by_join(&root(), &authors(), &books(), &join, &grouping)
            .await
            .unwrap();
        answers.push((algorithm, groups));
    }

    let (first_algorithm, first) = &answers[0];
    for (algorithm, groups) in &answers[1..] {
        assert_eq!(
            groups, first,
            "{algorithm:?} grouped a limited join into {groups:?}, where \
             {first_algorithm:?} produced {first:?}"
        );
    }
}

// --- grouping by a computed value -----------------------------------------

/// `GROUP BY length(shelf) ...` — well, `pages / 10`, which is an integer and
/// gives seven groups over the fixture.
fn bucketed() -> Vec<Scalar> {
    vec![Scalar::Div(
        Box::new(Scalar::Column(book_col("pages"))),
        Box::new(Scalar::Literal(Value::I64(10))),
    )]
}

fn computed_ordinal() -> Ordinal {
    Ordinal(books().columns().len())
}

/// A fold written out here: the groups `GROUP BY pages/10` must produce.
fn expected() -> Vec<Group> {
    let mut counts: std::collections::BTreeMap<i64, u64> = std::collections::BTreeMap::new();
    for id in 0..BOOK_ROWS {
        let pages = (id % 7) as i64 * 10;
        *counts.entry(pages / 10).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(bucket, count)| Group {
            key: vec![Value::I64(bucket)],
            values: vec![Value::U64(count)],
        })
        .collect()
}

/// Grouping by a computed value agrees with the fold, on the planner's choice.
#[tokio::test]
async fn grouping_by_a_computed_value_agrees_with_a_fold() {
    let store = store().await;
    let txn = store.begin().await.unwrap();
    let query = Query::all().computing(bucketed());
    let grouping = Grouping::by([computed_ordinal()], &[Aggregate::Count]);
    let got = txn
        .grouped(&root(), &books(), &query, &grouping)
        .await
        .unwrap();
    assert_eq!(got, expected());
}

/// And on every access path, which is the property an index that leaves the
/// computed value's input undecoded would break: the group key would be null
/// for every row and the whole table would come back as one group.
#[tokio::test]
async fn grouping_by_a_computed_value_agrees_down_every_access_path() {
    let store = store().await;
    let grouping = Grouping::by([computed_ordinal()], &[Aggregate::Count]);

    for (name, query) in [
        (
            "table scan",
            Query::all().computing(bucketed()).using_table_scan(),
        ),
        (
            "by_author",
            Query::all().computing(bucketed()).using_index(BY_AUTHOR),
        ),
        (
            "by_shelf",
            Query::all().computing(bucketed()).using_index(BY_SHELF),
        ),
    ] {
        let txn = store.begin().await.unwrap();
        let got = txn
            .grouped(&root(), &books(), &query, &grouping)
            .await
            .unwrap();
        assert_eq!(got, expected(), "GROUP BY pages/10 differed on {name}");
    }
}

// --- grouping ordinals a join does not validate ---------------------------

/// `Join::validate` refuses a `having` ordinal outside the joined space, in as
/// many words: "a `having` ordinal past the joined width names no column at
/// all, and would silently read as null — that is, as a condition nobody
/// wrote." Nothing does the same for a `Grouping`'s ordinals, and the two
/// arrive through the same call.
///
/// Two shapes followed, and both were recorded here as the current behaviour
/// rather than asserted to be wrong, on the grounds that "the fix is a
/// refusal, which is a decision rather than an arithmetic error". Both
/// decisions are now made and both shapes are refused; these tests assert the
/// refusals.
///
/// **Past the joined width**: refused, where it used to group everything under
/// null and return a table of numbers with a single row in it.
#[tokio::test]
async fn a_grouping_ordinal_past_the_joined_width_is_refused() {
    let store = store().await;
    let past = Ordinal(authors().columns().len() + books().columns().len() + 3);
    let join = Join::equating(author_col("id"), book_col("author_id"));
    let txn = store.begin().await.unwrap();
    let refused = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by([past], &[Aggregate::Count]),
        )
        .await
        .expect_err("an ordinal past the joined width names nothing");
    assert!(refused.to_string().contains("outside"), "{refused}");
}

/// An aggregate's ordinal is checked the same way, which is the half a
/// group-key-only check would miss: `sum()` of a column nobody has is a column
/// of nulls, and a sum of nulls is a plausible zero.
#[tokio::test]
async fn an_aggregate_ordinal_past_the_joined_width_is_refused() {
    let store = store().await;
    let past = Ordinal(authors().columns().len() + books().columns().len() + 1);
    let join = Join::equating(author_col("id"), book_col("author_id"));
    let txn = store.begin().await.unwrap();
    let refused = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by([author_col("region")], &[Aggregate::Sum(past)]),
        )
        .await
        .expect_err("an aggregate past the joined width names nothing");
    assert!(refused.to_string().contains("outside"), "{refused}");
}

/// **A left-side computed value's ordinal** was worse, because it is *in*
/// range — and this is now refused.
///
/// `JoinSchema` packs by declared table width, so the ordinal a left-side
/// query gives its first computed value — `left_width + 0` — is the right
/// table's column 0 in the joined space. `JoinedRow::flatten` truncates the
/// left row to the table's width, so the value never reached the grouper; the
/// group key was the right table's first column instead, with no error
/// anywhere. `records.proto` refuses exactly this reference on `having` ("a
/// computed value has no slot in it; the reference is refused rather than
/// landing on whatever column happens to sit at that offset"); the kernel's
/// `Grouping` accepted it.
///
/// This test used to pin that as behaviour, on the grounds that "the fix is a
/// refusal, which is a decision rather than an arithmetic error". The decision
/// is made: a side may not compute at all, and `Join::compute` — evaluated
/// over the joined row, so it can read either side — is where such a value
/// goes. The refusal names that field, because an error that does not say what
/// to do instead is half an error.
#[tokio::test]
async fn a_left_side_computed_column_is_refused_and_says_where_to_put_it() {
    let store = store().await;
    let join =
        Join::equating(author_col("id"), book_col("author_id")).left(Query::all().computing([
            Scalar::Upper(Box::new(Scalar::Column(author_col("region")))),
        ]));
    let txn = store.begin().await.unwrap();
    let refused = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by([Ordinal(authors().columns().len())], &[Aggregate::Count]),
        )
        .await
        .expect_err("a left-side computed column has nowhere to land");
    let message = refused.to_string();
    assert!(
        message.contains("Join::compute"),
        "the refusal should say where the value goes instead: {message}"
    );
}

/// The plain join path *keeps* them, which is why the refusal is not there.
///
/// This test was briefly the opposite. The refusal above was first put in
/// `Join::validate`, on the reasoning that "the value is dropped on every path
/// — the truncation is in `flatten` and in the cursor alike". The first half is
/// true and the second is not: the ungrouped path never flattens. It hands back
/// each side's row as it stands, computed values included, and the wire splits
/// them into `Row.computed` per input — a feature with its own test in
/// `slate-server`, `a_join_inputs_computed_values_come_back_beside_its_columns`,
/// which is what caught the over-reach.
///
/// So the rule is about flattening, not about joins, and it lives where the
/// flattening happens. This test pins the half that must keep working.
#[tokio::test]
async fn the_ungrouped_join_path_keeps_a_sides_computed_values() {
    let store = store().await;
    let join = Join::equating(author_col("id"), book_col("author_id"))
        .left(
            Query::all().computing([Scalar::Upper(Box::new(Scalar::Column(author_col(
                "region",
            ))))]),
        )
        .right(Query::all().computing([Scalar::Mul(
            Box::new(Scalar::Column(book_col("pages"))),
            Box::new(Scalar::Literal(Value::I64(2))),
        )]));
    let (left_table, right_table) = (authors(), books());
    let txn = store.begin().await.unwrap();
    let mut cursor = txn
        .join(&root(), &left_table, &right_table, &join)
        .await
        .expect("an ungrouped join may compute on either side");

    let mut seen = 0;
    while let Some(row) = cursor.next().await.unwrap() {
        let left = row.left.as_ref().expect("an inner join pairs both");
        let right = row.right.as_ref().expect("an inner join pairs both");
        // Each side is its table's width plus its own computed values, in its
        // own space — which is exactly what the joined space cannot express
        // and why grouping refuses it.
        assert_eq!(left.values().len(), authors().columns().len() + 1);
        assert_eq!(right.values().len(), books().columns().len() + 1);
        let region = left.values()[author_col("region").0].clone();
        let Value::Str(region) = region else {
            panic!("region is {region:?}")
        };
        assert_eq!(
            left.values()[authors().columns().len()],
            Value::Str(region.to_uppercase())
        );
        seen += 1;
    }
    assert_eq!(seen, BOOK_ROWS as usize);
}

/// A grouped *chain* refuses a step's computed column for the same reason.
///
/// Chains flatten by declared table width too, so the value would vanish and
/// the ordinal would land on the next table. A chain has no `compute` of its
/// own, so there is nowhere to redirect to — the refusal names the missing
/// feature rather than pretending the query worked.
#[tokio::test]
async fn a_grouped_chain_refuses_a_steps_computed_column() {
    use slate_kernel::{Chain, JoinStep};
    let store = store().await;
    let mut chain = Chain::from(Query::all());
    chain.steps = vec![
        JoinStep::equating(author_col("id"), book_col("author_id")).query(Query::all().computing(
            [Scalar::Mul(
                Box::new(Scalar::Column(book_col("pages"))),
                Box::new(Scalar::Literal(Value::I64(2))),
            )],
        )),
    ];
    let tables = [authors(), books()];
    let refs: Vec<&_> = tables.iter().collect();
    let txn = store.begin().await.unwrap();
    let refused = txn
        .group_by_chain(
            &root(),
            &refs,
            &chain,
            &Grouping::by([author_col("region")], &[Aggregate::Count]),
        )
        .await
        .expect_err("a step's computed column is truncated by the flatten");
    assert!(refused.to_string().contains("no slot"), "{refused}");
}

// --- computing over the joined row ----------------------------------------

/// The join's own computed value, grouped, against a fold.
///
/// `authors.id * 100` is a left-side expression, so this is the case the
/// refusal above sends here. Six authors, each with four books, so every group
/// counts four.
#[tokio::test]
async fn grouping_a_join_by_a_computed_value_agrees_with_a_fold() {
    let store = store().await;
    let schema = slate_kernel::JoinSchema::of(&authors(), &books()).computing(1);
    let join = Join::equating(author_col("id"), book_col("author_id")).computing([Scalar::Mul(
        Box::new(Scalar::Column(author_col("id"))),
        Box::new(Scalar::Literal(Value::U64(100))),
    )]);
    let grouping = Grouping::by([schema.computed(0)], &[Aggregate::Count]);

    let mut counts: std::collections::BTreeMap<u64, u64> = std::collections::BTreeMap::new();
    for id in 0..BOOK_ROWS {
        *counts.entry((id % AUTHOR_ROWS) * 100).or_default() += 1;
    }
    let want: Vec<Group> = counts
        .into_iter()
        .map(|(key, count)| Group {
            key: vec![Value::U64(key)],
            values: vec![Value::U64(count)],
        })
        .collect();

    let txn = store.begin().await.unwrap();
    let got = txn
        .group_by_join(&root(), &authors(), &books(), &join, &grouping)
        .await
        .unwrap();
    assert_eq!(got, want);
}

/// And it agrees whichever algorithm serves it, which is the property the
/// narrowed projection can break: a computed value's inputs are gathered from
/// the expressions rather than from the grouping, and a side that stopped
/// reading them would compute from nulls on that side only.
#[tokio::test]
async fn a_computed_join_key_does_not_depend_on_the_algorithm() {
    let store = store().await;
    let schema = slate_kernel::JoinSchema::of(&authors(), &books()).computing(1);
    // Reads *both* sides, which a per-side computed column could not express
    // at all and is the reason this lives on the join.
    let spanning = Scalar::Sub(
        Box::new(Scalar::Column(schema.right(book_col("pages")))),
        Box::new(Scalar::Column(schema.left(author_col("id")))),
    );
    let grouping = Grouping::by([schema.computed(0)], &[Aggregate::Count]);

    let mut answers: Vec<(JoinAlgorithm, Vec<Group>)> = Vec::new();
    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::NestedLoop,
    ] {
        let mut join =
            Join::equating(author_col("id"), book_col("author_id")).computing([spanning.clone()]);
        join.force = Some(algorithm);
        let txn = store.begin().await.unwrap();
        let groups = txn
            .group_by_join(&root(), &authors(), &books(), &join, &grouping)
            .await
            .unwrap();
        answers.push((algorithm, groups));
    }
    let (first_algorithm, first) = &answers[0];
    assert!(
        first.len() > 1,
        "a single group would make this agree trivially: {first:?}"
    );
    for (algorithm, groups) in &answers[1..] {
        assert_eq!(
            groups, first,
            "{algorithm:?} grouped a computed join into {groups:?}, where \
             {first_algorithm:?} produced {first:?}"
        );
    }
}

// --- what a computed value may name, and where it comes out ---------------

/// `Join::compute` shipped with **no validation of its own references at all**.
///
/// Every other ordinal in a joined read is checked — `having` against the
/// joined width, and `Grouping` since `validate_grouping` — but the scalars in
/// `compute` were converted straight through. `Scalar::Column` falls back to
/// `Value::Null` for an ordinal the row does not have, so grouping by a
/// computed value that read column 15 of a six-column joined row returned
/// `[Group { key: [Null], values: [U64(24)] }]`: one group, every row in it, no
/// error. That is the same failure `validate_grouping` was written to stop, one
/// level further down, and it was invisible for the same reason — an
/// out-of-range ordinal and a genuinely null column are indistinguishable once
/// the answer is a table of numbers.
#[tokio::test]
async fn a_computed_value_naming_a_column_nobody_has_is_refused() {
    let store = store().await;
    let width = authors().columns().len() + books().columns().len();
    let join = Join::equating(author_col("id"), book_col("author_id"))
        .computing([Scalar::Column(Ordinal(width + 9))]);
    let txn = store.begin().await.unwrap();
    let refused = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by([Ordinal(width)], &[Aggregate::Count]),
        )
        .await
        .expect_err("a computed value cannot read a column nobody has");
    let message = refused.to_string();
    assert!(message.contains("outside"), "{message}");
}

/// Naming a *later* computed value is the other half, and gets its own message.
///
/// The `i`th is evaluated with the `i` before it appended, so naming itself or
/// one produced after it reads as null — and the fix is not the same as for an
/// ordinal past the end, which is why the refusal distinguishes them: this one
/// is repaired by reordering the list.
#[tokio::test]
async fn a_computed_value_naming_a_later_one_is_refused_and_says_to_reorder() {
    let store = store().await;
    let width = authors().columns().len() + books().columns().len();
    // compute[0] reads compute[1], which does not exist yet when it runs.
    let join = Join::equating(author_col("id"), book_col("author_id")).computing([
        Scalar::Column(Ordinal(width + 1)),
        Scalar::Column(book_col("pages")),
    ]);
    let txn = store.begin().await.unwrap();
    let refused = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by([Ordinal(width)], &[Aggregate::Count]),
        )
        .await
        .expect_err("a computed value cannot read one produced after it");
    let message = refused.to_string();
    assert!(message.contains("move it later"), "{message}");
}

/// Reading **its own** slot is the boundary case, and the one a mutation found.
///
/// `a_computed_value_naming_a_later_one_is_refused` does not cover it: an
/// off-by-one in the bound (`columns + index + 1`) still refuses a reference to
/// a *later* value while quietly admitting a self-reference, which evaluates
/// against a row that does not contain it yet and is therefore always null.
#[tokio::test]
async fn a_computed_value_naming_its_own_slot_is_refused() {
    let store = store().await;
    let width = authors().columns().len() + books().columns().len();
    let join = Join::equating(author_col("id"), book_col("author_id"))
        .computing([Scalar::Column(Ordinal(width))]);
    let txn = store.begin().await.unwrap();
    let refused = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by([Ordinal(width)], &[Aggregate::Count]),
        )
        .await
        .expect_err("a computed value cannot read itself");
    assert!(refused.to_string().contains("move it later"), "{refused}");
}

/// Reading the value **before** it is the supported case, and still works.
#[tokio::test]
async fn a_computed_value_may_read_an_earlier_one() {
    let store = store().await;
    let width = authors().columns().len() + books().columns().len();
    let at = slate_kernel::JoinSchema::of(&authors(), &books());
    let join = Join::equating(author_col("id"), book_col("author_id")).computing([
        Scalar::Div(
            // The joined space, not `books`' own — a computed value is
            // evaluated over the flattened row, so `book_col("pages")` raw
            // would read `authors.width + 0`, which is `books.id`. That is
            // exactly the shift `JoinSchema::right` exists to remove, and
            // getting it wrong here produced three plausible groups instead of
            // seven.
            Box::new(Scalar::Column(at.right(book_col("pages")))),
            Box::new(Scalar::Literal(Value::I64(10))),
        ),
        Scalar::Mul(
            Box::new(Scalar::Column(Ordinal(width))),
            Box::new(Scalar::Literal(Value::I64(2))),
        ),
    ]);
    let txn = store.begin().await.unwrap();
    let groups = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by([Ordinal(width + 1)], &[Aggregate::Count]),
        )
        .await
        .expect("reading an earlier computed value is the whole point of the ordering");
    // `pages` is `(id % 7) * 10`, so `pages/10*2` is `(id % 7) * 2`: seven
    // groups at 0, 2, 4, 6, 8, 10, 12.
    let keys: Vec<Value> = groups.iter().map(|g| g.key[0].clone()).collect();
    assert_eq!(
        keys,
        (0..7).map(|n| Value::I64(n * 2)).collect::<Vec<_>>(),
        "the second computed value should be twice the first"
    );
}

/// An **ungrouped** join used to accept `Join::compute` and produce nothing.
///
/// The field was added for the grouped path and the values were produced where
/// that path flattens, so an ungrouped join with a computed column returned
/// each side at its declared width, no computed value anywhere, and no error:
/// `left=2 right=4` on a fixture whose join computed one value. Silently
/// dropping something the caller asked for is the same class of defect as
/// silently answering a different question.
///
/// They now come out on `JoinedRow::computed`, filled by `JoinCursor::next` —
/// the single funnel every joined row leaves through, which is what stops the
/// two paths drifting. It is a third field rather than a tail on one side's row
/// because a value spanning both sides belongs to neither.
#[tokio::test]
async fn an_ungrouped_join_returns_the_joins_computed_values() {
    let store = store().await;
    let at = slate_kernel::JoinSchema::of(&authors(), &books());
    let join = Join::equating(author_col("id"), book_col("author_id")).computing([Scalar::Mul(
        // Joined-space, as above.
        Box::new(Scalar::Column(at.right(book_col("pages")))),
        Box::new(Scalar::Literal(Value::I64(2))),
    )]);
    let (left_table, right_table) = (authors(), books());
    let txn = store.begin().await.unwrap();
    let mut cursor = txn
        .join(&root(), &left_table, &right_table, &join)
        .await
        .unwrap();

    let mut seen = 0;
    while let Some(row) = cursor.next().await.unwrap() {
        let right = row.right.as_ref().expect("an inner join pairs both");
        let Value::I64(pages) = right.values()[book_col("pages").0] else {
            panic!("pages is not an integer")
        };
        assert_eq!(
            row.computed,
            vec![Value::I64(pages * 2)],
            "the join's computed value should come back on every row"
        );
        // And the sides are still exactly their tables' widths, which is what
        // makes a third field necessary rather than a tail on one of them.
        assert_eq!(row.left.as_ref().unwrap().values().len(), 2);
        assert_eq!(right.values().len(), 4);
        seen += 1;
    }
    assert_eq!(seen, BOOK_ROWS as usize);
}

/// A computed value spanning **both** sides, which is the thing no side's own
/// `Query::compute` can express and therefore the reason this field exists.
#[tokio::test]
async fn a_computed_value_may_read_both_sides_at_once() {
    let store = store().await;
    let at = slate_kernel::JoinSchema::of(&authors(), &books());
    let join =
        Join::equating(author_col("id"), book_col("author_id")).computing([Scalar::Concat(vec![
            Scalar::Column(at.left(author_col("region"))),
            Scalar::Literal(Value::Str("/".into())),
            Scalar::Column(at.right(book_col("shelf"))),
        ])]);
    let (left_table, right_table) = (authors(), books());
    let txn = store.begin().await.unwrap();
    let mut cursor = txn
        .join(&root(), &left_table, &right_table, &join)
        .await
        .unwrap();
    let mut seen = 0;
    while let Some(row) = cursor.next().await.unwrap() {
        let Value::Str(region) =
            row.left.as_ref().unwrap().values()[author_col("region").0].clone()
        else {
            panic!("region is not a string")
        };
        let Value::Str(shelf) = row.right.as_ref().unwrap().values()[book_col("shelf").0].clone()
        else {
            panic!("shelf is not a string")
        };
        assert_eq!(row.computed, vec![Value::Str(format!("{region}/{shelf}"))]);
        seen += 1;
    }
    assert_eq!(seen, BOOK_ROWS as usize);
}
