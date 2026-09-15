//! Where the 1,290 bytes a row go.
//!
//! `docs/performance.md` §7b measured the in-memory store at about 1,290 bytes
//! for rows that serialise to roughly 110, called the factor twelve, and said
//! plainly that it had not been investigated. This investigates it.
//!
//! The old number came from **RSS divided by rows**, which is the wrong
//! instrument for the question. RSS counts the allocator's unreturned arenas,
//! fragmentation, and — the part that matters here — everything else the
//! process is still holding, including the source rows the store was built
//! from. It cannot tell you what the *store* costs.
//!
//! So this counts allocations instead. A global allocator wrapper tracks live
//! bytes exactly, and the run is staged so each stage's cost is the difference
//! between two readings rather than one reading attributed to a guess.
//!
//! It lives beside `bucket_layout` because both need the taxi sample, which
//! `slate-wasm` owns; the subject is `slate-kernel`'s `MemoryStore`, and making
//! `slate-kernel` dev-depend on a crate that depends on it to read one file
//! would be the worse trade.
//!
//! ```sh
//! cargo run --release -p slate-slatedb --example row_footprint
//! ```

#![allow(clippy::print_stdout, clippy::expect_used, clippy::cast_precision_loss)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::security::{Action, Grant, SecurityCatalog, SecurityContext};
use slate_kernel::{RecordStore, Statistics};
use std::io::Read;
use std::path::PathBuf;

/// `dhat` rather than a hand-rolled counting allocator, which is what this
/// started as: the workspace `forbid`s `unsafe_code`, and weakening that for a
/// measurement would be a poor trade against a crate whose whole job is to do
/// the unsafe part once, correctly, behind a safe API.
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Live heap bytes and live blocks, right now — zeroes without the feature,
/// which is what makes the timing run cheap.
#[cfg(feature = "dhat-heap")]
fn live() -> (usize, usize) {
    let stats = dhat::HeapStats::get();
    (stats.curr_bytes, stats.curr_blocks)
}

#[cfg(not(feature = "dhat-heap"))]
const fn live() -> (usize, usize) {
    (0, 0)
}

