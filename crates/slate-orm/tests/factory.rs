//! Generated rows: that they are insertable, reproducible, and refused loudly
//! when they would not be.
//!
//! The properties here are the ones a factory is worthless without. A
//! thousand generated rows that collide on the primary key are a thousand
//! lines of nothing; a fixture that differs between two runs turns a failing
//! test into a ghost; and a row the store rejects for a reason the factory
//! could have named is worse than no factory at all, because the caller now
//! has to bisect a batch.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_orm::{
    Action, Catalog, CheckDef, CmpOp, Expr, Factory, FactoryError, ForeignKeyDef, Grant, Managed,
    Ordinal, OrmError, Principal, Query, RecordStore, SecurityCatalog, SecurityContext, TableDef,
    TableId, Value, ValueType, memory::MemoryStore, seeding_context,
};
// `Value` is `Ord` but not `Hash` — an `f64` variant cannot be both — so the
// distinctness checks here use a `BTreeSet`.
use std::collections::BTreeSet;

const PEOPLE: TableId = TableId(1);
const NOTES: TableId = TableId(2);

/// A table with one column of every shape the factory has a rule for.
fn people() -> TableDef {
    TableDef::builder("people", PEOPLE)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .column("age", ValueType::I64)
        .column("score", ValueType::F64)
        .column("active", ValueType::Bool)
        .column("token", ValueType::Bytes)
        .column("external", ValueType::Uuid)
        .decimal_column("balance", 2)
        .array_column("tags", ValueType::Str)
        .nullable_column("nickname", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("a valid table")
}

fn store(table: TableDef, id: TableId) -> RecordStore<MemoryStore> {
    RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([table]).unwrap(),
        SecurityCatalog::new().grant(Grant::new("app", id, Action::ALL)),
    )
}

fn app() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

/// The whole point, end to end: generate a batch and write it.
///
/// A thousand rather than ten, because the failure this guards is birthday
/// collision on a generated key and ten rows would not find it. The write is
/// the oracle — nothing here asserts what a plausible value looks like, only
/// that the store accepts every one of them, which covers type, nullability,
/// key uniqueness and every unique index at once.
#[tokio::test]
async fn a_thousand_generated_rows_insert() {
    let table = people();
    let rows = Factory::new(&table).seed(7).rows(1_000).unwrap();
    assert_eq!(rows.len(), 1_000);

    let store = store(table.clone(), PEOPLE);
    let txn = store.begin().await.unwrap();
    txn.insert_many(&seeding_context(), &table, &rows)
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let found = txn
        .execute(&app(), &table, &Query::all())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    txn.rollback();
    assert_eq!(found.len(), 1_000, "a generated row went missing");
}

/// A row generated on its own equals the same row generated in a batch.
///
/// This is the property that the value-as-a-function design buys and a
/// streaming PRNG does not, so it is the one worth pinning. It also fails if
/// anything in `draw` starts depending on state carried between rows.
#[tokio::test]
async fn a_row_is_the_same_whether_it_is_generated_alone_or_in_a_batch() {
    let table = people();
    let factory = Factory::new(&table).seed(3);
    let batch = factory.rows(50).unwrap();
    for index in [0u64, 1, 17, 49] {
        assert_eq!(
            factory.row(index).unwrap(),
            batch[index as usize],
            "row {index} differs between `row` and `rows`"
        );
    }
    // And a short run is a prefix of a long one, which is what lets a fixture
    // grow without every existing assertion moving.
    assert_eq!(factory.rows(10).unwrap(), batch[..10]);
}

/// The seed is the only thing that changes the drawn values.
#[tokio::test]
async fn a_different_seed_gives_different_rows_and_the_same_seed_gives_the_same_ones() {
    let table = people();
    let a = Factory::new(&table).seed(1).rows(20).unwrap();
    let b = Factory::new(&table).seed(1).rows(20).unwrap();
    let c = Factory::new(&table).seed(2).rows(20).unwrap();
    assert_eq!(a, b, "the same seed produced different rows");
    assert_ne!(a, c, "two seeds produced the same rows");

    // The key is sequenced rather than drawn, so it is the one column the seed
    // does *not* move — and that is what makes two seeds' batches collide on
    // insert rather than being an independent thousand rows.
    let id = table.ordinal_of("id").unwrap();
    assert_eq!(a[5].get(id), c[5].get(id));
}

