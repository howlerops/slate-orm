//! The committed bucket listing describes the schema and the sample it says.
//!
//! `site/data/bucket.json` is a real listing of a real SlateDB bucket, taken by
//! `crates/slate-slatedb/examples/bucket_layout.rs`. It was also, until this
//! file existed, unchecked — recorded as: "It is a snapshot of one load of one
//! sample. Change the schema or the row count and it silently describes the
//! old thing."
//!
//! Comparing the objects against a fresh run is not available. The SST names
//! are ULIDs minted at write time and the byte counts move with SlateDB's
//! block packing, so an exact compare fails on a rerun that changed nothing,
//! and a tolerance loose enough to pass is loose enough to miss the drift that
//! matters. Running SlateDB in CI to take the picture is what the caveat
//! already rejected as not worth it for a picture, and it is still not.
//!
//! So this checks *what the listing is a listing of*, which is the thing that
//! actually goes stale: the schema, the row counts, and that the objects are
//! the kinds the page knows how to explain. It needs no SlateDB, no object
//! store and no bulk load, and it runs in about a millisecond.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value as Json;

const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../site/data/bucket.json");
const REGENERATE: &str = "regenerate it: cargo run --release -p slate-slatedb \
                          --example bucket_layout -- --json > site/data/bucket.json";

fn listing() -> Json {
    let text = std::fs::read_to_string(PATH).unwrap_or_else(|e| panic!("{PATH}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{PATH} is not JSON: {e}"))
}

#[test]
fn the_listing_describes_the_current_schema() {
    let got = listing()["provenance"]["schema"]
        .as_str()
        .unwrap()
        .to_owned();
    let want = slate_wasm::taxi::schema_fingerprint();
    assert_eq!(
        got, want,
        "the committed bucket listing was taken against a different schema, so \
         its object sizes describe rows that no longer exist — {REGENERATE}"
    );
}

#[test]
fn the_listing_describes_the_current_sample() {
    let listing = listing();
    let trips = listing["provenance"]["trips"].as_u64().unwrap();
    let zones = listing["provenance"]["zones"].as_u64().unwrap();

    // Counted from the committed sample rather than from a constant, so that
    // swapping the file for a bigger month fails here rather than leaving a
    // listing that quietly under-reports what the data costs.
    let bytes = {
        use std::io::Read;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../site/data/trips.bin.gz");
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
            .read_to_end(&mut out)
            .unwrap();
        out
    };
    assert_eq!(
        trips as usize,
        slate_wasm::taxi::decode(&bytes).unwrap().len(),
        "the listing is of a different number of trips than the site ships — {REGENERATE}"
    );
    assert_eq!(
        zones as usize,
        slate_wasm::taxi::zone_rows().len(),
        "the listing is of a different zone table than the site ships — {REGENERATE}"
    );
}

/// Every object falls into a kind the storage page can name.
///
/// `workbench.js` classifies by path fragment and has a final `else` reading
/// "compaction bookkeeping". A new kind of object — SlateDB growing a
/// directory, or a rename — would land in that bucket and be *described
/// wrongly* rather than shown as unknown. This is the assertion that fragment
/// list is complete, kept beside the data rather than in the browser check so
/// that it fails without a browser.
#[test]
fn every_object_is_a_kind_the_page_can_explain() {
    let known = [
        "/compacted/",
        "/wal/",
        "/manifest/",
        "/compactions/",
        "/gc/",
    ];
    for object in listing()["objects"].as_array().unwrap() {
        let path = object["path"].as_str().unwrap();
        assert!(
            known.iter().any(|fragment| path.contains(fragment)),
            "`{path}` is not a kind of object the storage page knows how to \
             label; teach `renderBucket` about it before shipping a listing \
             that contains one"
        );
        assert!(
            object["bytes"].as_u64().is_some(),
            "`{path}` has no byte count"
        );
    }
}

/// The listing does not double-count the rows.
///
/// This is the arithmetic `the-wal-the-collector-would-not-take` fixed: before
/// it, the WAL still held a copy of every trip and the total read 21.7 MB for
/// 11.0 MB of data. The browser check asserts the same thing about what is on
/// screen; this asserts it about the file, so a regenerated listing that
/// regressed fails in the Rust suite rather than only in the one job that
/// starts a browser.
#[test]
fn the_total_is_not_twice_the_data() {
    let listing = listing();
    let objects = listing["objects"].as_array().unwrap();
    let total: u64 = objects.iter().map(|o| o["bytes"].as_u64().unwrap()).sum();
    let largest = objects
        .iter()
        .map(|o| o["bytes"].as_u64().unwrap())
        .max()
        .unwrap();
    assert!(
        total < largest * 3 / 2,
        "the listing totals {total} bytes with a largest object of {largest}, \
         which means the WAL still holds a second copy of the rows — the load \
         needs a write past the WAL boundary before the collector can take it"
    );
}
