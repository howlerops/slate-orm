//! A head node with a `main`, and the in-process oracle the Python tests
//! compare against.
//!
//! # Why this exists
//!
//! `slate-server` is a library. It has no binary, no configuration file and no
//! `main`: `Head::new` takes a catalog, a security catalog, a writer store, a
//! list of replicas, a `Leadership` and an `Authenticator`, and every one of
//! those is a Rust value that only Rust can construct. So a client in any other
//! language cannot start the server it is a client of. This binary is the
//! smallest assembly that gets one running, written here because the Python
//! tests are required to run against a real head node.
//!
//! # The oracle
//!
//! A client test that only checks its own answers against itself proves that
//! the client is self-consistent. What is worth knowing is whether the client's
//! request means what the caller wrote, and the only thing that can say so is
//! the engine.
//!
//! So this process seeds a store, runs a fixed list of reads *in process*
//! against `slate-kernel`, and writes the answers to a JSON file. It then
//! serves the same store over gRPC. The Python tests build what should be the
//! same read with the Python SDK, ask it over the socket, and require the two
//! to agree.
//!
//! The two statements are deliberately independent — the Rust side is written
//! against the kernel's flat ordinals and the Python side against the wire's
//! `ColumnRef` — which is the whole point. A shared helper would make them
//! agree by construction. `crates/slate-server/tests/multi.rs` takes the other
//! approach (one kernel value converted to its wire form) and catches
//! conversion loss; this one catches a client that builds the wrong request,
//! which is the failure a client SDK actually has.
//!
//! # What it does not do
//!
//! No TLS, no real object store, no real lease. The writer is `MemoryStore` and
//! the lease always grants, because the properties under test here are the
//! client's, and a real backend would add flakiness without adding coverage.
//! `--frozen-replica` is the exception: freshness cannot be tested at all
//! without a replica that is genuinely behind, so there is one.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use serde_json::{Map, Value as Json, json};
use slate_kernel::memory::MemoryStore;
use slate_kernel::store::KvSnapshot;
use slate_kernel::{
    Action, Aggregate, Chain, CmpOp, Expr, Grant, Group, Join, JoinKey, JoinSchema, JoinStep,
    JoinType, KernelError, KvReadStore, Policy, Principal, Query, RecordStore, Scalar, ScanOrder,
    SecurityCatalog, SecurityContext, SortKey,
};
use slate_schema::{Catalog, ForeignKeyDef, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_server::leadership::Leadership;
use slate_server::lease::{Lease, LeaseError, Term};
use slate_server::{Head, HeadConfig, MetadataIdentity};
use slate_tuple::{Direction, Value, ValueType};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
use tokio_stream::StreamExt;

// --- the schema -----------------------------------------------------------
//
// The same shapes `crates/slate-server/tests/common/mod.rs` uses, restated
// because that module is a test module and is not exported. Restating it is a
// hazard worth naming: if the two drift, this binary serves a schema the Rust
// tests never exercise. The Python tests hold a third statement of the same
// thing (`tests/fixture.py`), and `test_fixture.py::test_declared_schema_
// matches_the_server` is what keeps that one honest — it is checked against the
// server rather than trusted.

const DOCS: TableId = TableId(1);
const USERS: TableId = TableId(2);
const AUTHORS: TableId = TableId(3);
const BOOKS: TableId = TableId(4);
const SALES: TableId = TableId(5);
const SECRETS: TableId = TableId(6);
const LIBRARIES: TableId = TableId(7);
const SHELVES: TableId = TableId(8);
const COPIES: TableId = TableId(9);

fn docs() -> TableDef {
    TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_kind", IndexId(1)).column("kind"))
        .index(IndexDef::builder("by_size", IndexId(2)).column_with("size", Direction::Asc))
        .build()
        .expect("valid schema")
}

fn users() -> TableDef {
    TableDef::builder("users", USERS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("owner", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(
            IndexDef::builder("by_email", IndexId(3))
                .column("email")
                .unique(),
        )
        .build()
        .expect("valid schema")
}

fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("owner", ValueType::U64)
        .column("name", ValueType::Str)
        .column("country", ValueType::Str)
        .column("born", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_country", IndexId(4)).column("country"))
        .build()
        .expect("valid schema")
}

fn books() -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("author_id", ValueType::U64)
        .column("title", ValueType::Str)
        .column("year", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_author", IndexId(5)).column("author_id"))
        .build()
        .expect("valid schema")
}

// A parent and a child with a **real foreign key**, for the `Related` call.
//
// Deliberately not `authors`/`books`, which have the same shape and no
// constraint between them. Adding one there would change what an insert into
// `books` is allowed to do, and six test files write books without writing an
// author first — so the constraint would be tested by breaking tests that are
// about something else. A relationship needs a foreign key to be nameable, so
// the fixture gets a pair that has one.
//
// The key is composite — `(tenant_id, library_id)` against `(tenant_id, id)` —
// because that is what a tenant-scoped schema produces, and it is the case the
// handler has to get right: the tenant is pinned by the security context and
// is not part of what relates the two rows.
fn libraries() -> TableDef {
    TableDef::builder("libraries", LIBRARIES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn shelves() -> TableDef {
    TableDef::builder("shelves", SHELVES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("library_id", ValueType::U64)
        .column("label", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_library", IndexId(7)).column("library_id"))
        .foreign_key(
            ForeignKeyDef::builder("shelf_library", LIBRARIES)
                .column("tenant_id")
                .column("library_id"),
        )
        .build()
        .expect("valid schema")
}

// A third level, so a relationship *path* has somewhere to go.
//
// `libraries → shelves → copies` is two steps and, on the wire, two reads. The
// pair above exists because a relationship needs a foreign key to be nameable;
// this exists because a path needs *two*, and the same argument that kept
// `authors`/`books` unconstrained applies here — a new table breaks no test
// that writes an existing one.
fn copies() -> TableDef {
    TableDef::builder("copies", COPIES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("shelf_id", ValueType::U64)
        .column("barcode", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_shelf", IndexId(8)).column("shelf_id"))
        .foreign_key(
            ForeignKeyDef::builder("copy_shelf", SHELVES)
                .column("tenant_id")
                .column("shelf_id"),
        )
        .build()
        .expect("valid schema")
}

fn sales() -> TableDef {
    TableDef::builder("sales", SALES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("book_id", ValueType::U64)
        .column("units", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_book", IndexId(6)).column("book_id"))
        .build()
        .expect("valid schema")
}

/// A table the `app` role has no grant on, so a read of it is denied.
///
/// The error-mapping tests need a `PERMISSION_DENIED` that is not a row policy
/// quietly returning fewer rows — a policy is invisible by design, and a test
/// that cannot tell "denied" from "empty" is not testing the denial.
fn secrets() -> TableDef {
    TableDef::builder("secrets", SECRETS)
        .column("id", ValueType::U64)
        .column("value", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn catalog() -> Catalog {
    Catalog::from_tables([
        docs(),
        users(),
        authors(),
        books(),
        sales(),
        secrets(),
        libraries(),
        shelves(),
        copies(),
    ])
    .expect("valid catalog")
}

fn at(table: &TableDef, column: &str) -> Ordinal {
    table
        .ordinal_of(column)
        .unwrap_or_else(|| panic!("`{}` has no column `{column}`", table.name()))
}

/// Grants and policies. Three policies of three different shapes, so a policy
/// applied to the wrong input of a join removes a different set of rows rather
/// than the same one.
fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        // `EVERYTHING` rather than `ALL`: `EXPLAIN` became its own action and
        // `ALL` deliberately excludes it, so a suite that explains has to say
        // so. The `reader` role below is the other half of that — it holds
        // `ALL` and must *not* be able to explain.
        .grant(Grant::new("app", DOCS, Action::EVERYTHING))
        .grant(Grant::new("app", USERS, Action::EVERYTHING))
        .grant(Grant::new("app", AUTHORS, Action::EVERYTHING))
        .grant(Grant::new("app", BOOKS, Action::EVERYTHING))
        .grant(Grant::new("app", SALES, Action::EVERYTHING))
        .grant(Grant::new("app", LIBRARIES, Action::EVERYTHING))
        .grant(Grant::new("app", SHELVES, Action::EVERYTHING))
        .grant(Grant::new("app", COPIES, Action::EVERYTHING))
        .grant(Grant::new("reader", DOCS, Action::ALL))
        // Deliberately no grant on `secrets`.
        .policy(Policy::new(
            "own_rows",
            USERS,
            Action::ALL,
            |context: &SecurityContext| {
                Expr::eq(at(&users(), "owner"), context.principal().id.clone())
            },
        ))
        .policy(Policy::new(
            "own_authors",
            AUTHORS,
            Action::ALL,
            |context: &SecurityContext| {
                Expr::eq(at(&authors(), "owner"), context.principal().id.clone())
            },
        ))
        .policy(Policy::new(
            "modern_books",
            BOOKS,
            Action::ALL,
            |_: &SecurityContext| Expr::compare(at(&books(), "year"), CmpOp::Ge, Value::I64(2000)),
        ))
        .policy(Policy::new(
            "real_sales",
            SALES,
            Action::ALL,
            |_: &SecurityContext| Expr::compare(at(&sales(), "units"), CmpOp::Gt, Value::I64(0)),
        ))
}

// --- the rows -------------------------------------------------------------

fn doc(id: u64, kind: &str, size: i64, note: Option<&str>) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(kind.to_owned()),
        Value::I64(size),
        note.map_or(Value::Null, |n| Value::Str(n.to_owned())),
    ])
}

fn author(tenant: u64, id: u64, owner: u64, name: &str, country: &str, born: i64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::U64(owner),
        Value::Str(name.to_owned()),
        Value::Str(country.to_owned()),
        Value::I64(born),
    ])
}

fn book(tenant: u64, id: u64, author_id: u64, title: &str, year: i64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::U64(author_id),
        Value::Str(title.to_owned()),
        Value::I64(year),
    ])
}

fn user(tenant: u64, id: u64, owner: u64, email: &str) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::U64(owner),
        Value::Str(email.to_owned()),
    ])
}

fn sale(tenant: u64, id: u64, book_id: u64, units: i64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::U64(book_id),
        Value::I64(units),
    ])
}

