//! `#[record(has_many(...))]` and `#[record(belongs_to(...))]`.
//!
//! The attribute is sugar over [`Related`], which
//! `crates/slate-orm/tests/relations.rs` tests by hand. What is left to test
//! here is only what the sugar can get wrong: which ordinal each side resolves
//! to, and what happens when one of them is defaulted.
//!
//! The interesting cases are the ones where the answer is *not* the obvious
//! position — a relationship on the third column of a struct whose fields are
//! in a different order from the other side's. A fixture where every ordinal is
//! 0 or 1 is passed by a macro that emits a constant.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use slate_orm::{
    Action, Catalog, Grant, Ordinal, Record, RecordStore, Records, Related, SecurityCatalog,
    SecurityContext, TableId, Value, load_one_related, load_related, memory::MemoryStore,
};

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "authors", id = 1)]
// The child's key column is named; there is no default for it, because a
// child's primary key is not its foreign key.
#[record(has_many(Book, foreign = author_id))]
struct Author {
    #[record(pk)]
    id: u64,
    name: String,
}

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "books", id = 2)]
// The other direction, with the foreign side defaulted: a book belongs to an
// author by its `author_id`, and what that matches is the author's primary key.
#[record(belongs_to(Author, local = author_id))]
struct Book {
    #[record(pk)]
    id: u64,
    title: String,
    // Deliberately third, and deliberately not where `Author::id` is. A macro
    // that emitted the same ordinal for both sides, or that used the field's
    // position on the wrong struct, passes a fixture where these line up.
    #[record(index(name = "by_author", id = 10))]
    author_id: u64,
}

/// A publisher whose key is composite, to pin what the defaults refuse.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "imprints", id = 3, tenant = "house")]
struct Imprint {
    #[record(pk)]
    house: u64,
    #[record(pk)]
    id: u64,
    name: String,
}

/// A belongs-to onto that composite key, naming its foreign column explicitly.
/// It is the escape hatch the panic message tells you to use, so it is tested.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "editions", id = 4)]
#[record(belongs_to(Imprint, local = imprint_id, foreign = id))]
struct Edition {
    #[record(pk)]
    id: u64,
    imprint_id: u64,
}

#[test]
fn a_has_many_resolves_its_own_key_and_the_childs_named_column() {
    // Local defaults to this struct's primary key: `Author::id`, ordinal 0.
    assert_eq!(<Author as Related<Book>>::local(), Ordinal(0));
    // Foreign is the named field of the *other* struct, where it is third.
    // This is the assertion that fails if the macro resolves a foreign name
    // against the struct the attribute is written on.
    assert_eq!(<Author as Related<Book>>::foreign(), Ordinal(2));
    assert_eq!(Book::COLUMNS.author_id, Ordinal(2));
}

#[test]
fn a_belongs_to_resolves_its_own_named_column_and_the_others_key() {
    assert_eq!(<Book as Related<Author>>::local(), Ordinal(2));
    // Defaulted: the other side's primary key, looked up on `Author`'s table.
    assert_eq!(<Book as Related<Author>>::foreign(), Ordinal(0));
}

#[test]
fn a_named_foreign_column_beats_the_default() {
    assert_eq!(<Edition as Related<Imprint>>::local(), Ordinal(1));
    // `Imprint`'s key is (house, id); the attribute named `id`, which is 1.
    // Without the name this would panic, which the next test pins.
    assert_eq!(<Edition as Related<Imprint>>::foreign(), Ordinal(1));
}

/// The relationship the derive refuses to guess. It compiles — the macro cannot
/// see `Imprint`'s key — and refuses on first use.
///
/// Derived rather than written by hand on purpose. An earlier draft of this
/// test copied what the macro emits, which tests the copy: the macro could stop
/// emitting it, or emit `[first, ..]` instead of `[only]`, and the copy would
/// go on passing.
#[derive(Record, Debug)]
#[record(table = "guesses", id = 5)]
#[record(belongs_to(Imprint, local = imprint_id))]
struct Guess {
    #[record(pk)]
    id: u64,
    imprint_id: u64,
}

#[test]
#[should_panic(expected = "primary key of 2 columns")]
fn defaulting_the_foreign_side_of_a_composite_key_refuses_rather_than_guesses() {
    // Answering with the first column would be a match on a key prefix, which
    // for a tenant-scoped table relates across every tenant. Refusing is the
    // whole point, so the refusal is a test.
    let _ = <Guess as Related<Imprint>>::foreign();
}

/// A single primary key that is *not* the first field, which is what makes the
/// has-many default falsifiable. Everywhere else in this file the key sits at
/// ordinal 0, so a macro that emitted the constant 0 rather than the position
/// of the key would pass.
#[derive(Record, Debug)]
#[record(table = "series", id = 6)]
#[record(has_many(Volume, foreign = series_key))]
struct Series {
    label: String,
    #[record(pk)]
    key: u64,
}

#[derive(Record, Debug)]
#[record(table = "volumes", id = 7)]
struct Volume {
    #[record(pk)]
    id: u64,
    series_key: u64,
}

#[test]
fn the_local_default_is_the_primary_key_wherever_it_sits() {
    assert_eq!(<Series as Related<Volume>>::local(), Ordinal(1));
    assert_eq!(<Series as Related<Volume>>::foreign(), Ordinal(1));
}

#[tokio::test]
async fn a_derived_relationship_loads_like_a_hand_written_one() {
    let catalog =
        Catalog::from_tables([Author::table().clone(), Book::table().clone()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("member", TableId(1), Action::EVERYTHING))
        .grant(Grant::new("member", TableId(2), Action::EVERYTHING));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let ctx = SecurityContext::new(slate_orm::Principal::new(Value::U64(1)).with_role("member"));

    let txn = store.begin().await.unwrap();
    for (id, name) in [(1u64, "Le Guin"), (2, "Borges")] {
        txn.insert_record(
            &ctx,
            &Author {
                id,
                name: name.to_owned(),
            },
        )
        .await
        .unwrap();
    }
    for (id, title, author_id) in [
        (10u64, "A Wizard of Earthsea", 1u64),
        (11, "The Dispossessed", 1),
    ] {
        txn.insert_record(
            &ctx,
            &Book {
                id,
                title: title.to_owned(),
                author_id,
            },
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let authors: Vec<Author> = txn
        .find_records(&ctx, slate_orm::Expr::True, slate_orm::ScanOrder::Ascending)
        .await
        .unwrap();
    let books = load_related::<_, Author, Book>(&txn, &ctx, &authors)
        .await
        .unwrap();
    let by_name: Vec<(&str, usize)> = authors
        .iter()
        .map(|a| a.name.as_str())
        .zip(books.iter().map(Vec::len))
        .collect();
    assert!(by_name.contains(&("Le Guin", 2)), "{by_name:?}");
    assert!(by_name.contains(&("Borges", 0)), "{by_name:?}");

    // And back, through the derived belongs-to.
    let all: Vec<Book> = txn
        .find_records(&ctx, slate_orm::Expr::True, slate_orm::ScanOrder::Ascending)
        .await
        .unwrap();
    let owners = load_one_related::<_, Book, Author>(&txn, &ctx, &all)
        .await
        .unwrap();
    assert_eq!(owners.len(), 2);
    for owner in &owners {
        assert_eq!(owner.as_ref().expect("an author").name, "Le Guin");
    }
}
