//! Running a plan.
//!
//! The executor walks the access path the planner chose and evaluates the
//! residual predicate on every candidate row. Since the residual carries the
//! security filter (see [`crate::security`]), a row that reaches a caller has
//! passed the policy by evaluation, not by the planner having correctly turned
//! it into a range.

use crate::error::{KernelError, Result};
use crate::expr::Expr;
use crate::plan::{Access, Plan};
use crate::query::{NullsOrder, SortKey};
use crate::read::{self, IndexCursor, RowCursor};
use crate::store::KvSnapshot;
use futures::future::BoxFuture;
use futures::stream::{FuturesOrdered, StreamExt as _};
use slate_schema::{IndexDef, Row, TableDef};
use slate_tuple::Direction;
use std::sync::Arc;

/// Where a cursor's candidate rows come from.
enum Source<'a> {
    /// Rows read straight out of the table's key range.
    Rows(RowCursor<'a>),
    /// Primary keys read from an index, each fetched from the table.
    ///
    /// The fetches are overlapped rather than done one at a time: each is a
    /// round trip, and a hundred of them in sequence is a hundred round trips
    /// of waiting. See [`DEFAULT_PREFETCH`].
    Index {
        cursor: IndexCursor<'a>,
        index: &'a IndexDef,
        inflight: FuturesOrdered<BoxFuture<'a, Result<Option<Row>>>>,
        exhausted: bool,
    },
    /// Rows assembled from index entries, with no table read at all.
    CoveringIndex {
        cursor: IndexCursor<'a>,
        index: &'a IndexDef,
    },
    /// A single row, fetched by primary key.
    Point(Option<Row>),
    /// Rows already read, sorted, and waiting to be handed out.
    Sorted(std::vec::IntoIter<Row>),
    /// The plan proved there is nothing to read.
    Empty,
}

/// How many row reads an index scan keeps in flight at once.
///
/// Each read is a round trip, so issuing them one at a time makes a scan's
/// latency the sum of its lookups. Overlapping them makes it roughly the
/// slowest of each batch instead.
///
/// The number is a compromise. Too low and the waiting dominates; too high and
/// a query with a small limit fetches rows it will discard, and a burst of
/// concurrent requests each opens a pile of connections. Sixteen keeps the
/// waste bounded — at most fifteen wasted reads per cursor — while removing
/// most of the serialisation.
pub const DEFAULT_PREFETCH: usize = 16;

/// A cursor over the rows a plan admits.
pub struct QueryCursor<'a> {
    snapshot: &'a dyn KvSnapshot,
    table: &'a TableDef,
    source: Source<'a>,
    residual: Arc<Expr>,
    prefetch: usize,
    limit: Option<usize>,
    offset: usize,
    skipped: usize,
    yielded: usize,
}

impl core::fmt::Debug for QueryCursor<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("QueryCursor")
            .field("table", &self.table.name())
            .field("yielded", &self.yielded)
            .finish_non_exhaustive()
    }
}