/// The fixture, chosen so each join type, each policy and each aggregate has
/// something that distinguishes it. Mirrors `multi.rs`'s reasoning:
///
/// - `di` has no books, so a left outer join has an unmatched left row.
/// - `orphan` has no author, so a right or full outer join has an unmatched
///   right row — the case a nested loop cannot serve.
/// - `cy` is owned by somebody else and `a-two` is too old, so each side's
///   policy hides a row for a different reason.
/// - tenant 2 repeats the same principal id, which is what a tenant check
///   that only looked at the principal would let through.
async fn seed(backing: &Arc<MemoryStore>) {
    let store = RecordStore::new(Arc::clone(backing), catalog(), security());
    let root = SecurityContext::superuser();
    let txn = store.begin().await.expect("begin");
    txn.insert_many(
        &root,
        &docs(),
        &[
            doc(1, "kind-a", 5, Some("first")),
            doc(2, "kind-a", 15, None),
            doc(3, "kind-b", 25, Some("third")),
            doc(4, "kind-b", 35, None),
            doc(5, "kind-c", 45, Some("fifth")),
            doc(6, "kind-c", 55, Some("sixth")),
            doc(7, "kind-d", 65, None),
        ],
    )
    .await
    .expect("seed docs");
    txn.insert_many(
        &root,
        &authors(),
        &[
            author(1, 1, 1, "ada", "UK", 1815),
            author(1, 2, 1, "bo", "US", 1900),
            author(1, 3, 2, "cy", "UK", 1950),
            author(1, 4, 1, "di", "FR", 1970),
            author(2, 1, 1, "zz", "UK", 1800),
        ],
    )
    .await
    .expect("seed authors");
    txn.insert_many(
        &root,
        &books(),
        &[
            book(1, 10, 1, "a-one", 2001),
            book(1, 11, 1, "a-two", 1990),
            book(1, 12, 2, "b-one", 2010),
            book(1, 13, 3, "c-one", 2005),
            book(1, 14, 9, "orphan", 2020),
            book(1, 15, 1, "a-three", 2003),
            book(2, 10, 1, "zz-book", 2001),
        ],
    )
    .await
    .expect("seed books");
    txn.insert_many(
        &root,
        &sales(),
        &[
            sale(1, 20, 10, 5),
            sale(1, 21, 10, 0),
            sale(1, 22, 12, 7),
            sale(1, 23, 99, 3),
            sale(2, 20, 10, 4),
        ],
    )
    .await
    .expect("seed sales");
    // Three users: one the caller owns, one owned by somebody else in the same
    // tenant, and one the caller owns in a *different* tenant. The policy and
    // the tenant scoping hide a different one each, so a client that dropped
    // either would see a different row count from the oracle.
    txn.insert_many(
        &root,
        &users(),
        &[
            user(1, 1, 1, "mine@example.com"),
            user(1, 2, 2, "theirs@example.com"),
            user(2, 1, 1, "other-tenant@example.com"),
        ],
    )
    .await
    .expect("seed users");
    txn.insert_many(
        &root,
        &secrets(),
        &[Row::new(vec![
            Value::U64(1),
            Value::Str("hidden".to_owned()),
        ])],
    )
    .await
    .expect("seed secrets");
    txn.commit().await.expect("commit the fixture");
}

