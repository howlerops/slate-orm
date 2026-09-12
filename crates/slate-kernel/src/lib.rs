//! The slate-orm record layer kernel.
//!
//! This is where the value of the project lives. The kernel owns the keyspace,
//! the record store and — critically — the security enforcement point: policies
//! compile into scan bounds and mandatory filters *here*, below any typed API,
//! so a caller that goes around the ORM does not go around the policy.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod keys;
pub mod memory;
pub mod record;
pub mod store;

pub use error::{KernelError, Result, StorageError};
pub use keys::{IndexEntry, decode_index_entry, decode_row_key, index_entry, row_key};
pub use record::{IndexCursor, RecordStore, RecordTransaction, RowCursor};
pub use store::{KeyRange, KeyValue, KvIterator, KvStore, KvTransaction, ScanOrder};