/// Two batches of a growing fixture do not collide.
#[tokio::test]
async fn starting_at_continues_the_sequence_rather_than_restarting_it() {
    let table = people();
    let first = Factory::new(&table).rows(100).unwrap();
    let second = Factory::new(&table).starting_at(101).rows(100).unwrap();

    let store = store(table.clone(), PEOPLE);
    for batch in [&first, &second] {
        let txn = store.begin().await.unwrap();
        txn.insert_many(&seeding_context(), &table, batch)
            .await
            .unwrap();
        txn.commit().await.unwrap();
    }

    let id = table.ordinal_of("id").unwrap();
    let keys: BTreeSet<Value> = first
        .iter()
        .chain(&second)
        .map(|row| row.get(id).cloned().unwrap())
        .collect();
    assert_eq!(keys.len(), 200, "the two batches shared a key");
}

/// A unique index is sequenced too, not only the primary key.
///
/// The failure this catches is silent and expensive: the key is unique, the
/// insert of ten rows works, and the batch that matters fails at row two
/// hundred because a drawn `u64` repeated.
#[tokio::test]
async fn a_unique_index_column_gets_its_own_value_in_every_row() {
    let table = TableDef::builder("accounts", PEOPLE)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .column("plan", ValueType::Str)
        .primary_key(["id"])
        .index(
            slate_orm::IndexDef::builder("by_email", slate_orm::IndexId(1))
                .column("email")
                .unique(),
        )
        .build()
        .expect("a valid table");

    let rows = Factory::new(&table).rows(500).unwrap();
    let email = table.ordinal_of("email").unwrap();
    let plan = table.ordinal_of("plan").unwrap();
    let emails: BTreeSet<&Value> = rows.iter().filter_map(|row| row.get(email)).collect();
    assert_eq!(emails.len(), 500, "a unique column repeated");

    // And a column in *no* index is left alone: a fixture whose every value is
    // distinct has nothing to group by, which is the opposite of useful.
    let plans: BTreeSet<&Value> = rows.iter().filter_map(|row| row.get(plan)).collect();
    assert!(
        plans.len() < 500,
        "a non-unique column was sequenced anyway"
    );

    let store = store(table.clone(), PEOPLE);
    let txn = store.begin().await.unwrap();
    txn.insert_many(&seeding_context(), &table, &rows)
        .await
        .unwrap();
    txn.commit().await.unwrap();
}

