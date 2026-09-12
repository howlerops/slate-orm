//! What a caller is asking for.
//!
//! Filter, order, projection, limit and offset arrive together because the
//! planner needs all of them at once: the projection decides whether an index
//! can answer without reading rows, and the limit decides how much of the
//! chosen path will be walked. Discovering either after planning means planning
//! for a query nobody asked.

use crate::expr::Expr;
use crate::plan::Projection;
use crate::store::ScanOrder;
use slate_schema::Ordinal;

/// A read request.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// Rows to keep.
    pub filter: Expr,
    /// Direction to walk the access path.
    pub order: ScanOrder,
    /// Columns to return.
    pub projection: Projection,
    /// Maximum rows to return.
    pub limit: Option<usize>,
    /// Rows to discard before returning any.
    pub offset: usize,
}

impl Default for Query {
    fn default() -> Self {
        Self::all()
    }
}

impl Query {
    /// Every row, every column.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            filter: Expr::True,
            order: ScanOrder::Ascending,
            projection: Projection::All,
            limit: None,
            offset: 0,
        }
    }

    /// Rows matching `filter`.
    #[must_use]
    pub fn filter(mut self, filter: Expr) -> Self {
        self.filter = filter;
        self
    }

    /// Add another condition, conjoined with whatever is already there.
    #[must_use]
    pub fn and(mut self, filter: Expr) -> Self {
        self.filter = core::mem::replace(&mut self.filter, Expr::True).and(filter);
        self
    }

    /// Walk the access path in `order`.
    #[must_use]
    pub const fn order(mut self, order: ScanOrder) -> Self {
        self.order = order;
        self
    }

    /// Walk the access path backwards.
    #[must_use]
    pub const fn descending(self) -> Self {
        self.order(ScanOrder::Descending)
    }

    /// Return only these columns.
    ///
    /// Naming fewer columns can remove the row read entirely; see
    /// [`Projection`].
    #[must_use]
    pub fn select<I: IntoIterator<Item = Ordinal>>(mut self, columns: I) -> Self {
        self.projection = Projection::Columns(columns.into_iter().collect());
        self
    }

    /// Return no columns, for counting.
    #[must_use]
    pub fn count_only(mut self) -> Self {
        self.projection = Projection::none();
        self
    }

    /// Return at most `limit` rows.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Discard the first `offset` matching rows.
    ///
    /// Costs the same as returning them: the rows still have to be found. A
    /// deep offset is a range predicate on the key waiting to be written.
    #[must_use]
    pub const fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    /// Rows the access path must produce to satisfy the limit *and* the offset.
    pub(crate) const fn planning_limit(&self) -> Option<usize> {
        match self.limit {
            Some(limit) => Some(limit.saturating_add(self.offset)),
            None => None,
        }
    }
}
