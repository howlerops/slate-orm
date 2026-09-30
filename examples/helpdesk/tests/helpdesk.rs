//! The helpdesk, driven the way its callers drive it.
//!
//! Over `MemoryStore`, because these assert behaviour and must be quick.
//! `Helpdesk<S>` is generic over `slate_kernel::KvStore`, so the same service
//! should run over SlateDB and answer the same — *should*, because nothing in
//! this crate has yet run it over anything else. That is a claim, not a
//! result, and `ledger/2026-09-30-an-application-written-against-the-rust-surface.md`
//! records it as one.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_helpdesk::{Agent, Helpdesk, HelpdeskError, Ticket, caller};
use slate_orm::{Records, SecurityContext, Value, memory::MemoryStore};
use uuid::Uuid;

const ACME: Uuid = Uuid::from_u128(1);
const GLOBEX: Uuid = Uuid::from_u128(2);
const DANA: Uuid = Uuid::from_u128(100);
const RAJ: Uuid = Uuid::from_u128(200);

fn ticket(tenant: Uuid, id: u128, reference: &str, subject: &str, priority: i64) -> Ticket {
    Ticket {
        tenant_id: tenant,
        id: Uuid::from_u128(id),
        reference: reference.to_owned(),
        subject: subject.to_owned(),
        body: format!("{subject}. Please advise."),
        status: "open".to_owned(),
        priority,
        assignee_id: None,
        hours_logged: 0,
        opened_at: 0,
        updated_at: 0,
        closed_at: None,
    }
}

async fn seeded() -> Helpdesk<MemoryStore> {
    let desk = Helpdesk::open(MemoryStore::new()).expect("the catalog is valid");
    let root = SecurityContext::superuser();
    for (tenant, name, agent, email) in [
        (ACME, "Acme", DANA, "dana@acme.example"),
        (GLOBEX, "Globex", RAJ, "raj@globex.example"),
    ] {
        desk.found(
            &root,
            &slate_helpdesk::Tenant {
                id: tenant,
                name: name.to_owned(),
            },
            &Agent {
                tenant_id: tenant,
                id: agent,
                email: email.to_owned(),
                name: name.to_owned(),
            },
        )
        .await
        .expect("founding a tenant");
    }
    desk
}

#[tokio::test]
async fn a_ticket_is_raised_and_read_back_by_its_reference() {
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    desk.open_ticket(&dana, ticket(ACME, 10, "ACME-1", "Printer on fire", 1))
        .await
        .expect("opening a ticket");

    let found = desk
        .by_reference(&dana, "ACME-1")
        .await
        .expect("reading it back");
    assert_eq!(found.subject, "Printer on fire");
    assert_eq!(found.status, "open");
}