// --- JSON ------------------------------------------------------------------
//
// Tagged rather than bare, because the tag is most of what is being compared.
// A bare `5` cannot tell `U64(5)` from `I64(5)`, and the kernel's ordering is
// type-first — a client that sent the wrong integer width would build a
// predicate that matches nothing and a bare-JSON oracle would agree with it.

fn value_json(value: &Value) -> Json {
    match value {
        Value::Null => json!({ "null": Json::Null }),
        Value::Bool(b) => json!({ "bool": b }),
        Value::Str(s) => json!({ "str": s }),
        Value::I64(n) => json!({ "i64": n }),
        Value::U64(n) => json!({ "u64": n }),
        // As a string: JSON has one number type and a float that round-trips
        // through it is not something worth relying on in a comparison whose
        // whole job is to be exact.
        Value::F64(x) => json!({ "f64": x.to_string() }),
        Value::Bytes(b) => json!({ "bytes": b.iter().map(|x| *x as u64).collect::<Vec<_>>() }),
        Value::Uuid(u) => json!({ "uuid": u.to_string() }),
        Value::Vector(v) => {
            json!({ "vector": v.iter().map(|x| x.to_string()).collect::<Vec<_>>() })
        }
        other => json!({ "unrepresentable": other.type_name() }),
    }
}

