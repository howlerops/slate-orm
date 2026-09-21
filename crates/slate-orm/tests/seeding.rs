//! Seeding from Rust, which the comparison table says has no entry point.
//!
//! It has one, and this file is it written down: `insert_records` under
//! `SecurityContext::superuser()`. That is what `slate-serverd --seed` does at
//! one remove — `seed.rs`'s `load` parses TOML into rows and calls
//! `insert_many` with the same context, which is the untyped call
//! `insert_records` delegates to — so a Rust program needs no daemon, no wire
//! and no new API to seed.
//!
//! Worth a test rather than a sentence for two reasons. The claim it corrects
//! is in `docs/orm-comparison.md`, so it should be demonstrated rather than
//! asserted; and the *property* that makes seeding a distinct operation — rows
//! landing for owners who are not the writer, before any of them could have
//! read the row they are given — is one nothing else in this crate exercises.
//!
//! What is genuinely missing is the other half of that row: nothing
//! *generates* rows. There is no factory, and the two rows below are written
//! out by hand, which is exactly what a factory would remove.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_orm::{
    Action, Catalog, Expr, Grant, Policy, Principal, Query, Record, RecordStore, Records,
    SecurityCatalog, SecurityContext, TableId, Value, memory::MemoryStore,
};

const NOTES: TableId = TableId(1);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "notes", id = 1)]
struct Note {
    #[record(pk)]
    id: u64,
    owner: u64,
    body: String,
}

/// A catalog whose policy admits only a principal's own rows.
///
/// The policy is the point. A seed writes rows for owners who are not the
/// writer — that is what makes it a seed rather than an insert — so a table
/// with no policy would demonstrate nothing: every context would succeed and
/// the superuser would be decoration.
fn store() -> RecordStore<MemoryStore> {
    let security = SecurityCatalog::new()
        .grant(Grant::new("app", NOTES, Action::ALL))
        .policy(Policy::new(
            "own_rows",
            NOTES,
            Action::ALL,
            |context: &SecurityContext| {
                Expr::eq(
                    Note::table().ordinal_of("owner").expect("owner"),
                    context.principal().id.clone(),
                )
            },
        ));
    RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([Note::table().clone()]).unwrap(),
        security,
    )
}

fn caller(id: u64) -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(id)).with_role("app"))
}

fn rows() -> Vec<Note> {
    vec![
        Note {
            id: 1,
            owner: 10,
            body: "for ten".to_owned(),
        },
        Note {
            id: 2,
            owner: 20,
            body: "for twenty".to_owned(),
        },
    ]
}

/// The entry point, in full: `insert_records` as a superuser.
///
/// Five lines and no daemon. The comparison table said the Rust library had
/// none; what it does not have is anything that *generates* the rows.
#[tokio::test]
async fn a_rust_program_can_seed_rows_for_owners_that_are_not_itself() {
    let store = store();
    let txn = store.begin().await.unwrap();
    txn.insert_records(&SecurityContext::superuser(), &rows())
        .await
        .unwrap();
    txn.commit().await.unwrap();

    // Each owner reads back exactly their own row, under the policy that was
    // never consulted when the rows were written.
    for (owner, expected) in [(10_u64, "for ten"), (20, "for twenty")] {
        let txn = store.begin().await.unwrap();
        let found: Vec<Note> = txn
            .query_records(&caller(owner), &Query::all())
            .await
            .unwrap();
        txn.rollback();
        assert_eq!(found.len(), 1, "owner {owner} saw {found:?}");
        assert_eq!(found[0].body, expected);
    }
}

/// The superuser step is doing work, not decoration.
///
/// Without it this would be the *same* two rows written by an ordinary caller,
/// and the test above would pass for the wrong reason. The policy is a
/// `WITH CHECK` on writes too, so a caller cannot write a row it could not
/// read — which is exactly why seeding needs a context that bypasses it.
#[tokio::test]
async fn an_ordinary_caller_cannot_write_a_row_it_could_not_read() {
    let store = store();
    let txn = store.begin().await.unwrap();
    let refused = txn.insert_records(&caller(10), &rows()).await;
    assert!(
        refused.is_err(),
        "a caller wrote a row owned by somebody else: {refused:?}"
    );
    txn.rollback();

    // Deliberately not asserting that the refused batch left no prefix behind.
    // That is a real property and the kernel already pins it, in
    // `constraints.rs`'s `a_bulk_insert_with_one_failing_check_lands_nothing`,
    // which *commits* after the refusal and then looks — strictly stronger
    // than anything reachable from here. The version this test carried for an
    // hour read the table back inside this transaction and asserted it empty,
    // and mutation testing showed it could not fail: two independent guards in
    // `insert_many` each stop a prefix landing, so no single-point mutation
    // reaches it. An assertion no mutation can break is decoration.
}
