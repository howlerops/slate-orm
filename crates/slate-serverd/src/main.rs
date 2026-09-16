//! `slate-serverd`: the head node as a process.
//!
//! # Why this exists
//!
//! `slate-server` is a library. [`Head::new`] takes a
//! [`Catalog`](slate_schema::Catalog), a
//! [`SecurityCatalog`](slate_kernel::SecurityCatalog), a writer store, a list
//! of replicas, a [`Leadership`](slate_server::Leadership) and an
//! [`Authenticator`](slate_server::Authenticator) — every one a Rust value
//! that only Rust can construct. So the first consumer of the wire from
//! outside Rust had to write 936 lines of Rust before it could write a line of
//! Python, and had to restate the fixture schema because the head node's test
//! module is not exported. Finding 1 of
//! `clients/python/PROTOCOL-FINDINGS.md` records that, and its last sentence
//! is the specification for this crate: *every other-language client will
//! write the same binary, differently, and each will get the `Authenticator`
//! choice slightly wrong in its own way.*
//!
//! Four things follow from taking that seriously rather than shipping a
//! `main` that hard-codes a fixture.
//!
//! **The schema has to be declarable without Rust**, including the parts that
//! are `Expr` and `Scalar` values today: a partial index's predicate, an
//! expression index's key, a `CHECK`. [`config`] argues the format and
//! [`lang`] argues the little language those three are written in, along with
//! the honest list of what it cannot say.
//!
//! **Authentication has no default and no accidentally-insecure path.**
//! [`auth`] is the module to read; the rule is that a mode whose safety rests
//! on something outside this process must name that thing, and only when the
//! bind address makes it matter.
//!
//! **Nothing here can produce a superuser from the wire.** The word appears
//! twice, both in [`seed`], both at startup, both behind a command-line flag
//! that a configuration file cannot supply.
//!
//! **The leadership path is the real one on every backend**, including
//! `memory`, which runs the genuine object-store lease against an object store
//! nobody else can see rather than a fake lease that always grants. See
//! [`storage`].
//!
//! [`Head::new`]: slate_server::Head::new

#![forbid(unsafe_code)]

mod auth;
mod cli;
mod config;
mod error;
mod filelease;
mod lang;
mod schema;
mod security;
mod seed;
mod serve;
mod storage;
mod value;

use clap::Parser;
use core::time::Duration;
use error::{Fault, Started};
use slate_kernel::{ExecutionLimits, KvReadStore, KvStore, RoutingPolicy, Statistics};
use slate_schema::Catalog;
use slate_server::{Cadence, Head, HeadConfig, Leadership, Limits, maintain};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use storage::Writer;

/// Exit code for a configuration this server will not start on.
///
/// Distinct from 1 so a supervisor can tell "would not start" from "started
/// and then failed", which are different problems with different fixes.
const REFUSED: u8 = 2;

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = cli::Cli::parse();
    match run(arguments).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(fault) => {
            eprintln!("slate-serverd: {fault}");
            ExitCode::from(REFUSED)
        }
    }
}

