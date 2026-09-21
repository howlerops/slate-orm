//! A name bound to a predicate over one table, held beside the catalog.
//!
//! # Why this type is so thin
//!
//! A [`View`] is a base table's name and an [`Expr`], and nothing else. The
//! reasoning is in `docs/views.md`; the three sentences that decide the shape
//! of this file:
//!
//! - **§1** a view must not be a `TableDef`. `row_filter_with` keys row-level
//!   security on a `TableId`, and a view's own id has no policy, so a view
//!   with an id would come back `Expr::True` over the base table's rows. A
//!   view therefore names its base table and a read through one authorises
//!   against *that*.
//! - **§2** a view is not a privilege boundary. The caller needs the grant on
//!   the base table, and holding it can read past the view by naming the table
//!   directly. Nothing here pretends otherwise.
//! - **§3a** a view kept out of the [`Catalog`](slate_schema::Catalog) is
//!   refused by every path that resolves a table name, because there is
//!   nothing to find. That is why this is a separate map and why a read path
//!   has to opt in by name rather than opt out.
//!
//! # Why no SQL
//!
//! The predicate arrives already lowered. `slate-serverd` parses the operator's
//! `SELECT` with `slate-sql` and hands the result over, so this crate — which
//! every client harness links — does not gain a SQL parser it would only use
//! at startup. It also means a caller embedding `slate-server` as a library
//! can declare a view by building an `Expr`, without writing SQL to have it
//! parsed straight back.
//!
//! The ordinals in that `Expr` are the **base table's**, which is what makes
//! composition a plain `and`: the caller's own filter was resolved against the
//! same `TableDef`. A view that could narrow *columns* would break exactly
//! that, and is refused at load for this reason among others.

use slate_kernel::Expr;
use std::collections::BTreeMap;

/// One view: the table it reads, and the predicate to `AND` onto a caller's.
#[derive(Debug, Clone)]
pub struct View {
    /// The base table's name, resolved against the catalog on every read.
    ///
    /// A name and not a [`TableId`](slate_schema::TableId), so the resolution
    /// goes through the same `authorized_table` every other read uses and
    /// cannot skip the grant check by holding an id nobody looked up.
    pub table: String,
    /// Rows the view admits, in the base table's ordinals.
    pub predicate: Expr,
}

impl View {
    /// A view over `table` admitting the rows `predicate` keeps.
    #[must_use]
    pub fn new(table: impl Into<String>, predicate: Expr) -> Self {
        Self {
            table: table.into(),
            predicate,
        }
    }
}

/// Every view a node serves, by the name a query uses.
///
/// Deliberately not a `Catalog`: see the module docs. Nothing here is
/// reachable from `Catalog::table_by_name`, which is what keeps every read
/// path that has not opted in refusing a view without carrying a check for
/// one.
pub type Views = BTreeMap<String, View>;
