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
use slate_kernel::{KvReadStore, KvStore, RoutingPolicy, Statistics};
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
    let routing = routing(&document.routing)?;
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
    if !leader && prepared.is_shared() {
        // Refusing rather than serving reads, which is what `slate-server`
        // does for a node that is fenced *while running*. The two cases differ:
        // a fenced node already holds an open store, and this one would have to
        // open the database to build a `Head` at all — `Head::new` takes a
        // writer, there is no read-only head — and opening it would fence the
        // healthy leader. Refusing to start is the only option that does not
        // make a second node worse than no second node.
        //
        // This is the largest thing this binary wants from `slate-server` and
        // cannot have: a head node that serves reads from replicas with no
        // writer store of its own.
        let holder = leadership
            .lease()
            .observe()
            .await
            .ok()
            .flatten()
            .map_or_else(|| "another node".to_owned(), |term| term.holder);
        return Err(Fault::new(format!(
            "the writer lease at `{}` is held by `{holder}`.\n\
             This node will not start: opening the database would fence that writer, and `slate-server` has no head node that serves reads without opening one. Stop the other node, or wait for its lease to lapse (terms are {term:?}).",
            document.lease.path
        )));
    }
    tokio::spawn(maintain(Arc::clone(&leadership), Cadence::for_term(term)));

    let (writer, replicas) = storage::open(&prepared, &document.replicas).await?;

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
        if leader { "leader" } else { "follower" },
    );

    let common = Common {
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
        Writer::Memory(store) => start(store, common, None).await,
        Writer::Slate(store) => {
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
            .with_limits(limits),
        writer,
        replicas,
        Arc::clone(&leadership),
        authenticator,
    );

    // The handshake. Printed on standard output, after the listener is bound
    // and before anything is served, and spelled exactly as
    // `clients/python/testserver` spells it so a harness written against that
    // one needs no change.
    println!("LISTENING {bound}");
    use std::io::Write;
    std::io::stdout()
        .flush()
        .map_err(|why| Fault::new(format!("cannot write the banner: {why}")))?;

    serve::run(head, listener, leadership, grace).await?;

    if let Some(store) = closing {
        store
            .close()
            .await
            .map_err(|why| Fault::new(format!("the database did not close cleanly: {why}")))?;
    }
    Ok(())
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
