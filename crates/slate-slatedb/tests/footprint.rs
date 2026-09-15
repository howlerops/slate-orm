//! The store does not hold the encoders' capacity guesses.
//!
//! `slate-kernel`'s own `tests/footprint.rs` covers the behaviour — shrinking
//! reallocates, and the bytes and their order must survive it. It cannot cover
//! the *footprint*, because a `Bytes`'s capacity is not observable through any
//! public API, and the first attempt to infer it from `Vec::from(Bytes)` passed
//! with the shrink removed.
//!
//! This measures instead. `dhat` counts the bytes each allocation *requested*,
//! not the allocator's rounding or the process's RSS, so the number is a
//! property of the program and reproducible on any machine — which is what
//! makes a threshold assertable at all.
//!
//! It lives here because installing a global allocator is not something
//! `slate-kernel` may do to its own test binaries, and because the taxi sample
//! this measures against belongs to `slate-wasm`, already a dev-dependency
//! here for `bucket_layout`.
//!
//! Ten thousand rows rather than the hundred thousand the example uses: the
//! per-row figure is flat across that range and dhat's allocator is an order of
//! magnitude slower than the system one, so the larger sample would buy a
//! third significant figure at the cost of every CI run.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::panic
)]

use slate_kernel::RecordStore;
use slate_kernel::memory::MemoryStore;
use slate_kernel::security::{Action, Grant, SecurityCatalog, SecurityContext};
use std::io::Read;
use std::path::PathBuf;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const ROWS: usize = 10_000;

/// What the store may hold per row before this is a regression.
///
/// Measured at 403 bytes a row with the shrink and 491 without, so the bound
/// sits between them with room for the smaller sample and for a change to the
/// schema that adds a column. It is a ceiling, not a target: a change that
/// takes it to 300 should lower this line.
const CEILING: f64 = 450.0;

#[tokio::test]
async fn the_store_does_not_keep_the_encoders_slack() {
    let profiler = dhat::Profiler::builder().testing().build();

    let mut rows = slate_wasm::taxi::decode(&trip_bytes()).expect("decode");
    rows.truncate(ROWS);
    let trips = slate_wasm::taxi::trips();

    let kv = MemoryStore::new();
    let store = RecordStore::new(
        kv.clone(),
        slate_wasm::taxi::catalog(),
        SecurityCatalog::new().grant(Grant::new(
            "app",
            slate_wasm::taxi::TRIPS,
            Action::EVERYTHING,
        )),
    );
    let ctx = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    txn.insert_many(&ctx, &trips, &rows).await.unwrap();
    txn.commit().await.unwrap();

    // The source rows are what made the old RSS-based figure read 1,290 for a
    // store that holds 403. Dropping them before the reading is the whole
    // difference between measuring the store and measuring the process.
    let logical: usize = kv
        .entries()
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum();
    drop(rows);

    let held = dhat::HeapStats::get().curr_bytes;
    let per_row = held as f64 / ROWS as f64;
    let per_row_logical = logical as f64 / ROWS as f64;

    assert!(
        per_row < CEILING,
        "the store holds {per_row:.0} bytes a row for {per_row_logical:.0} of keys and values, \
         over the {CEILING:.0} ceiling — the likeliest cause is a buffer reaching \
         `Bytes::from` without a `shrink_to_fit`, which makes the entry keep the encoder's \
         capacity guess for life"
    );

    // A floor as well as a ceiling. Without one, a change that broke
    // `insert_many` into storing nothing would pass this file, and a footprint
    // test that passes on an empty store is not a test.
    assert!(
        per_row > per_row_logical,
        "the store holds {per_row:.0} bytes a row for {per_row_logical:.0} of data, which is \
         less than the data — is anything being stored?"
    );
    assert_eq!(kv.len(), ROWS * 2, "a row and its index entry each");

    drop(profiler);
}

fn trip_bytes() -> Vec<u8> {
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
    let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(file)
        .read_to_end(&mut out)
        .expect("gzip");
    out
}