/// A composite key is distinct as a tuple, not only column by column.
///
/// Every key column is treated as needing its own value per row, which makes
/// the tuple distinct trivially — but "trivially" is how a rule gets changed
/// to something cheaper and nobody notices. A tenant-scoped table is the
/// ordinary shape here and it had no test.
#[tokio::test]
async fn a_composite_key_is_distinct_as_a_whole() {
    // The first key column is a `Str` on purpose. A drawn uuid is unique by
    // accident — 200 of them never collide — so a table keyed on one is
    // distinct whether or not the rule holds, and the test proves nothing. A
    // drawn phrase comes from a vocabulary of 256, so at 200 rows it collides
    // freely and only the sequencing rule keeps the key distinct. Mutation
    // testing is what found this: with a uuid here, sequencing *only* the last
    // key column passed every assertion below.
    let table = TableDef::builder("memberships", PEOPLE)
        .column("region", ValueType::Str)
        .column("person", ValueType::U64)
        .column("role", ValueType::Str)
        .primary_key(["region", "person"])
        .build()
        .expect("a valid table");

    let rows = Factory::new(&table).rows(200).unwrap();
    let region = table.ordinal_of("region").unwrap();
    let person = table.ordinal_of("person").unwrap();
    let keys: BTreeSet<(Value, Value)> = rows
        .iter()
        .map(|row| {
            (
                row.get(region).cloned().unwrap(),
                row.get(person).cloned().unwrap(),
            )
        })
        .collect();
    assert_eq!(keys.len(), 200, "two rows shared a composite key");

    // The realistic shape, and the reason every key column is sequenced rather
    // than just one: pinning the region to five values has to leave the key
    // distinct, and it does, because `person` is sequenced too. A rule that
    // sequenced only one key column would work here only if it happened to
    // pick the column the caller did *not* override.
    let five: Vec<Value> = (0..5).map(|n| Value::Str(format!("r{n}"))).collect();
    let scoped = Factory::new(&table)
        .cycle("region", five)
        .unwrap()
        .rows(200)
        .unwrap();
    let regions: BTreeSet<&Value> = scoped.iter().filter_map(|row| row.get(region)).collect();
    assert_eq!(regions.len(), 5, "the cycled region did not stay pinned");
    let scoped_keys: BTreeSet<(Value, Value)> = scoped
        .iter()
        .map(|row| {
            (
                row.get(region).cloned().unwrap(),
                row.get(person).cloned().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        scoped_keys.len(),
        200,
        "pinning the region collapsed the key"
    );

    // And the same from the other end. Both directions are here because
    // sequencing only *one* key column passes whichever of these two happens
    // to leave that column free — mutation testing found exactly that, once
    // per end.
    let few: Vec<Value> = (0..5).map(Value::U64).collect();
    let by_person = Factory::new(&table)
        .cycle("person", few)
        .unwrap()
        .rows(200)
        .unwrap();
    let people: BTreeSet<&Value> = by_person.iter().filter_map(|row| row.get(person)).collect();
    assert_eq!(people.len(), 5, "the cycled person did not stay pinned");
    let by_person_keys: BTreeSet<(Value, Value)> = by_person
        .iter()
        .map(|row| {
            (
                row.get(region).cloned().unwrap(),
                row.get(person).cloned().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        by_person_keys.len(),
        200,
        "pinning the person collapsed the key"
    );

    // The store is the oracle, as everywhere else here.
    let store = store(table.clone(), PEOPLE);
    let txn = store.begin().await.unwrap();
    txn.insert_many(&seeding_context(), &table, &rows)
        .await
        .unwrap();
    txn.commit().await.unwrap();
}

/// The soft-delete column is left null, so a seeded table is not empty.
///
/// The trap the module docs call the worst one available: the column is
/// nullable, the ordinary rule fills nullable columns, and a filled
/// soft-delete column means every generated row arrives already retired and
/// the whole fixture reads back as nothing.
#[tokio::test]
async fn a_seeded_table_with_a_soft_delete_column_reads_back_full() {
    let table = TableDef::builder("posts", NOTES)
        .column("id", ValueType::U64)
        .column("title", ValueType::Str)
        .nullable_column("deleted_at", ValueType::I64)
        .primary_key(["id"])
        .soft_delete("deleted_at")
        .build()
        .expect("a valid table");

    let rows = Factory::new(&table).rows(25).unwrap();
    let deleted_at = table.ordinal_of("deleted_at").unwrap();
    assert!(
        rows.iter()
            .all(|row| row.get(deleted_at) == Some(&Value::Null)),
        "a generated row arrived already soft-deleted"
    );

    let store = store(table.clone(), NOTES);
    let txn = store.begin().await.unwrap();
    txn.insert_many(&seeding_context(), &table, &rows)
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let found = txn
        .execute(&app(), &table, &Query::all())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    txn.rollback();
    assert_eq!(found.len(), 25, "the seed produced rows nobody can see");
}

/// `cycle` is the foreign-key tool, and the children point at real parents.
#[tokio::test]
async fn children_generated_with_cycle_reference_parents_that_exist() {
    let parents = people();
    let children = TableDef::builder("notes", NOTES)
        .column("id", ValueType::U64)
        .column("person_id", ValueType::U64)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .foreign_key(
            // The referencing columns are given in the parent's primary-key
            // order; the parent's own columns are not named.
            ForeignKeyDef::builder("notes_person", PEOPLE).column("person_id"),
        )
        .build()
        .expect("a valid table");

    let parent_rows = Factory::new(&parents).rows(10).unwrap();
    let id = parents.ordinal_of("id").unwrap();
    let keys: Vec<Value> = parent_rows
        .iter()
        .map(|row| row.get(id).cloned().unwrap())
        .collect();

    let child_rows = Factory::new(&children)
        .cycle("person_id", keys.clone())
        .unwrap()
        .rows(37)
        .unwrap();

    let store = RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([parents.clone(), children.clone()]).unwrap(),
        SecurityCatalog::new()
            .grant(Grant::new("app", PEOPLE, Action::ALL))
            .grant(Grant::new("app", NOTES, Action::ALL)),
    );
    let txn = store.begin().await.unwrap();
    txn.insert_many(&seeding_context(), &parents, &parent_rows)
        .await
        .unwrap();
    // The foreign key is the oracle: a `person_id` the factory invented rather
    // than cycled would be refused here, and nothing in this test has to know
    // what a valid parent key looks like.
    txn.insert_many(&seeding_context(), &children, &child_rows)
        .await
        .unwrap();
    txn.commit().await.unwrap();

    // Every parent is referenced, which is what cycling buys over drawing: a
    // fixture where nine of ten parents have no children tests very little.
    let person_id = children.ordinal_of("person_id").unwrap();
    let referenced: BTreeSet<&Value> = child_rows.iter().filter_map(|r| r.get(person_id)).collect();
    assert_eq!(referenced.len(), 10);
}

/// A `CHECK` the factory cannot satisfy is named at generation time.
#[test]
fn a_check_the_generated_rows_fail_names_the_check_and_the_column() {
    let table = TableDef::builder("people", PEOPLE)
        .column("id", ValueType::U64)
        .column("age", ValueType::I64)
        .primary_key(["id"])
        .check(
            // Nothing the factory draws satisfies this, which is the point:
            // the check is a predicate and the factory cannot read one.
            CheckDef::new(
                "age_is_impossible",
                Expr::compare(Ordinal(1), CmpOp::Eq, Value::I64(-1)),
            )
            .with_column("age"),
        )
        .build()
        .expect("a valid table");

    let error = Factory::new(&table).rows(1).unwrap_err();
    let OrmError::Factory(FactoryError::CheckViolation { check, column, .. }) = &error else {
        panic!("expected a check violation, got {error:?}");
    };
    assert_eq!(check, "age_is_impossible");
    assert_eq!(column.as_deref(), Some("age"));
    // The message has to say what to do about it, because the caller's next
    // move is a `set` on exactly that column.
    let text = error.to_string();
    assert!(text.contains("age"), "{text}");
    assert!(text.contains("set"), "{text}");

    // And setting the column is the fix, which is the claim the message makes.
    let fixed = Factory::new(&table)
        .set("age", Value::I64(-1))
        .unwrap()
        .rows(5)
        .unwrap();
    assert_eq!(fixed.len(), 5);
}

/// A vector column is refused rather than given a made-up dimension.
#[test]
fn a_vector_column_is_refused_and_says_so() {
    let table = TableDef::builder("embeddings", PEOPLE)
        .column("id", ValueType::U64)
        .column("embedding", ValueType::Vector)
        .primary_key(["id"])
        .build()
        .expect("a valid table");

    let error = Factory::new(&table).rows(1).unwrap_err();
    let OrmError::Factory(FactoryError::CannotGenerate { ty, reason, .. }) = &error else {
        panic!("expected a refusal, got {error:?}");
    };
    assert_eq!(*ty, ValueType::Vector);
    // The *reason* matters and is asserted, not just the refusal. Falling
    // through to the `#[non_exhaustive]` wildcard would also refuse a vector,
    // with an identical outcome and a message saying the factory has a hole in
    // it — which would send a reader looking for a bug here instead of
    // reaching for `set`. Mutation testing found exactly that: with the
    // vector arm disabled the wildcard caught it and no test noticed.
    assert_eq!(*reason, slate_orm::factory::NO_DIMENSION);
    assert!(error.to_string().contains("embedding"), "{error}");
    assert!(error.to_string().contains("dimension"), "{error}");

    // `set` is the documented way through, and it works.
    let rows = Factory::new(&table)
        .set("embedding", Value::Vector(vec![0.1, 0.2, 0.3]))
        .unwrap()
        .rows(3)
        .unwrap();
    assert_eq!(rows.len(), 3);
}

/// A managed column is a placeholder on the way in and the clock on the way out.
#[tokio::test]
async fn a_managed_column_is_written_by_the_store_not_the_factory() {
    let table = TableDef::builder("people", PEOPLE)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .column("created_at", ValueType::I64)
        .primary_key(["id"])
        .managed_for("created_at", Managed::CreatedAt)
        .build()
        .expect("a valid table");

    let rows = Factory::new(&table).rows(5).unwrap();
    let created_at = table.ordinal_of("created_at").unwrap();
    assert!(
        rows.iter()
            .all(|row| row.get(created_at) == Some(&Value::I64(0))),
        "the factory invented a timestamp the store was going to overwrite"
    );

    let store = store(table.clone(), PEOPLE);
    let txn = store.begin().await.unwrap();
    txn.insert_many(&seeding_context(), &table, &rows)
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let found = txn
        .execute(&app(), &table, &Query::all())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    txn.rollback();
    assert!(
        found
            .iter()
            .all(|row| row.get(created_at) != Some(&Value::I64(0))),
        "the placeholder survived the write"
    );
}

/// A column's `DEFAULT` is the schema's own answer and beats a draw.
#[test]
fn a_default_is_used_where_there_is_one_and_ignored_on_a_key() {
    let table = TableDef::builder("people", PEOPLE)
        .column("id", ValueType::U64)
        .column("plan", ValueType::Str)
        .primary_key(["id"])
        .default_for("plan", Value::Str("free".into()))
        .default_for("id", Value::U64(9))
        .build()
        .expect("a valid table");

    let rows = Factory::new(&table).rows(4).unwrap();
    let plan = table.ordinal_of("plan").unwrap();
    let id = table.ordinal_of("id").unwrap();
    assert!(
        rows.iter()
            .all(|row| row.get(plan) == Some(&Value::Str("free".into()))),
        "the default was drawn over"
    );
    // A default on a key column cannot serve four rows, so it is deliberately
    // ignored: honouring it would make every batch of more than one collide.
    let keys: BTreeSet<&Value> = rows.iter().filter_map(|row| row.get(id)).collect();
    assert_eq!(
        keys.len(),
        4,
        "a default on the key was honoured and collided"
    );
}

/// A nullable column is filled unless asked, and asking on a non-nullable one
/// is refused rather than quietly ignored.
#[test]
fn null_for_is_asked_for_and_is_refused_where_it_cannot_apply() {
    let table = people();
    let nickname = table.ordinal_of("nickname").unwrap();

    let filled = Factory::new(&table).rows(5).unwrap();
    assert!(
        filled
            .iter()
            .all(|row| row.get(nickname) != Some(&Value::Null)),
        "a nullable column came back null without being asked"
    );

    let nulled = Factory::new(&table)
        .null_for("nickname")
        .unwrap()
        .rows(5)
        .unwrap();
    assert!(
        nulled
            .iter()
            .all(|row| row.get(nickname) == Some(&Value::Null))
    );

    let error = Factory::new(&table).null_for("name").unwrap_err();
    assert!(
        matches!(
            error,
            OrmError::Factory(FactoryError::NullForNonNullable { .. })
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("name"), "{error}");
}

/// The two mistakes a caller makes at the keyboard, both named.
#[test]
fn a_misspelled_column_and_an_empty_cycle_are_both_refused() {
    let table = people();

    let error = Factory::new(&table).set("nmae", Value::Null).unwrap_err();
    assert!(
        matches!(error, OrmError::Factory(FactoryError::NoSuchColumn { .. })),
        "{error:?}"
    );
    assert!(error.to_string().contains("nmae"), "{error}");

    // An empty list is what a caller passes when the parent query came back
    // empty. Treating it as "no override" would generate unrelated values and
    // produce a fixture whose foreign keys all dangle.
    let error = Factory::new(&table).cycle("name", vec![]).unwrap_err();
    assert!(
        matches!(error, OrmError::Factory(FactoryError::EmptyCycle { .. })),
        "{error:?}"
    );
}

/// A bool in the key is refused, because two rows exhaust it.
#[test]
fn a_bool_primary_key_is_refused_rather_than_silently_colliding() {
    let table = TableDef::builder("flags", PEOPLE)
        .column("on", ValueType::Bool)
        .column("label", ValueType::Str)
        .primary_key(["on"])
        .build()
        .expect("a valid table");

    let error = Factory::new(&table).rows(3).unwrap_err();
    assert!(
        matches!(
            error,
            OrmError::Factory(FactoryError::CannotSequence { .. })
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("on"), "{error}");
}

/// Every `ValueType` is either generated or refused by name — no silent hole.
///
/// `ValueType` is `#[non_exhaustive]` and the factory is a downstream crate,
/// so the match in `draw` needs a wildcard and the compiler cannot tell anyone
/// that a new type fell into it. This is the roster that can: it drives every
/// type through a real column and requires the outcome to be a row or a
/// refusal naming that type. A new variant fails here — a named test — rather
/// than producing a `CannotGenerate` from inside somebody's seed six months
/// later.
///
/// `REFUSED` is the `EXPECTED_REFUSALS` idiom this repository uses elsewhere:
/// a list you are forced to edit is a list that stays true. Adding a type to
/// it is a deliberate act with a reason beside it.
#[test]
fn every_value_type_is_generated_or_refused() {
    /// Types the factory will not invent a value for, and why.
    const REFUSED: [(ValueType, &str); 1] = [(
        ValueType::Vector,
        "a vector's dimension is not in the schema, so a generated one would \
         be whatever width the factory picked",
    )];

    for ty in ValueType::ALL {
        let mut builder = TableDef::builder("probe", PEOPLE).column("id", ValueType::U64);
        builder = match ty {
            ValueType::Decimal => builder.decimal_column("value", 2),
            ValueType::Array => builder.array_column("value", ValueType::Str),
            other => builder.column("value", other),
        };
        let table = builder
            .primary_key(["id"])
            .build()
            .unwrap_or_else(|e| panic!("{} is not a column type: {e}", ty.name()));

        let outcome = Factory::new(&table).rows(3);
        let expected = REFUSED.iter().find(|(refused, _)| *refused == ty);
        match (outcome, expected) {
            (Ok(rows), None) => {
                let value = table.ordinal_of("value").unwrap();
                assert!(
                    rows.iter().all(|row| row.get(value).is_some()),
                    "{} generated a row with no value in it",
                    ty.name()
                );
            }
            (Ok(_), Some((_, reason))) => panic!(
                "{} is listed as refused — {reason} — but the factory generated it. \
                 If that is now right, take it out of REFUSED.",
                ty.name()
            ),
            (Err(error), Some(_)) => {
                let OrmError::Factory(FactoryError::CannotGenerate { reason, .. }) = &error else {
                    panic!(
                        "{} is refused but not as CannotGenerate: {error:?}",
                        ty.name()
                    );
                };
                // A listed refusal is a decision and must carry the decision's
                // own reason. `NO_GENERATOR` here would mean the type reached
                // the wildcard and only looks refused on purpose.
                assert_ne!(
                    *reason,
                    slate_orm::factory::NO_GENERATOR,
                    "{} is in REFUSED but fell through to the wildcard, so the \
                     refusal is an accident rather than the decision it claims",
                    ty.name()
                );
            }
            (Err(error), None) => panic!(
                "{} has no generator and is not in REFUSED, so a caller meets \
                 this from inside their own seed: {error}",
                ty.name()
            ),
        }
    }
}
