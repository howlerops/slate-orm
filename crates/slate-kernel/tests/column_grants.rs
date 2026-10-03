//! Column-level grants: a role that may read only some of a table's columns.
//!
//! `docs/column-grants.md` is the design, and its §4 is a map of every path a
//! row leaves by. The tests here are in three groups:
//!
//! 1. **The sentinel oracle.** Every hidden column holds a value that occurs
//!    nowhere else in the fixture. Every path in the battery is run as the
//!    narrowed role, and everything that comes back — rows, groups, join rows,
//!    and the text of every error — is scanned for the sentinels. A hit is a
//!    leak, whatever path produced it, which is what makes this stronger than
//!    one assertion per path: a path the map forgot still has to get its
//!    output past the scan.
//! 2. **The refusals.** A reference to a hidden column anywhere in a query,
//!    and the whole-row operations, refuse with `ColumnsWithheld`.
//! 3. **Integrity.** The predicate writes read whole rows, so a narrowed
//!    caller's `update_where` must not null the columns it cannot see, and its
//!    `delete_where` must not leave index entries behind for them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::window::{Window, WindowFunction};
use slate_kernel::{
    AccessHint, Action, Aggregate, CmpOp, Expr, Grant, Grouping, Join, JoinAlgorithm, KernelError,
    Policy, Principal, Projection, Query, RecordStore, Scalar, SecurityCatalog, SecurityContext,
    Side, SortKey, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const STAFF: TableId = TableId(1);
const TEAMS: TableId = TableId(2);
const BY_SALARY: IndexId = IndexId(10);

/// Values that occur only in hidden columns.
const SALARY: u64 = 987_654_321;
const OWNER: u64 = 424_242_424;
const SECRET: &str = "SENTINEL-SECRET";
const SENTINELS: [&str; 3] = ["987654321", "424242424", SECRET];

fn staff() -> TableDef {
    TableDef::builder("staff", STAFF)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .column("salary", ValueType::U64)
        .column("owner", ValueType::U64)
        .column("team", ValueType::I64)
        .primary_key(["id"])
        // An index on the hidden column, so a predicate write that erased
        // entries from a nulled row would leave this one dangling.
        .index(IndexDef::builder("by_salary", BY_SALARY).column("salary"))
        .build()
        .expect("valid schema")
}

fn teams() -> TableDef {
    TableDef::builder("teams", TEAMS)
        .column("id", ValueType::I64)
        .column("label", ValueType::Str)
        .column("secret", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn s(name: &str) -> Ordinal {
    staff().ordinal_of(name).expect("column")
}

fn t(name: &str) -> Ordinal {
    teams().ordinal_of(name).expect("column")
}

/// `analyst` reads `id, name, team` of staff and `id, label` of teams;
/// `editor` additionally writes staff at the table level; `auditor` reads
/// staff whole. The row policy filters on `owner`, which `analyst` cannot
/// read — the case §3 of the design is about.
fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::read_columns("analyst", &staff(), [s("id"), s("name"), s("team")]).unwrap())
        .grant(Grant::read_columns("analyst", &teams(), [t("id"), t("label")]).unwrap())
        .grant(Grant::new(
            "editor",
            STAFF,
            [Action::Insert, Action::Update, Action::Delete],
        ))
        .grant(Grant::new(
            "auditor",
            STAFF,
            [Action::Read, Action::Explain],
        ))
        .grant(Grant::new("explainer", STAFF, [Action::Explain]))
        .policy(Policy::new(
            "own",
            STAFF,
            Action::ALL,
            |c: &SecurityContext| Expr::eq(s("owner"), c.principal().id.clone()),
        ))
}

fn as_roles(roles: &[&str]) -> SecurityContext {
    let mut principal = Principal::new(Value::U64(OWNER));
    for role in roles {
        principal = principal.with_role(*role);
    }
    SecurityContext::new(principal)
}

fn analyst() -> SecurityContext {
    as_roles(&["analyst"])
}

fn person(id: u64, name: &str, team: i64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(name.to_owned()),
        Value::U64(SALARY),
        Value::U64(OWNER),
        Value::I64(team),
    ])
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([staff(), teams()]).unwrap();
    let store = RecordStore::new(MemoryStore::new(), catalog, security());
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    txn.insert_many(
        &root,
        &staff(),
        &[person(1, "ann", 1), person(2, "bob", 1), person(3, "cy", 2)],
    )
    .await
    .unwrap();
    txn.insert_many(
        &root,
        &teams(),
        &[
            Row::new(vec![
                Value::I64(1),
                Value::Str("red".into()),
                Value::Str(SECRET.into()),
            ]),
            Row::new(vec![
                Value::I64(2),
                Value::Str("blue".into()),
                Value::Str(SECRET.into()),
            ]),
        ],
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    store
}

