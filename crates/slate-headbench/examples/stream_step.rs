//! The ~2.3 ms step in first-row latency at a stream batch of about 125.
//!
//! ```sh
//! cargo run --release -p slate-headbench --example stream_step
//! cargo run --release -p slate-headbench --example stream_step -- width split
//! ```
//!
//! `docs/performance.md` records a reproducible step: first-row latency is flat
//! at 620–680 µs up to a batch of 124 and jumps to ~2.9 ms at 126, and stays
//! there. It looked exactly like tokio's 128-operation cooperative budget until
//! a 31-run sweep produced a fast run at 128, which that hypothesis forbids, so
//! it was written down as unexplained rather than explained wrongly.
//!
//! This example does not add repetitions to the same sweep. Repetitions were
//! what killed the last explanation; what is needed is to *change one thing at
//! a time* and see whether the step moves, and each arm below is chosen because
//! a different candidate explanation predicts a different answer:
//!
//! | arm | what changes | what it separates |
//! |---|---|---|
//! | `base` | nothing | that the step is here at all on this machine |
//! | `width` | a row is ~5x wider | a **row count** threshold from a **byte count** one |
//! | `memory` | `MemoryStore` under the head | storage from the head node |
//! | `split` | client on its own runtime | the server from the harness sharing its cores |
//! | `single` | server on one worker thread | a scheduler hand-off from work |
//! | `short` | the table holds one batch | the *next* batch being produced from the first being sent |
//!
//! Each arm reports **two** times, which the original sweep did not separate:
//! the header message — sent by the head node before it has produced a single
//! row — and the first message carrying rows. Everything up to and including
//! routing and opening the cursor is in the first; only producing and shipping
//! the batch is in the difference. A step that is in the header is not a
//! batching step at all.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use slate_headbench::fixture::{catalog, context, events, principal_request, security};
use slate_headbench::harness::{Backend, InProcess, leading, memory, serve_with_nagle};
use slate_headbench::stats::{Measure, duration};
use slate_kernel::{Expr, KvReadStore, KvStore, Query, RecordStore, ScanOrder};
use slate_schema::{Row, TableDef};
use slate_server::convert::query_to_proto;
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_server::{Head, Limits};
use slate_tuple::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use tonic::transport::Channel;

const TENANT: u64 = 1;

fn env_usize(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(fallback)
}

fn runs() -> usize {
    env_usize("HEADBENCH_RUNS", 15)
}

/// Batch sizes swept.
///
/// Wide either side of 125 rather than tight around it, because two of the arms
/// predict the threshold *moves*, and a sweep that only looks at 118–130 cannot
/// see a threshold that has moved to 24.
fn batches() -> Vec<usize> {
    match std::env::var("HEADBENCH_BATCHES") {
        Ok(list) => list
            .split(',')
            .filter_map(|part| part.trim().parse().ok())
            .filter(|n| *n > 0)
            .collect(),
        Err(_) => vec![8, 16, 24, 32, 48, 64, 96, 112, 120, 124, 126, 128, 160, 256],
    }
}

/// Rows in the fixture table.
fn table_rows() -> u64 {
    env_usize("HEADBENCH_ROWS", 20_000) as u64
}

/// A narrow row: `note` is null, which is what `fixture::row` produces.
fn narrow(tenant: u64, id: u64) -> Row {
    slate_headbench::fixture::row(tenant, id)
}

/// A wide row: the same, with a 220-byte note.
///
/// Five or so times the protobuf width of a narrow row, which is the point. If
/// the threshold is a byte count it should land at roughly a fifth of the row
/// count; if it is a row count it should not move at all.
fn wide(tenant: u64, id: u64) -> Row {
    let mut values = narrow(tenant, id).into_values();
    let note = format!("{id:0>4}").repeat(55);
    if let Some(last) = values.last_mut() {
        *last = Value::Str(note);
    }
    Row::new(values)
}

/// One thing changed against the baseline.
#[derive(Debug, Clone, Copy)]
struct Arm {
    name: &'static str,
    /// `false` for SlateDB, `true` for the kernel's `MemoryStore`.
    memory_backend: bool,
    /// Rows carry a 220-byte note.
    wide_rows: bool,
    /// Client on its own runtime rather than the server's.
    split_runtimes: bool,
    /// Worker threads for the server's runtime, when it has its own.
    server_workers: Option<usize>,
    /// The query carries `LIMIT batch`, so the scan ends with the first batch.
    limit_to_batch: bool,
    /// Nagle's algorithm left on, which is what every earlier measurement of
    /// this step ran under. See `harness::serve_with_nagle`.
    nagle: bool,
}

