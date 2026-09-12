//! Typed ORM surface over the slate-orm record layer.
//!
//! This crate is deliberately thin. The record layer below it owns the
//! keyspace, index maintenance and — importantly — security enforcement, so
//! everything here is translation between Rust types and rows. Nothing is
//! checked at this level that is not also checked below it, because a check
//! that lived only here could be skipped by using the kernel directly.
//!
//! ```
//! use slate_orm::{Record, TableId, IndexId};
//! use uuid::Uuid;
//!
//! #[derive(Record)]
//! #[record(table = "users", id = 1, tenant = "tenant_id")]
//! struct User {
//!     #[record(pk)]
//!     tenant_id: Uuid,
//!     #[record(pk)]
//!     id: u64,
//!     #[record(index(name = "by_email", id = 10, unique))]
//!     email: String,
//!     nickname: Option<String>,
//! }
//!
//! let table = User::table();
//! assert_eq!(table.name(), "users");
//! assert_eq!(table.primary_key().len(), 2);
//! // `Option<String>` declared itself nullable, without the macro reading the type.
//! assert!(table.column(table.ordinal_of("nickname").unwrap()).unwrap().is_nullable());
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod ext;
pub mod field;
pub mod record;

pub use error::{OrmError, Result};
pub use ext::Records;
pub use field::{Field, FieldError};
pub use record::{Record, RecordError};

/// Derive [`Record`] for a struct. See the crate docs for the attributes.
pub use slate_derive::Record;

// The typed layer is not a wall around the kernel; re-export what a caller
// needs so they are not forced to depend on four crates to write a query.
pub use slate_kernel::{
    Access, Action, CmpOp, Expr, Grant, KernelError, KeyRange, Plan, Policy, Principal,
    QueryCursor, RecordStore, RecordTransaction, ScanOrder, SecurityCatalog, SecurityContext,
    Truth, memory,
};
pub use slate_schema::{
    Catalog, ColumnDef, IndexColumn, IndexDef, IndexId, Ordinal, Row, SchemaError, TableDef,
    TableId,
};
pub use slate_tuple::{Direction, Value, ValueType};
