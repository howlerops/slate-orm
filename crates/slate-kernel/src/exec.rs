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
use crate::read::{self, IndexCursor, RawRow, RowCursor};
use crate::store::KvSnapshot;
use futures::future::BoxFuture;
use futures::stream::{FuturesOrdered, StreamExt as _};
use slate_schema::{ColumnSet, IndexDef, Row, TableDef, decode_row_columns};
use slate_tuple::Direction;
use std::sync::Arc;

/// Where a cursor's candidate rows come from.
enum Source<'a> {
    /// Rows read straight out of the table's key range, still encoded.
    Rows(RowCursor<'a>),
    /// Primary keys read from an index, each fetched from the table.
    ///
    /// The fetches are overlapped rather than done one at a time: each is a
    /// round trip, and a hundred of them in sequence is a hundred round trips
    /// of waiting. See [`DEFAULT_PREFETCH`].
    Index {
        cursor: IndexCursor<'a>,
        index: &'a IndexDef,
        inflight: FuturesOrdered<BoxFuture<'a, Result<Option<RawRow>>>>,
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
    /// Two-phase decoding: filter on these, then materialise the rest.
    ///
    /// Skipping a column is advancing a cursor; decoding one can be an
    /// allocation. On a scan that rejects most rows, the difference is most of
    /// the work.
    filter_columns: ColumnSet,
    output_columns: ColumnSet,
    /// Whether the output needs anything the filter did not already decode.
    needs_second_phase: bool,
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

/// Decode `raw` far enough to filter it, then far enough to return it.
///
/// The first pass reads only what the predicate needs; the second runs only for
/// rows that survived. A row the filter rejects never pays to decode the
/// columns its caller asked for, which on a selective scan is most of the rows.
///
/// A free function rather than a method so the caller can hold a mutable borrow
/// of the cursor's source while it runs.
fn materialise(
    table: &TableDef,
    residual: &Expr,
    filter_columns: &ColumnSet,
    output_columns: &ColumnSet,
    two_phase: bool,
    raw: &RawRow,
) -> Result<Option<Row>> {
    // One pass when the residual is not expected to reject much: decoding the
    // output columns anyway costs less than walking the row twice.
    let first = if two_phase {
        filter_columns
    } else {
        output_columns
    };
    let decoded = decode_row_columns(table, &raw.primary_key, &raw.body, wanted(first, table))?;
    if !residual.admits(&decoded) {
        return Ok(None);
    }
    if !two_phase {
        return Ok(Some(decoded));
    }
    Ok(Some(decode_row_columns(
        table,
        &raw.primary_key,
        &raw.body,
        wanted(output_columns, table),
    )?))
}

/// `None` when the set holds every column, so the decoder can stop asking.
fn wanted<'a>(columns: &'a ColumnSet, table: &TableDef) -> Option<&'a ColumnSet> {
    if columns.covers_all(table.columns().len()) {
        None
    } else {
        Some(columns)
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
            needs_second_phase: plan.filter_first
                && !plan.predicate_columns.contains_all(&plan.output_columns),
            filter_columns: plan.predicate_columns,
            output_columns: plan.output_columns,
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
        // Both row-reading sources decode the same way: filter on the
        // predicate's columns, then materialise the rest. Keeping that in one
        // place is not only less code — having a projection honoured on a table
        // scan and ignored on an index scan is exactly the kind of difference
        // nobody notices until a plan changes.
        let Self {
            source,
            table,
            snapshot,
            residual,
            filter_columns,
            output_columns,
            needs_second_phase,
            prefetch,
            ..
        } = self;

        match source {
            Source::Rows(cursor) => {
                while let Some(raw) = cursor.next_raw().await? {
                    if let Some(row) = materialise(
                        table,
                        residual,
                        filter_columns,
                        output_columns,
                        *needs_second_phase,
                        &raw,
                    )? {
                        return Ok(Some(row));
                    }
                }
                Ok(None)
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
                    while !*exhausted && inflight.len() < *prefetch {
                        match cursor.next().await? {
                            Some((_, primary_key)) => {
                                let snapshot = *snapshot;
                                let table = *table;
                                inflight.push_back(Box::pin(async move {
                                    let body =
                                        read::read_row_body(snapshot, table, &primary_key).await?;
                                    Ok(body.map(|body| RawRow { primary_key, body }))
                                }));
                            }
                            None => *exhausted = true,
                        }
                    }

                    match inflight.next().await {
                        Some(Ok(Some(raw))) => {
                            if let Some(row) = materialise(
                                table,
                                residual,
                                filter_columns,
                                output_columns,
                                *needs_second_phase,
                                &raw,
                            )? {
                                return Ok(Some(row));
                            }
                        }
                        Some(Err(error)) => return Err(error),
                        // Index entries and rows are written in one transaction,
                        // so on a point-in-time view an entry without a row means
                        // the two have diverged on disk. On a replica that
                        // follows the manifest it means only that the view
                        // advanced between the two reads.
                        Some(Ok(None)) => {
                            if snapshot.is_point_in_time() {
                                return Err(KernelError::CorruptIndexEntry {
                                    table: table.name().to_owned(),
                                    index: index.name().to_owned(),
                                });
                            }
                        }
                        None => return Ok(None),
                    }
                }
            }
            _ => {
                while let Some(row) = self.next_candidate().await? {
                    if self.residual.admits(&row) {
                        return Ok(Some(row));
                    }
                }
                Ok(None)
            }
        }
    }

    async fn next_candidate(&mut self) -> Result<Option<Row>> {
        match &mut self.source {
            Source::Empty => Ok(None),
            Source::Sorted(rows) => Ok(rows.next()),
            Source::Point(row) => Ok(row.take()),
            Source::Rows(_) => unreachable!("handled by next_admitted"),
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
            Source::Index { .. } => unreachable!("handled by next_admitted"),
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
