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

use slate_helpdesk::{Agent, Helpdesk, HelpdeskError, Indexed, Ticket, caller};
use slate_orm::{Aggregate, Record, Records, SecurityContext, Value, memory::MemoryStore};
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
    txn.insert_record(&dana, &Indexed(wrong))
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

    // The stale edit is to `hours_logged`, not to `assignee_id`, so the
    // refusal can only be about the row having moved. An assignee edit would
    // also be refused — by `agents_work_their_own_tickets` — and the test
    // would pass for the wrong reason.
    let mut stale = before.clone();
    stale.hours_logged = 99;
    let txn = desk.store().begin().await.expect("begin");
    let refused = txn
        .replace_record(&dana, &Indexed(before), &Indexed(stale))
        .await;
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

#[tokio::test]
async fn the_hand_written_tickets_table_matches_the_derived_one() {
    // The cost of the text-index workaround, made into a test rather than
    // left as a comment. `TICKETS_TABLE` restates every column
    // `#[derive(Record)]` produced so that it can add two indexes the derive
    // cannot declare; if a field is added to `Ticket` and not here, the row
    // codec and the schema disagree and nothing else would say so.
    let derived = Ticket::table();
    let written = Indexed::table();

    assert_eq!(written.name(), derived.name());
    assert_eq!(written.id(), derived.id());
    assert_eq!(written.primary_key(), derived.primary_key());
    assert_eq!(written.tenant_column(), derived.tenant_column());
    assert_eq!(written.soft_delete(), derived.soft_delete());
    assert_eq!(written.schema_version(), derived.schema_version());

    let shape = |table: &slate_orm::TableDef| {
        table
            .columns()
            .iter()
            .map(|c| {
                (
                    c.name().to_owned(),
                    c.value_type(),
                    c.is_nullable(),
                    c.managed(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shape(written),
        shape(derived),
        "the hand-written tickets table has drifted from `#[derive(Record)]`"
    );

    // And the only difference is the two text indexes.
    let names = |table: &slate_orm::TableDef| {
        let mut out = table
            .indexes()
            .iter()
            .map(|i| i.name().to_owned())
            .collect::<Vec<_>>();
        out.sort();
        out
    };
    assert_eq!(
        names(written),
        [
            "tickets_by_body_term",
            "tickets_by_priority",
            "tickets_by_reference",
            "tickets_by_subject_term",
        ]
    );
    assert_eq!(
        names(derived),
        ["tickets_by_priority", "tickets_by_reference"]
    );
}

#[tokio::test]
async fn a_search_finds_a_ticket_by_a_word_in_its_body() {
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    for (id, reference, subject) in [
        (60, "ACME-10", "Printer jammed"),
        (61, "ACME-11", "Invoice wrong"),
        (62, "ACME-12", "Laptop will not boot"),
    ] {
        desk.open_ticket(&dana, ticket(ACME, id, reference, subject, 2))
            .await
            .expect("opening");
    }

    // The subject index.
    let found = desk.search(&dana, "printer").await.expect("searching");
    assert_eq!(
        found
            .iter()
            .map(|t| t.reference.as_str())
            .collect::<Vec<_>>(),
        ["ACME-10"]
    );

    // The body index: `ticket()` writes "<subject>. Please advise.", and
    // "advise" appears in no subject.
    let found = desk.search(&dana, "advise").await.expect("searching");
    assert_eq!(found.len(), 3, "every body says it");

    // Two terms is a conjunction within one column, not a disjunction.
    let found = desk.search(&dana, "laptop boot").await.expect("searching");
    assert_eq!(
        found
            .iter()
            .map(|t| t.reference.as_str())
            .collect::<Vec<_>>(),
        ["ACME-12"]
    );

    // A blank box matches nothing rather than everything, which is
    // `Expr::contains`'s documented answer for an empty term list.
    assert!(
        desk.search(&dana, "   ")
            .await
            .expect("searching")
            .is_empty()
    );

    // And the tenant wall holds over an index the query never names.
    let raj = caller(RAJ, GLOBEX, "agent");
    assert!(
        desk.search(&raj, "printer")
            .await
            .expect("searching")
            .is_empty()
    );
}

#[tokio::test]
async fn the_inbox_pages_without_repeating_or_skipping_a_ticket() {
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    for n in 0..7_u128 {
        desk.open_ticket(
            &dana,
            ticket(
                ACME,
                100 + n,
                &format!("ACME-2{n}"),
                "Queued",
                i64::try_from(n).expect("small"),
            ),
        )
        .await
        .expect("opening");
    }

    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = desk.inbox(&dana, cursor, 3).await.expect("a page");
        pages += 1;
        seen.extend(page.tickets.iter().map(|t| t.reference.clone()));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
        assert!(pages < 10, "the cursor is not advancing");
    }

    // 7 rows at 3 a page: 3, 3, 1 — and the last is short, so it is the last.
    assert_eq!(pages, 3);
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        7,
        "a ticket was repeated or skipped: {seen:?}"
    );

    // A closed ticket leaves the queue.
    desk.close(&dana, "ACME-20").await.expect("closing");
    let page = desk.inbox(&dana, None, 50).await.expect("a page");
    assert_eq!(page.tickets.len(), 6);
    assert!(page.is_last());
}

#[tokio::test]
async fn most_urgent_is_sorted_and_the_paged_read_is_not() {
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    // Inserted worst-last, so an unsorted read cannot pass by accident.
    for (id, reference, priority) in [
        (200, "ACME-30", 3),
        (201, "ACME-31", 1),
        (202, "ACME-32", 2),
    ] {
        desk.open_ticket(&dana, ticket(ACME, id, reference, "Sorted", priority))
            .await
            .expect("opening");
    }

    let urgent = desk.most_urgent(&dana, 10).await.expect("the urgent list");
    assert_eq!(
        urgent.iter().map(|t| t.priority).collect::<Vec<_>>(),
        [1, 2, 3]
    );

    // And a limit takes the worst, not the first three found.
    let worst = desk.most_urgent(&dana, 1).await.expect("the worst");
    assert_eq!(worst.len(), 1);
    assert_eq!(worst[0].reference, "ACME-31");
}

#[tokio::test]
async fn a_sorted_inbox_page_is_refused_rather_than_silently_misordered() {
    // The finding behind `Helpdesk::inbox` paging in key order rather than
    // in priority order: `Query::after` pins the access path to the table's
    // key range, so a page cannot also be sorted. The refusal is the right
    // answer — a cursor into an order the key does not describe would skip
    // and repeat rows — but it does mean an inbox is *either* paged *or*
    // prioritised, and an application has to pick.
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    desk.open_ticket(&dana, ticket(ACME, 300, "ACME-40", "Sorted page", 1))
        .await
        .expect("opening");

    let query = slate_orm::Query::all()
        .sort_by([slate_orm::SortKey::asc(Ticket::COLUMNS.priority)])
        .limit(2);
    let txn = desk.store().begin().await.expect("begin");
    let said = txn
        .page_records::<Indexed>(&dana, &query)
        .await
        .expect_err(
            "a sorted page was served, so the cursor it returns names a row \
             in an order the primary key does not give",
        )
        .to_string();
    // Named rather than just "is_err": the same call would also fail with no
    // limit, and a test that cannot tell those apart passes for the wrong
    // reason. (This one nearly did — the limit is set two lines above
    // precisely so the sort is the only thing left to refuse.)
    assert!(
        said.contains("sorted into an order the primary key does not give"),
        "refused, but not for the reason this test is about: {said}"
    );
}

#[tokio::test]
async fn the_workload_rolls_up_per_agent_and_keeps_the_unassigned_pile() {
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    for (id, reference) in [(400, "ACME-50"), (401, "ACME-51"), (402, "ACME-52")] {
        desk.open_ticket(&dana, ticket(ACME, id, reference, "Work", 2))
            .await
            .expect("opening");
    }
    desk.assign(&dana, "ACME-50", DANA).await.expect("taking");
    desk.assign(&dana, "ACME-51", DANA).await.expect("taking");
    desk.log_time(&dana, "ACME-50", 150).await.expect("logging");
    desk.log_time(&dana, "ACME-51", 25).await.expect("logging");

    let mut rolled = desk.workload(&dana).await.expect("the roll-up");
    rolled.sort_by_key(|w| w.assignee_id);

    // The unassigned pile sorts first because `None < Some(_)`, and it is
    // there at all because `GROUP BY` keeps the nulls together rather than
    // dropping them — the thing an application most wants to see.
    assert_eq!(rolled.len(), 2, "{rolled:?}");
    assert_eq!(rolled[0].assignee_id, None);
    assert_eq!(rolled[0].tickets, 1);
    assert_eq!(rolled[0].hundredths, 0);
    assert_eq!(rolled[1].assignee_id, Some(DANA));
    assert_eq!(rolled[1].tickets, 2);
    assert_eq!(rolled[1].hundredths, 175, "no lost cent");
}

#[tokio::test]
async fn an_agent_cannot_push_work_onto_a_colleague() {
    // The row policy, doing something. `agents_take_unassigned_tickets` and
    // `agents_work_their_own_tickets` are checked against the row as it will
    // be as well as the row as it was, so handing a ticket to somebody else
    // produces a row the writer's own policies do not admit.
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    desk.open_ticket(&dana, ticket(ACME, 500, "ACME-60", "Mine", 2))
        .await
        .expect("opening");
    desk.assign(&dana, "ACME-60", DANA).await.expect("taking");

    // Dana may not hand it to Raj.
    let said = desk
        .assign(&dana, "ACME-60", RAJ)
        .await
        .expect_err("an agent reassigned a ticket to a colleague")
        .to_string();
    assert!(
        said.contains("row-level security"),
        "refused, but not by a policy: {said}"
    );

    // A supervisor may, and that is what the role is for.
    let boss = caller(DANA, ACME, "supervisor");
    desk.assign(&boss, "ACME-60", RAJ)
        .await
        .expect("a supervisor moves work");

    // And now Dana cannot touch it at all: it is neither unassigned nor hers.
    let refused = desk.log_time(&dana, "ACME-60", 10).await;
    assert!(
        refused.is_err(),
        "an agent logged time against a colleague's ticket: {refused:?}"
    );
}

#[tokio::test]
async fn a_write_a_policy_forbids_says_two_different_things() {
    // A finding, found by running the application rather than by reading:
    // the row policy refuses an **insert** with "row-level security forbids
    // writing this row" and refuses an **update** with "table `tickets` has
    // no row with this primary key".
    //
    // The second is indistinguishable from a row that is genuinely gone, and
    // an application that wants to say "somebody else has this one" cannot.
    // It is very likely deliberate — it is the same disclosure decision
    // `Helpdesk::by_reference` makes on purpose, because telling a caller
    // that a row they may not touch *exists* is a leak — but the read path's
    // version is documented and this one is not, so an application meets it
    // as a puzzle rather than as a rule.
    //
    // Asserted here so that a later decision either way is a test change
    // somebody makes on purpose.
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    let boss = caller(DANA, ACME, "supervisor");
    desk.open_ticket(&dana, ticket(ACME, 600, "ACME-70", "Handed over", 2))
        .await
        .expect("opening");
    desk.assign(&boss, "ACME-70", RAJ)
        .await
        .expect("a supervisor moves work");

    // The update path: a row Dana can read and cannot write.
    let visible = desk
        .by_reference(&dana, "ACME-70")
        .await
        .expect("the read policy is open within the tenant");
    assert_eq!(visible.assignee_id, Some(RAJ));
    let said = desk
        .log_time(&dana, "ACME-70", 10)
        .await
        .expect_err("an agent wrote a colleague's ticket")
        .to_string();
    assert!(
        said.contains("no row with this primary key"),
        "the update refusal has changed shape, which is the good outcome — \
         rewrite this test to assert the new one: {said}"
    );

    // The insert path, for contrast: same rule, different sentence.
    let mut theirs = ticket(ACME, 601, "ACME-71", "Born assigned", 2);
    theirs.assignee_id = Some(RAJ);
    let said = desk
        .open_ticket(&dana, theirs)
        .await
        .expect_err("an agent raised a ticket already assigned to somebody else")
        .to_string();
    assert!(
        said.contains("row-level security"),
        "the insert refusal has changed shape: {said}"
    );

    // And the supervisor can, which is what makes both refusals about the
    // policy rather than about the row being missing or the table being odd.
    desk.log_time(&boss, "ACME-70", 10)
        .await
        .expect("a supervisor works any ticket");
}

#[tokio::test]
async fn reordering_the_aggregate_list_does_not_change_what_the_roll_up_reads() {
    // The finding behind `Records::grouped_records`, demonstrated from both
    // sides. `group_records` answers positionally, so swapping the two
    // aggregates swaps the two numbers and nothing says so; `grouped_records`
    // is asked by name and answers the same either way.
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    desk.open_ticket(&dana, ticket(ACME, 700, "ACME-80", "Billable", 2))
        .await
        .expect("opening");
    desk.assign(&dana, "ACME-80", DANA).await.expect("taking");
    desk.log_time(&dana, "ACME-80", 275).await.expect("logging");

    let query = slate_orm::Query::all();
    let by = [Ticket::COLUMNS.assignee_id];
    let count_first = [
        Aggregate::Count,
        Aggregate::Sum(Ticket::COLUMNS.hours_logged),
    ];
    let sum_first = [
        Aggregate::Sum(Ticket::COLUMNS.hours_logged),
        Aggregate::Count,
    ];

    let txn = desk.store().begin().await.expect("begin");

    // Positionally: `values[0]` is a count under one list and a sum under the
    // other. This is what a call site reading `values[0]` would get, and the
    // assertion is what makes the hazard a demonstration rather than a claim.
    let one = txn
        .group_records::<Indexed>(&dana, &query, &by, &count_first)
        .await
        .expect("grouping");
    let other = txn
        .group_records::<Indexed>(&dana, &query, &by, &sum_first)
        .await
        .expect("grouping");
    assert_eq!(one[0].values[0], Value::I64(1), "position 0 is the count");
    assert_eq!(other[0].values[0], Value::I64(275), "and now it is the sum");

    // By name: the same question, the same answer, either order.
    for list in [count_first, sum_first] {
        let rolled = txn
            .grouped_records::<Indexed>(&dana, &query, &by, &list)
            .await
            .expect("grouping");
        assert_eq!(rolled[0].get(Aggregate::Count), Some(&Value::I64(1)));
        assert_eq!(
            rolled[0].get(Aggregate::Sum(Ticket::COLUMNS.hours_logged)),
            Some(&Value::I64(275))
        );
        assert_eq!(
            rolled[0].key(Ticket::COLUMNS.assignee_id),
            Some(&Value::Uuid(DANA))
        );
        // An aggregate nobody asked for is `None`, never a neighbour's value.
        assert_eq!(
            rolled[0].get(Aggregate::Max(Ticket::COLUMNS.priority)),
            None
        );
        assert_eq!(rolled[0].key(Ticket::COLUMNS.status), None);
    }
}

#[tokio::test]
async fn an_ambiguous_grouped_request_is_refused_before_it_is_read() {
    // `[Count, Count]` makes `get(Count)` ambiguous, and every way of
    // resolving it hides the mistake further from where it was made. The
    // refusal happens at the request, which is the only place the caller can
    // see both entries.
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    let txn = desk.store().begin().await.expect("begin");

    let said = txn
        .grouped_records::<Indexed>(
            &dana,
            &slate_orm::Query::all(),
            &[Ticket::COLUMNS.assignee_id],
            &[Aggregate::Count, Aggregate::Count],
        )
        .await
        .expect_err("a duplicated aggregate was accepted")
        .to_string();
    assert!(
        said.contains("requested twice") && said.contains("ambiguous"),
        "refused, but not for the reason this test is about: {said}"
    );

    // And the same for a grouping column.
    let said = txn
        .grouped_records::<Indexed>(
            &dana,
            &slate_orm::Query::all(),
            &[Ticket::COLUMNS.assignee_id, Ticket::COLUMNS.assignee_id],
            &[Aggregate::Count],
        )
        .await
        .expect_err("a duplicated grouping column was accepted")
        .to_string();
    assert!(
        said.contains("requested twice"),
        "refused, but not for the reason this test is about: {said}"
    );

    // The plain `group_records` still takes both, which is correct: the
    // kernel can compute them, and it is this layer that cannot say which
    // one a lookup meant.
    let both = txn
        .group_records::<Indexed>(
            &dana,
            &slate_orm::Query::all(),
            &[Ticket::COLUMNS.assignee_id],
            &[Aggregate::Count, Aggregate::Count],
        )
        .await
        .expect("the kernel is happy to count twice");
    assert!(both.iter().all(|g| g.values.len() == 2));
}

#[tokio::test]
async fn the_priority_index_wins_somewhere_between_12k_and_13k_rows() {
    // `most_urgent` sorts by `(priority ASC, opened_at DESC)`, which is
    // exactly the order `tickets_by_priority` stores — and the docstring
    // used to say the planner "can serve it without a sort". Nothing checked
    // that, the ledger recorded it as a gap, and checking it found the claim
    // true only above a size this application will probably never reach.
    //
    // Three plans, measured rather than asserted from reading. Asked through
    // `Helpdesk::explain_most_urgent`, which builds its query from the same
    // `urgent()` the method calls — a test that rebuilt the query would
    // assert about a query nobody runs.
    let mut desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    let boss = caller(DANA, ACME, "supervisor");
    let root = SecurityContext::superuser();

    async fn raise(desk: &Helpdesk<MemoryStore>, who: &SecurityContext, from: u128, to: u128) {
        for n in from..to {
            desk.open_ticket(
                who,
                ticket(
                    ACME,
                    1000 + n,
                    &format!("ACME-P{n}"),
                    "Planned",
                    i64::try_from(n % 3).expect("small") + 1,
                ),
            )
            .await
            .expect("opening");
        }
    }

    // 1. No statistics at all. This is a *fresh store*, and it is the half
    //    that was the finding: nothing gathers statistics for you, so the
    //    planner costs everything against `TableStats::assumed` — no rows —
    //    and a scan of a table it believes is empty is free.
    raise(&desk, &dana, 0, 30).await;
    let cold = desk.explain_most_urgent(&boss, 5).await.expect("a plan");
    assert_eq!(cold.access, slate_orm::AccessSummary::TableScan, "{cold}");
    assert!(cold.sorts, "{cold}");
    // 0.0162 rows, printed as `rows=0`: `assumed` times the filter's
    // selectivity. A bound rather than a literal — the exact product is the
    // cost model's business, not this test's.
    assert!(
        cold.estimated_rows < 1.0,
        "the planner already believes there are rows here, so the plan above \
         is not explained by missing statistics: {cold}"
    );

    // 2. Analysed, at 30 rows. Still a sort, and this is *correct*: walking
    //    an index and then doing a point read per row costs more than
    //    scanning thirty rows and sorting them. The index does not earn its
    //    keep at this size and the planner is right to say so.
    desk.analyze(&root).await.expect("gathering statistics");
    let small = desk.explain_most_urgent(&boss, 5).await.expect("a plan");
    assert_eq!(small.access, slate_orm::AccessSummary::TableScan, "{small}");
    assert!(small.sorts, "{small}");
    assert!(
        small.estimated_rows >= 1.0,
        "statistics did not land: {small}"
    );

    // 3. Analysed, at 13,000 rows. Now the index wins and the sort is gone.
    //    Measured crossover: table scan at 12,000, index scan at 13,000. The
    //    index plan's cost is flat at 6.00 because a `LIMIT 5` walk stops
    //    after five entries whatever the table holds, while the scan-and-sort
    //    grows with the table — 1.01 at 30 rows, 2.07 at 3,000, 5.75 at
    //    12,000. 13,000 rather than 12,500 so the test is not sitting on the
    //    boundary; a cost-model change that moves it will be a loud failure
    //    here rather than a silent regression in production.
    raise(&desk, &dana, 30, 13_000).await;
    desk.analyze(&root).await.expect("re-gathering statistics");
    let large = desk.explain_most_urgent(&boss, 5).await.expect("a plan");
    assert_eq!(
        large.access,
        slate_orm::AccessSummary::IndexScan {
            index: "tickets_by_priority".to_owned()
        },
        "the index stores exactly this order and the planner still will not \
         use it at 13,000 rows: {large}"
    );
    assert!(
        !large.sorts,
        "the index gives the order and the executor is sorting anyway: {large}"
    );
    assert_eq!(large.limit, Some(5));

    // And the plan describes the read that actually happens.
    let urgent = desk.most_urgent(&boss, 5).await.expect("the urgent list");
    assert_eq!(urgent.len(), 5);
    assert!(
        urgent.windows(2).all(|w| w[0].priority <= w[1].priority),
        "worst first: {:?}",
        urgent.iter().map(|t| t.priority).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn an_agent_may_not_ask_for_a_plan() {
    // The other half of the grant above, and a finding about the kernel's
    // own design rather than about this application: `Action::ALL` is
    // `[Read, Insert, Update, Delete]` — four, not five. An `agent` holds
    // `Action::ALL` on tickets and still cannot ask for a plan, because a
    // plan is costed against statistics gathered over the whole table and so
    // leaks the shape of rows the row policy hides.
    //
    // Worth a test rather than a comment: "ALL" reads as *all*, and the one
    // thing it does not include is the one an application reaches for when
    // it wants to know why a read is slow.
    let desk = seeded().await;
    let dana = caller(DANA, ACME, "agent");
    let said = desk
        .explain_most_urgent(&dana, 5)
        .await
        .expect_err("`Action::ALL` has quietly grown a fifth action")
        .to_string();
    assert!(
        said.contains("explain"),
        "refused, but not over the explain grant: {said}"
    );
}