fn leaks(what: &str, output: &str) -> Vec<String> {
    SENTINELS
        .iter()
        .filter(|sentinel| output.contains(*sentinel))
        .map(|sentinel| format!("{what}: `{sentinel}` in {output}"))
        .collect()
}

/// Every path the narrowed role can take, and everything each one returns,
/// scanned for a hidden value.
#[tokio::test]
async fn no_hidden_value_reaches_a_narrowed_reader_by_any_path() {
    let store = seeded().await;
    let me = analyst();
    let txn = store.begin().await.unwrap();
    let mut out: Vec<(&str, String)> = Vec::new();

    let rows = |q: Query| {
        let txn = &txn;
        let me = me.clone();
        async move {
            match txn.execute(&me, &staff(), &q).await {
                Ok(cursor) => format!("{:?}", cursor.collect().await),
                Err(error) => format!("{error:?} {error}"),
            }
        }
    };

    out.push(("every column", rows(Query::all()).await));
    out.push((
        "a filter on a readable column",
        rows(Query::all().filter(Expr::eq(s("name"), Value::Str("ann".into())))).await,
    ));
    out.push((
        "a sort on a readable column",
        rows(Query::all().sort_by([SortKey::desc(s("name"))]).limit(2)).await,
    ));
    out.push((
        "a named projection",
        rows(Query {
            projection: Projection::Columns(vec![s("id"), s("name")]),
            ..Query::all()
        })
        .await,
    ));
    out.push((
        "a computed value",
        rows(Query::all().computing([Scalar::column(s("team")) * 2i64])).await,
    ));
    out.push((
        "a window",
        rows(
            Query::all().windowing([Window::new(
                WindowFunction::RowNumber,
                vec![s("team")],
                vec![SortKey::asc(s("id"))],
            )
            .unwrap()]),
        )
        .await,
    ));
    out.push((
        "an IN over the key",
        rows(Query::all().filter(Expr::In {
            column: s("id"),
            values: (1..=3).map(Value::U64).collect(),
        }))
        .await,
    ));
    out.push((
        "a hint onto the hidden column's index",
        rows(Query {
            hint: Some(AccessHint::Index(BY_SALARY)),
            ..Query::all()
        })
        .await,
    ));
    out.push((
        "paging",
        rows(Query {
            paging: true,
            ..Query::all().limit(2)
        })
        .await,
    ));

    out.push((
        "get",
        format!("{:?}", txn.get(&me, &staff(), &[Value::U64(1)]).await),
    ));
    out.push((
        "count",
        format!(
            "{:?}",
            txn.aggregate(
                &me,
                &staff(),
                &Query::all(),
                &[Aggregate::Count, Aggregate::Sum(s("team"))]
            )
            .await
        ),
    ));
    out.push((
        "grouped",
        format!(
            "{:?}",
            txn.grouped(
                &me,
                &staff(),
                &Query::all(),
                &Grouping::by([s("team")], &[Aggregate::Count])
            )
            .await
        ),
    ));
    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::NestedLoop,
    ] {
        let join = Join::equating(s("team"), t("id")).using(algorithm);
        let joined = match txn.join(&me, &staff(), &teams(), &join).await {
            Ok(cursor) => format!("{:?}", cursor.collect().await),
            Err(error) => format!("{error:?} {error}"),
        };
        out.push(("a join", joined));
    }

    // And every refusal's text, since an error is a path out too.
    for (what, q) in refusals() {
        let _ = what;
        out.push(("a refused query", rows(q).await));
    }
    out.push((
        "explain",
        format!("{:?}", txn.explain(&me, &staff(), &Query::all())),
    ));
    out.push(("analyze", format!("{:?}", txn.analyze(&me, &staff()).await)));

    let found: Vec<String> = out
        .iter()
        .flat_map(|(what, text)| leaks(what, text))
        .collect();
    assert!(
        found.is_empty(),
        "hidden values reached the caller:\n{}",
        found.join("\n")
    );

    // The oracle is only worth something if the sentinels are really in the
    // store and really reachable: the same battery's first path, as a reader
    // of the whole table, must find them.
    let whole = txn
        .execute(&as_roles(&["auditor"]), &staff(), &Query::all())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(
        !leaks("auditor", &format!("{whole:?}")).is_empty(),
        "the sentinels are not in the store, so the scan above proved nothing"
    );
}

