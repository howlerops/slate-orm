//! Schema and catalog metadata for the slate-orm record layer.
//!
//! Schemas are Rust values, not dynamic DDL. A table is defined once through a
//! builder that validates everything the kernel later assumes — key columns
//! exist and are non-nullable, index columns resolve, a tenant column leads the
//! key — so nothing downstream has to re-check it.
//!
//! ```
//! use slate_schema::{IndexDef, IndexId, TableDef, TableId};
//! use slate_tuple::ValueType;
//!
//! let users = TableDef::builder("users", TableId(1))
//!     .column("tenant_id", ValueType::Uuid)
//!     .column("id", ValueType::Uuid)
//!     .column("email", ValueType::Str)
//!     .nullable_column("display_name", ValueType::Str)
//!     .primary_key(["tenant_id", "id"])
//!     .tenant_column("tenant_id")
//!     .index(IndexDef::builder("by_email", IndexId(1)).column("email").unique())
//!     .build()
//!     .unwrap();
//!
//! assert_eq!(users.primary_key().len(), 2);
//! assert!(users.index_by_name("by_email").unwrap().is_unique());
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod catalog;
pub mod error;
pub mod row;
pub mod table;

pub use catalog::Catalog;
pub use error::{Result, SchemaError};
pub use row::{Row, decode_row, encode_body};
pub use table::{
    ColumnDef, IndexBuilder, IndexColumn, IndexDef, IndexId, Ordinal, TableBuilder, TableDef,
    TableId,
};