fn row_json(row: &Row) -> Json {
    Json::Array(row.values().iter().map(value_json).collect())
}

fn wide_json(row: &[Option<Row>]) -> Json {
    Json::Array(
        row.iter()
            .map(|r| r.as_ref().map_or(Json::Null, row_json))
            .collect(),
    )
}

fn group_json(group: &Group) -> Json {
    json!({
        "key": Json::Array(group.key.iter().map(value_json).collect::<Vec<_>>()),
        "values": Json::Array(group.values.iter().map(value_json).collect::<Vec<_>>()),
    })
}

// --- the oracle ------------------------------------------------------------

/// The identity every oracle case runs as, and the one the Python tests use.
fn ctx() -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(1))
            .with_role("app"),
    )
}

async fn run_query(store: &RecordStore<Arc<MemoryStore>>, table: &TableDef, query: &Query) -> Json {
    let txn = store.begin().await.expect("begin");
    let mut cursor = txn.execute(&ctx(), table, query).await.expect("execute");
    let mut rows = Vec::new();
    while let Some(row) = cursor.next().await.expect("a row") {
        rows.push(row_json(&row));
    }
    Json::Array(rows)
}

async fn run_join(
    store: &RecordStore<Arc<MemoryStore>>,
    left: &TableDef,
    right: &TableDef,
    join: &Join,
) -> Json {
    let txn = store.begin().await.expect("begin");
    let mut cursor = txn.join(&ctx(), left, right, join).await.expect("join");
    let mut rows = Vec::new();
    while let Some(row) = cursor.next().await.expect("a joined row") {
        rows.push(wide_json(&[row.left, row.right]));
    }
    Json::Array(rows)
}