/// Every way a query can *reference* a column, pointed at a hidden one.
fn refusals() -> Vec<(&'static str, Query)> {
    vec![
        (
            "a filter",
            Query::all().filter(Expr::compare(s("salary"), CmpOp::Gt, Value::U64(1))),
        ),
        (
            "a projection",
            Query {
                projection: Projection::Columns(vec![s("id"), s("salary")]),
                ..Query::all()
            },
        ),
        ("a sort", Query::all().sort_by([SortKey::asc(s("salary"))])),
        (
            "a computed value",
            Query::all().computing([Scalar::column(s("owner"))]),
        ),
        (
            "a window's partition",
            Query::all().windowing([Window::new(
                WindowFunction::RowNumber,
                vec![s("salary")],
                vec![SortKey::asc(s("id"))],
            )
            .unwrap()]),
        ),
        (
            "a window's order",
            Query::all().windowing([Window::new(
                WindowFunction::RowNumber,
                vec![],
                vec![SortKey::asc(s("salary"))],
            )
            .unwrap()]),
        ),
    ]
}

#[tokio::test]
async fn a_reference_to_a_hidden_column_is_refused_wherever_it_is() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    for (what, query) in refusals() {
        let refused = txn.execute(&analyst(), &staff(), &query).await.err();
        assert!(
            matches!(refused, Some(KernelError::ColumnsWithheld { .. })),
            "{what} on a hidden column was not refused: {refused:?}"
        );
    }
    // Aggregating and grouping reach the same check through the projection
    // `narrowed` builds, and a join key through the probe's filter.
    let summed = txn
        .aggregate(
            &analyst(),
            &staff(),
            &Query::all(),
            &[Aggregate::Sum(s("salary"))],
        )
        .await;
    assert!(
        matches!(summed, Err(KernelError::ColumnsWithheld { .. })),
        "{summed:?}"
    );
    let grouped = txn
        .grouped(
            &analyst(),
            &staff(),
            &Query::all(),
            &Grouping::by([s("salary")], &[Aggregate::Count]),
        )
        .await;
    assert!(
        matches!(grouped, Err(KernelError::ColumnsWithheld { .. })),
        "{grouped:?}"
    );
    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::NestedLoop,
    ] {
        let join = Join::equating(s("team"), t("secret")).using(algorithm);
        let joined = txn.join(&analyst(), &staff(), &teams(), &join).await.err();
        assert!(
            matches!(joined, Some(KernelError::ColumnsWithheld { .. })),
            "{algorithm:?}: {joined:?}"
        );
    }
}

/// "Every column" is narrowed, and what the policy read to filter is withheld.
#[tokio::test]
async fn every_column_means_every_readable_column_and_the_policys_are_withheld() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(&analyst(), &staff(), &Query::all())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        3,
        "the policy admits all three: they are the caller's"
    );
    for row in &rows {
        assert_eq!(row.get(s("salary")), Some(&Value::Null));
        // The policy filtered on `owner`, so it was decoded; it is not handed out.
        assert_eq!(row.get(s("owner")), Some(&Value::Null));
        assert!(matches!(row.get(s("name")), Some(Value::Str(_))));
    }
    let got = txn
        .get(&analyst(), &staff(), &[Value::U64(1)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.get(s("salary")), Some(&Value::Null));
    assert_eq!(got.get(s("name")), Some(&Value::Str("ann".into())));
}

