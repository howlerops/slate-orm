//! The keyspace viewer, checked against the layout the kernel documents.
//!
//! The viewer reads the store's raw bytes and re-describes them. That is a
//! second reading of a format the kernel owns, and a second reading can drift.
//! These tests are what stop it: every assertion here is about the *layout*,
//! not about the panel.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Value as Json, json};
use slate_wasm::Playground;

fn keyspace(playground: &Playground) -> Vec<Json> {
    serde_json::from_str(&playground.keyspace()).expect("the keyspace is JSON")
}

fn group<'a>(groups: &'a [Json], path: &str) -> &'a Json {
    groups
        .iter()
        .find(|g| g["path"] == json!(path))
        .unwrap_or_else(|| panic!("no group {path} in {:?}", paths(groups)))
}

fn paths(groups: &[Json]) -> Vec<String> {
    groups
        .iter()
        .map(|g| g["path"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn every_table_and_index_appears_as_its_own_prefix() {
    let playground = Playground::new();
    let groups = keyspace(&playground);
    for path in [
        "rows/books",
        "rows/authors",
        "rows/zones",
        "index/books.by_author",
    ] {
        let _ = group(&groups, path);
    }
    assert_eq!(group(&groups, "rows/zones")["keys"], json!(265));
    assert_eq!(group(&groups, "rows/books")["keys"], json!(4824));
}

#[test]
fn a_row_and_its_index_entry_are_two_keys_in_one_space() {
    let playground = Playground::new();
    let groups = keyspace(&playground);
    // The claim the whole viewer exists to make visible: there is no separate
    // index structure. The index is more keys in the same ordered map, and
    // there are exactly as many of them as there are rows.
    assert_eq!(
        group(&groups, "index/books.by_author")["keys"],
        group(&groups, "rows/books")["keys"],
        "one index entry per row"
    );
    assert_eq!(group(&groups, "rows/books")["space"], json!("rows"));
    assert_eq!(
        group(&groups, "index/books.by_author")["space"],
        json!("index")
    );
}

#[test]
fn the_viewer_reads_the_layout_the_kernel_writes() {
    let playground = Playground::new();
    let groups = keyspace(&playground);
    let books = group(&groups, "rows/books");

    // `0x01 <table id : u32 BE> | <primary key tuple>`, straight out of the
    // module docs in `slate_kernel::keys`. If the layout ever changes, this is
    // the test that says the viewer is now lying rather than the panel quietly
    // showing the wrong prefix.
    let sample = books["samples"][0]["key"].as_str().unwrap();
    assert!(sample.starts_with("01 00000002 | "), "got {sample:?}");

    let index = group(&groups, "index/books.by_author");
    let entry = index["samples"][0]["key"].as_str().unwrap();
    assert!(entry.starts_with("02 0000000a | "), "got {entry:?}");
}

#[test]
fn keys_decode_back_to_the_values_they_encode() {
    let playground = Playground::new();
    let groups = keyspace(&playground);

    let row = group(&groups, "rows/books")["samples"][0]["decoded"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(row.starts_with("books row  id="), "got {row:?}");

    // An index entry carries the indexed value *and* the primary key, which is
    // what lets a covering scan answer without reading a row. The decoded text
    // has to show both or the panel is not making that point.
    let entry = group(&groups, "index/books.by_author")["samples"][0]["decoded"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(entry.contains("by_author = "), "got {entry:?}");
    assert!(entry.contains("->  row "), "got {entry:?}");
}

#[test]
fn a_write_shows_up_in_the_keyspace() {
    let playground = Playground::new();
    let before = keyspace(&playground);
    let rows = group(&before, "rows/books")["keys"].as_u64().unwrap();
    let entries = group(&before, "index/books.by_author")["keys"]
        .as_u64()
        .unwrap();

    let written = playground.insert(
        "books",
        &json!(["9001", "2", "Something", "2024"]).to_string(),
    );
    assert!(written.contains("inserted"), "{written}");

    // The view is of the live store, not a snapshot taken at seed time. Both
    // counts move, which is the atomicity claim made visible: one insert, two
    // keys.
    let after = keyspace(&playground);
    assert_eq!(group(&after, "rows/books")["keys"], json!(rows + 1));
    assert_eq!(
        group(&after, "index/books.by_author")["keys"],
        json!(entries + 1),
        "the index entry was not written with the row"
    );

    let deleted = playground.delete("books", &json!(["9001"]).to_string());
    assert!(deleted.contains("deleted"), "{deleted}");
    let gone = keyspace(&playground);
    assert_eq!(group(&gone, "rows/books")["keys"], json!(rows));
    assert_eq!(
        group(&gone, "index/books.by_author")["keys"],
        json!(entries),
        "the index entry outlived the row it pointed at"
    );
}

#[test]
fn the_byte_columns_are_real_sizes() {
    let playground = Playground::new();
    let groups = keyspace(&playground);
    let books = group(&groups, "rows/books");
    let keys = books["keys"].as_u64().unwrap();
    let key_bytes = books["keyBytes"].as_u64().unwrap();
    let value_bytes = books["valueBytes"].as_u64().unwrap();

    // A row key is the 5-byte header plus an encoded u64 primary key, so it
    // cannot be under six bytes or over about twenty.
    assert!(key_bytes / keys >= 6, "{key_bytes} over {keys} keys");
    assert!(key_bytes / keys <= 24, "{key_bytes} over {keys} keys");
    // And a value holds the whole row, which for books is three more columns
    // including a title.
    assert!(value_bytes > key_bytes, "values should outweigh keys");
}
