//! The helpdesk, run over SlateDB on an object store, and re-read after a
//! restart.
//!
//! Everything else in `examples/helpdesk` runs over `MemoryStore`, which is a
//! `BTreeMap`. That is the right thing for tests — they assert behaviour and
//! must be quick — and it means the crate's central claim, *this service is
//! generic over `KvStore` and answers the same over any of them*, was a claim
//! about a type parameter and nothing else. This is the run that settles it.
//!
//! ```sh
//! sh examples/helpdesk/run.sh            # a local-filesystem bucket
//! sh examples/helpdesk/run.sh --s3       # a real signed S3 server
//! ```
//!
//! # What "object store" means here, and why the default is the filesystem
//!
//! SlateDB writes to an `ObjectStore`, and `LocalFileSystem` is one. The LSM
//! above it is the same LSM: the same SSTs, the same WAL, the same manifest,
//! the same compaction. `crates/slate-slatedb/examples/bucket_layout.rs` uses
//! the filesystem for exactly this reason and says so. What the filesystem
//! does *not* reproduce is S3's latency, its conditional-write semantics and
//! its error shapes, and none of those is something this runner asserts — so
//! the default costs nothing and needs no server. `--s3` runs the same code
//! against `s3s` speaking actual signed S3, which is the layer where those
//! differences live.
//!
//! # Why it re-opens the store
//!
//! A run that writes and reads inside one process proves the memtable works.
//! The whole point of an LSM on a bucket is that the bytes survive the
//! process, so the second half closes the store, opens a fresh one over the
//! same prefix, and re-asks every question. A schema that was migrated and
//! an index that was maintained both have to still be there — and the index
//! is the interesting half, because `ledger/2026-09-15-a-new-index-returns-nothing.md`
//! is about an index that existed and was empty.

#![allow(
    clippy::print_stdout,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use slate_helpdesk::{
    Agent, Helpdesk, HelpdeskError, Tenant, Ticket, apply_schema, caller, catalog,
};
use slate_orm::{SecurityContext, migrate};
use slate_slatedb::{S3Config, SlateStore};
use slatedb::object_store::local::LocalFileSystem;
use std::sync::Arc;
use uuid::Uuid;

const ACME: Uuid = Uuid::from_u128(1);
const GLOBEX: Uuid = Uuid::from_u128(2);
const DANA: Uuid = Uuid::from_u128(100);
const RAJ: Uuid = Uuid::from_u128(200);

/// How many tickets Acme raises. `--smoke` cuts it to the smallest number
/// that still exercises every path: paging needs more than one page, and the
/// roll-up needs more than one group.
const TICKETS: u128 = 40;
const SMOKE_TICKETS: u128 = 7;

/// Every failed expectation, rather than the first.
///
/// A runner that stops at the first mismatch reports one defect per run, and
/// a run over an object store is the expensive kind. This collects and exits
/// non-zero at the end, which is the same shape `scripts/check.sh` uses and
/// for the same reason.
#[derive(Default)]
struct Ledger {
    problems: Vec<String>,
    checks: usize,
}

impl Ledger {
    fn ok<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, got: T, want: T) {
        self.checks += 1;
        if got == want {
            println!("ok    {what}");
        } else {
            let said = format!("{what}: got {got:?}, wanted {want:?}");
            println!("FAIL  {said}");
            self.problems.push(said);
        }
    }

    fn refused(&mut self, what: &str, got: Result<impl std::fmt::Debug, HelpdeskError>) {
        self.checks += 1;
        match got {
            Err(said) => println!("ok    {what} ({said})"),
            Ok(allowed) => {
                let said = format!("{what}: allowed, and returned {allowed:?}");
                println!("FAIL  {said}");
                self.problems.push(said);
            }
        }
    }
}

fn ticket(tenant: Uuid, id: u128, reference: &str, subject: &str, body: &str) -> Ticket {
    Ticket {
        tenant_id: tenant,
        id: Uuid::from_u128(id),
        reference: reference.to_owned(),
        subject: subject.to_owned(),
        body: body.to_owned(),
        status: "open".to_owned(),
        // Spread across the three priorities so `most_urgent` has something
        // to sort and the queue is not one flat block.
        priority: i64::try_from(id % 3).expect("small") + 1,
        assignee_id: None,
        hours_logged: 0,
        opened_at: 0,
        updated_at: 0,
        closed_at: None,
    }
}