async fn run(arguments: cli::Cli) -> Started<()> {
    let document = config::Document::read(&arguments.config)?;

    // The address is settled before anything else, because the authentication
    // rules depend on it: the same `[auth]` section is valid on loopback and a
    // refusal on a public interface.
    let requested = arguments
        .listen
        .as_ref()
        .unwrap_or(&document.listen.address);
    let address: SocketAddr = requested.parse().map_err(|why| {
        Fault::new(format!(
            "`{requested}` is not a socket address ({why}); write it as `127.0.0.1:50051` or `[::1]:50051`"
        ))
    })?;

    let mut warnings = Vec::new();
    let catalog = schema::catalog(&document.tables)?;

    if arguments.print_schema {
        println!("{}", describe(&catalog));
        return Ok(());
    }

    let security = security::catalog(&document.security, &catalog, &mut warnings)?;
    let chosen = auth::choose(document.auth.as_ref(), &address, &mut warnings)?;
    let limits = limits(&document.limits)?;
    let execution = execution_limits(&document.limits)?;
    let concurrency = document.limits.max_concurrent_requests;
    if concurrency == Some(0) {
        return Err(Fault::new(
            "`[limits] max_concurrent_requests = 0` would serve nobody; \
             leave it unset for no limit",
        ));
    }
    let request_timeout = config::optional_duration(
        document.limits.request_timeout.as_ref(),
        "limits.request_timeout",
    )?;
    let routing = routing(&document.routing)?;
    // Checked here rather than where the replicas are opened, because a
    // warning has to reach `warnings` and the replicas are opened after those
    // have been printed. See `storage::check_catch_up`.
    storage::check_catch_up(&document.replicas, routing.catch_up, &mut warnings);
    let grace = config::optional_duration(document.shutdown.grace.as_ref(), "shutdown.grace")?
        .unwrap_or(Duration::from_secs(10));
    let term = storage::term(&document.lease)?;
    let fixture = arguments
        .seed
        .as_deref()
        .map(seed::Fixture::read)
        .transpose()?;

    if arguments.check {
        for warning in &warnings {
            eprintln!("slate-serverd: warning: {warning}");
        }
        println!(
            "{} is valid: {} table{}, auth {}, storage {}, listening on {address}",
            arguments.config.display(),
            catalog.tables().len(),
            if catalog.tables().len() == 1 { "" } else { "s" },
            chosen.description,
            document.storage.backend,
        );
        return Ok(());
    }

    // The object store, but not the database. Opening a SlateDB writer fences
    // whatever writer was there, so a node has to know it holds the lease
    // before it does that; see `storage`.
    let prepared = storage::prepare(&document.storage)?;

    // Which lease depends on what the storage can do; see `storage::lease`.
    let leadership = Leadership::new(prepared.lease(&document.lease, term));

    // Campaign before opening the database, and before the maintenance task
    // starts, so that the first attempt is this one and its result is known
    // here.
    let leader = leadership.campaign().await;

    // A node that lost the campaign starts as a reader. It must never open the
    // database — opening a SlateDB writer fences the node that won — so it
    // opens only its replicas and builds a `Head` with no writer store at all.
    //
    // This used to be a refusal to start, and the comment here used to say why:
    // `Head::new` took a writer and there was no read-only head, so a second
    // node could only choose between fencing a healthy leader and not running.
    // `slate-server` grew `Head::read_only`, and this is the case it grew it
    // for. The refusal was the workaround, not the design: `topology.md` has
    // always said reads scale and writes do not, and until now no process could
    // be the reading half.
    //
    // `memory` is excluded because nothing else can be contending for it; if
    // its private lease somehow refused, the old behaviour of starting with a
    // writer and refusing writes locally is still the right one.
    let read_only = !leader && prepared.is_shared();

    if read_only && fixture.is_some() {
        return Err(Fault::new(
            "`--seed` inserts rows, and this node did not win the writer lease, so it has no writer to insert them with. Seed from the node that holds the lease",
        ));
    }

    let (writer, replicas) = if read_only {
        // `follow`, not `maintain`: this node has nothing to write to, so
        // winning the lease would take the writer role away from a node that
        // could have used it and serve nothing while looking healthy.
        // `follow` reads the lease so the `slate-leader` redirect keeps naming
        // a node that exists, and never acquires.
        tokio::spawn(slate_server::follow(
            Arc::clone(&leadership),
            Cadence::for_term(term),
        ));
        let replicas =
            storage::open_read_only(&prepared, &document.replicas, routing.catch_up).await?;
        (None, replicas)
    } else {
        tokio::spawn(maintain(Arc::clone(&leadership), Cadence::for_term(term)));
        let (writer, replicas) =
            storage::open(&prepared, &document.replicas, routing.catch_up).await?;
        (Some(writer), replicas)
    };

    // The schema, before the socket. An index the keyspace does not hold makes
    // the queries that would use it return *no rows* — see
    // `slate_kernel::migrate` — so this runs before anything can ask.
    //
    // Before binding rather than after: a node that binds and then migrates is
    // a node that accepts a connection and answers it wrongly, which is the
    // failure being prevented rather than a smaller version of it.
    match &writer {
        Some(Writer::Memory(store)) => {
            reconcile(store.as_ref(), &catalog, document.schema.migrate_on_start).await?;
        }
        Some(Writer::Slate(store)) => {
            reconcile(store.as_ref(), &catalog, document.schema.migrate_on_start).await?;
        }
        // A read-only node has no writer and cannot migrate anything. It still
        // has to know, because it serves reads from replicas of the same
        // keyspace and would return the same empty answers — so it checks what
        // it can reach and says so. A warning and not a refusal: a follower
        // that starts beside a leader may look before the leader has finished,
        // and a restart loop on a node that is about to be correct is a worse
        // outage than the window it closes. The window itself is recorded in
        // the ledger rather than hidden here.
        None => {
            // One replica is enough: they are replicas of one keyspace, so a
            // second would report the same thing in different words.
            if let Some(replica) = replicas.first() {
                let snapshot = replica
                    .snapshot()
                    .await
                    .map_err(|why| Fault::new(format!("cannot read a replica: {why}")))?;
                if let Err(why) =
                    slate_kernel::migrate::verify_of(snapshot.as_ref(), &catalog).await
                {
                    warnings.push(format!(
                        "this node holds no writer and the keyspace it reads is not migrated \
                         ({why}). Reads through an unbuilt index return no rows rather than an \
                         error. It becomes correct on its own once the node holding the writer \
                         lease migrates and this replica catches up"
                    ));
                }
            }
        }
    }

    // Bound before the banner, so a client that connects the instant it reads
    // the banner finds the socket already accepting rather than racing it.
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|why| Fault::new(format!("cannot bind {address}: {why}")))?;
    let bound = listener
        .local_addr()
        .map_err(|why| Fault::new(format!("cannot read the bound address: {why}")))?;

    for warning in &warnings {
        eprintln!("slate-serverd: warning: {warning}");
    }
    eprintln!(
        "slate-serverd: {} table{}, storage {}, {} replica{}, auth {}, {}",
        catalog.tables().len(),
        if catalog.tables().len() == 1 { "" } else { "s" },
        prepared.description,
        replicas.len(),
        if replicas.len() == 1 { "" } else { "s" },
        chosen.description,
        if leader {
            "leader"
        } else if read_only {
            // Spelled out rather than left at "follower": an operator reading
            // this line needs to know the node will refuse writes *and* that
            // it did not open the database, which is the fact that makes it
            // safe to have started at all.
            "follower (read-only: no writer store, writes are redirected)"
        } else {
            "follower"
        },
    );

    let common = Common {
        execution,
        concurrency,
        request_timeout,
        catalog,
        security,
        limits,
        routing,
        grace,
        fixture,
        analyze: document.planner.analyze_on_start,
        replicas,
        authenticator: chosen.authenticator,
        leadership,
        listener,
        bound,
    };

    // The two backends differ only in the concrete writer type, which `Head`
    // is generic over. Two call sites of one generic function rather than an
    // `Arc<dyn KvStore>`, because the writer also joins the replica pool and a
    // trait object there would lose `SlateStore::close`.
    match writer {
        // A read-only node still has to name a writer type, because `Head` is
        // generic over one whether or not it holds it. `SlateStore` is the
        // type this node *would* have had: a read-only node only happens on a
        // shared backend, and every shared backend is a `SlateStore`.
        None => start_read_only::<slate_slatedb::SlateStore>(common).await,
        Some(Writer::Memory(store)) => start(store, common, None).await,
        Some(Writer::Slate(store)) => {
            let closing = Arc::clone(&store);
            start(store, common, Some(closing)).await
        }
    }
}