const BASE: Arm = Arm {
    name: "base",
    memory_backend: false,
    wide_rows: false,
    split_runtimes: false,
    server_workers: None,
    limit_to_batch: false,
    nagle: false,
};

fn main() {
    let requested: Vec<String> = std::env::args().skip(1).collect();
    let wanted = |name: &str| requested.is_empty() || requested.iter().any(|s| s == name);

    let arms = [
        Arm {
            name: "nagle",
            nagle: true,
            ..BASE
        },
        BASE,
        Arm {
            name: "width",
            wide_rows: true,
            ..BASE
        },
        Arm {
            name: "widenagle",
            wide_rows: true,
            nagle: true,
            ..BASE
        },
        Arm {
            name: "memory",
            memory_backend: true,
            ..BASE
        },
        Arm {
            name: "split",
            split_runtimes: true,
            ..BASE
        },
        Arm {
            name: "single",
            split_runtimes: true,
            server_workers: Some(1),
            ..BASE
        },
        Arm {
            name: "short",
            limit_to_batch: true,
            ..BASE
        },
    ];

    println!("# The step in first-row latency at a batch of about 125");
    println!();
    println!(
        "{} runs per point; {} rows in the table; medians with the best run beside them.",
        runs(),
        table_rows()
    );
    println!(
        "`header` is the empty first message the head node sends before producing any\n\
         row. `first row` is the first message with rows in it. `produce` is the\n\
         difference — routing and cursor-open are in the header, and only making and\n\
         shipping the batch is in the difference."
    );

    for arm in arms {
        if !wanted(arm.name) {
            continue;
        }
        run_arm(arm);
    }
}

fn run_arm(arm: Arm) {
    println!("\n\n## arm `{}`", arm.name);
    println!("{:-<96}", "");
    println!(
        "backend {}, rows {}, runtimes {}, server workers {}, {}",
        if arm.memory_backend {
            "MemoryStore"
        } else {
            "SlateDB"
        },
        if arm.wide_rows {
            "wide (~220 B note)"
        } else {
            "narrow (null note)"
        },
        if arm.split_runtimes {
            "split"
        } else {
            "shared"
        },
        arm.server_workers
            .map_or_else(|| "default".to_owned(), |n| n.to_string()),
        if arm.limit_to_batch {
            "query limited to one batch"
        } else {
            "whole table"
        }
    );
    println!(
        "socket: {}",
        if arm.nagle {
            "Nagle on (what every earlier sweep of this step ran under)"
        } else {
            "TCP_NODELAY, as tonic's own Server sets by default"
        }
    );
    println!();

    let server_rt = if arm.split_runtimes {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.thread_name("hd-worker").enable_all();
        if let Some(workers) = arm.server_workers {
            builder.worker_threads(workers);
        }
        Some(builder.build().expect("build the server runtime"))
    } else {
        None
    };
    let client_rt = tokio::runtime::Builder::new_multi_thread()
        .thread_name("ld-worker")
        .enable_all()
        .build()
        .expect("build the client runtime");

    if arm.memory_backend {
        let writer = memory();
        sweep(arm, server_rt.as_ref(), &client_rt, writer);
    } else {
        let setup = server_rt.as_ref().unwrap_or(&client_rt);
        let backend = setup.block_on(Backend::open());
        let writer = backend.visible();
        sweep(arm, server_rt.as_ref(), &client_rt, writer);
    }
}

