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
use slate_tuple::Direction;

/// Where nulls go in a sort.
///
/// The default is where the storage layout already puts them: nulls sort below
/// every value, so they come first ascending and last descending. Asking for
/// the other arrangement is allowed and means the rows have to be sorted, since
/// no index is stored that way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullsOrder {
    /// Nulls before values.
    First,
    /// Nulls after values.
    Last,
}

impl NullsOrder {
    /// Where the encoding already puts nulls, for a column stored in
    /// `direction`.
    #[must_use]
    pub const fn natural_for(direction: Direction) -> Self {
        match direction {
            Direction::Asc => Self::First,
            Direction::Desc => Self::Last,
        }
    }
}

/// One column of an `ORDER BY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    /// The column to order by.
    pub column: Ordinal,
    /// Ascending or descending.
    pub direction: Direction,
    /// Where nulls go.
    pub nulls: NullsOrder,
}

impl SortKey {
    /// Ascending, nulls where the storage puts them.
    #[must_use]
    pub const fn asc(column: Ordinal) -> Self {
        Self {
            column,
            direction: Direction::Asc,
            nulls: NullsOrder::First,
        }
    }

    /// Descending, nulls where the storage puts them.
    #[must_use]
    pub const fn desc(column: Ordinal) -> Self {
        Self {
            column,
            direction: Direction::Desc,
            nulls: NullsOrder::Last,
        }
    }

    /// Put nulls first regardless of direction.
    #[must_use]
    pub const fn nulls_first(mut self) -> Self {
        self.nulls = NullsOrder::First;
        self
    }

    /// Put nulls last regardless of direction.
    #[must_use]
    pub const fn nulls_last(mut self) -> Self {
        self.nulls = NullsOrder::Last;
        self
    }

    /// Whether this key matches how a column stored in `direction` is already
    /// laid out, when walked in `order`.
    #[must_use]
    pub const fn matches_storage(self, stored: Direction, order: ScanOrder) -> bool {
        let effective = match (stored, order) {
            (Direction::Asc, ScanOrder::Ascending) | (Direction::Desc, ScanOrder::Descending) => {
                Direction::Asc
            }
            _ => Direction::Desc,
        };
        matches!(
            (self.direction, effective),
            (Direction::Asc, Direction::Asc) | (Direction::Desc, Direction::Desc)
        ) && matches!(
            (self.nulls, NullsOrder::natural_for(effective)),
            (NullsOrder::First, NullsOrder::First) | (NullsOrder::Last, NullsOrder::Last)
        )
    }
}

/// An access path a caller insists on, instead of the cheapest one.
///
/// For measuring what the planner's alternatives would have cost, and for a
/// caller who knows something the statistics do not. A hint that cannot be
/// honoured — an index that does not exist, or one the projection is not
/// covered by when a covering scan was asked for — is ignored rather than
/// refused: a hint is advice, and a query that stops working because an index
/// was renamed is worse than one that gets slower.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessHint {
    /// Read the table's own key range, whatever an index would have offered.
    TableScan,
    /// Walk this index.
    Index(slate_schema::IndexId),
}

/// A read request.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// Rows to keep.
    pub filter: Expr,
    /// Direction to walk the access path.
    pub order: ScanOrder,
    /// Columns to return.
    pub projection: Projection,
    /// Columns to order by. Empty means whatever order the access path gives.
    pub sort: Vec<SortKey>,
    /// Maximum rows to return.
    pub limit: Option<usize>,
    /// Rows to discard before returning any.
    pub offset: usize,
    /// An access path to use instead of the cheapest one. See [`AccessHint`].
    pub hint: Option<AccessHint>,
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
            sort: Vec::new(),
            limit: None,
            offset: 0,
            hint: None,
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

    /// Order the results.
    ///
    /// When an index already produces this order the rows stream out of it. When
    /// none does, the whole result is materialised and sorted, which is why a
    /// limit cannot make an unordered plan cheap: every matching row has to be
    /// found before the first one can be returned.
    #[must_use]
    pub fn sort_by<I: IntoIterator<Item = SortKey>>(mut self, keys: I) -> Self {
        self.sort = keys.into_iter().collect();
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

    /// Read through this index rather than the cheapest path.
    #[must_use]
    pub const fn using_index(mut self, index: slate_schema::IndexId) -> Self {
        self.hint = Some(AccessHint::Index(index));
        self
    }

    /// Read the table's own key range rather than the cheapest path.
    #[must_use]
    pub const fn using_table_scan(mut self) -> Self {
        self.hint = Some(AccessHint::TableScan);
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
