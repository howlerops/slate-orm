//! Fixtures: a catalog, a set of rules, and a head node serving them.
//!
//! The head node is served over a real loopback socket rather than an in-memory
//! channel. That is deliberate: the things most likely to be wrong at a wire
//! boundary — a stream that never terminates, a status that loses its code, a
//! message too large — are exactly the things an in-process shortcut skips.

// A shared test module is compiled into each test binary, so items it exposes
// look unused from whichever binary does not call them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    dead_code,
    unreachable_pub
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, CmpOp, Expr, Grant, KvReadStore, KvStore, Policy, Principal, RecordStore,
    SecurityCatalog, SecurityContext,
};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_server::leadership::Leadership;
use slate_server::lease::{Clock, Lease, LeaseError, ObjectStoreLease, Term};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_server::{Head, HeadConfig, MetadataIdentity};
use slate_tuple::{Direction, Value, ValueType};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tonic::Request;
use tonic::transport::Channel;

pub const DOCS: TableId = TableId(1);
pub const USERS: TableId = TableId(2);
pub const AUTHORS: TableId = TableId(3);
pub const BOOKS: TableId = TableId(4);
pub const SALES: TableId = TableId(5);

/// A plain table: no tenant, two indexes, one nullable column.
pub fn docs() -> TableDef {
    // Index ids are unique across the whole catalog, not per table: an index entry
    // is keyed on the index id with no table id in it, so two tables sharing an id
    // share one key range and a scan of either walks both. `Catalog::insert`
    // refuses it now — before it did, every table below declared `IndexId(1)` and
    // this fixture had five indexes in one keyspace. A decade per table, so there
    // is room to add one and the id says which table it belongs to.
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

/// A tenant-scoped table with a row policy, for the security and routing tests.
pub fn users() -> TableDef {
    TableDef::builder("users", USERS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("owner", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(
            IndexDef::builder("by_email", IndexId(10))
                .column("email")
                .unique(),
        )
        .build()
        .expect("valid schema")
}

/// Three tenant-scoped tables that join into a chain, each with a policy of a
/// *different* shape.
///
/// Different shapes on purpose: a matrix where every side's policy is
/// `owner = me` would pass while a join applied one side's filter to both, and
/// that is exactly the bug worth catching. Here the first side hides rows by
/// owner, the second by a value comparison and the third by another, so a
/// policy applied to the wrong side changes which rows disappear.
pub fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("owner", ValueType::U64)
        .column("name", ValueType::Str)
        .column("country", ValueType::Str)
        // An integer column on this side too, so a cross-side condition has
        // two columns of the same type to compare. `Value`'s order is
        // type-first, so the kernel refuses a comparison across types and a
        // join condition written between a string and an integer would test
        // the refusal rather than the join.
        .column("born", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_country", IndexId(20)).column("country"))
        .build()
        .expect("valid schema")
}

pub fn books() -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("author_id", ValueType::U64)
        .column("title", ValueType::Str)
        .column("year", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_author", IndexId(30)).column("author_id"))
        .build()
        .expect("valid schema")
}

pub fn sales() -> TableDef {
    TableDef::builder("sales", SALES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("book_id", ValueType::U64)
        .column("units", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_book", IndexId(40)).column("book_id"))
        .build()
        .expect("valid schema")
}

pub fn catalog() -> Catalog {
    Catalog::from_tables([docs(), users(), authors(), books(), sales()]).expect("catalog")
}

/// The ordinal of a column of one of the fixture tables, by name.
pub fn at(table: &TableDef, column: &str) -> slate_schema::Ordinal {
    table
        .ordinal_of(column)
        .unwrap_or_else(|| panic!("`{}` has no column `{column}`", table.name()))
}

/// Grants for the `app` role, plus a row policy on `users`: a caller sees only
/// the rows it owns.
pub fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("app", DOCS, Action::ALL))
        .grant(Grant::new("app", USERS, Action::ALL))
        .policy(Policy::new(
            "own_rows",
            USERS,
            Action::ALL,
            |context: &SecurityContext| {
                let owner = users().ordinal_of("owner").expect("owner");
                Expr::eq(owner, context.principal().id.clone())
            },
        ))
        .grant(Grant::new("app", AUTHORS, Action::ALL))
        .grant(Grant::new("app", BOOKS, Action::ALL))
        .grant(Grant::new("app", SALES, Action::ALL))
        // Hides rows by who owns them.
        .policy(Policy::new(
            "own_authors",
            AUTHORS,
            Action::ALL,
            |context: &SecurityContext| {
                Expr::eq(at(&authors(), "owner"), context.principal().id.clone())
            },
        ))
        // Hides rows by a value, so a policy applied to the wrong side of a
        // join removes a different set of rows rather than the same one.
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

pub fn doc(id: u64, kind: &str, size: i64, note: Option<&str>) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(kind.to_owned()),
        Value::I64(size),
        note.map_or(Value::Null, |n| Value::Str(n.to_owned())),
    ])
}

pub fn author(tenant: u64, id: u64, owner: u64, name: &str, country: &str, born: i64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::U64(owner),
        Value::Str(name.to_owned()),
        Value::Str(country.to_owned()),
        Value::I64(born),
    ])
}

