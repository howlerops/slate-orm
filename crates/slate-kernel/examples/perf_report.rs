//! What each query shape actually costs in I/O.
//!
//! Wall-clock numbers move with the machine; the number of point reads a plan
//! issues does not. This prints both, so a change can be argued for on the
//! count and confirmed on the clock.
//!
//! ```sh
//! cargo run --release -p slate-kernel --example perf_report
//! ```
//!
//! Charges are modelled at object-storage scale: a point read costs a
//! millisecond, and a scan pays that once to open plus once per block.

#![allow(clippy::expect_used, clippy::print_stdout)]

use slate_kernel::latency::{LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Chain, CmpOp, Expr, Grant, Join, JoinAlgorithm, JoinSchema, JoinStep, Projection,
    Query, RecordStore, ScanOrder, SecurityCatalog, SecurityContext, Side, Statistics,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};
use std::time::Instant;
use uuid::Uuid;

const EVENTS: TableId = TableId(1);
const ACTORS: TableId = TableId(2);
const TEAMS: TableId = TableId(3);
const TENANTS: u128 = 4;
const ROWS_PER_TENANT: u64 = 2_500;

/// Rows per tenant, overridable with `KERNELBENCH_ROWS`.
///
/// Per tenant rather than in total, because the tenant count is part of what
/// this measures — a prefix boundary — and dividing a total by four would make
/// the knob change two things at once.
///
/// `scripts/run_examples.sh --smoke` sets it small. The numbers a smoke run
/// prints are worthless and it says so; what it proves is that every query
/// shape here still executes. A run with the variable unset is the recorded
/// size, which is what `docs/performance.md` quotes.
fn rows_per_tenant() -> u64 {
    std::env::var("KERNELBENCH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(ROWS_PER_TENANT)
}
const ACTOR_COUNT: u64 = 500;
const TEAM_COUNT: u64 = 10;

fn events() -> TableDef {
    TableDef::builder("events", EVENTS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("actor", ValueType::Str)
        .column("at", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_kind", IndexId(10)).column("kind"))
        .index(IndexDef::builder("by_actor", IndexId(11)).column("actor"))
        .index(IndexDef::builder("by_at_desc", IndexId(12)).column_with("at", Direction::Desc))
        .build()
        .expect("valid schema")
}

/// The other side of the join: one row per actor named in `events`.
fn actors() -> TableDef {
    TableDef::builder("actors", ACTORS)
        .column("tenant_id", ValueType::Uuid)
        .column("name", ValueType::Str)
        .column("team", ValueType::Str)
        .primary_key(["tenant_id", "name"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn actor_column(name: &str) -> Ordinal {
    actors().ordinal_of(name).expect("column exists")
}

fn actor_row(tenant: u128, id: u64) -> Row {
    Row::new(vec![
        Value::Uuid(Uuid::from_u128(tenant)),
        Value::Str(format!("actor-{id}")),
        Value::Str(format!("team-{}", id % 10)),
    ])
}

/// The third link: the team each actor belongs to.
fn teams() -> TableDef {
    TableDef::builder("teams", TEAMS)
        .column("tenant_id", ValueType::Uuid)
        .column("name", ValueType::Str)
        .column("region", ValueType::Str)
        .primary_key(["tenant_id", "name"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn team_column(name: &str) -> Ordinal {
    teams().ordinal_of(name).expect("column exists")
}

fn team_row(tenant: u128, id: u64) -> Row {
    Row::new(vec![
        Value::Uuid(Uuid::from_u128(tenant)),
        Value::Str(format!("team-{id}")),
        Value::Str(format!("region-{}", id % 3)),
    ])
}

fn column(name: &str) -> Ordinal {
    events().ordinal_of(name).expect("column exists")
}

fn row(tenant: u128, id: u64) -> Row {
    Row::new(vec![
        Value::Uuid(Uuid::from_u128(tenant)),
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 25)),
        Value::Str(format!("actor-{}", id % 500)),
        Value::I64(id as i64),
        Value::Null,
    ])
}

#[tokio::main]
async fn main() {
    let table = events();
    let actor_table = actors();
    let team_table = teams();
    let catalog = Catalog::from_tables([table.clone(), actor_table.clone(), team_table.clone()])
        .expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("bench", EVENTS, Action::ALL))
        .grant(Grant::new("bench", ACTORS, Action::ALL))
        .grant(Grant::new("bench", TEAMS, Action::ALL));
    let root = SecurityContext::superuser();

    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), security.clone());
    for tenant in 0..TENANTS {
        let txn = loader.begin().await.expect("begin");
        for id in 0..rows_per_tenant() {
            txn.insert(&root, &table, &row(tenant, id))
                .await
                .expect("insert");
        }
        txn.commit().await.expect("commit");
    }
    for tenant in 0..TENANTS {
        let txn = loader.begin().await.expect("begin");
        for id in 0..ACTOR_COUNT {
            txn.insert(&root, &actor_table, &actor_row(tenant, id))
                .await
                .expect("insert");
        }
        for id in 0..TEAM_COUNT {
            txn.insert(&root, &team_table, &team_row(tenant, id))
                .await
                .expect("insert");
        }
        txn.commit().await.expect("commit");
    }

    // Statistics first: without them the planner has to guess how many rows a
    // predicate selects, and guessing structurally is what made it pick a plan
    // 30x slower than the alternative.
    let (analyzed, actors_analyzed, teams_analyzed) = {
        let txn = loader.begin().await.expect("begin");
        (
            txn.analyze(&root, &table).await.expect("analyze"),
            txn.analyze(&root, &actor_table).await.expect("analyze"),
            txn.analyze(&root, &team_table).await.expect("analyze"),
        )
    };
    println!(
        "analyzed {} rows; kind has {} distinct values, at has {}\n",
        analyzed.row_count,
        analyzed.column(column("kind")).distinct,
        analyzed.column(column("at")).distinct,
    );

    let slow = LatencyStore::new(backing, LatencyProfile::object_storage());
    let counters = slow.counters();
    let store = RecordStore::new(slow, catalog, security).with_statistics(
        Statistics::new()
            .with(EVENTS, analyzed)
            .with(ACTORS, actors_analyzed)
            .with(TEAMS, teams_analyzed),
    );

    let tenant = Value::Uuid(Uuid::from_u128(0));
    let by_tenant = || Expr::eq(column("tenant_id"), tenant.clone());
    let kind_7 = || Expr::eq(column("kind"), Value::Str("kind-7".into()));
    let early = || Expr::compare(column("at"), CmpOp::Lt, Value::I64(500));

    let all = Projection::All;
    let keys_only = Projection::Columns(vec![column("id"), column("kind")]);
    // Counting needs no columns of its own, only the predicate's.
    let nothing = Projection::none();

    struct Case<'a> {
        label: &'a str,
        filter: Expr,
        limit: Option<usize>,
        projection: &'a Projection,
    }

    // The two labels that name a row count are computed, not written.
    //
    // They were literals — `whole tenant (2500 rows)`, `indexed equality (~100
    // rows)` — which was true while the size was a `const` and became a lie
    // the moment `KERNELBENCH_ROWS` could change it: a smoke run printed
    // `whole tenant (2500 rows)` beside a `rows` column reading 500. A label
    // that disagrees with the number beside it is worse than no label.
    //
    // The `~` one divides by 25 because the seed writes `kind-{id % 25}` and
    // this filter asks for one of them; it is an estimate in the same sense it
    // always was.
    let whole = format!("whole tenant ({} rows)", rows_per_tenant());
    let indexed = format!("indexed equality (~{} rows)", rows_per_tenant() / 25);
    let cases = vec![
        Case {
            label: "point get by primary key",
            filter: by_tenant().and(Expr::eq(column("id"), Value::U64(1234))),
            limit: None,
            projection: &all,
        },
        Case {
            label: &whole,
            filter: by_tenant(),
            limit: None,
            projection: &all,
        },
        Case {
            label: &indexed,
            filter: by_tenant().and(kind_7()),
            limit: None,
            projection: &all,
        },
        Case {
            label: "indexed equality, limit 10",
            filter: by_tenant().and(kind_7()),
            limit: Some(10),
            projection: &all,
        },
        Case {
            label: "indexed range (~500 rows)",
            filter: by_tenant().and(early()),
            limit: None,
            projection: &all,
        },
        Case {
            label: "unindexed filter (0 rows, full scan)",
            filter: by_tenant().and(Expr::eq(column("note"), Value::Str("never".into()))),
            limit: None,
            projection: &all,
        },
        // The same two queries, asking only for columns an index already holds.
        Case {
            label: "covered: indexed equality, keys only",
            filter: by_tenant().and(kind_7()),
            limit: None,
            projection: &keys_only,
        },
        Case {
            label: "covered: count over an index",
            filter: by_tenant().and(early()),
            limit: None,
            projection: &nothing,
        },
    ];

    println!(
        "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12}  plan",
        "query", "rows", "gets", "scans", "scan rows", "wall"
    );
    println!("{:-<118}", "");

    for query in cases {
        let mut request = Query::all()
            .filter(query.filter)
            .order(ScanOrder::Ascending);
        request.projection = query.projection.clone();
        if let Some(limit) = query.limit {
            request = request.limit(limit);
        }

        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        let described = txn
            .explain(&root, &table, &request)
            .expect("explain")
            .access
            .to_string();
        let rows = txn
            .execute(&root, &table, &request)
            .await
            .expect("query")
            .count()
            .await
            .expect("count");
        let elapsed = started.elapsed();

        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}  {}",
            query.label,
            rows,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            elapsed,
            described
        );
    }

    // Does the cost model's per-read charge match what the executor does? It
    // charges one round trip per point read; the executor issues sixteen at a
    // time. Forcing each path and timing both is the only way to know.
    println!("\nreading a set of keys");
    println!("{:-<118}", "");
    for n in [10usize, 50, 200] {
        let ids: Vec<Value> = (0..n as u64).map(|i| Value::U64(i * 7)).collect();
        let query = Query::all().filter(by_tenant().and(Expr::In {
            column: column("id"),
            values: ids,
        }));
        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        let described = txn.explain(&root, &table, &query).expect("explain");
        let rows = txn
            .execute(&root, &table, &query)
            .await
            .expect("query")
            .count()
            .await
            .expect("count");
        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}  cost={:.1} {}",
            format!("{n} keys by primary key"),
            rows,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            started.elapsed(),
            described.estimated_cost,
            described.access
        );
    }

    println!("\nforced access paths");
    println!("{:-<118}", "");
    let forced: Vec<(&str, Query)> = vec![
        (
            "indexed equality, table scan",
            Query::all()
                .filter(by_tenant().and(kind_7()))
                .using_table_scan(),
        ),
        (
            "indexed equality, forced by_kind",
            Query::all()
                .filter(by_tenant().and(kind_7()))
                .using_index(IndexId(10)),
        ),
        (
            "indexed equality limit 10, table scan",
            Query::all()
                .filter(by_tenant().and(kind_7()))
                .limit(10)
                .using_table_scan(),
        ),
        (
            "indexed equality limit 10, forced index",
            Query::all()
                .filter(by_tenant().and(kind_7()))
                .limit(10)
                .using_index(IndexId(10)),
        ),
        (
            "narrow range (0.4% of rows), planner",
            Query::all().filter(by_tenant().and(Expr::compare(
                column("at"),
                CmpOp::Lt,
                Value::I64(10),
            ))),
        ),
        (
            "narrow range, forced by_at_desc",
            Query::all()
                .filter(by_tenant().and(Expr::compare(column("at"), CmpOp::Lt, Value::I64(10))))
                .using_index(IndexId(12)),
        ),
        (
            "indexed range, table scan",
            Query::all()
                .filter(by_tenant().and(early()))
                .using_table_scan(),
        ),
        (
            "indexed range, forced by_at_desc",
            Query::all()
                .filter(by_tenant().and(early()))
                .using_index(IndexId(12)),
        ),
    ];
    for query in forced {
        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        let described = txn.explain(&root, &table, &query.1).expect("explain");
        let rows = txn
            .execute(&root, &table, &query.1)
            .await
            .expect("query")
            .count()
            .await
            .expect("count");
        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}  cost={:.1} {}",
            query.0,
            rows,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            started.elapsed(),
            described.estimated_cost,
            described.access
        );
    }

    println!("\njoins");
    println!("{:-<118}", "");
    let on_actor = || Join::equating(actor_column("name"), column("actor"));
    let one_actor = || {
        Query::all().filter(
            Expr::eq(actor_column("tenant_id"), tenant.clone())
                .and(Expr::eq(actor_column("name"), Value::Str("actor-7".into()))),
        )
    };
    let joins: Vec<(&str, Join)> = vec![
        (
            "every actor to their events (hash)",
            on_actor()
                .left(Query::all().filter(Expr::eq(actor_column("tenant_id"), tenant.clone())))
                .right(Query::all().filter(by_tenant())),
        ),
        (
            "every actor to events, forced loop",
            on_actor()
                .left(Query::all().filter(Expr::eq(actor_column("tenant_id"), tenant.clone())))
                .right(Query::all().filter(by_tenant()))
                .using(JoinAlgorithm::NestedLoop),
        ),
        (
            "one actor's events (planner's choice)",
            on_actor()
                .left(one_actor())
                .right(Query::all().filter(by_tenant())),
        ),
        (
            "one actor's events, forced hash",
            on_actor()
                .left(one_actor())
                .right(Query::all().filter(by_tenant()))
                .using(JoinAlgorithm::Hash { build: Side::Right }),
        ),
    ];
    for (label, join) in joins {
        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        let described = txn
            .explain_join(&root, &actor_table, &table, &join)
            .expect("explain");
        let algorithm = match described.algorithm {
            JoinAlgorithm::NestedLoop => "Nested Loop".to_owned(),
            JoinAlgorithm::Hash { build } => format!("Hash (build {build:?})"),
        };
        let rows = txn
            .join(&root, &actor_table, &table, &join)
            .await
            .expect("join")
            .count()
            .await
            .expect("count");
        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}  {}",
            label,
            rows,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            started.elapsed(),
            algorithm
        );
    }

    println!("\nchains");
    println!("{:-<118}", "");
    let chain_tables: Vec<&TableDef> = vec![&team_table, &actor_table, &table];
    let at = JoinSchema::over(chain_tables.iter().copied());
    let one_team = || {
        Query::all().filter(
            Expr::eq(team_column("tenant_id"), tenant.clone())
                .and(Expr::eq(team_column("name"), Value::Str("team-3".into()))),
        )
    };
    let all_teams = || Query::all().filter(Expr::eq(team_column("tenant_id"), tenant.clone()));
    let to_actors = || {
        JoinStep::equating(at.at(0, team_column("name")), actor_column("team"))
            .query(Query::all().filter(Expr::eq(actor_column("tenant_id"), tenant.clone())))
    };
    let to_events = || {
        JoinStep::equating(at.at(1, actor_column("name")), column("actor"))
            .query(Query::all().filter(by_tenant()))
    };
    let chains: Vec<(&str, Chain)> = vec![
        (
            "every team -> actors -> events",
            Chain::from(all_teams()).join(to_actors()).join(to_events()),
        ),
        (
            "one team -> actors -> events",
            Chain::from(one_team()).join(to_actors()).join(to_events()),
        ),
    ];
    for (label, chain) in chains {
        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        let plan = txn
            .explain_chain(&root, &chain_tables, &chain)
            .expect("explain");
        let algorithms: Vec<String> = plan
            .steps
            .iter()
            .map(|step| match step.algorithm {
                JoinAlgorithm::NestedLoop => "loop".to_owned(),
                JoinAlgorithm::Hash { .. } => "hash".to_owned(),
            })
            .collect();
        let cursor = txn
            .chain(&root, &chain_tables, &chain)
            .await
            .expect("chain");
        let steps = format!("{:?}", cursor.step_counts());
        let rows = cursor.count().await.expect("count");
        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}  {} {}",
            label,
            rows,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            started.elapsed(),
            algorithms.join("+"),
            steps
        );
    }

    println!("\nwrites");
    println!("{:-<118}", "");
    let mut next_id = 1_000_000u64;
    for (label, batch) in [
        ("insert 1 row", 1u64),
        ("insert 100 rows, one at a time", 100),
    ] {
        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        for _ in 0..batch {
            txn.insert(&root, &table, &row(0, next_id))
                .await
                .expect("insert");
            next_id += 1;
        }
        txn.commit().await.expect("commit");
        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}",
            label,
            batch,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            started.elapsed()
        );
    }

    // The same work, with the reads issued together instead of in turn.
    for batch in [100u64, 1000] {
        let rows: Vec<_> = (0..batch)
            .map(|_| {
                let r = row(0, next_id);
                next_id += 1;
                r
            })
            .collect();
        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        txn.insert_many(&root, &table, &rows)
            .await
            .expect("insert_many");
        txn.commit().await.expect("commit");
        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}",
            format!("insert {batch} rows, batched"),
            batch,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            started.elapsed()
        );
    }
}