/// Everything that does not depend on which backend is in use.
struct Common {
    catalog: Catalog,
    security: slate_kernel::SecurityCatalog,
    limits: Limits,
    /// The kernel's per-request ceilings. See [`ExecutionLimits`].
    execution: ExecutionLimits,
    /// Requests in flight per connection, unset for unbounded.
    concurrency: Option<usize>,
    /// How long one request may run, unset for no timeout.
    request_timeout: Option<Duration>,
    routing: RoutingPolicy,
    grace: Duration,
    fixture: Option<seed::Fixture>,
    analyze: bool,
    replicas: Vec<Arc<dyn KvReadStore>>,
    authenticator: Arc<dyn slate_server::Authenticator>,
    leadership: Arc<Leadership>,
    listener: tokio::net::TcpListener,
    bound: SocketAddr,
}

async fn start<S: KvStore + KvReadStore>(
    writer: Arc<S>,
    common: Common,
    closing: Option<Arc<slate_slatedb::SlateStore>>,
) -> Started<()> {
    let Common {
        execution,
        concurrency,
        request_timeout,
        catalog,
        security,
        limits,
        routing,
        grace,
        fixture,
        analyze,
        replicas,
        authenticator,
        leadership,
        listener,
        bound,
    } = common;

    if let Some(fixture) = &fixture {
        let store = seed::store_for(&writer, &catalog, &security);
        seed::load(&store, &catalog, fixture).await?;
        eprintln!("slate-serverd: seeded {} rows", fixture.row_count());
    }

    let statistics = if analyze {
        let store = seed::store_for(&writer, &catalog, &security);
        let measured = seed::analyze(&store, &catalog).await?;
        eprintln!("slate-serverd: analyzed {} tables", catalog.tables().len());
        measured
    } else {
        Statistics::new()
    };

    let head = Head::new(
        HeadConfig::new(catalog, security)
            .with_statistics(statistics)
            .with_routing(routing)
            .with_limits(limits)
            .with_execution_limits(execution),
        writer,
        replicas,
        Arc::clone(&leadership),
        authenticator,
    );

    announce_and_serve(
        head,
        listener,
        bound,
        leadership,
        grace,
        concurrency,
        request_timeout,
    )
    .await?;

    if let Some(store) = closing {
        store
            .close()
            .await
            .map_err(|why| Fault::new(format!("the database did not close cleanly: {why}")))?;
    }
    Ok(())
}