/// Open a store over whatever object store the environment names.
///
/// `SLATE_S3_BUCKET` and friends come from
/// `cargo run -p slate-slatedb --example s3_server`, which prints them in a
/// shape a shell can `eval`. Absent them, a directory.
async fn open(prefix: &str, dir: &str) -> SlateStore {
    match S3Config::from_env() {
        Some(config) => {
            println!("      store: s3 bucket {}", config.bucket());
            SlateStore::open_s3(prefix, config)
                .await
                .expect("opening the bucket")
        }
        None => {
            std::fs::create_dir_all(dir).expect("making the directory");
            println!("      store: filesystem at {dir}");
            let object_store =
                Arc::new(LocalFileSystem::new_with_prefix(dir).expect("the directory"));
            SlateStore::open(prefix, object_store)
                .await
                .expect("opening the store")
        }
    }
}

#[tokio::main]
async fn main() {
    let mut smoke = false;
    let mut dir =
        std::env::var("HELPDESK_DIR").unwrap_or_else(|_| "/tmp/slate-helpdesk".to_owned());
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--smoke" => smoke = true,
            "--dir" => dir = args.next().expect("--dir needs a path"),
            other => {
                eprintln!("usage: over_slatedb [--smoke] [--dir PATH]; `{other}` is not an option");
                std::process::exit(2);
            }
        }
    }
    let raised = if smoke { SMOKE_TICKETS } else { TICKETS };
    let prefix = "helpdesk";
    let mut said = Ledger::default();

    println!("=== first process: migrate, seed, serve");
    let store = open(prefix, &dir).await;

    // The migration, shown before it runs. An application at boot wants to
    // know what it is about to do to a bucket somebody else's data is in.
    let plan = migrate::plan(&store, &catalog().expect("the catalog"))
        .await
        .expect("planning");
    println!("      migration: {} step(s)", plan.steps.len());
    assert!(
        !plan.is_blocked(),
        "the migration is blocked: {}",
        plan.why_blocked()
    );
    let report = apply_schema(&store).await.expect("migrating");
    println!("      applied: {report:?}");

    // A second apply must be a no-op, which is what makes it safe to run on
    // every boot rather than behind a flag somebody forgets to set.
    let again = migrate::plan(&store, &catalog().expect("the catalog"))
        .await
        .expect("re-planning");
    said.ok(
        "a second migration has nothing to do",
        again.is_empty(),
        true,
    );

    let root = SecurityContext::superuser();
    let dana = caller(DANA, ACME, "agent");
    let boss = caller(DANA, ACME, "supervisor");
    let raj = caller(RAJ, GLOBEX, "agent");

    let desk = Helpdesk::open(store).expect("the catalog is valid");
    for (tenant, name, agent, email) in [
        (ACME, "Acme", DANA, "dana@acme.example"),
        (GLOBEX, "Globex", RAJ, "raj@globex.example"),
    ] {
        desk.found(
            &root,
            &Tenant {
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

    // The queue. One ticket in twenty mentions a printer, so the search has
    // an answer that is neither empty nor everything.
    for n in 0..raised {
        let printer = n % 20 == 0;
        let subject = if printer {
            "Printer jammed"
        } else {
            "Cannot log in"
        };
        let body = if printer {
            "The printer on the third floor is jammed again. Please advise."
        } else {
            "Password reset did not arrive. Please advise."
        };
        desk.open_ticket(
            &dana,
            ticket(ACME, 1000 + n, &format!("ACME-{n}"), subject, body),
        )
        .await
        .expect("raising a ticket");
    }
    // And one for the other tenant, so every read below has something it
    // must *not* return.
    desk.open_ticket(
        &raj,
        ticket(
            GLOBEX,
            9000,
            "GLOBEX-1",
            "Printer jammed",
            "Ours, not theirs.",
        ),
    )
    .await
    .expect("raising a ticket");

    said.ok(
        "the whole queue is paged without loss",
        paged(&desk, &dana).await,
        usize::try_from(raised).expect("small"),
    );
    said.ok(
        "the worst ticket is priority 1",
        desk.most_urgent(&dana, 1).await.expect("urgent")[0].priority,
        1,
    );
    let printers = desk.search(&dana, "printer").await.expect("searching");
    said.ok(
        "the text index finds the printers, and only Acme's",
        printers.len(),
        usize::try_from(raised.div_ceil(20)).expect("small"),
    );
    said.ok(
        "a term in no ticket finds nothing",
        desk.search(&dana, "aardvark")
            .await
            .expect("searching")
            .len(),
        0,
    );
    said.ok(
        "the other tenant sees one printer: their own",
        desk.search(&raj, "printer").await.expect("searching").len(),
        1,
    );

    // Work the queue, so the roll-up has two groups and a non-zero sum.
    desk.assign(&dana, "ACME-0", DANA).await.expect("taking");
    desk.assign(&dana, "ACME-1", DANA).await.expect("taking");
    desk.log_time(&dana, "ACME-0", 150).await.expect("logging");
    desk.log_time(&dana, "ACME-1", 25).await.expect("logging");
    desk.comment(&dana, "ACME-0", DANA, "Replaced the toner.")
        .await
        .expect("commenting");
    desk.close(&dana, "ACME-1").await.expect("closing");

    said.refused(
        "an agent cannot hand a ticket to a colleague",
        desk.assign(&dana, "ACME-0", RAJ).await,
    );
    desk.assign(&boss, "ACME-0", RAJ)
        .await
        .expect("a supervisor moves work");
    said.refused(
        "and then the agent cannot touch it",
        desk.log_time(&dana, "ACME-0", 10).await,
    );
    said.refused(
        "the other tenant cannot read this one",
        desk.by_reference(&raj, "ACME-2").await,
    );

    // Close the store. `Helpdesk` owns it, so this is the drop plus an
    // explicit flush of the WAL — without which the reopen below would be
    // testing recovery rather than durability, and would still pass.
    desk.store()
        .backend()
        .close()
        .await
        .expect("closing the store");
    drop(desk);

    println!("=== second process: the same bucket, a fresh store");
    let store = open(prefix, &dir).await;
    // Nothing to migrate: the schema is in the bucket, not in the process.
    let after = migrate::plan(&store, &catalog().expect("the catalog"))
        .await
        .expect("planning");
    said.ok("the schema survived the restart", after.is_empty(), true);

    let desk = Helpdesk::open(store).expect("the catalog is valid");
    said.ok(
        "the tickets survived",
        paged(&desk, &dana).await,
        usize::try_from(raised).expect("small") - 1, // one was closed
    );
    // The index is the half worth naming: it is maintained on write and read
    // from the bucket, so a search that answers here is an index that was
    // persisted rather than rebuilt.
    said.ok(
        "the text index survived",
        desk.search(&dana, "printer")
            .await
            .expect("searching")
            .len(),
        usize::try_from(raised.div_ceil(20)).expect("small"),
    );
    said.ok(
        "the comment survived",
        desk.thread(&dana, "ACME-0")
            .await
            .expect("the thread")
            .len(),
        1,
    );
    let mut rolled = desk.workload(&dana).await.expect("the roll-up");
    rolled.sort_by_key(|w| w.assignee_id);
    said.ok("two groups: unassigned, and Raj", rolled.len(), 2);
    said.ok(
        "the logged time survived, to the cent",
        rolled.iter().map(|w| w.hundredths).sum::<i64>(),
        150,
    );
    said.refused(
        "and the closed ticket is still closed",
        desk.by_reference(&dana, "ACME-1").await,
    );

    desk.store()
        .backend()
        .close()
        .await
        .expect("closing the store");

    println!();
    if said.problems.is_empty() {
        println!("{} checks, all of them", said.checks);
    } else {
        println!("{} checks, {} FAILED:", said.checks, said.problems.len());
        for problem in &said.problems {
            println!("  {problem}");
        }
        std::process::exit(1);
    }
}

/// Page the whole live queue and return how many distinct tickets it held.
///
/// Three at a time, which is small enough that a run of forty makes fourteen
/// requests — the point is to exercise the cursor, not to be quick.
async fn paged(desk: &Helpdesk<SlateStore>, who: &SecurityContext) -> usize {
    let mut seen = std::collections::BTreeSet::new();
    let mut cursor = None;
    for _ in 0..1000 {
        let page = desk.inbox(who, cursor, 3).await.expect("a page");
        for one in &page.tickets {
            seen.insert(one.reference.clone());
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => return seen.len(),
        }
    }
    panic!("the cursor never reached the end of the queue");
}