/// Order two rows by `keys`, which is the comparison `ORDER BY` asks for.
///
/// [`Value`](slate_tuple::Value) already has a total order matching the storage
/// encoding, where nulls sort below everything; the only extra work is
/// respecting a caller who wants them at the other end.
fn compare_rows(left: &Row, right: &Row, keys: &[SortKey]) -> core::cmp::Ordering {
    use core::cmp::Ordering;

    for key in keys {
        let a = left.get(key.column);
        let b = right.get(key.column);
        let (Some(a), Some(b)) = (a, b) else { continue };

        let ordering = match (a.is_null(), b.is_null()) {
            (true, true) => Ordering::Equal,
            (true, false) | (false, true) => {
                let nulls_low = matches!(key.nulls, NullsOrder::First);
                let a_first = a.is_null() == nulls_low;
                // Null placement is absolute, so it is not flipped by the
                // direction the values are sorted in.
                return if a_first {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (false, false) => match key.direction {
                Direction::Asc => a.cmp(b),
                Direction::Desc => b.cmp(a),
            },
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// Rebuild a row from an index entry, leaving uncovered columns null.
///
/// Sound only when the planner has established that the query reads nothing
/// outside the index and the primary key; see `Access::IndexScan::covering`.
fn row_from_index_entry(
    table: &TableDef,
    index: &IndexDef,
    indexed: &[slate_tuple::Value],
    primary_key: &[slate_tuple::Value],
) -> Row {
    let mut values = vec![slate_tuple::Value::Null; table.columns().len()];
    for (column, value) in index.columns().iter().zip(indexed) {
        if let Some(slot) = values.get_mut(column.ordinal.0) {
            *slot = value.clone();
        }
    }
    for (ordinal, value) in table.primary_key().iter().zip(primary_key) {
        if let Some(slot) = values.get_mut(ordinal.0) {
            *slot = value.clone();
        }
    }
    Row::new(values)
}

impl<'a> QueryCursor<'a> {
    /// Open a cursor for `plan` on `table`.
    pub(crate) async fn open(
        snapshot: &'a dyn KvSnapshot,
        table: &'a TableDef,
        plan: Plan,
    ) -> Result<Self> {
        let source = match &plan.access {
            Access::Nothing => Source::Empty,
            Access::PointGet { key } => {
                Source::Point(read::read_row_unchecked(snapshot, table, key).await?)
            }
            Access::TableScan { range } => {
                Source::Rows(read::scan_rows(snapshot, table, range.clone(), plan.order).await?)
            }
            Access::IndexScan {
                index,
                range,
                covering,
            } => {
                let index = table
                    .index(*index)
                    .ok_or(KernelError::UnknownTable(table.id()))?;
                let cursor =
                    read::scan_index(snapshot, table, index, range.clone(), plan.order).await?;
                if *covering {
                    Source::CoveringIndex { cursor, index }
                } else {
                    Source::Index {
                        cursor,
                        index,
                        inflight: FuturesOrdered::new(),
                        exhausted: false,
                    }
                }
            }
        };
        let mut cursor = Self {
            snapshot,
            table,
            source,
            residual: plan.residual,
            prefetch: DEFAULT_PREFETCH,
            limit: None,
            offset: 0,
            skipped: 0,
            yielded: 0,
        };

        // No access path produced the requested order, so the rows have to be
        // collected and sorted. This is the one place the cursor stops being a
        // stream: nothing can be returned until everything has been read.
        if let Some(keys) = plan.sort {
            let mut rows = Vec::new();
            while let Some(row) = cursor.next_admitted().await? {
                rows.push(row);
            }
            rows.sort_by(|a, b| compare_rows(a, b, &keys));
            cursor.source = Source::Sorted(rows.into_iter());
            // The residual has already been applied to every row.
            cursor.residual = Arc::new(Expr::True);
        }
        Ok(cursor)
    }

    /// Stop after `limit` rows.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Change how many row reads are kept in flight. See [`DEFAULT_PREFETCH`].
    ///
    /// One disables overlapping entirely, which is what a caller wants when the
    /// cost of a wasted read is higher than the latency it saves.
    #[must_use]
    pub const fn prefetch(mut self, prefetch: usize) -> Self {
        self.prefetch = if prefetch == 0 { 1 } else { prefetch };
        self
    }

    /// Apply a limit and an offset together.
    #[must_use]
    pub(crate) const fn with_window(mut self, limit: Option<usize>, offset: usize) -> Self {
        self.limit = limit;
        self.offset = offset;
        self
    }

    /// The next admitted row.
    pub async fn next(&mut self) -> Result<Option<Row>> {
        if self.limit.is_some_and(|l| self.yielded >= l) {
            return Ok(None);
        }
        while let Some(row) = self.next_admitted().await? {
            // An offset still has to find the rows it discards; there is no
            // cheaper way to know which ones they are.
            if self.skipped < self.offset {
                self.skipped += 1;
                continue;
            }
            self.yielded += 1;
            return Ok(Some(row));
        }
        Ok(None)
    }

    /// The next row that passes the residual, ignoring the limit and offset.
    async fn next_admitted(&mut self) -> Result<Option<Row>> {
        while let Some(row) = self.next_candidate().await? {
            if self.residual.admits(&row) {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }

    async fn next_candidate(&mut self) -> Result<Option<Row>> {
        match &mut self.source {
            Source::Empty => Ok(None),
            Source::Sorted(rows) => Ok(rows.next()),
            Source::Point(row) => Ok(row.take()),
            Source::Rows(cursor) => cursor.next().await,
            Source::CoveringIndex { cursor, index } => {
                // The entry already holds every column this query reads, so
                // there is nothing to fetch. This is the whole point of a
                // covering index: no read per matching row.
                let Some((indexed, primary_key)) = cursor.next().await? else {
                    return Ok(None);
                };
                Ok(Some(row_from_index_entry(
                    self.table,
                    index,
                    &indexed,
                    &primary_key,
                )))
            }
            Source::Index {
                cursor,
                index,
                inflight,
                exhausted,
            } => {
                loop {
                    // Keep the pipeline full. Reading the next key from the
                    // index is a scan step and cheap; the row read it implies is
                    // a round trip, so the reads are issued together and
                    // collected in order.
                    while !*exhausted && inflight.len() < self.prefetch {
                        match cursor.next().await? {
                            Some((_, primary_key)) => {
                                let snapshot = self.snapshot;
                                let table = self.table;
                                inflight.push_back(Box::pin(async move {
                                    read::read_row_unchecked(snapshot, table, &primary_key).await
                                }));
                            }
                            None => *exhausted = true,
                        }
                    }

                    match inflight.next().await {
                        Some(Ok(Some(row))) => return Ok(Some(row)),
                        Some(Err(error)) => return Err(error),
                        // Index entries and rows are written in one
                        // transaction, so on a point-in-time view an entry
                        // without a row means the two have diverged on disk. On
                        // a replica that follows the manifest it means only that
                        // the view advanced between the two reads.
                        Some(Ok(None)) => {
                            if self.snapshot.is_point_in_time() {
                                return Err(KernelError::CorruptIndexEntry {
                                    table: self.table.name().to_owned(),
                                    index: index.name().to_owned(),
                                });
                            }
                        }
                        None => return Ok(None),
                    }
                }
            }
        }
    }

    /// Drain the cursor into a vector.
    pub async fn collect(mut self) -> Result<Vec<Row>> {
        let mut out = Vec::new();
        while let Some(row) = self.next().await? {
            out.push(row);
        }
        Ok(out)
    }

    /// Count the admitted rows.
    pub async fn count(mut self) -> Result<usize> {
        let mut n = 0;
        while self.next().await?.is_some() {
            n += 1;
        }
        Ok(n)
    }
}
