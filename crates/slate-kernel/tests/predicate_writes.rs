//! `delete_where` and `update_where`: the two writes that name rows by a
//! predicate rather than by key.
//!
//! Three things are being established here, and only the first is about the
//! feature working.
//!
//! **It agrees with an oracle.** A predicate write must leave the store in
//! exactly the state a full scan plus a per-key write would leave it in. That
//! is the comparison, over generated data, because the interesting failures are
//! off-by-one in the matched set and those pass every hand-written case
//! somebody thought to write.
//!
//! **It cannot touch a row the policy hides.** This is the security-relevant
//! half. A predicate write reads through [`RecordTransaction::execute`], the
//! same path a read takes, which ANDs the row policy into the predicate — so
//! the property should hold for free. "Should hold for free" is exactly the
//! claim that needs a test, because the free part stops being true the moment
//! somebody writes a faster private scan.
//!
//! **It does not lose a concurrent increment.** The reason `update_where`
//! exists at all: `SET n = n + 1` twice must produce 2, where read-modify-write
//! produces 1 and `update_if_unchanged` produces an error. The test asserts all
//! three, because the value of the new path is only legible next to what it
//! replaces.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, CmpOp, Expr, Grant, Policy, Principal, Query, RecordStore, Scalar, SecurityCatalog,
    SecurityContext,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const COUNTERS: TableId = TableId(1);

/// `counters(id u64 pk, owner u64, n i64, m i64, label str)`.
///
/// Two `i64` columns rather than one, and the second exists for a single test:
/// a swap (`n = m, m = n`) is the only assignment pair that can tell
/// simultaneous evaluation from left-to-right, and a swap needs two columns of
/// the same type to swap between.
fn table() -> TableDef {
    TableDef::builder("counters", COUNTERS)
        .column("id", ValueType::U64)
        .column("owner", ValueType::U64)
        .column("n", ValueType::I64)
        .column("m", ValueType::I64)
        .column("label", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

const ID: Ordinal = Ordinal(0);
const OWNER: Ordinal = Ordinal(1);
const N: Ordinal = Ordinal(2);
const M: Ordinal = Ordinal(3);
const LABEL: Ordinal = Ordinal(4);

fn row(id: u64, owner: u64, n: i64, label: &str) -> Row {
    with_m(id, owner, n, 0, label)
}

fn with_m(id: u64, owner: u64, n: i64, m: i64, label: &str) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::U64(owner),
        Value::I64(n),
        Value::I64(m),
        Value::Str(label.to_owned()),
    ])
}

/// The same table with a secondary index on `owner`.
fn indexed_table() -> TableDef {
    TableDef::builder("counters", COUNTERS)
        .column("id", ValueType::U64)
        .column("owner", ValueType::U64)
        .column("n", ValueType::I64)
        .column("m", ValueType::I64)
        .column("label", ValueType::Str)
        .primary_key(["id"])
        .index(
            slate_schema::IndexDef::builder("by_owner", slate_schema::IndexId(10)).column("owner"),
        )
        .build()
        .expect("valid schema")
}

fn open() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("w", COUNTERS, Action::ALL))
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