pub fn book(tenant: u64, id: u64, author_id: u64, title: &str, year: i64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::U64(author_id),
        Value::Str(title.to_owned()),
        Value::I64(year),
    ])
}

pub fn sale(tenant: u64, id: u64, book_id: u64, units: i64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::U64(book_id),
        Value::I64(units),
    ])
}

pub fn user(tenant: u64, id: u64, owner: u64, email: &str) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::U64(owner),
        Value::Str(email.to_owned()),
    ])
}

/// A record store over an in-memory backend, with this catalog and rules.
pub fn store(backing: Arc<MemoryStore>) -> RecordStore<Arc<MemoryStore>> {
    RecordStore::new(backing, catalog(), security())
}

// --- leases ---------------------------------------------------------------

/// A clock the test moves by hand.
///
/// Every property worth establishing about a lease is about expiry, and a test
/// that establishes them by sleeping is slow when it passes and flaky when the
/// machine is busy.
#[derive(Debug)]
pub struct TestClock {
    millis: AtomicU64,
}

impl TestClock {
    pub fn new() -> Arc<Self> {
        // Not zero: an expiry of `UNIX_EPOCH` is what a malformed record
        // decodes to, and starting there would make the two indistinguishable.
        Arc::new(Self {
            millis: AtomicU64::new(1_700_000_000_000),
        })
    }

    pub fn advance(&self, by: Duration) {
        self.millis
            .fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.millis.load(Ordering::SeqCst))
    }
}

/// A lease that always grants, for tests about something other than the lease.
#[derive(Debug, Default)]
pub struct AlwaysLeader {
    generation: AtomicU64,
}

#[tonic::async_trait]
impl Lease for AlwaysLeader {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Term {
            generation,
            holder: "always".to_owned(),
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
        "always"
    }
}

/// A lease held by somebody else, forever.
#[derive(Debug)]
pub struct HeldByAnother;

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

/// An object-store lease under a clock the test controls.
pub fn lease_at(
    store: Arc<dyn object_store::ObjectStore>,
    holder: &str,
    clock: Arc<TestClock>,
    term: Duration,
) -> ObjectStoreLease {
    ObjectStoreLease::with_holder(store, "leases/writer", holder.to_owned())
        .with_term_length(term)
        .with_clock(clock)
}

// --- serving --------------------------------------------------------------

/// A head node and the socket it is answering on.
pub struct Serving {
    pub address: SocketAddr,
    pub server: JoinHandle<()>,
}

impl Serving {
    /// A client connected to it.
    pub async fn client(&self) -> RecordsClient<Channel> {
        RecordsClient::connect(format!("http://{}", self.address))
            .await
            .expect("connect to the head node")
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Build a head node over `writer`, reading through `replicas`.
pub fn head(
    writer: Arc<MemoryStore>,
    replicas: Vec<Arc<dyn KvReadStore>>,
    leadership: Arc<Leadership>,
) -> Head<MemoryStore> {
    Head::new(
        HeadConfig::new(catalog(), security()),
        writer,
        replicas,
        leadership,
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    )
}

/// Serve a head node on a loopback port the operating system chooses.
pub async fn serve<S: KvStore + KvReadStore>(head: Head<S>) -> Serving {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let address = listener.local_addr().expect("local address");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let server = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(head.into_service())
            .serve_with_incoming(incoming)
            .await;
    });