async fn run_chain(
    store: &RecordStore<Arc<MemoryStore>>,
    tables: &[&TableDef],
    chain: &Chain,
) -> Json {
    let txn = store.begin().await.expect("begin");
    let rows = txn
        .chain(&ctx(), tables, chain)
        .await
        .expect("chain")
        .collect()
        .await
        .expect("chain rows");
    Json::Array(
        rows.iter()
            .map(|row| {
                wide_json(
                    &(0..tables.len())
                        .map(|i| row.at(i).cloned())
                        .collect::<Vec<_>>(),
                )
            })
            .collect(),
    )
}

async fn run_groups(
    store: &RecordStore<Arc<MemoryStore>>,
    table: &TableDef,
    query: &Query,
    group: &[Ordinal],
    aggregates: &[Aggregate],
    having: &Expr,
) -> Json {
    let txn = store.begin().await.expect("begin");
    let groups = txn
        .group_by_having(&ctx(), table, query, group, aggregates, having)
        .await
        .expect("group by");
    Json::Array(groups.iter().map(group_json).collect())
}

/// Every case the Python tests compare themselves against.
///
/// Each name is written out in `tests/test_oracle.py` too, and the Python side
/// asserts that it consumed every name this side produced. A case added here
/// and not there would otherwise be a case nobody runs — the failure mode of
/// every fixture file that is loaded by key.
async fn oracle(backing: &Arc<MemoryStore>) -> Json {
    let store = RecordStore::new(Arc::clone(backing), catalog(), security());
    let (d, a, b, s) = (docs(), authors(), books(), sales());
    let mut out = Map::new();

    // A plain scan, so a disagreement anywhere else is not just "the client
    // cannot read a row".
    out.insert(
        "docs_all".into(),
        run_query(&store, &d, &Query::all()).await,
    );

    // Filter, sort, limit and offset together: the sort is what makes the
    // limit deterministic, which is the property `docs/correctness.md` says a
    // paging test without a total order silently loses.
    out.insert(
        "docs_filtered_paged".into(),
        run_query(
            &store,
            &d,
            &Query::all()
                .filter(Expr::compare(at(&d, "size"), CmpOp::Ge, Value::I64(15)))
                .sort_by([SortKey::desc(at(&d, "size")), SortKey::asc(at(&d, "id"))])
                .limit(3)
                .offset(1),
        )
        .await,
    );

    // A disjunction with a NULL in the mix: three-valued logic is the part of
    // a predicate a client is most likely to encode wrongly.
    out.insert(
        "docs_null_or_like".into(),
        run_query(
            &store,
            &d,
            &Query::all().filter(Expr::Or(vec![
                Expr::is_null(at(&d, "note")),
                Expr::like(at(&d, "kind"), "kind-c%"),
            ])),
        )
        .await,
    );

    // Computed values, filtered on one of them. `Query::computed` is the
    // kernel's own flat-ordinal helper; the Python side names it as
    // `computed(0)` and never learns the table's width.
    out.insert(
        "docs_computed".into(),
        run_query(
            &store,
            &d,
            &Query::all()
                .computing([
                    Scalar::column(at(&d, "size")) * 2i64,
                    Scalar::Upper(Box::new(Scalar::column(at(&d, "kind")))),
                ])
                .filter(Expr::compare(
                    Query::computed(&d, 0),
                    CmpOp::Gt,
                    Value::I64(40),
                ))
                .sort_by([SortKey::asc(at(&d, "id"))]),
        )
        .await,
    );

    // A descending scan, which reaches a different access path.
    out.insert(
        "docs_descending".into(),
        run_query(&store, &d, &Query::all().order(ScanOrder::Descending)).await,
    );

    // A projection: columns outside it come back null, and that is a
    // *difference* the client has to reproduce rather than paper over.
    out.insert(
        "docs_projected".into(),
        run_query(
            &store,
            &d,
            &Query::all().select([at(&d, "id"), at(&d, "kind")]),
        )
        .await,
    );

    // The four join types over the same pair. Each is a separate case because
    // each returns a different set, which is what makes the comparison sharp.
    for (name, join_type) in [
        ("inner", JoinType::Inner),
        ("left", JoinType::Left),
        ("right", JoinType::Right),
        ("full", JoinType::Full),
    ] {
        let mut join = Join::equating(at(&a, "id"), at(&b, "author_id"));
        join.join_type = join_type;
        out.insert(
            format!("join_{name}"),
            run_join(&store, &a, &b, &join).await,
        );
    }

    // A cross-input condition, which is the thing `ColumnRef` has to get right
    // on both sides at once: it names input 0's column and input 1's column in
    // one expression.
    let schema = JoinSchema::of(&a, &b);
    let mut cross = Join::equating(at(&a, "id"), at(&b, "author_id"));
    cross.having = Expr::compare_columns(
        schema.right(at(&b, "year")),
        CmpOp::Gt,
        schema.left(at(&a, "born")),
    );
    out.insert(
        "join_cross_condition".into(),
        run_join(&store, &a, &b, &cross).await,
    );

    // A per-input filter on the *second* input, so a client that put every
    // filter on input 0 would disagree here and nowhere else.
    let filtered = Join::equating(at(&a, "id"), at(&b, "author_id"))
        .right(Query::all().filter(Expr::like(at(&b, "title"), "a-%")));
    out.insert(
        "join_right_filtered".into(),
        run_join(&store, &a, &b, &filtered).await,
    );

    // Two equalities, so a client that only ever sends the first is caught.
    let two_keys = Join::on([
        JoinKey::new(at(&a, "id"), at(&b, "author_id")),
        JoinKey::new(at(&a, "tenant_id"), at(&b, "tenant_id")),
    ]);
    out.insert(
        "join_two_keys".into(),
        run_join(&store, &a, &b, &two_keys).await,
    );

    // A three-table chain: the wire has one shape for a join and a chain, and
    // the server dispatches on the count. A client that got the third input's
    // `ColumnRef.input` wrong is caught here and only here.
    // The accumulated side of a chain step is named in the *joined* space —
    // `JoinSchema::at(position, ordinal)`. That is the kernel's flat
    // arithmetic, and it is exactly what the wire's `ColumnRef` removes: the
    // Python side writes `books.column("id")` and never sees a width. Both
    // statements have to name the same column or the oracle is comparing two
    // different chains, which is why this is spelled out rather than helped.
    let chain_schema = JoinSchema::over([&a, &b, &s]);
    let chain = Chain::from(Query::all())
        .join(JoinStep::on([JoinKey::new(
            chain_schema.at(0, at(&a, "id")),
            at(&b, "author_id"),
        )]))
        .join(JoinStep::on([JoinKey::new(
            chain_schema.at(1, at(&b, "id")),
            at(&s, "book_id"),
        )]));
    out.insert(
        "chain_authors_books_sales".into(),
        run_chain(&store, &[&a, &b, &s], &chain).await,
    );

    // A self-join: two inputs over one table, which is the case a qualified
    // name could not have addressed at all.
    let self_join = Join::equating(at(&b, "author_id"), at(&b, "author_id"));
    out.insert(
        "join_self_books".into(),
        run_join(&store, &b, &b, &self_join).await,
    );

    // Aggregates over one group, one of each function that takes a column.
    out.insert(
        "agg_plain".into(),
        run_groups(
            &store,
            &b,
            &Query::all(),
            &[],
            &[
                Aggregate::Count,
                Aggregate::CountColumn(at(&b, "year")),
                Aggregate::Min(at(&b, "year")),
                Aggregate::Max(at(&b, "year")),
                Aggregate::Sum(at(&b, "year")),
                Aggregate::Avg(at(&b, "year")),
                Aggregate::CountDistinct(at(&b, "author_id")),
            ],
            &Expr::True,
        )
        .await,
    );

    // GROUP BY with a HAVING over an aggregate: `aggregate(0)` on the wire,
    // and the refusal that goes with it is tested separately in Python.
    out.insert(
        "agg_group_having".into(),
        run_groups(
            &store,
            &b,
            &Query::all(),
            &[at(&b, "author_id")],
            &[Aggregate::Count, Aggregate::Sum(at(&b, "year"))],
            // `>= 2`, not `>= 1`. Every group has at least one row, so `>= 1`
            // keeps all of them — and a client that addressed the group key
            // where it meant the aggregate would return the same groups and
            // the oracle would agree with it. Mutation testing found this:
            // "an aggregate reference is sent as a group key" survived.
            &Expr::compare(Ordinal(1), CmpOp::Ge, Value::U64(2)),
        )
        .await,
    );

    // HAVING over a *key* rather than an aggregate — the other half of the
    // group space, and the half a client is likelier to misaddress.
    out.insert(
        "agg_group_having_key".into(),
        run_groups(
            &store,
            &b,
            &Query::all(),
            &[at(&b, "author_id")],
            &[Aggregate::Count],
            &Expr::compare(Ordinal(0), CmpOp::Eq, Value::U64(1)),
        )
        .await,
    );

    // Grouping by a computed value: the group key is `computed(0)` on the
    // wire, which is a `ColumnRef` kind the plain cases never reach.
    out.insert(
        "agg_group_by_computed".into(),
        run_groups(
            &store,
            &b,
            &Query::all().computing([Scalar::Lower(Box::new(Scalar::column(at(&b, "title"))))]),
            &[Query::computed(&b, 0)],
            &[Aggregate::Count],
            &Expr::True,
        )
        .await,
    );

    // A policy that hides rows: `users` is filtered to the caller's own, and
    // the Python side must see exactly the same subset rather than "some rows".
    out.insert(
        "users_visible".into(),
        run_query(&store, &users(), &Query::all()).await,
    );

    Json::Object(out)
}