fn mb(bytes: usize) -> String {
    format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Held for the whole run: `HeapStats::get` reports nothing without it.
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::builder().testing().build();

    let rows = slate_wasm::taxi::decode(&trip_bytes()?)?;
    let n = rows.len();
    let trips = slate_wasm::taxi::trips();
    let per = |bytes: usize| bytes as f64 / n as f64;
    let row = |label: &str, bytes: usize| {
        println!("  {label:<36}{:>11}{:>10.0}", mb(bytes), per(bytes));
    };

    // The source rows are held exactly as the old harness held them, so their
    // cost can be named rather than blamed on the store.
    let (source, _) = live();

    let kv_store = MemoryStore::new();
    let store = RecordStore::new(
        kv_store.clone(),
        slate_wasm::taxi::catalog(),
        SecurityCatalog::new().grant(Grant::new(
            "app",
            slate_wasm::taxi::TRIPS,
            Action::EVERYTHING,
        )),
    );
    let ctx = SecurityContext::superuser();

    let started = std::time::Instant::now();
    let txn = store.begin().await?;
    txn.insert_many(&ctx, &trips, &rows).await?;
    txn.commit().await?;
    let write = started.elapsed();
    let (inserted, blocks_inserted) = live();

    let analysis = store.begin().await?;
    let _stats = Statistics::new().with(
        slate_wasm::taxi::TRIPS,
        analysis.analyze(&ctx, &trips).await?,
    );
    drop(analysis);
    let (analysed, _) = live();

    // The difference between "what the process holds" and "what the store
    // holds", and most of the gap the old RSS number could not see.
    drop(rows);
    let (store_only, blocks_store) = live();

    // `Shared::history` keeps a `BTreeSet` of every key a commit wrote, for
    // conflict detection, and `trim_history` runs only from `unregister` —
    // when a *transaction* is dropped. So a store that has committed and has
    // nothing open still holds that set until something touches it. Beginning
    // and dropping one transaction is the cheapest way to find out what it is
    // worth.
    drop(store.begin().await?);
    let (trimmed, blocks_trimmed) = live();

    println!(
        "{n} trips, {} keys per row, loaded in {:.2} s ({:.0}k rows/s)",
        kv_store.len() / n,
        write.as_secs_f64(),
        n as f64 / write.as_secs_f64() / 1000.0
    );
    println!();
    println!("  {:<36}{:>11}{:>10}", "", "total", "per row");
    row("source rows, before the store", source);
    row("+ insert_many", inserted);
    row("+ analyze", analysed);
    row("- the source rows: the store itself", store_only);
    row("- the commit's conflict history", trimmed);
    println!(
        "  {:<36}{:>11}{:>10.1}",
        "live allocations per row",
        "",
        per(blocks_trimmed)
    );
    println!(
        "  {:<36}{:>11}{:>10.1}",
        "  before the history went",
        "",
        per(blocks_store)
    );
    let _ = blocks_inserted;

    // Everything above is the store. What follows is the floor underneath it.
    let kv = kv_store.entries();
    let logical: usize = kv.iter().map(|(key, value)| key.len() + value.len()).sum();

    // Exactly these pairs in a plain `BTreeMap<Bytes, Bytes>`: no store, no
    // snapshot `Arc`, no history. Copied rather than cloned, because cloning a
    // `Bytes` shares the buffer and would measure a map of pointers into the
    // store rather than a map of its own.
    //
    // The record layer cannot get under this without changing the
    // representation itself; the gap above it is what the layer adds.
    let (before_map, _) = live();
    let bare: std::collections::BTreeMap<bytes::Bytes, bytes::Bytes> = kv
        .iter()
        .map(|(key, value)| {
            (
                bytes::Bytes::copy_from_slice(key),
                bytes::Bytes::copy_from_slice(value),
            )
        })
        .collect();
    let (after_map, _) = live();
    let map_only = after_map - before_map;
    let headers = kv.len() * 2 * std::mem::size_of::<bytes::Bytes>();
    drop(bare);

    // The same pairs again, but built the way the write path builds them:
    // `Vec::with_capacity(n * 9)` for a tuple of n values, grown by doubling
    // if that guess was low, and handed to `Bytes::from` — which keeps the
    // Vec's *capacity*, not its length, for the life of the entry. The
    // difference between this and the exact map above is the slack.
    let (before_slack, _) = live();
    let slacked: std::collections::BTreeMap<bytes::Bytes, bytes::Bytes> = kv
        .iter()
        .map(|(key, value)| (as_written(key), as_written(value)))
        .collect();
    let (after_slack, _) = live();
    let slack_map = after_slack - before_slack;
    drop(slacked);

    println!();
    row("keys and values, logical", logical);
    row("the same pairs, a bare BTreeMap", map_only);
    row("  of which inline Bytes headers", headers);
    row("the same pairs, built as the writer does", slack_map);
    println!();
    println!(
        "  {:<36}{:>11}{:>10.1}",
        "store over logical",
        "",
        trimmed as f64 / logical as f64
    );
    println!(
        "  {:<36}{:>11}{:>10.1}",
        "bare map over logical",
        "",
        map_only as f64 / logical as f64
    );
    println!(
        "  {:<36}{:>11}{:>10.0}",
        "what the store adds over the map",
        "",
        per(trimmed - map_only)
    );

    Ok(())
}

/// A `Bytes` holding `data`, allocated the way the write path allocates: a
/// `Vec` sized by the encoder's guess of nine bytes a value, grown by doubling
/// when the guess is low, then handed to `Bytes::from`.
///
/// The guess is the encoder's, not this example's — `slate_tuple::encode` uses
/// `values.len() * 9` and `slate_kernel::keys` uses a header plus the same per
/// value. Reproducing it here rather than calling the encoder keeps the
/// comparison to one variable: identical bytes, different capacity.
fn as_written(data: &[u8]) -> bytes::Bytes {
    // Eleven values in a trip row, one in a primary key. The exact guess does
    // not matter to the shape of the answer; what matters is that it is a
    // guess and the capacity outlives it.
    let guess = if data.len() > 32 { 11 * 9 } else { 5 + 9 };
    let mut out = Vec::with_capacity(guess);
    out.extend_from_slice(data);
    bytes::Bytes::from(out)
}

fn trip_bytes() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "site",
        "data",
        "trips.bin.gz",
    ]
    .iter()
    .collect();
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path)?).read_to_end(&mut out)?;
    Ok(out)
}