    Serving { address, server }
}

/// A head node that already holds the lease, serving on a loopback port.
pub async fn serving_leader(writer: Arc<MemoryStore>) -> Serving {
    let leadership = Leadership::new(Arc::new(AlwaysLeader::default()));
    assert!(leadership.campaign().await, "the fake lease always grants");
    serve(head(writer, Vec::new(), leadership)).await
}

// --- requests -------------------------------------------------------------

/// Attach an identity to a request, the way the proxy in front of a real
/// deployment would.
pub fn as_principal<T>(message: T, id: &str, tenant: Option<&str>, roles: &str) -> Request<T> {
    let mut request = Request::new(message);
    request
        .metadata_mut()
        .insert("slate-principal", id.parse().expect("ascii"));
    if let Some(tenant) = tenant {
        request
            .metadata_mut()
            .insert("slate-tenant", tenant.parse().expect("ascii"));
    }
    request
        .metadata_mut()
        .insert("slate-roles", roles.parse().expect("ascii"));
    request
}

/// The identity the `docs` tests use: a member of `app`, with no tenant.
pub fn app<T>(message: T) -> Request<T> {
    as_principal(message, "u64:1", None, "app")
}

/// A member of `app` in tenant `tenant`, with principal id `id`.
pub fn app_in<T>(message: T, id: u64, tenant: u64) -> Request<T> {
    as_principal(
        message,
        &format!("u64:{id}"),
        Some(&format!("u64:{tenant}")),
        "app",
    )
}

/// Collect a query stream into rows, and the `served_by` its header carried.
pub async fn drain(
    stream: tonic::Streaming<pb::QueryResponse>,
) -> (Vec<pb::Row>, Option<pb::ServedBy>) {
    let mut stream = stream;
    let mut rows = Vec::new();
    let mut served_by = None;
    while let Some(message) = stream.message().await.expect("a query message") {
        if served_by.is_none() {
            served_by = message.served_by.clone();
        }
        rows.extend(message.rows);
    }
    (rows, served_by)
}

/// The `id` column of a `docs` row, for comparing results without noise.
pub fn doc_ids(rows: &[pb::Row]) -> Vec<u64> {
    rows.iter()
        .map(
            |row| match row.values.first().and_then(|v| v.kind.as_ref()) {
                Some(pb::value::Kind::Uint64Value(id)) => *id,
                other => panic!("first column of a docs row was {other:?}"),
            },
        )
        .collect()
}

/// A query over `table` with everything defaulted — and with the schema check
/// a correct client sends.
///
/// Attached by default rather than left off, so that every test in the suite
/// that builds a request by hand puts a fingerprint through the server. A test
/// about the check itself sets `schema` to something else.
pub fn plain_query(table: &str) -> pb::Query {
    pb::Query {
        table: table.to_owned(),
        filter: None,
        order: pb::ScanOrder::Ascending as i32,
        projection: None,
        sort: Vec::new(),
        limit: None,
        offset: 0,
        hint: None,
        compute: Vec::new(),
        // `None` for a table this catalog does not have, so that a test about
        // an unknown table still reaches the server's own refusal rather than
        // being stopped here.
        schema: maybe_claim(table),
    }
}

/// The schema check a correct client sends for one of the fixture tables.
pub fn claim(table: &str) -> pb::SchemaCheck {
    maybe_claim(table).unwrap_or_else(|| panic!("no fixture table named `{table}`"))
}

fn maybe_claim(table: &str) -> Option<pb::SchemaCheck> {
    let catalog = catalog();
    let table = catalog.table_by_name(table)?;
    Some(slate_server::fingerprint::claim(table))
}

/// A wire row of stored values, with no computed values beside them.
pub fn wire_row(values: Vec<pb::Value>) -> pb::Row {
    pb::Row {
        values,
        computed: Vec::new(),
    }
}