#[tokio::test]
async fn the_operations_that_need_every_column_are_refused() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let me = as_roles(&["analyst", "editor", "explainer"]);
    let row = person(1, "ann", 9);
    let withheld = |result: Result<(), KernelError>, what: &str| {
        assert!(
            matches!(result, Err(KernelError::ColumnsWithheld { .. })),
            "{what}: {result:?}"
        );
    };
    withheld(txn.update(&me, &staff(), &row).await, "update");
    withheld(txn.upsert(&me, &staff(), &row).await, "upsert");
    withheld(
        txn.update_if_unchanged(&me, &staff(), &row, &person(1, "ann", 1))
            .await,
        "update_if_unchanged",
    );
    withheld(
        txn.delete_if_unchanged(&me, &staff(), &[Value::U64(1)], &person(1, "ann", 1))
            .await,
        "delete_if_unchanged",
    );
    withheld(
        txn.explain(&me, &staff(), &Query::all()).map(|_| ()),
        "explain",
    );
    withheld(txn.analyze(&me, &staff()).await.map(|_| ()), "analyze");
    withheld(
        txn.update_many(&me, &staff(), std::slice::from_ref(&row))
            .await,
        "update_many",
    );
    withheld(
        txn.upsert_many(&me, &staff(), std::slice::from_ref(&row))
            .await,
        "upsert_many",
    );
    // What does not need every column is not refused.
    assert!(txn.insert(&me, &staff(), &person(4, "di", 1)).await.is_ok());
    assert!(txn.delete(&me, &staff(), &[Value::U64(4)]).await.unwrap());
}

#[tokio::test]
async fn a_role_with_no_grant_is_still_told_it_has_none() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let nobody = as_roles(&["nobody"]);
    let refused = txn.execute(&nobody, &staff(), &Query::all()).await.err();
    assert!(
        matches!(refused, Some(KernelError::AccessDenied { .. })),
        "{refused:?}"
    );
    // `analyze` asks about columns before it reads, so it has to ask about
    // the grant before that, or a stranger is told columns are withheld.
    let analysed = txn.analyze(&nobody, &staff()).await.err();
    assert!(
        matches!(analysed, Some(KernelError::AccessDenied { .. })),
        "{analysed:?}"
    );
}

/// A role that may write and not read is not a narrowed reader: it has always
/// been able to replace whole rows, and column grants do not change that.
#[tokio::test]
async fn a_writer_with_no_read_grant_is_not_treated_as_narrowed() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let writer = as_roles(&["editor"]);
    txn.update(&writer, &staff(), &person(1, "ann", 5))
        .await
        .unwrap();
    // And reading is refused for the old reason, not the new one.
    let read = txn.execute(&writer, &staff(), &Query::all()).await.err();
    assert!(
        matches!(read, Some(KernelError::AccessDenied { .. })),
        "{read:?}"
    );
}

/// Grants add: a table-level grant held beside a column grant is the whole table.
#[tokio::test]
async fn a_table_grant_beside_a_column_grant_reads_everything() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let both = as_roles(&["analyst", "auditor"]);
    let got = txn
        .get(&both, &staff(), &[Value::U64(1)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.get(s("salary")), Some(&Value::U64(SALARY)));
    assert!(txn.explain(&both, &staff(), &Query::all()).is_ok());
}

