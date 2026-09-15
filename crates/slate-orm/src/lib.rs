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
pub mod relation;

pub use error::{OrmError, Result};
pub use ext::{Page, Records};
pub use field::{Field, FieldError, Units};
pub use record::{Record, RecordError};
pub use relation::{Related, load_one_related, load_related, related_filter};

/// Derive [`Record`] for a struct. See the crate docs for the attributes.
///
/// # Partial indexes
///
/// `only_where(...)` on an index attribute holds a predicate, and the index
/// then holds an entry only for the rows that predicate admits. It is written
/// as an ordinary Rust expression in which every **field** of the struct names
/// its own [`Ordinal`], because the ordinals are what
/// [`IndexBuilder::only_where`](slate_schema::IndexBuilder::only_where) takes
/// and resolving them from a name at runtime would mean building the table the
/// predicate is part of.
///
/// ```
/// use slate_orm::{Expr, Record};
///
/// #[derive(Record)]
/// #[record(table = "docs", id = 1)]
/// #[record(index(
///     name = "live_by_author",
///     id = 11,
///     columns("author"),
///     only_where(Expr::is_null(deleted_at))
/// ))]
/// struct Doc {
///     #[record(pk)]
///     id: u64,
///     author: u64,
///     deleted_at: Option<i64>,
/// }
///
/// let index = Doc::table().index(slate_orm::IndexId(11)).unwrap();
/// assert!(index.predicate().is_some());
/// ```
///
/// A name that is not a field of the struct does not compile. The predicate is
/// the one attribute the macro cannot check itself — it is arbitrary Rust — so
/// what protects a typo from becoming an index over the wrong column is that
/// nothing else of that name is in scope:
///
/// ```compile_fail
/// use slate_orm::{Expr, Record};
///
/// #[derive(Record)]
/// #[record(table = "docs", id = 1)]
/// #[record(index(name = "live", id = 11, columns("author"),
///                only_where(Expr::is_null(delted_at))))]
/// struct Doc {
///     #[record(pk)]
///     id: u64,
///     author: u64,
///     deleted_at: Option<i64>,
/// }
/// ```
///
/// Nor does a predicate that is not an [`Expr`]. The builder accepts any
/// [`Predicate`](slate_schema::Predicate) and the write path can run any of
/// them, but the planner reads one only by downcasting it to `Expr` — so an
/// index declared with anything else is not a broken index, it is one no query
/// ever chooses. The macro pins the type rather than emit that:
///
/// ```compile_fail
/// use slate_orm::Record;
/// use slate_schema::{Predicate, Row};
///
/// struct Everything;
/// impl Predicate for Everything {
///     fn truth(&self, _row: &Row) -> Option<bool> {
///         Some(true)
///     }
/// }
///
/// #[derive(Record)]
/// #[record(table = "docs", id = 1)]
/// #[record(index(name = "live", id = 11, columns("author"),
///                only_where(Everything)))]
/// struct Doc {
///     #[record(pk)]
///     id: u64,
///     author: u64,
/// }
/// ```
///
/// Two predicates on one index are refused rather than merged or last-wins,
/// since either of those leaves a predicate in the source that reads as though
/// it were in force and is not. Terms are combined with `Expr::and`.
///
/// ```compile_fail
/// use slate_orm::{Expr, Record};
///
/// #[derive(Record)]
/// #[record(table = "docs", id = 1)]
/// #[record(index(name = "live", id = 11, columns("author"),
///                only_where(Expr::is_null(deleted_at)),
///                only_where(Expr::eq(author, slate_orm::Value::U64(1)))))]
/// struct Doc {
///     #[record(pk)]
///     id: u64,
///     author: u64,
///     deleted_at: Option<i64>,
/// }
/// ```
///
/// The same predicate handed straight to the builder is accepted, which is what
/// makes the refusal above the macro's doing rather than the trait's:
///
/// ```
/// use slate_orm::{IndexDef, IndexId, TableDef, TableId, ValueType};
/// use slate_schema::{Predicate, Row};
///
/// struct Everything;
/// impl Predicate for Everything {
///     fn truth(&self, _row: &Row) -> Option<bool> {
///         Some(true)
///     }
/// }
///
/// let table = TableDef::builder("docs", TableId(1))
///     .column("id", ValueType::U64)
///     .column("author", ValueType::U64)
///     .primary_key(["id"])
///     .index(
///         IndexDef::builder("live", IndexId(11))
///             .column("author")
///             .only_where(Everything),
///     )
///     .build()
///     .unwrap();
/// // Maintained, and never planned for: the planner cannot read it.
/// assert!(table.index(IndexId(11)).unwrap().predicate().unwrap().as_any().is_none());
/// ```
/// # Relationships
///
/// `has_many` and `belongs_to` emit a [`Related`] impl, which
/// [`load_related`] and [`load_one_related`] use to fetch every parent's
/// related rows in one read.
///
/// ```
/// use slate_orm::{Ordinal, Record, Related};
///
/// #[derive(Record)]
/// #[record(table = "authors", id = 1)]
/// #[record(has_many(Book, foreign = author_id))]
/// struct Author {
///     #[record(pk)]
///     id: u64,
/// }
///
/// #[derive(Record)]
/// #[record(table = "books", id = 2)]
/// #[record(belongs_to(Author, local = author_id))]
/// struct Book {
///     #[record(pk)]
///     id: u64,
///     title: String,
///     author_id: u64,
/// }
///
/// // The local side defaults to this struct's primary key; the foreign side
/// // is the named field of the other one, wherever it happens to sit.
/// assert_eq!(<Author as Related<Book>>::local(), Ordinal(0));
/// assert_eq!(<Author as Related<Book>>::foreign(), Ordinal(2));
/// // And the other way, with the foreign side defaulted to what a foreign key
/// // points at: the other side's primary key.
/// assert_eq!(<Book as Related<Author>>::local(), Ordinal(2));
/// assert_eq!(<Book as Related<Author>>::foreign(), Ordinal(0));
/// ```
///
/// Columns are named by **field ident**, not by a string, so the compiler
/// checks them. A string would be resolved at runtime against a table built
/// from the other type, which turns a typo into a panic on first use — or, if
/// the typo happens to name a real column, into a relationship over the wrong
/// one. The foreign side is emitted as `Other::COLUMNS.field`, so it does not
/// compile:
///
/// ```compile_fail
/// use slate_orm::Record;
///
/// #[derive(Record)]
/// #[record(table = "authors", id = 1)]
/// #[record(has_many(Book, foreign = auther_id))]
/// struct Author {
///     #[record(pk)]
///     id: u64,
/// }
///
/// #[derive(Record)]
/// #[record(table = "books", id = 2)]
/// struct Book {
///     #[record(pk)]
///     id: u64,
///     author_id: u64,
/// }
/// ```
///
/// The local side is checked in the macro, which can see this struct's fields
/// and so reports it on the attribute:
///
/// ```compile_fail
/// use slate_orm::Record;
///
/// #[derive(Record)]
/// #[record(table = "books", id = 2)]
/// #[record(belongs_to(Author, local = auther_id))]
/// struct Book {
///     #[record(pk)]
///     id: u64,
///     author_id: u64,
/// }
/// # #[derive(Record)]
/// # #[record(table = "authors", id = 1)]
/// # struct Author { #[record(pk)] id: u64 }
/// ```
///
/// Neither side is guessed when guessing could be wrong. A `has_many` has no
/// default for its foreign column, because the child's primary key is not its
/// foreign key:
///
/// ```compile_fail
/// use slate_orm::Record;
///
/// #[derive(Record)]
/// #[record(table = "authors", id = 1)]
/// #[record(has_many(Book))]
/// struct Author {
///     #[record(pk)]
///     id: u64,
/// }
/// # #[derive(Record)]
/// # #[record(table = "books", id = 2)]
/// # struct Book { #[record(pk)] id: u64, author_id: u64 }
/// ```
///
/// and a `has_many` on a struct with a composite primary key has no single
/// local column either. Answering with the first would be a match on a key
/// prefix, which under `#[record(tenant = ...)]` means relating across every
/// tenant:
///
/// ```compile_fail
/// use slate_orm::Record;
///
/// #[derive(Record)]
/// #[record(table = "authors", id = 1, tenant = "house")]
/// #[record(has_many(Book, foreign = author_id))]
/// struct Author {
///     #[record(pk)]
///     house: u64,
///     #[record(pk)]
///     id: u64,
/// }
/// # #[derive(Record)]
/// # #[record(table = "books", id = 2)]
/// # struct Book { #[record(pk)] id: u64, author_id: u64 }
/// ```
///
/// A `belongs_to` has no default for its *local* column either, because a
/// struct may belong to two things and its own primary key is neither of them.
/// Defaulting it to the primary key would compile, and relate on the wrong
/// column:
///
/// ```compile_fail
/// use slate_orm::Record;
///
/// #[derive(Record)]
/// #[record(table = "books", id = 2)]
/// #[record(belongs_to(Author))]
/// struct Book {
///     #[record(pk)]
///     id: u64,
///     author_id: u64,
/// }
/// # #[derive(Record)]
/// # #[record(table = "authors", id = 1)]
/// # struct Author { #[record(pk)] id: u64 }
/// ```
///
/// A `belongs_to` *does* default its foreign column, to the other side's
/// primary key, because that is what a foreign key points at. When that key is
/// composite there is no single column and it panics on first use rather than
/// pick one; name it with `foreign = <field>` instead.
pub use slate_derive::Record;

// The typed layer is not a wall around the kernel; re-export what a caller
// needs so they are not forced to depend on four crates to write a query.
pub use slate_kernel::{
    Access, AccessSummary, Action, Aggregate, Chain, ChainCursor, ChainPlan, ChainRow, CmpOp,
    ColumnStats, Explanation, Expr, Grant, Group, Join, JoinAlgorithm, JoinCursor, JoinExplanation,
    JoinKey, JoinSchema, JoinStep, JoinStepPlan, JoinType, JoinedRow, KernelError, KeyRange,
    MigrationPlan, MigrationReport, MigrationStep, NullsOrder, Plan, Policy, Principal, Projection,
    Query, QueryCursor, RecordSnapshot, RecordStore, RecordTransaction, Refusal, ReplicaPool,
    RetryPolicy, ScanOrder, SecurityCatalog, SecurityContext, Side, SortKey, Statistics,
    TableState, TableStats, Truth, latency, memory, migrate,
};
pub use slate_schema::{
    Catalog, ColumnDef, IndexColumn, IndexDef, IndexId, Ordinal, Row, SchemaError, TableDef,
    TableId,
};
pub use slate_tuple::{Direction, Value, ValueType};