async fn seeded(security: SecurityCatalog, rows: &[Row]) -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("valid catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();
    for r in rows {
        txn.insert(&root(), &table(), r).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

/// Every row in the store, by ascending id, as `(id, owner, n, m, label)`.
async fn contents(store: &RecordStore<MemoryStore>) -> Vec<(u64, u64, i64, i64, String)> {
    let txn = store.begin().await.unwrap();
    let table = table();
    let mut cursor = txn.execute(&root(), &table, &Query::all()).await.unwrap();
    let mut out = Vec::new();
    while let Some(r) = cursor.next().await.unwrap() {
        out.push((
            match r.values()[0] {
                Value::U64(v) => v,
                ref other => panic!("id is {other:?}"),
            },
            match r.values()[1] {
                Value::U64(v) => v,
                ref other => panic!("owner is {other:?}"),
            },
            match r.values()[2] {
                Value::I64(v) => v,
                ref other => panic!("n is {other:?}"),
            },
            match r.values()[3] {
                Value::I64(v) => v,
                ref other => panic!("m is {other:?}"),
            },
            match &r.values()[4] {
                Value::Str(v) => v.clone(),
                other => panic!("label is {other:?}"),
            },
        ));
    }
    drop(cursor);
    txn.rollback();
    out.sort_unstable();
    out
}

// --- delete_where -----------------------------------------------------------

#[tokio::test]
async fn delete_where_removes_exactly_the_matching_rows() {
    let store = seeded(
        open(),
        &[
            row(1, 7, 10, "a"),
            row(2, 7, 20, "b"),
            row(3, 8, 30, "c"),
            row(4, 8, 40, "d"),
        ],
    )
    .await;

    let txn = store.begin().await.unwrap();
    let removed = txn
        .delete_where(
            &root(),
            &table(),
            Expr::compare(OWNER, CmpOp::Eq, Value::U64(7)),
        )
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(removed.len(), 2);
    let left: Vec<u64> = contents(&store).await.into_iter().map(|r| r.0).collect();
    assert_eq!(left, vec![3, 4]);
}

/// A predicate matching nothing is not an error, and writes nothing.
#[tokio::test]
async fn delete_where_matching_nothing_removes_nothing() {
    let store = seeded(open(), &[row(1, 7, 10, "a")]).await;

    let txn = store.begin().await.unwrap();
    let removed = txn
        .delete_where(
            &root(),
            &table(),
            Expr::compare(OWNER, CmpOp::Eq, Value::U64(999)),
        )
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(removed.len(), 0);
    assert_eq!(contents(&store).await.len(), 1);
}

/// The index is maintained, which a delete that only removed the row key would
/// fail — and would fail *invisibly*, because the planner picks a table scan on
/// a table this small and would never read the stale entry.
#[tokio::test]
async fn delete_where_maintains_the_secondary_index() {
    let indexed = indexed_table();
    let catalog = Catalog::from_tables([indexed.clone()]).expect("valid catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, open());
    let txn = store.begin().await.unwrap();
    for r in [row(1, 7, 10, "a"), row(2, 7, 20, "b"), row(3, 8, 30, "c")] {
        txn.insert(&root(), &indexed, &r).await.unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.delete_where(
        &root(),
        &indexed,
        Expr::compare(OWNER, CmpOp::Eq, Value::U64(7)),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    // Read back *through the index*: a stale entry points at a row that is no
    // longer there, and the only way to see it is to make the planner use it.
    let txn = store.begin().await.unwrap();
    let mut cursor = txn
        .execute(
            &root(),
            &indexed,
            &Query::all().filter(Expr::compare(OWNER, CmpOp::Eq, Value::U64(7))),
        )
        .await
        .unwrap();
    let mut found = 0;
    while cursor.next().await.unwrap().is_some() {
        found += 1;
    }
    drop(cursor);
    txn.rollback();
    assert_eq!(found, 0, "the index still points at a deleted row");
}

// --- update_where -----------------------------------------------------------

#[tokio::test]
async fn update_where_assigns_over_the_rows_own_values() {
    let store = seeded(open(), &[row(1, 7, 10, "a"), row(2, 8, 20, "b")]).await;

    let txn = store.begin().await.unwrap();
    let written = txn
        .update_where(
            &root(),
            &table(),
            Expr::compare(OWNER, CmpOp::Eq, Value::U64(7)),
            &[(
                N,
                Scalar::Add(
                    Box::new(Scalar::column(N)),
                    Box::new(Scalar::literal(Value::I64(5))),
                ),
            )],
        )
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(written.len(), 1);
    let got = contents(&store).await;
    assert_eq!(got[0].2, 15, "{got:?}");
    assert_eq!(got[1].2, 20, "the unmatched row changed: {got:?}");

    // A string column too, through a different `Scalar` arm: arithmetic and
    // concatenation take separate paths through `evaluate`, and a write path
    // that only ever sees integers is a write path with half its type surface
    // untested.
    let txn = store.begin().await.unwrap();
    txn.update_where(
        &root(),
        &table(),
        Expr::True,
        &[(
            LABEL,
            Scalar::Concat(vec![
                Scalar::column(LABEL),
                Scalar::literal(Value::Str("!".to_owned())),
            ]),
        )],
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    let got = contents(&store).await;
    assert_eq!(got[0].4, "a!", "{got:?}");
    assert_eq!(got[1].4, "b!", "{got:?}");
}

/// Assignments read the *original* row, so `a = b, b = a` swaps.
///
/// The other reading — apply left to right, so both end up holding `b` — is the
/// one that surprises people, and it is what a naive implementation that
/// evaluates against the row it is building would do.
#[tokio::test]
async fn assignments_are_simultaneous_not_sequential() {
    let store = seeded(open(), &[with_m(1, 7, 10, 3, "a")]).await;

    let txn = store.begin().await.unwrap();
    txn.update_where(
        &root(),
        &table(),
        Expr::compare(ID, CmpOp::Eq, Value::U64(1)),
        &[(N, Scalar::column(M)), (M, Scalar::column(N))],
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let got = contents(&store).await;
    // Simultaneous: 10 and 3 trade places. Left to right, `n` becomes 3 and
    // then `m` reads the *new* `n` and becomes 3 as well — so the tell is `m`.
    assert_eq!(got[0].2, 3, "n: {got:?}");
    assert_eq!(got[0].3, 10, "m read a half-written row: {got:?}");
}

#[tokio::test]
async fn assigning_the_same_column_twice_is_refused() {
    let store = seeded(open(), &[row(1, 7, 10, "a")]).await;
    let txn = store.begin().await.unwrap();
    let err = txn
        .update_where(
            &root(),
            &table(),
            Expr::True,
            &[
                (N, Scalar::literal(Value::I64(1))),
                (N, Scalar::literal(Value::I64(2))),
            ],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            slate_kernel::error::KernelError::DuplicateAssignment { .. }
        ),
        "got {err:?}"
    );
    txn.rollback();
    // And nothing was written on the way to discovering it.
    assert_eq!(contents(&store).await[0].2, 10);
}

#[tokio::test]
async fn assigning_a_column_the_table_does_not_have_is_refused() {
    let store = seeded(open(), &[row(1, 7, 10, "a")]).await;
    let txn = store.begin().await.unwrap();
    let err = txn
        .update_where(
            &root(),
            &table(),
            Expr::True,
            &[(Ordinal(9), Scalar::literal(Value::I64(1)))],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, slate_kernel::error::KernelError::NoSuchColumn { .. }),
        "got {err:?}"
    );
    txn.rollback();
}

// --- the policy half --------------------------------------------------------

/// A row the policy hides is a row the predicate write cannot touch.
///
/// Both halves asserted: the hidden row survives a `delete_where` whose
/// predicate plainly matches it, and survives an `update_where` too. Without
/// the policy the same predicate removes everything, which is what makes this
/// a test of the policy rather than of the predicate.
#[tokio::test]
async fn a_predicate_write_cannot_reach_a_row_the_policy_hides() {
    let mine = SecurityCatalog::new()
        .grant(Grant::new("w", COUNTERS, Action::ALL))
        .policy(Policy::new(
            "own",
            COUNTERS,
            Action::ALL,
            |ctx: &SecurityContext| Expr::compare(OWNER, CmpOp::Eq, ctx.principal().id.clone()),
        ));
    let rows = [row(1, 7, 10, "a"), row(2, 8, 20, "b"), row(3, 7, 30, "c")];

    let store = seeded(mine.clone(), &rows).await;
    let seven = SecurityContext::new(Principal::new(Value::U64(7)).with_role("w"));

    let txn = store.begin().await.unwrap();
    let removed = txn
        .delete_where(&seven, &table(), Expr::True)
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(removed.len(), 2, "owner 7 has two rows");
    let left: Vec<u64> = contents(&store).await.into_iter().map(|r| r.0).collect();
    assert_eq!(left, vec![2], "owner 8's row was deleted by owner 7");

    // The same predicate, unpoliced, takes everything — so the survival above
    // is the policy and not a quirk of `Expr::True`.
    let store = seeded(mine.clone(), &rows).await;
    let txn = store.begin().await.unwrap();
    let removed = txn
        .delete_where(&root(), &table(), Expr::True)
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(removed.len(), 3);
    assert!(contents(&store).await.is_empty());

    // And the update half.
    let store = seeded(mine, &rows).await;
    let txn = store.begin().await.unwrap();
    let written = txn
        .update_where(
            &seven,
            &table(),
            Expr::True,
            &[(N, Scalar::literal(Value::I64(0)))],
        )
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(written.len(), 2);
    let got = contents(&store).await;
    assert_eq!(
        got[1].2, 20,
        "owner 8's row was updated by owner 7: {got:?}"
    );
}

/// Editing your way *out* of a policy is refused, as it is for a keyed update.
#[tokio::test]
async fn a_predicate_update_cannot_write_a_row_the_policy_would_hide() {
    let mine = SecurityCatalog::new()
        .grant(Grant::new("w", COUNTERS, Action::ALL))
        .policy(Policy::new(
            "own",
            COUNTERS,
            Action::ALL,
            |ctx: &SecurityContext| Expr::compare(OWNER, CmpOp::Eq, ctx.principal().id.clone()),
        ));
    let store = seeded(mine, &[row(1, 7, 10, "a")]).await;
    let seven = SecurityContext::new(Principal::new(Value::U64(7)).with_role("w"));

    let txn = store.begin().await.unwrap();
    let err = txn
        .update_where(
            &seven,
            &table(),
            Expr::True,
            &[(OWNER, Scalar::literal(Value::U64(8)))],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, slate_kernel::error::KernelError::RowCheckFailed { .. }),
        "got {err:?}"
    );
    txn.rollback();
    assert_eq!(contents(&store).await[0].1, 7);
}

// --- the concurrency property -----------------------------------------------

/// Two increments make two, which read-modify-write cannot promise.
///
/// All three spellings in one test, because the value of `update_where` is only
/// legible beside what it replaces:
///
/// - read-modify-write in two transactions loses one increment, silently;
/// - `update_if_unchanged` refuses the second, which is better and is still
///   not the answer the caller wanted;
/// - `update_where` with `n = n + 1` gives 2.
#[tokio::test]
async fn two_increments_make_two() {
    // Read-modify-write: both read 0, both write 1.
    let store = seeded(open(), &[row(1, 7, 0, "a")]).await;
    let key = [Value::U64(1)];
    let a = {
        let txn = store.begin().await.unwrap();
        let got = txn.get(&root(), &table(), &key).await.unwrap().unwrap();
        txn.rollback();
        got
    };
    let b = {
        let txn = store.begin().await.unwrap();
        let got = txn.get(&root(), &table(), &key).await.unwrap().unwrap();
        txn.rollback();
        got
    };
    for stale in [&a, &b] {
        let mut values = stale.values().to_vec();
        values[2] = Value::I64(match values[2] {
            Value::I64(v) => v + 1,
            ref other => panic!("n is {other:?}"),
        });
        let txn = store.begin().await.unwrap();
        txn.update(&root(), &table(), &Row::new(values))
            .await
            .unwrap();
        txn.commit().await.unwrap();
    }
    assert_eq!(
        contents(&store).await[0].2,
        1,
        "read-modify-write did not lose an increment, which this test assumes"
    );

    // `update_if_unchanged` catches it, and still does not apply it.
    let store = seeded(open(), &[row(1, 7, 0, "a")]).await;
    let mut first = a.values().to_vec();
    first[2] = Value::I64(1);
    let txn = store.begin().await.unwrap();
    txn.update_if_unchanged(&root(), &table(), &Row::new(first), &a)
        .await
        .unwrap();
    txn.commit().await.unwrap();
    let mut second = b.values().to_vec();
    second[2] = Value::I64(1);
    let txn = store.begin().await.unwrap();
    let err = txn
        .update_if_unchanged(&root(), &table(), &Row::new(second), &b)
        .await
        .unwrap_err();
    assert!(
        matches!(err, slate_kernel::error::KernelError::RowChanged { .. }),
        "got {err:?}"
    );
    txn.rollback();
    assert_eq!(contents(&store).await[0].2, 1);

    // `update_where`: neither caller reads first, and both increments land.
    let store = seeded(open(), &[row(1, 7, 0, "a")]).await;
    for _ in 0..2 {
        let txn = store.begin().await.unwrap();
        txn.update_where(
            &root(),
            &table(),
            Expr::compare(ID, CmpOp::Eq, Value::U64(1)),
            &[(
                N,
                Scalar::Add(
                    Box::new(Scalar::column(N)),
                    Box::new(Scalar::literal(Value::I64(1))),
                ),
            )],
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
    }
    assert_eq!(contents(&store).await[0].2, 2, "an increment was lost");
}

// --- the oracles ------------------------------------------------------------

/// Rows with distinct ids, so a generated set is a legal table.
fn rows_strategy() -> impl Strategy<Value = Vec<(u64, u64, i64)>> {
    proptest::collection::hash_set(0u64..40, 0..14).prop_flat_map(|ids| {
        let ids: Vec<u64> = ids.into_iter().collect();
        let n = ids.len();
        (
            Just(ids),
            proptest::collection::vec(0u64..4, n),
            proptest::collection::vec(-50i64..50, n),
        )
            .prop_map(|(ids, owners, ns)| {
                ids.into_iter()
                    .zip(owners)
                    .zip(ns)
                    .map(|((id, owner), n)| (id, owner, n))
                    .collect()
            })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// `delete_where(p)` leaves exactly the rows `!p` selects.
    ///
    /// The oracle is a fold over the generated rows, computed without touching
    /// the database — which is the point: a shared helper that both sides used
    /// would agree with itself about a wrong answer.
    #[test]
    fn delete_where_agrees_with_filtering_by_hand(
        rows in rows_strategy(),
        owner in 0u64..4,
        floor in -50i64..50,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            let seed: Vec<Row> = rows
                .iter()
                .map(|(id, o, n)| row(*id, *o, *n, "x"))
                .collect();
            let store = seeded(open(), &seed).await;

            // `owner = :owner AND n >= :floor`, a conjunction so the matched
            // set is not simply "all" or "none" for most inputs.
            let predicate = Expr::all([
                Expr::compare(OWNER, CmpOp::Eq, Value::U64(owner)),
                Expr::compare(N, CmpOp::Ge, Value::I64(floor)),
            ]);

            let txn = store.begin().await.unwrap();
            let removed = txn.delete_where(&root(), &table(), predicate).await.unwrap();
            txn.commit().await.unwrap();

            let mut expected: Vec<u64> = rows
                .iter()
                .filter(|(_, o, n)| !(*o == owner && *n >= floor))
                .map(|(id, _, _)| *id)
                .collect();
            expected.sort_unstable();
            let left: Vec<u64> = contents(&store).await.into_iter().map(|r| r.0).collect();

            prop_assert_eq!(left, expected.clone());
            prop_assert_eq!(removed.len(), rows.len() - expected.len());
            Ok(())
        })?;
    }

    /// `update_where(p, n = n * 2)` doubles exactly the matched rows.
    #[test]
    fn update_where_agrees_with_mapping_by_hand(
        rows in rows_strategy(),
        owner in 0u64..4,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            let seed: Vec<Row> = rows
                .iter()
                .map(|(id, o, n)| row(*id, *o, *n, "x"))
                .collect();
            let store = seeded(open(), &seed).await;

            let txn = store.begin().await.unwrap();
            let written = txn
                .update_where(
                    &root(),
                    &table(),
                    Expr::compare(OWNER, CmpOp::Eq, Value::U64(owner)),
                    &[(N, Scalar::Mul(
                        Box::new(Scalar::column(N)),
                        Box::new(Scalar::literal(Value::I64(2))),
                    ))],
                )
                .await
                .unwrap();
            txn.commit().await.unwrap();

            let mut expected: Vec<(u64, i64)> = rows
                .iter()
                .map(|(id, o, n)| (*id, if *o == owner { n * 2 } else { *n }))
                .collect();
            expected.sort_unstable();
            let got: Vec<(u64, i64)> = contents(&store)
                .await
                .into_iter()
                .map(|(id, _, n, _, _)| (id, n))
                .collect();

            prop_assert_eq!(got, expected);
            prop_assert_eq!(
                written.len(),
                rows.iter().filter(|(_, o, _)| *o == owner).count()
            );
            Ok(())
        })?;
    }
}

// --- the three a mutation found missing -------------------------------------
//
// Each of these exists because breaking the thing it covers changed no test.
// They are written up together rather than filed beside their neighbours so
// that the reason they exist stays attached to them.

/// A caller without the grant cannot delete by predicate, and the refusal
/// happens before anything is read.
///
/// The keyed `delete` authorizes and so does this; a mutation removing the
/// check from `delete_where` alone passed every other test in this file,
/// because every other test uses a context that holds the grant.
#[tokio::test]
async fn a_predicate_write_needs_the_grant() {
    let reader = SecurityCatalog::new().grant(Grant::new("r", COUNTERS, [Action::Read]));
    let store = seeded(reader, &[row(1, 7, 10, "a")]).await;
    let guest = SecurityContext::new(Principal::new(Value::U64(1)).with_role("r"));

    let txn = store.begin().await.unwrap();
    let err = txn
        .delete_where(&guest, &table(), Expr::True)
        .await
        .unwrap_err();
    assert!(
        matches!(err, slate_kernel::error::KernelError::AccessDenied { .. }),
        "got {err:?}"
    );
    txn.rollback();
    assert_eq!(contents(&store).await.len(), 1);

    let txn = store.begin().await.unwrap();
    let err = txn
        .update_where(
            &guest,
            &table(),
            Expr::True,
            &[(N, Scalar::literal(Value::I64(0)))],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, slate_kernel::error::KernelError::AccessDenied { .. }),
        "got {err:?}"
    );
    txn.rollback();
    assert_eq!(contents(&store).await[0].2, 10);
}

/// An update that moves a row between index entries maintains both.
///
/// The delete side of this was already covered; the update side was not, and
/// dropping the previous row from `write_row` — which is how the index learns
/// what to remove — changed no test.
///
/// # Why this has to be an index-*only* read
///
/// The first version of this test read back through an ordinary filter and
/// passed against the broken build, which is worth recording. A scan that
/// fetches the row re-checks the predicate against what it fetched, so a stale
/// entry pointing at a row whose `owner` no longer matches is silently filtered
/// out — the executor is doing its job and hiding the bug while it does. Only a
/// covering read, which answers from the entry and never fetches the row, has
/// nothing left to re-check it against. So the projection here is exactly the
/// indexed column plus the key, and the assertion is that the entry is gone
/// rather than that the query is right.
#[tokio::test]
async fn update_where_moves_the_row_between_index_entries() {
    let indexed = indexed_table();
    let catalog = Catalog::from_tables([indexed.clone()]).expect("valid catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, open());

    let txn = store.begin().await.unwrap();
    for r in [row(1, 7, 10, "a"), row(2, 8, 20, "b")] {
        txn.insert(&root(), &indexed, &r).await.unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.update_where(
        &root(),
        &indexed,
        Expr::compare(ID, CmpOp::Eq, Value::U64(1)),
        &[(OWNER, Scalar::literal(Value::U64(8)))],
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    /// The ids the index reports for an owner, answered from entries alone.
    async fn entries_for(
        store: &RecordStore<MemoryStore>,
        table: &TableDef,
        owner: u64,
    ) -> Vec<u64> {
        let txn = store.begin().await.unwrap();
        let mut cursor = txn
            .query_projected(
                &root(),
                table,
                Expr::compare(OWNER, CmpOp::Eq, Value::U64(owner)),
                slate_kernel::ScanOrder::Ascending,
                &slate_kernel::Projection::Columns(vec![ID, OWNER]),
            )
            .await
            .unwrap();
        let mut ids = Vec::new();
        while let Some(r) = cursor.next().await.unwrap() {
            ids.push(match r.values()[0] {
                Value::U64(v) => v,
                ref other => panic!("id is {other:?}"),
            });
        }
        drop(cursor);
        txn.rollback();
        ids.sort_unstable();
        ids
    }

    // The plan really is index-only, or the assertions below prove nothing:
    // a read that fetches the row re-checks the predicate against it and
    // filters a stale entry out before anybody can see it.
    let chosen = slate_kernel::plan_projected(
        &indexed,
        &Expr::compare(OWNER, CmpOp::Eq, Value::U64(7)),
        slate_kernel::ScanOrder::Ascending,
        &slate_kernel::Projection::Columns(vec![ID, OWNER]),
    );
    assert!(
        matches!(
            chosen.access,
            slate_kernel::Access::IndexScan { covering: true, .. }
        ),
        "not a covering read: {:?}",
        chosen.access
    );

    assert_eq!(
        entries_for(&store, &indexed, 7).await,
        Vec::<u64>::new(),
        "the old index entry survived the move"
    );
    assert_eq!(entries_for(&store, &indexed, 8).await, vec![1, 2]);
}

/// A predicate delete follows the same referential rules the keyed one does.
///
/// Both halves: a `CASCADE` child goes with its parent, and a `RESTRICT` child
/// blocks the whole delete rather than letting some rows through. The second is
/// the one worth having — a partial delete that stopped halfway would leave the
/// store in a state no single operation could have produced, and the keyed
/// `delete` is careful about exactly that.
#[tokio::test]
async fn a_predicate_delete_obeys_foreign_keys() {
    const NOTES: TableId = TableId(2);
    const PINS: TableId = TableId(3);

    fn child(id: TableId, name: &str, action: slate_schema::ReferentialAction) -> TableDef {
        TableDef::builder(name, id)
            .column("id", ValueType::U64)
            .column("counter_id", ValueType::U64)
            .primary_key(["id"])
            .foreign_key(
                slate_schema::ForeignKeyDef::builder(name, COUNTERS)
                    .column("counter_id")
                    .on_delete(action),
            )
            .build()
            .expect("valid schema")
    }
    fn link(id: u64, counter: u64) -> Row {
        Row::new(vec![Value::U64(id), Value::U64(counter)])
    }

    let notes = child(NOTES, "notes", slate_schema::ReferentialAction::Cascade);
    let security = SecurityCatalog::new()
        .grant(Grant::new("w", COUNTERS, Action::ALL))
        .grant(Grant::new("w", NOTES, Action::ALL))
        .grant(Grant::new("w", PINS, Action::ALL));

    // CASCADE: deleting the parents by predicate takes the children too.
    let catalog = Catalog::from_tables([table(), notes.clone()]).expect("valid catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security.clone());
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table(), &row(1, 7, 10, "a"))
        .await
        .unwrap();
    txn.insert(&root(), &table(), &row(2, 8, 20, "b"))
        .await
        .unwrap();
    txn.insert(&root(), &notes, &link(10, 1)).await.unwrap();
    txn.insert(&root(), &notes, &link(11, 2)).await.unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.delete_where(
        &root(),
        &table(),
        Expr::compare(OWNER, CmpOp::Eq, Value::U64(7)),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let mut cursor = txn.execute(&root(), &notes, &Query::all()).await.unwrap();
    let mut left = Vec::new();
    while let Some(r) = cursor.next().await.unwrap() {
        left.push(match r.values()[0] {
            Value::U64(v) => v,
            ref other => panic!("id is {other:?}"),
        });
    }
    drop(cursor);
    txn.rollback();
    assert_eq!(left, vec![11], "the cascade did not reach the child");

    // RESTRICT: one blocking child refuses the whole delete, including the
    // rows that had no child at all.
    let pins = child(PINS, "pins", slate_schema::ReferentialAction::Restrict);
    let catalog = Catalog::from_tables([table(), pins.clone()]).expect("valid catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table(), &row(1, 7, 10, "a"))
        .await
        .unwrap();
    txn.insert(&root(), &table(), &row(2, 7, 20, "b"))
        .await
        .unwrap();
    txn.insert(&root(), &pins, &link(10, 2)).await.unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let err = txn
        .delete_where(
            &root(),
            &table(),
            Expr::compare(OWNER, CmpOp::Eq, Value::U64(7)),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            slate_kernel::error::KernelError::Schema(
                slate_schema::SchemaError::ForeignKeyRestricted { .. }
            )
        ),
        "got {err:?}"
    );
    txn.rollback();
    assert_eq!(
        contents(&store).await.len(),
        2,
        "a refused delete still removed the unblocked row"
    );
}