/// Serve with no writer store: this node lost the campaign.
///
/// Deliberately not `start` with an `Option<Arc<S>>`. Half of `start` is the
/// two things a node does with a writer before it serves — seed rows and
/// `analyze` — and both are writes or reads *of the writer*. Threading an
/// option through them would put four `if let Some` in a function whose whole
/// job is the sequence, and the interesting property of this path is exactly
/// what it does *not* do.
///
/// `analyze` is the one real loss. It reads the tables to measure them, and
/// this node has no store that can be read that way — the replicas are behind
/// `Arc<dyn KvReadStore>`, and `RecordStore` needs a `KvStore`. Measuring
/// through a `RecordSnapshot` would work and is not done: statistics differing
/// between a leader and its followers would make a plan depend on which node
/// answered, which is the same "same query, different answer" this project
/// treats as the worst kind of bug. A follower plans on the defaults and says
/// so.
async fn start_read_only<S: KvStore + KvReadStore>(common: Common) -> Started<()> {
    let Common {
        execution,
        concurrency,
        request_timeout,
        catalog,
        security,
        limits,
        routing,
        grace,
        fixture,
        analyze,
        replicas,
        authenticator,
        leadership,
        listener,
        bound,
    } = common;

    // Refused earlier, where the message can name `--seed`. Belt and braces:
    // a fixture reaching here would be silently ignored.
    debug_assert!(fixture.is_none(), "a read-only node cannot seed");
    if analyze {
        eprintln!(
            "slate-serverd: warning: `analyze_on_start` is skipped on a node with no writer store; the planner runs on default statistics until this node is restarted as the writer"
        );
    }

    let head = Head::<S>::read_only(
        HeadConfig::new(catalog, security)
            .with_routing(routing)
            .with_limits(limits)
            .with_execution_limits(execution),
        replicas,
        Arc::clone(&leadership),
        authenticator,
    );

    announce_and_serve(
        head,
        listener,
        bound,
        leadership,
        grace,
        concurrency,
        request_timeout,
    )
    .await
}

/// The handshake, then serving.
///
/// Printed on standard output, after the listener is bound and before anything
/// is served, and spelled exactly as `clients/python/testserver` spells it so a
/// harness written against that one needs no change. Shared by both start
/// paths so that a read-only node's handshake cannot drift from a writer's —
/// a test harness waits on this line and does not know which kind it started.
async fn announce_and_serve<S: KvStore + KvReadStore>(
    head: Head<S>,
    listener: tokio::net::TcpListener,
    bound: SocketAddr,
    leadership: Arc<Leadership>,
    grace: Duration,
    concurrency: Option<usize>,
    request_timeout: Option<Duration>,
) -> Started<()> {
    println!("LISTENING {bound}");
    use std::io::Write;
    std::io::stdout()
        .flush()
        .map_err(|why| Fault::new(format!("cannot write the banner: {why}")))?;

    serve::run(
        head,
        listener,
        leadership,
        grace,
        concurrency,
        request_timeout,
    )
    .await
}

