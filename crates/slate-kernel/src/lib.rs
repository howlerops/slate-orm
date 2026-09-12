//! The slate-orm record layer kernel.
//!
//! This is where the value of the project lives. The kernel owns the keyspace,
//! the record store and — critically — the security enforcement point: policies
//! compile into scan bounds and mandatory filters *here*, below any typed API,
//! so a caller that goes around the ORM does not go around the policy.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod exec;
pub mod expr;
pub mod keys;
pub mod latency;
pub mod memory;
pub mod plan;
pub mod pool;
pub mod read;
pub mod record;
pub mod retry;
pub mod security;
pub mod store;
pub mod token;

pub use error::{KernelError, Result, StorageError};
pub use exec::QueryCursor;
pub use expr::{CmpOp, Expr, Truth};
pub use keys::{IndexEntry, decode_index_entry, decode_row_key, index_entry, row_key};
pub use plan::{Access, Plan, plan};
pub use pool::{ReplicaPool, RoutingPolicy};
pub use read::{IndexCursor, RowCursor};
pub use record::{RecordSnapshot, RecordStore, RecordTransaction};
pub use retry::{RetryPolicy, with_retries};
pub use security::{
    Action, Grant, Policy, PolicyPredicate, Principal, SecurityCatalog, SecurityContext,
};
pub use store::{
    KeyRange, KeyValue, KvIterator, KvReadStore, KvSnapshot, KvStore, KvTransaction, ScanOrder,
};
pub use token::{Freshness, ReadToken, ReadWatermark};