// --- leadership ------------------------------------------------------------

/// A lease that always grants.
#[derive(Debug, Default)]
struct AlwaysLeader {
    generation: AtomicU64,
}

#[tonic::async_trait]
impl Lease for AlwaysLeader {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Term {
            generation,
            holder: "testserver".to_owned(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
    }
    async fn renew(&self) -> Result<Term, LeaseError> {
        self.acquire().await
    }
    async fn release(&self) -> Result<(), LeaseError> {
        Ok(())
    }
    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        Ok(None)
    }
    fn held(&self) -> Option<Term> {
        None
    }
    fn holder(&self) -> &str {
        "testserver"
    }
}

/// A lease somebody else holds, forever. Used by `--follower`, which is the
/// only way to reach the `UNAVAILABLE` + `slate-leader` redirect from a client.
#[derive(Debug)]
struct HeldByAnother;

#[tonic::async_trait]
impl Lease for HeldByAnother {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        Err(LeaseError::Held {
            holder: "the-other-node".to_owned(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
    }
    async fn renew(&self) -> Result<Term, LeaseError> {
        Err(LeaseError::NotHeld)
    }
    async fn release(&self) -> Result<(), LeaseError> {
        Err(LeaseError::NotHeld)
    }
    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        Ok(None)
    }
    fn held(&self) -> Option<Term> {
        None
    }
    fn holder(&self) -> &str {
        "this-node"
    }
}

// --- a replica that is genuinely behind -------------------------------------

/// An empty replica that will never catch up.
///
/// Extreme on purpose, and copied from `crates/slate-server/tests/freshness.rs`
/// for the reason stated there: a replica one write behind and a replica forty
/// behind fail a freshness check identically, and an empty one makes the
/// difference between the two answers unmistakable. Without it the Python
/// read-your-writes test would pass against a pool with no lag, where the token
/// does nothing.
#[derive(Debug)]
struct Frozen {
    empty: MemoryStore,
}

#[tonic::async_trait]
impl KvReadStore for Frozen {
    async fn snapshot(&self) -> Result<Box<dyn KvSnapshot + Send + '_>, KernelError> {
        self.empty.snapshot().await
    }