fn limits(settings: &config::LimitSettings) -> Started<Limits> {
    let mut limits = Limits::default();
    if let Some(max) = settings.max_transactions {
        if max == 0 {
            return Err(Fault::new(
                "`[limits] max_transactions = 0` would refuse every transaction; leave it unset for the default",
            ));
        }
        limits.max_transactions = max;
    }
    if let Some(idle) =
        config::optional_duration(settings.idle_timeout.as_ref(), "limits.idle_timeout")?
    {
        limits.idle_timeout = idle;
    }
    if let Some(rows) = settings.rows_per_message {
        if rows == 0 {
            return Err(Fault::new(
                "`[limits] rows_per_message = 0` would send no rows; leave it unset for the default of 256",
            ));
        }
        limits.rows_per_message = rows;
    }
    if let Some(operations) = settings.max_batch_operations {
        if operations == 0 {
            return Err(Fault::new(
                "`[limits] max_batch_operations = 0` would refuse every batch; leave it unset for the default of 1000",
            ));
        }
        limits.max_batch_operations = Some(operations);
    }
    Ok(limits)
}

/// The kernel's per-request ceilings, from the same `[limits]` table.
///
/// Zero is refused for each rather than treated as "no limit": a zero here
/// would refuse every grouped query, and a config that silently disables the
/// feature it appears to configure is worse than one that will not start.
/// Unbounded is spelled by leaving the key out.
fn execution_limits(settings: &config::LimitSettings) -> Started<ExecutionLimits> {
    let mut limits = ExecutionLimits::default();
    for (value, name, field) in [
        (settings.max_groups, "max_groups", 0usize),
        (settings.max_distinct, "max_distinct", 1),
        (settings.max_sort_rows, "max_sort_rows", 2),
    ] {
        let Some(value) = value else { continue };
        if value == 0 {
            return Err(Fault::new(format!(
                "`[limits] {name} = 0` would refuse every such query; \
                 leave it unset for the default"
            )));
        }
        match field {
            0 => limits.max_groups = value,
            1 => limits.max_distinct = value,
            _ => limits.max_sort_rows = value,
        }
    }
    Ok(limits)
}

fn routing(settings: &config::Routing) -> Started<RoutingPolicy> {
    let mut routing = RoutingPolicy::default();
    if let Some(catch_up) =
        config::optional_duration(settings.catch_up.as_ref(), "routing.catch_up")?
    {
        routing.catch_up = catch_up;
    }
    if let Some(affinity) = settings.tenant_affinity {
        routing.tenant_affinity = affinity;
    }
    Ok(routing)
}

