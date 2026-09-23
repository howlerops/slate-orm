//! The schema every measurement here runs against, and the identities that
//! reach it.
//!
//! One tenant-scoped table with two secondary indexes: tenant scoping so the
//! routing measurement has something to key affinity on, and two indexes so a
//! write costs what a write costs rather than what the cheapest possible write
//! costs.

use slate_kernel::{Action, Grant, Principal, SecurityCatalog, SecurityContext};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_server::auth::{PRINCIPAL_KEY, ROLES_KEY, TENANT_KEY};
use slate_tuple::{Direction, Value, ValueType};
use tonic::Request;

/// The one table.
pub const EVENTS: TableId = TableId(1);

/// A tenant-scoped table with two secondary indexes.
#[must_use]
pub fn events() -> TableDef {
    TableDef::builder("events", EVENTS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("actor", ValueType::Str)
        .column("at", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_kind", IndexId(10)).column("kind"))
        .index(IndexDef::builder("by_at_desc", IndexId(12)).column_with("at", Direction::Desc))
        .build()
        .expect("the fixture schema is valid")
}

/// The catalog the head node and the in-process comparison both serve.
#[must_use]
pub fn catalog() -> Catalog {
    Catalog::from_tables([events()]).expect("the fixture catalog is valid")
}

/// One role, with every action on the one table.
///
/// `EVERYTHING` rather than `ALL`, and the difference is not cosmetic: `ALL`
/// is the four *data* actions and deliberately excludes `Explain`, so a role
/// holding it cannot run the `explain` this crate measures. It said `ALL` for
/// as long as nothing ran `head_report`, whose `rpc` section panicked on
/// `PERMISSION_DENIED` at the explain call — the same shape as the
/// `leadership` breakage, from the commit that made `EXPLAIN` privileged
/// rather than the one that made `leadership` authenticate.
///
/// Widening a *benchmark* fixture's grant is not the usual answer to a denial.
/// It is the answer here because the grant is the subject of no measurement:
/// this crate times the head node, and a role that cannot reach the call being
/// timed measures nothing. `slate-server`'s own suites are where the narrow
/// grants are asserted.
///
/// No row policy: a policy would add a predicate to every read, which is a
/// cost of the security layer rather than of the head node, and this crate is
/// measuring the head node.
#[must_use]
pub fn security() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("app", EVENTS, Action::EVERYTHING))
}

/// A row. `at` descends with `id` so the descending index is not written in
/// key order.
#[must_use]
pub fn row(tenant: u64, id: u64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 25)),
        Value::Str(format!("actor-{}", id % 500)),
        Value::I64(-(id as i64)),
        Value::Null,
    ])
}

/// The in-process equivalent of what [`principal_request`] puts on the wire.
///
/// The two must agree, or the comparison in `head_report`'s first section is
/// between two different reads rather than between two transports.
#[must_use]
pub fn context(tenant: u64) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(tenant))
            .with_role("app"),
    )
}

/// A request carrying the identity a proxy in front of a real deployment would
/// set. The head node never reads an identity out of a request body.
pub fn principal_request<T>(message: T, tenant: u64) -> Request<T> {
    let mut request = Request::new(message);
    let metadata = request.metadata_mut();
    metadata.insert(PRINCIPAL_KEY, "u64:1".parse().expect("ascii"));
    metadata.insert(TENANT_KEY, format!("u64:{tenant}").parse().expect("ascii"));
    metadata.insert(ROLES_KEY, "app".parse().expect("ascii"));
    request
}
