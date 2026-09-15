//! A serialized document in a `Str` column, and the one hazard that comes with
//! it.
//!
//! Run with `cargo test -p slate-orm --features json`, which CI does — a
//! feature nothing builds is a feature nobody has debugged.

#![cfg(feature = "json")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde::{Deserialize, Serialize};
use slate_kernel::memory::MemoryStore;
use slate_orm::{
    Action, Expr, Field, FieldError, Grant, Json, Query, Record, RecordStore, Records,
    SecurityCatalog, SecurityContext, Value, ValueType,
};
use slate_schema::{Catalog, TableId};
use std::collections::{BTreeMap, HashMap};

const PROFILES: TableId = TableId(1);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct Settings {
    theme: String,
    rows: u32,
    tags: Vec<String>,
}

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "profiles", id = 1)]
struct Profile {
    #[record(pk)]
    id: u64,
    settings: Json<Settings>,
}

fn store() -> (RecordStore<MemoryStore>, SecurityContext) {
    let catalog = Catalog::from_tables([Profile::table().clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", PROFILES, Action::EVERYTHING));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    (store, SecurityContext::superuser())
}

fn settings() -> Settings {
    Settings {
        theme: "dark".to_owned(),
        rows: 50,
        tags: vec!["a".to_owned(), "b".to_owned()],
    }
}

#[test]
fn a_json_column_is_a_string_column() {
    let table = Profile::table();
    assert_eq!(
        table.column(Profile::COLUMNS.settings).unwrap().value_type(),
        ValueType::Str
    );
}

#[tokio::test]
async fn a_document_round_trips_through_the_store() {
    let (store, ctx) = store();
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &ctx,
        &Profile {
            id: 1,
            settings: Json::new(settings()).unwrap(),
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let read: Profile = txn
        .get_record(&ctx, &[Value::U64(1)])
        .await
        .unwrap()
        .expect("the row");
    txn.rollback();
    assert_eq!(read.settings.get(), &settings());
}

/// A row read back and written again stores the same text.
///
/// The `encoded` string has to survive `from_value`, and a mutation proved
/// nothing was checking it: blanking it there left every other test green,
/// because they all construct with `Json::new` and never write back what they
/// read. A `Json` that forgot its encoding would store an empty string on the
/// second write — a silent, total data loss on exactly the read-modify-write
/// path an ORM exists for.
#[tokio::test]
async fn a_row_read_and_written_back_stores_the_same_text() {
    let (store, ctx) = store();
    let original = Json::new(settings()).unwrap();
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &ctx,
        &Profile {
            id: 1,
            settings: original.clone(),
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    // Read, change nothing, write back.
    let txn = store.begin().await.unwrap();
    let read: Profile = txn
        .get_record(&ctx, &[Value::U64(1)])
        .await
        .unwrap()
        .expect("the row");
    assert_eq!(
        read.settings.as_json(),
        original.as_json(),
        "the encoding did not survive the read"
    );
    txn.update_record(&ctx, &read).await.unwrap();
    txn.commit().await.unwrap();

    // And the stored value is still the document, not an empty string.
    let txn = store.begin().await.unwrap();
    let again: Profile = txn
        .get_record(&ctx, &[Value::U64(1)])
        .await
        .unwrap()
        .expect("the row");
    txn.rollback();
    assert_eq!(again.settings.get(), &settings());
    assert_eq!(again.settings.to_value(), original.to_value());
}

/// The stored text is what a predicate compares, and nothing knows it is JSON.
///
/// Asserted rather than described, because "you cannot query inside it" is the
/// kind of limitation a reader assumes is a documentation hedge.
#[tokio::test]
async fn a_predicate_compares_the_whole_serialized_string() {
    let (store, ctx) = store();
    let stored = Json::new(settings()).unwrap();
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &ctx,
        &Profile {
            id: 1,
            settings: stored.clone(),
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    // The whole document, byte for byte: this matches.
    let hit: Vec<Profile> = txn
        .query_records(
            &ctx,
            &Query::all().filter(Expr::eq(
                Profile::COLUMNS.settings,
                Value::Str(stored.as_json().into()),
            )),
        )
        .await
        .unwrap();
    assert_eq!(hit.len(), 1, "{hit:?}");

    // A field inside it: no. There is no path expression, and the closest
    // thing — comparing against the field's own JSON — matches nothing,
    // because the column holds the whole object.
    let miss: Vec<Profile> = txn
        .query_records(
            &ctx,
            &Query::all().filter(Expr::eq(
                Profile::COLUMNS.settings,
                Value::Str(r#""dark""#.into()),
            )),
        )
        .await
        .unwrap();
    txn.rollback();
    assert!(miss.is_empty(), "{miss:?}");
}

/// Serialization failure happens at construction, where it can be handled.
#[test]
fn a_document_that_cannot_be_json_fails_at_construction_not_at_write_time() {
    // A map with a non-string key: JSON objects are keyed by strings, so
    // `serde_json` refuses this rather than inventing a spelling.
    let mut awkward: BTreeMap<(u8, u8), u8> = BTreeMap::new();
    awkward.insert((1, 2), 3);
    let error = Json::new(awkward).unwrap_err();
    assert!(!error.to_string().is_empty());
}

#[test]
fn stored_text_that_is_not_this_document_is_an_error() {
    let error = <Json<Settings> as Field>::from_value(&Value::Str("{}".into())).unwrap_err();
    match error {
        FieldError::OutOfRange { target } => assert!(target.contains("Json"), "{target}"),
        other => panic!("{other:?}"),
    }

    let error = <Json<Settings> as Field>::from_value(&Value::U64(1)).unwrap_err();
    assert!(
        matches!(error, FieldError::TypeMismatch { .. }),
        "{error:?}"
    );
}

/// A struct's encoding is stable, so two equal documents are one string.
///
/// This is what equality and a unique index depend on, and it is the half that
/// holds.
#[test]
fn a_struct_encodes_the_same_way_every_time() {
    let one = Json::new(settings()).unwrap();
    let two = Json::new(settings()).unwrap();
    assert_eq!(one.as_json(), two.as_json());
    assert_eq!(one.to_value(), two.to_value());
    // And declaration order, not alphabetical — which is what makes it
    // predictable rather than merely repeatable.
    assert_eq!(
        one.as_json(),
        r#"{"theme":"dark","rows":50,"tags":["a","b"]}"#
    );
}

/// And the half that does not: a `HashMap` has no stable encoding.
///
/// Two maps a caller considers equal store as two different strings, so a
/// unique index over the column would admit both and an `Expr::eq` against one
/// would miss the other. Demonstrated rather than warned about.
///
/// The first draft of this test asserted the encodings all *agree* within one
/// process, on the belief that `RandomState`'s seed is per process. It is not:
/// each `HashMap` takes a fresh seed from a thread-local counter, so sixteen
/// maps built from identical pairs in one function produced several different
/// strings. The wrong version failed, which is the only reason this comment
/// exists — a hazard reasoned about rather than run would have shipped the
/// weaker claim.
///
/// Deliberately *not* fixed by sorting keys inside `Json`: that would silently
/// change the encoding of every other type and buy consistency for one
/// container at the cost of surprise everywhere else. `BTreeMap` is the fix,
/// and the first half of this test is what makes that a recommendation rather
/// than an assertion.
#[test]
fn json_hash_maps_do_not_have_a_stable_encoding() {
    let pairs = [("a", 1), ("b", 2), ("c", 3), ("d", 4), ("e", 5), ("f", 6)];

    let ordered: BTreeMap<&str, i32> = pairs.iter().copied().collect();
    let first = Json::new(ordered.clone()).unwrap();
    let second = Json::new(ordered).unwrap();
    assert_eq!(
        first.as_json(),
        second.as_json(),
        "a BTreeMap is ordered by key, so its encoding is stable"
    );

    // Enough separately built maps that seeing only one encoding would be a
    // claim about the hasher rather than luck.
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..16 {
        let map: HashMap<&str, i32> = pairs.iter().copied().collect();
        seen.insert(Json::new(map).unwrap().as_json().to_owned());
    }
    assert!(
        seen.len() > 1,
        "sixteen identical HashMaps all encoded the same way: {seen:?}"
    );
    // Every one of them is the same document, which is the point — the
    // difference is in the text and nowhere else, so nothing downstream can
    // notice and correct for it.
    for encoding in &seen {
        let back: BTreeMap<String, i32> = serde_json::from_str(encoding).unwrap();
        let want: BTreeMap<String, i32> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect();
        assert_eq!(back, want, "{encoding}");
    }
}