fn sweep<S: KvStore + KvReadStore + 'static>(
    arm: Arm,
    server_rt: Option<&Runtime>,
    client_rt: &Runtime,
    writer: Arc<S>,
) {
    let table = events();
    let rows = table_rows();
    let inproc = InProcess::new(catalog(), security(), Arc::clone(&writer), Vec::new());
    let setup = server_rt.unwrap_or(client_rt);
    setup.block_on(seed(&inproc.writer, &table, rows, arm.wide_rows));

    println!(
        "{:>8} {:>9} {:>12} {:>12} {:>12} {:>12} {:>12} {:>10}",
        "batch",
        "messages",
        "header med",
        "first med",
        "first best",
        "first worst",
        "produce med",
        "drain best"
    );
    println!("{:-<96}", "");

    for batch in batches() {
        let query = if arm.limit_to_batch {
            Query::all()
                .filter(Expr::True)
                .order(ScanOrder::Ascending)
                .limit(batch)
        } else {
            Query::all().filter(Expr::True).order(ScanOrder::Ascending)
        };
        let wire = query_to_proto(&table, &query);

        let head: Head<S> = slate_headbench::harness::head(
            catalog(),
            security(),
            Arc::clone(&writer),
            Vec::new(),
            setup.block_on(leading()),
            Limits {
                rows_per_message: batch,
                ..Limits::default()
            },
        );
        let serving = setup.block_on(serve_with_nagle(head, arm.nagle));
        let address = serving.address;

        let (header, first, drain, messages, seen) = client_rt.block_on(async move {
            let mut client = RecordsClient::connect(format!("http://{address}"))
                .await
                .expect("connect");
            warm(&mut client).await;

            let mut header = Measure::new("header", 1);
            let mut first = Measure::new("first row", 1);
            let mut drain = Measure::new("drain", 1);
            let mut messages_seen = 0usize;
            let mut rows_seen = 0usize;
            for run in 0..runs() + 1 {
                let started = Instant::now();
                let mut stream = client
                    .query(principal_request(
                        pb::QueryRequest {
                            transaction: String::new(),
                            query: Some(wire.clone()),
                            freshness: Some(pb::Freshness {
                                level: Some(pb::freshness::Level::Any(0)),
                            }),
                        },
                        TENANT,
                    ))
                    .await
                    .expect("query")
                    .into_inner();
                let mut at_header = None;
                let mut at_first = None;
                let mut messages = 0usize;
                let mut rows = 0usize;
                while let Some(message) = stream.message().await.expect("message") {
                    messages += 1;
                    if at_header.is_none() {
                        at_header = Some(started.elapsed());
                    }
                    if at_first.is_none() && !message.rows.is_empty() {
                        at_first = Some(started.elapsed());
                    }
                    rows += message.rows.len();
                }
                let elapsed = started.elapsed();
                if run > 0 {
                    messages_seen = messages;
                    rows_seen = rows;
                    header.run(at_header.unwrap_or(elapsed));
                    first.run(at_first.unwrap_or(elapsed));
                    drain.run(elapsed);
                }
            }
            (header, first, drain, messages_seen, rows_seen)
        });

        // A batch size that silently did not take effect would look like a null
        // result, so it is checked rather than trusted.
        let expected_rows = if arm.limit_to_batch {
            batch.min(rows as usize)
        } else {
            rows as usize
        };
        assert_eq!(seen, expected_rows, "batch {batch} must return every row");
        let expected_messages = 1 + expected_rows.div_ceil(batch);
        assert_eq!(
            messages, expected_messages,
            "batch {batch} should send {expected_messages} messages"
        );

        println!(
            "{:>8} {:>9} {:>12} {:>12} {:>12} {:>12} {:>12} {:>10}",
            batch,
            messages,
            duration(header.median()),
            duration(first.median()),
            duration(first.min()),
            duration(first.max()),
            duration(first.median() - header.median()),
            duration(drain.min()),
        );
        drop(serving);
    }
}

async fn seed<S: KvStore + KvReadStore>(
    store: &RecordStore<Arc<S>>,
    table: &TableDef,
    count: u64,
    wide_rows: bool,
) {
    let ctx = context(TENANT);
    let build = if wide_rows { wide } else { narrow };
    for chunk in (0..count).step_by(1_000) {
        let rows: Vec<Row> = (chunk..(chunk + 1_000).min(count))
            .map(|i| build(TENANT, i + 1))
            .collect();
        let transaction = store.begin().await.expect("begin");
        transaction
            .insert_many(&ctx, table, &rows)
            .await
            .expect("seed insert");
        transaction.commit().await.expect("seed commit");
    }
}

/// The same warm-up `head_report` uses: HTTP/2 grows its flow-control window,
/// and a channel that has not finished settling is a different channel.
async fn warm(client: &mut RecordsClient<Channel>) {
    for _ in 0..2_000 {
        let _ = client
            .leadership(tonic::Request::new(pb::LeadershipRequest {}))
            .await
            .expect("leadership");
    }
    // Let the connection go quiet again: the sweep's runs are separated by a
    // whole drain, not by nothing.
    tokio::time::sleep(Duration::from_millis(50)).await;
}