/// Render the catalog as JSON.
///
/// This is not the catalog fingerprint finding 2 asks for, and does not
/// pretend to be: it is produced at startup from the same file the server
/// reads, so a client that compares against it is comparing against the file
/// rather than against the running node. What it does remove is the *hand*
/// copying — a client generator can read this instead of a person transcribing
/// ordinals into another language, which is the mechanical half of the drift.
/// Bring the keyspace into line with the catalog, or refuse to serve.
///
/// `migrate` when configured to, `verify` when not. The two are not "do it" and
/// "skip it": the second still refuses to start on anything outstanding, so
/// turning the migration off buys the operator control over *when* a backfill
/// runs, never permission to serve without one.
async fn reconcile<S: slate_kernel::store::KvStore + ?Sized>(
    store: &S,
    catalog: &Catalog,
    migrate: bool,
) -> Started<()> {
    use slate_kernel::migrate::{self, Step};

    if !migrate {
        return migrate::verify(store, catalog).await.map_err(|why| {
            Fault::new(format!(
                "{why}. `[schema] migrate_on_start` is false, so this node will not build it \
                 itself; run a node with it enabled, or set it to true here"
            ))
        });
    }

    let plan = migrate::plan(store, catalog)
        .await
        .map_err(|why| Fault::new(format!("cannot read the schema state: {why}")))?;
    if plan.is_empty() {
        return Ok(());
    }
    if plan.is_blocked() {
        return Err(Fault::new(format!(
            "this binary's schema cannot be applied to the data already stored: {}",
            plan.why_blocked()
        )));
    }

    // Announced before it runs, not after. A backfill of a large table is the
    // one startup step that can take minutes, and an operator watching a node
    // that has printed nothing cannot tell working from hung.
    let building: Vec<&str> = plan
        .steps
        .iter()
        .filter_map(|step| match step {
            Step::BuildIndex { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    if !building.is_empty() {
        eprintln!(
            "slate-serverd: building {} index{} before serving: {}",
            building.len(),
            if building.len() == 1 { "" } else { "es" },
            building.join(", "),
        );
    }

    let started = std::time::Instant::now();
    let report = migrate::apply(store, catalog, &plan)
        .await
        .map_err(|why| Fault::new(format!("migration failed: {why}")))?;
    let entries: usize = report.entries_written.iter().sum();
    if !building.is_empty() {
        eprintln!(
            "slate-serverd: built {entries} index entr{} in {:.1?}",
            if entries == 1 { "y" } else { "ies" },
            started.elapsed(),
        );
    }
    Ok(())
}

fn describe(catalog: &Catalog) -> String {
    let tables: Vec<serde_json::Value> = catalog
        .tables()
        .iter()
        .map(|table| {
            let columns: Vec<serde_json::Value> = table
                .columns()
                .iter()
                .enumerate()
                .map(|(ordinal, column)| {
                    serde_json::json!({
                        "ordinal": ordinal,
                        "name": column.name(),
                        "type": column.value_type().name(),
                        "nullable": column.is_nullable(),
                        "added_in": column.added_in(),
                        "dropped_in": column.dropped_in(),
                        "previous_names": column.previous_names(),
                    })
                })
                .collect();
            let indexes: Vec<serde_json::Value> = table
                .indexes()
                .iter()
                .map(|index| {
                    serde_json::json!({
                        "name": index.name(),
                        "id": index.id().0,
                        "unique": index.is_unique(),
                        "partial": index.predicate().is_some(),
                        "expression": index.expression().map(|e| e.produces().name()),
                        "columns": index
                            .columns()
                            .iter()
                            .map(|column| serde_json::json!({
                                "ordinal": column.ordinal.0,
                                "descending": column.direction == slate_tuple::Direction::Desc,
                            }))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect();
            serde_json::json!({
                "name": table.name(),
                "id": table.id().0,
                "schema_version": table.schema_version(),
                "primary_key": table.primary_key().iter().map(|o| o.0).collect::<Vec<_>>(),
                "tenant_column": table.tenant_column().map(|o| o.0),
                "columns": columns,
                "indexes": indexes,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "tables": tables }))
        .unwrap_or_else(|why| format!("{{\"error\": \"{why}\"}}"))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn zero_limits_are_refused_rather_than_silently_useless() {
        let settings: config::LimitSettings = toml::from_str("max_transactions = 0").unwrap();
        assert!(
            limits(&settings)
                .unwrap_err()
                .to_string()
                .contains("refuse every transaction")
        );

        let settings: config::LimitSettings = toml::from_str("rows_per_message = 0").unwrap();
        assert!(
            limits(&settings)
                .unwrap_err()
                .to_string()
                .contains("send no rows")
        );
    }

    #[test]
    fn routing_defaults_are_kept_unless_overridden() {
        let empty: config::Routing = toml::from_str("").unwrap();
        assert!(routing(&empty).unwrap().tenant_affinity);
        let off: config::Routing = toml::from_str("tenant_affinity = false").unwrap();
        assert!(!routing(&off).unwrap().tenant_affinity);
    }

    #[test]
    fn the_schema_description_names_ordinals_and_types() {
        let document: config::Document = toml::from_str(
            r#"
[listen]
address = "127.0.0.1:0"
[storage]
backend = "memory"
[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "kind", type = "str" },
]
primary_key = ["id"]
[[tables.indexes]]
name = "by_kind"
id = 1
columns = ["kind"]
where = "id > 0"
"#,
        )
        .unwrap();
        let catalog = schema::catalog(&document.tables).unwrap();
        let json = describe(&catalog);
        assert!(json.contains("\"ordinal\": 1"), "{json}");
        assert!(json.contains("\"kind\""), "{json}");
        assert!(json.contains("\"partial\": true"), "{json}");
    }
}