    fn visible_sequence(&self) -> Option<u64> {
        Some(0)
    }

    async fn wait_for_sequence(
        &self,
        sequence: u64,
        _timeout: Duration,
    ) -> Result<(), KernelError> {
        Err(KernelError::ReplicaTooStale {
            replica: "stale-replica".to_owned(),
            required: sequence,
            visible: 0,
        })
    }

    fn replica_name(&self) -> &str {
        "stale-replica"
    }
}

// --- main ------------------------------------------------------------------

struct Options {
    port: u16,
    oracle_out: Option<String>,
    frozen_replica: bool,
    follower: bool,
    seed: bool,
}

fn parse_args() -> Options {
    let mut options = Options {
        port: 0,
        oracle_out: None,
        frozen_replica: false,
        follower: false,
        seed: true,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                options.port = args
                    .next()
                    .expect("--port takes a value")
                    .parse()
                    .expect("--port must be a number");
            }
            "--oracle-out" => {
                options.oracle_out = Some(args.next().expect("--oracle-out takes a path"))
            }
            "--frozen-replica" => options.frozen_replica = true,
            "--follower" => options.follower = true,
            "--empty" => options.seed = false,
            other => panic!("unknown argument `{other}`"),
        }
    }
    options
}

#[tokio::main]
async fn main() {
    let options = parse_args();
    let backing = Arc::new(MemoryStore::new());
    if options.seed {
        seed(&backing).await;
    }

    // The oracle runs before the socket opens, so a Python test that reads the
    // file after seeing the address cannot race it.
    if let Some(path) = &options.oracle_out {
        let answers = oracle(&backing).await;
        std::fs::write(
            path,
            serde_json::to_string_pretty(&answers).expect("serialise"),
        )
        .expect("write the oracle file");
    }

    let leadership = if options.follower {
        let leadership = Leadership::new(Arc::new(HeldByAnother));
        // Campaign once and lose, so the node knows who the leader is and can
        // put it in the `slate-leader` trailer.
        let _ = leadership.campaign().await;
        leadership
    } else {
        let leadership = Leadership::new(Arc::new(AlwaysLeader::default()));
        assert!(leadership.campaign().await, "the fake lease always grants");
        leadership
    };

    let replicas: Vec<Arc<dyn KvReadStore>> = if options.frozen_replica {
        vec![Arc::new(Frozen {
            empty: MemoryStore::new(),
        })]
    } else {
        Vec::new()
    };

    let head = Head::new(
        HeadConfig::new(catalog(), security()),
        Arc::clone(&backing),
        replicas,
        leadership,
        // Correct only behind a proxy that sets the identity headers. This is a
        // test server on loopback; there is no proxy and there is nothing worth
        // stealing in it.
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", options.port))
        .await
        .expect("bind a loopback port");
    let address = listener.local_addr().expect("local address");

    // The handshake with the Python side. Printed before `serve` is awaited but
    // after the listener is bound, so a client that connects the instant it
    // reads this line finds the port already accepting — polling the port
    // instead would be a race the test loses once a week.
    println!("LISTENING {address}");
    use std::io::Write;
    std::io::stdout().flush().expect("flush");

    // `serve_with_incoming` ignores tonic's own `TCP_NODELAY` default — its
    // documentation says so — so a server that binds its own listener accepts
    // Nagled sockets unless it says otherwise. On a gRPC server stream, which
    // is a header message followed by a batch of rows, that measured 44.01 ms
    // against 267 µs for a ten-row query. See `slate-serverd`'s `serve.rs`.
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener)
        .map(|accepted| accepted.inspect(|socket| socket.set_nodelay(true).expect("nodelay")));
    tonic::transport::Server::builder()
        .add_service(head.into_service())
        .serve_with_incoming(incoming)
        .await
        .expect("serve");
}