#[tokio::test]
async fn a_status_the_application_does_not_know_is_refused() {
    // And it is refused *here*, by the service method, not by the schema. A
    // `CHECK` is what should do this and `#[derive(Record)]` cannot declare
    // one — see the note on `Ticket::status`. The test asserts what is true
    // rather than what should be.
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    let mut wrong = ticket(ACME, 11, "ACME-2", "Typo", 3);
    wrong.status = "opne".to_owned();
    match desk.open_ticket(&dana, wrong).await {
        Err(HelpdeskError::NotAStatus(said)) => assert_eq!(said, "opne"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn the_same_status_written_past_the_service_is_not_refused() {
    // The other half of the finding, demonstrated rather than argued: the
    // store takes `"opne"` without complaint, because nothing below this
    // crate knows the four words. If the derive ever grows `check`, this test
    // starts failing and that is the signal to delete it.
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    let mut wrong = ticket(ACME, 12, "ACME-3", "Straight to the store", 3);
    wrong.status = "opne".to_owned();

    let txn = desk.store().begin().await.expect("begin");
    txn.insert_record(&dana, &wrong)
        .await
        .expect("the store does not know the four words");
    txn.commit().await.expect("commit");

    let found = desk
        .by_reference(&dana, "ACME-3")
        .await
        .expect("reading it back");
    assert_eq!(
        found.status, "opne",
        "stored verbatim, which is the finding"
    );
}

#[tokio::test]
async fn one_tenant_cannot_see_another_s_tickets() {
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    let raj = caller(RAJ, GLOBEX, "agent");
    desk.open_ticket(&dana, ticket(ACME, 20, "ACME-9", "Acme only", 2))
        .await
        .expect("opening");

    // Not "empty result" — the same error a missing reference gives, because
    // telling them apart tells a stranger which references exist.
    match desk.by_reference(&raj, "ACME-9").await {
        Err(HelpdeskError::NoSuchTicket(said)) => assert_eq!(said, "ACME-9"),
        other => panic!("Globex read an Acme ticket: {other:?}"),
    }
}

#[tokio::test]
async fn assigning_a_ticket_somebody_else_moved_is_refused() {
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    desk.open_ticket(&dana, ticket(ACME, 30, "ACME-4", "Contended", 2))
        .await
        .expect("opening");

    // Two agents read the same row, then both write. The second must lose:
    // `replace_record` compares against the row the caller decided from.
    let before = desk.by_reference(&dana, "ACME-4").await.expect("read");
    desk.assign(&dana, "ACME-4", DANA)
        .await
        .expect("the first write wins");

    let mut stale = before.clone();
    stale.assignee_id = Some(RAJ);
    let txn = desk.store().begin().await.expect("begin");
    let refused = txn.replace_record(&dana, &before, &stale).await;
    assert!(
        refused.is_err(),
        "the second write overwrote a decision it never saw"
    );
}

#[tokio::test]
async fn a_closed_ticket_is_hidden_and_cannot_be_reopened_from_here() {
    // Soft delete is declarable from the derive — `#[record(soft_delete)]` —
    // and the close works: the row is stamped rather than erased, which is
    // the whole point of the column. The half that is missing is the read.
    //
    // `Deleted` does not appear anywhere in `crates/slate-orm/src`. The
    // kernel has `visible_row_with(..., Deleted::Visible)` and it is private;
    // the wire protocol has `include_deleted` as a privileged read. On the
    // Rust surface there is no include-deleted read and no `restore`, so an
    // application that closes a ticket cannot offer a reopen button. Written
    // up in `ledger/2026-09-30-an-application-written-against-the-rust-surface.md`.
    //
    // This test asserts what is true today. When the surface grows the read,
    // the second half stops being a finding and this test should be rewritten
    // to exercise the reopen rather than to record its absence.
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    desk.open_ticket(&dana, ticket(ACME, 40, "ACME-5", "Solved", 3))
        .await
        .expect("opening");
    desk.close(&dana, "ACME-5").await.expect("closing");

    match desk.by_reference(&dana, "ACME-5").await {
        Err(HelpdeskError::NoSuchTicket(_)) => {}
        other => panic!("a closed ticket is still visible: {other:?}"),
    }

    // Not even the superuser, and that is the finding rather than a policy:
    // the bypass is about *permission*, and hiding a retired row is not a
    // permission. Reading one needs a knob this surface does not have.
    let txn = desk.store().begin().await.expect("begin");
    let hidden: Option<Ticket> = txn
        .get_record(
            &SecurityContext::superuser(),
            &[Value::Uuid(ACME), Value::Uuid(Uuid::from_u128(40))],
        )
        .await
        .expect("the read itself succeeds");
    assert!(
        hidden.is_none(),
        "a superuser read one back, so the ORM does have an include-deleted \
         path after all and this finding is wrong"
    );
}

#[tokio::test]
async fn a_comment_belongs_to_its_ticket_and_the_thread_reads_back() {
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    desk.open_ticket(&dana, ticket(ACME, 50, "ACME-6", "Discuss", 2))
        .await
        .expect("opening");
    desk.comment(&dana, "ACME-6", DANA, "Looking into it")
        .await
        .expect("first");
    desk.comment(&dana, "ACME-6", DANA, "Fixed")
        .await
        .expect("second");

    let thread = desk
        .thread(&dana, "ACME-6")
        .await
        .expect("reading the thread");
    assert_eq!(thread.len(), 2);
    assert!(thread.iter().all(|c| c.tenant_id == ACME));
}

#[tokio::test]
async fn logged_time_accumulates_in_hundredths() {
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    desk.open_ticket(&dana, ticket(ACME, 60, "ACME-7", "Billable", 2))
        .await
        .expect("opening");
    desk.log_time(&dana, "ACME-7", 150)
        .await
        .expect("an hour and a half");
    let after = desk
        .log_time(&dana, "ACME-7", 75)
        .await
        .expect("three quarters");
    assert_eq!(after.hours_logged, 225, "integers, so no cent goes missing");
}