/// A query over `docs` with everything defaulted.
pub fn docs_query() -> pb::Query {
    plain_query("docs")
}

/// The identity [`app_in`] attaches, built in process.
///
/// The oracle tests need the *same* context on both sides: the point is that
/// gRPC and the kernel answer identically, and they cannot if they are running
/// as different principals. Built here rather than in each test so the two
/// statements of one identity sit next to each other.
pub fn app_context(id: u64, tenant: u64) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(id))
            .with_tenant(Value::U64(tenant))
            .with_role("app"),
    )
}

/// Collect a join stream into rows, and the `served_by` its header carried.
pub async fn drain_joined(
    stream: tonic::Streaming<pb::JoinResponse>,
) -> (Vec<pb::JoinedRow>, Option<pb::ServedBy>) {
    let mut stream = stream;
    let mut rows = Vec::new();
    let mut served_by = None;
    while let Some(message) = stream.message().await.expect("a join message") {
        if served_by.is_none() {
            served_by = message.served_by.clone();
        }
        rows.extend(message.rows);
    }
    (rows, served_by)
}

/// Collect an aggregate stream into groups, and the `served_by` its header
/// carried.
pub async fn drain_groups(
    stream: tonic::Streaming<pb::AggregateResponse>,
) -> (Vec<pb::Group>, Option<pb::ServedBy>) {
    let mut stream = stream;
    let mut groups = Vec::new();
    let mut served_by = None;
    while let Some(message) = stream.message().await.expect("an aggregate message") {
        if served_by.is_none() {
            served_by = message.served_by.clone();
        }
        groups.extend(message.groups);
    }
    (groups, served_by)
}

/// Collect a query stream into rows and the warnings its header carried.
///
/// Separate from [`drain`] rather than folded into it, because "the warnings
/// are on the *first* message and nowhere else" is itself part of the
/// contract: this asserts it rather than concatenating them all and hiding a
/// server that sent them on every batch.
pub async fn drain_warned(
    stream: tonic::Streaming<pb::QueryResponse>,
) -> (Vec<pb::Row>, Vec<String>) {
    let mut stream = stream;
    let (mut rows, mut warnings, mut first) = (Vec::new(), Vec::new(), true);
    while let Some(message) = stream.message().await.expect("a query message") {
        if first {
            warnings = message.warnings.clone();
            first = false;
        } else {
            assert!(
                message.warnings.is_empty(),
                "warnings belong on the first message only"
            );
        }
        rows.extend(message.rows);
    }
    (rows, warnings)
}

/// The same, for a join.
pub async fn drain_joined_warned(
    stream: tonic::Streaming<pb::JoinResponse>,
) -> (Vec<pb::JoinedRow>, Vec<String>) {
    let mut stream = stream;
    let (mut rows, mut warnings, mut first) = (Vec::new(), Vec::new(), true);
    while let Some(message) = stream.message().await.expect("a join message") {
        if first {
            warnings = message.warnings.clone();
            first = false;
        } else {
            assert!(
                message.warnings.is_empty(),
                "warnings belong on the first message only"
            );
        }
        rows.extend(message.rows);
    }
    (rows, warnings)
}

/// The same, for an aggregate.
pub async fn drain_groups_warned(
    stream: tonic::Streaming<pb::AggregateResponse>,
) -> (Vec<pb::Group>, Vec<String>) {
    let mut stream = stream;
    let (mut groups, mut warnings, mut first) = (Vec::new(), Vec::new(), true);
    while let Some(message) = stream.message().await.expect("an aggregate message") {
        if first {
            warnings = message.warnings.clone();
            first = false;
        } else {
            assert!(
                message.warnings.is_empty(),
                "warnings belong on the first message only"
            );
        }
        groups.extend(message.groups);
    }
    (groups, warnings)
}

/// An access hint naming an index that does not exist.
pub fn missing_index_hint() -> pb::AccessHint {
    pb::AccessHint {
        path: Some(pb::access_hint::Path::Index("by_nothing".to_owned())),
    }
}