/// A list that happens to name every column is the whole table today, and
/// stops being it the moment a column is added.
#[test]
fn a_complete_column_list_is_the_whole_table_until_the_table_grows() {
    let narrow = TableDef::builder("t", TableId(9))
        .column("id", ValueType::U64)
        .column("a", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap();
    let grown = TableDef::builder("t", TableId(9))
        .column("id", ValueType::U64)
        .column("a", ValueType::Str)
        .nullable_column("b", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap();
    let catalog = SecurityCatalog::new()
        .grant(Grant::read_columns("r", &narrow, [Ordinal(0), Ordinal(1)]).unwrap());
    let me = SecurityContext::new(Principal::new(Value::U64(1)).with_role("r"));
    assert_eq!(catalog.readable(&me, &narrow), None);
    assert_eq!(
        catalog.readable(&me, &grown),
        Some([Ordinal(0), Ordinal(1)].into())
    );
}

#[test]
fn a_column_grant_must_name_real_columns_and_the_whole_key() {
    let missing_key = Grant::read_columns("r", &staff(), [s("name")]);
    assert!(
        matches!(missing_key, Err(KernelError::InvalidGrant { .. })),
        "{missing_key:?}"
    );
    let stray = Grant::read_columns("r", &staff(), [s("id"), Ordinal(99)]);
    assert!(
        matches!(stray, Err(KernelError::InvalidGrant { .. })),
        "{stray:?}"
    );
}

/// `update_where` writes back the whole row, so the columns its caller cannot
/// see must survive it untouched, and what it returns must not show them.
#[tokio::test]
async fn a_narrowed_update_where_keeps_what_it_cannot_see() {
    let store = seeded().await;
    let me = as_roles(&["analyst", "editor"]);
    let txn = store.begin().await.unwrap();
    let returned = txn
        .update_where(
            &me,
            &staff(),
            Expr::eq(s("name"), Value::Str("ann".into())),
            &[(s("team"), Scalar::column(s("team")) + 10i64)],
            None,
        )
        .await
        .unwrap();
    assert_eq!(returned.len(), 1);
    assert_eq!(
        returned[0].get(s("salary")),
        Some(&Value::Null),
        "returned unconcealed"
    );
    assert_eq!(returned[0].get(s("team")), Some(&Value::I64(11)));
    let stored = txn
        .get(&SecurityContext::superuser(), &staff(), &[Value::U64(1)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.get(s("salary")),
        Some(&Value::U64(SALARY)),
        "a hidden column was overwritten"
    );
    assert_eq!(stored.get(s("owner")), Some(&Value::U64(OWNER)));

    // An assignment that *reads* a hidden column copies it somewhere visible.
    let copied = txn
        .update_where(
            &me,
            &staff(),
            Expr::True,
            &[(s("name"), Scalar::column(s("salary")))],
            None,
        )
        .await;
    assert!(
        matches!(copied, Err(KernelError::ColumnsWithheld { .. })),
        "{copied:?}"
    );
    // And a predicate on one is the same probe a query's would be.
    let probed = txn
        .update_where(
            &me,
            &staff(),
            Expr::compare(s("salary"), CmpOp::Gt, Value::U64(1)),
            &[(s("team"), Scalar::column(s("team")))],
            None,
        )
        .await;
    assert!(
        matches!(probed, Err(KernelError::ColumnsWithheld { .. })),
        "{probed:?}"
    );
}

/// `delete_where` erases index entries by the row's values, so it must have
/// read the hidden ones: the index on `salary` and a table scan must agree
/// afterwards.
#[tokio::test]
async fn a_narrowed_delete_where_leaves_no_index_entry_behind() {
    let store = seeded().await;
    let me = as_roles(&["analyst", "editor"]);
    let txn = store.begin().await.unwrap();
    let removed = txn
        .delete_where(&me, &staff(), Expr::eq(s("team"), Value::I64(1)), None)
        .await
        .unwrap();
    assert_eq!(removed.len(), 2);
    assert!(
        removed
            .iter()
            .all(|r| r.get(s("salary")) == Some(&Value::Null)),
        "returned unconcealed"
    );

    let root = SecurityContext::superuser();
    let on_salary = Expr::eq(s("salary"), Value::U64(SALARY));
    let count = |hint| {
        let txn = &txn;
        let root = root.clone();
        let on_salary = on_salary.clone();
        async move {
            txn.execute(
                &root,
                &staff(),
                &Query {
                    hint: Some(hint),
                    ..Query::all().filter(on_salary)
                },
            )
            .await
            .unwrap()
            .count()
            .await
            .unwrap()
        }
    };
    assert_eq!(count(AccessHint::TableScan).await, 1);
    assert_eq!(
        count(AccessHint::Index(BY_SALARY)).await,
        1,
        "the index kept entries for deleted rows"
    );
}
