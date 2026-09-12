//! Running a plan.
//!
//! The executor walks the access path the planner chose and evaluates the
//! residual predicate on every candidate row. Since the residual carries the
//! security filter (see [`crate::security`]), a row that reaches a caller has
//! passed the policy by evaluation, not by the planner having correctly turned
//! it into a range.
//!
//! # Computed values, and what a row comes back holding
//!
//! Values a query computes are appended after the table's own columns, and two
//! rules keep them from making a row's contents depend on its plan:
//!
//! - the columns a computed value *reads* are decoded and then put back to
//!   null, because they are not part of the answer — see `transient`;
//! - the one computed value an index entry already holds is taken from the
//!   entry rather than evaluated, which is what lets an index keyed on
//!   `lower(title)` answer a query without reading a row — see `from_entry`.
//!
//! The second is only sound because of the first. An entry keyed on a computed
//! value cannot produce the column underneath it, so a scan of one returns null
//! there; if a table scan of the same query returned the column, the two paths
//! would answer differently.

use crate::error::{KernelError, Result};
use crate::expr::Expr;
use crate::plan::{Access, Plan};
use crate::query::{NullsOrder, SortKey};
use crate::read::{self, IndexCursor, RawRow, RowCursor};
use crate::scalar::Scalar;
use crate::store::{KeyRange, KvSnapshot, ScanOrder};
use futures::future::BoxFuture;
use futures::stream::{FuturesOrdered, StreamExt as _};
use slate_schema::{ColumnSet, IndexDef, IndexId, Ordinal, Row, TableDef, decode_row_columns};
use slate_tuple::{Direction, Value};
use std::collections::BinaryHeap;
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
        /// Ranges still to walk, for `Access::IndexScans`. Empty for an
        /// ordinary single-range scan, which is the same code path with
        /// nothing left over.
        rest: std::vec::IntoIter<KeyRange>,
        order: ScanOrder,
        inflight: FuturesOrdered<BoxFuture<'a, Result<Option<RawRow>>>>,
        exhausted: bool,
    },
    /// Rows assembled from index entries, with no table read at all.
    CoveringIndex {
        cursor: IndexCursor<'a>,
        index: &'a IndexDef,
        rest: std::vec::IntoIter<KeyRange>,
        order: ScanOrder,
    },
    /// A single row, fetched by primary key.
    Point(Option<Row>),
    /// Several rows, fetched by primary key and overlapped.
    ///
    /// The reads go out together for the same reason an index scan's do: each
    /// is a round trip, and a set of them issued in turn is a set of round
    /// trips waited for in turn.
    Points {
        keys: std::vec::IntoIter<Vec<Value>>,
        inflight: FuturesOrdered<BoxFuture<'a, Result<Option<RawRow>>>>,
        exhausted: bool,
    },
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

/// A row carrying the order it is ranked by, so a heap can compare two.
///
/// The keys are shared rather than copied per row: a heap of ten thousand
/// would otherwise hold ten thousand copies of the same `ORDER BY`.
struct Ranked {
    row: Row,
    keys: Arc<[SortKey]>,
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == core::cmp::Ordering::Equal
    }
}

impl Eq for Ranked {}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        compare_rows(&self.row, &other.row, &self.keys)
    }
}

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
    /// Columns to decode: the answer's own, plus whatever the computed values
    /// read. Not the same set — see `transient`.
    decode_columns: ColumnSet,
    /// Whether the output needs anything the filter did not already decode.
    needs_second_phase: bool,
    /// Columns decoded only because a computed value reads them, which are
    /// therefore not part of the answer.
    ///
    /// `SELECT id, lower(title)` has to read `title` and did not ask for it,
    /// and a row that carried it anyway would make the contents of a row
    /// depend on the plan: an expression index holds `lower(title)` and not
    /// `title`, so a covering scan of one *cannot* return the column, and a
    /// table scan of the same query must not either. Nulled after the computed
    /// values are evaluated, which is the last moment anything needs them.
    ///
    /// A `Vec` rather than a [`ColumnSet`]: this is walked once per row and is
    /// almost always empty or a single entry, where a bitset would be walked
    /// word by word to find the same one or two ordinals.
    transient: Vec<Ordinal>,
    /// Which computed value, if any, comes out of the index entry rather than
    /// being evaluated. See [`crate::plan::expression_position`].
    from_entry: Option<usize>,
    /// Values computed per row and appended after the table's own columns, so
    /// a filter, sort or grouping can name one by ordinal.
    compute: Vec<Scalar>,
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
#[allow(clippy::too_many_arguments)]
fn materialise(
    table: &TableDef,
    residual: &Expr,
    filter_columns: &ColumnSet,
    decode_columns: &ColumnSet,
    two_phase: bool,
    compute: &[Scalar],
    transient: &[Ordinal],
    raw: &RawRow,
) -> Result<Option<Row>> {
    // Computed values are appended after the table's own columns, and the
    // residual may name one — so everything it could read has to exist before
    // the filter runs. That rules out the two-phase split, which exists to
    // avoid decoding columns a rejected row never needed.
    if !compute.is_empty() {
        let decoded = decode_row_columns(
            table,
            &raw.primary_key,
            &raw.body,
            wanted(decode_columns, table),
        )?;
        let extended = extend(decoded, compute, transient, None);
        return Ok(residual.admits(&extended).then_some(extended));
    }

    // One pass when the residual is not expected to reject much: decoding the
    // output columns anyway costs less than walking the row twice.
    let first = if two_phase {
        filter_columns
    } else {
        decode_columns
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
        wanted(decode_columns, table),
    )?))
}

/// Append a row's computed values, in order, after its own columns, then drop
/// the columns that were read only to produce them.
///
/// Each is evaluated against the row as it stands, so a later expression can
/// read an earlier one — which is what makes a chain of them expressible
/// without nesting.
///
/// `from_entry` is the one computed value an index entry already holds, for a
/// covering scan of an *expression* index. Its inputs are exactly the columns
/// such an entry does not carry — `lower(title)` is in the entry and `title` is
/// not — so evaluating it there would yield `lower(null)` and answer
/// differently from every other access path. Taking the value the writer
/// computed instead is what makes the entry answer the query on its own.
fn extend(
    row: Row,
    compute: &[Scalar],
    transient: &[Ordinal],
    from_entry: Option<(usize, &Value)>,
) -> Row {
    if compute.is_empty() {
        return row;
    }
    let mut values = row.into_values();
    values.reserve(compute.len());
    for (position, scalar) in compute.iter().enumerate() {
        let value = match from_entry {
            Some((from, value)) if from == position => value.clone(),
            // Evaluated against the values as a slice rather than a rebuilt
            // `Row`. Rebuilding cloned every value once per computed column,
            // which is quadratic: ClickBench's ninety-sum query spent ninety
            // seconds in it.
            _ => {
                let so_far: &[Value] = &values;
                scalar.evaluate(so_far)
            }
        };
        values.push(value);
    }
    // After the last computed value and before anything else sees the row: the
    // residual, the sort comparator and the caller all read the same row, and
    // whichever access path produced it.
    for ordinal in transient {
        if let Some(slot) = values.get_mut(ordinal.0) {
            *slot = Value::Null;
        }
    }
    Row::new(values)
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
///
/// `wanted` is what the query decodes — its answer's columns plus whatever its
/// computed values read — and filling in anything else would be a bug rather
/// than a bonus: an index entry often carries columns the caller did not ask
/// for, and handing those back makes the contents of a row depend on which
/// access path the planner chose. Found by the planner oracle, which caught
/// `by_kind_size` returning `size` on a query that projected only the primary
/// key, where a table scan returned null for it.
fn row_from_index_entry(
    table: &TableDef,
    index: &IndexDef,
    indexed: &[slate_tuple::Value],
    primary_key: &[slate_tuple::Value],
    wanted: &ColumnSet,
) -> Row {
    let mut values: Vec<slate_tuple::Value> = core::iter::repeat_with(|| slate_tuple::Value::Null)
        .take(table.columns().len())
        .collect();
    for (column, value) in index.columns().iter().zip(indexed) {
        if !wanted.contains(column.ordinal) {
            continue;
        }
        if let Some(slot) = values.get_mut(column.ordinal.0) {
            *slot = value.clone();
        }
    }
    // The primary key arrives decoded whatever the projection says, exactly as
    // it does on every other path.
    for (ordinal, value) in table.primary_key().iter().zip(primary_key) {
        if let Some(slot) = values.get_mut(ordinal.0) {
            *slot = value.clone();
        }
    }
    Row::new(values)
}

/// Open an index walk over one or more ranges.
///
/// Only the first range is opened here; the rest are held and reached one at a
/// time, so a query with a small limit never opens a range it does not get to.
async fn index_source<'a>(
    snapshot: &'a dyn KvSnapshot,
    table: &'a TableDef,
    index: IndexId,
    ranges: Vec<KeyRange>,
    covering: bool,
    order: ScanOrder,
) -> Result<Source<'a>> {
    let index = table
        .index(index)
        .ok_or(KernelError::UnknownTable(table.id()))?;
    let mut rest = ranges.into_iter();
    // The planner never emits a rangeless index access, and a walk over no
    // ranges reads nothing rather than everything — which is the safe way round
    // for a case that should not arise.
    let Some(first) = rest.next() else {
        return Ok(Source::Empty);
    };
    let cursor = read::scan_index(snapshot, table, index, first, order).await?;
    Ok(if covering {
        Source::CoveringIndex {
            cursor,
            index,
            rest,
            order,
        }
    } else {
        Source::Index {
            cursor,
            index,
            rest,
            order,
            inflight: FuturesOrdered::new(),
            exhausted: false,
        }
    })
}

/// The next index entry, crossing into the next range when this one runs out.
///
/// The ranges of an `Access::IndexScans` are disjoint and in the order they are
/// to be walked, so concatenating them is the whole of what makes a multi-range
/// scan produce the index's own order — there is no merge here, and nothing to
/// get wrong beyond stopping at the right place.
async fn next_index_entry<'a>(
    snapshot: &'a dyn KvSnapshot,
    table: &'a TableDef,
    index: &'a IndexDef,
    order: ScanOrder,
    cursor: &mut IndexCursor<'a>,
    rest: &mut std::vec::IntoIter<KeyRange>,
) -> Result<Option<(Vec<Value>, Vec<Value>)>> {
    loop {
        if let Some(entry) = cursor.next().await? {
            return Ok(Some(entry));
        }
        let Some(range) = rest.next() else {
            return Ok(None);
        };
        *cursor = read::scan_index(snapshot, table, index, range, order).await?;
    }
}

impl<'a> QueryCursor<'a> {
    /// Open a cursor for `plan` on `table`, returning at most `limit` rows
    /// after discarding `offset`.
    ///
    /// The window is taken here rather than applied afterwards because a sort
    /// needs it: sorting to return ten rows should not hold a million.
    pub(crate) async fn open(
        snapshot: &'a dyn KvSnapshot,
        table: &'a TableDef,
        plan: Plan,
        limit: Option<usize>,
        offset: usize,
        compute: Vec<Scalar>,
    ) -> Result<Self> {
        // What has to come off the row, which is the answer's columns plus
        // whatever the computed values read. `plan.output_columns` is the
        // answer; the difference is put back to null once the computed values
        // have been evaluated, so that a query's rows do not depend on which
        // access path produced them. See `transient`.
        let width = table.columns().len();
        let mut decode_columns = plan.output_columns.clone();
        let mut transient: Vec<Ordinal> = Vec::new();
        for scalar in &compute {
            for input in scalar.columns() {
                // A scalar may read an earlier computed value, which is not a
                // column and is decoded from nothing.
                if input.0 >= width || decode_columns.contains(input) {
                    continue;
                }
                decode_columns.insert(input);
                transient.push(input);
            }
        }

        // Which computed value the chosen index's entries hold outright, for a
        // covering scan of an expression index. Answered by the same function
        // the planner used to decide the scan covers the query at all, so the
        // two cannot disagree about which value the entry stands for.
        let from_entry = match &plan.access {
            Access::IndexScan {
                index,
                covering: true,
                ..
            }
            | Access::IndexScans {
                index,
                covering: true,
                ..
            } => table
                .index(*index)
                .and_then(|index| crate::plan::expression_position(index, &compute)),
            _ => None,
        };

        let source = match &plan.access {
            Access::Nothing => Source::Empty,
            Access::PointGet { key } => Source::Point(
                read::read_row_projected(snapshot, table, key, &decode_columns).await?,
            ),
            Access::PointGets { keys } => Source::Points {
                keys: keys.clone().into_iter(),
                inflight: FuturesOrdered::new(),
                exhausted: false,
            },
            Access::TableScan { range } => {
                Source::Rows(read::scan_rows(snapshot, table, range.clone(), plan.order).await?)
            }
            Access::IndexScan {
                index,
                range,
                covering,
            } => {
                index_source(
                    snapshot,
                    table,
                    *index,
                    vec![range.clone()],
                    *covering,
                    plan.order,
                )
                .await?
            }
            // Several disjoint ranges of one index, walked in turn. The planner
            // holds them in ascending key order; a descending walk takes them
            // in reverse, so that the concatenation is still the index's own
            // order and an `ORDER BY` it satisfies does not need a sort.
            Access::IndexScans {
                index,
                ranges,
                covering,
            } => {
                let mut ranges = ranges.clone();
                if matches!(plan.order, ScanOrder::Descending) {
                    ranges.reverse();
                }
                index_source(snapshot, table, *index, ranges, *covering, plan.order).await?
            }
        };
        let mut cursor = Self {
            snapshot,
            table,
            source,
            needs_second_phase: plan.filter_first
                && !plan.predicate_columns.contains_all(&decode_columns),
            filter_columns: plan.predicate_columns,
            decode_columns,
            transient,
            from_entry,
            residual: plan.residual,
            prefetch: DEFAULT_PREFETCH,
            compute,
            limit: None,
            offset: 0,
            skipped: 0,
            yielded: 0,
        };

        // No access path produced the requested order, so the rows have to be
        // collected and sorted. This is the one place the cursor stops being a
        // stream: nothing can be returned until everything has been read.
        if let Some(keys) = plan.sort {
            let window = limit.map(|limit| limit.saturating_add(offset));
            let rows = match window {
                // `ORDER BY … LIMIT k` needs the best k, not every row in
                // order. Keeping k costs O(n log k) against O(n log n), but
                // the reason to do it is memory: sorting a million rows to
                // return ten holds a million decoded rows, and on a wide
                // table that is gigabytes to produce a handful.
                Some(window) if window > 0 => {
                    cursor.top_n(window, Arc::from(keys.as_slice())).await?
                }
                Some(_) => Vec::new(),
                None => {
                    let mut rows = Vec::new();
                    while let Some(row) = cursor.next_admitted().await? {
                        rows.push(row);
                    }
                    rows.sort_by(|a, b| compare_rows(a, b, &keys));
                    rows
                }
            };
            cursor.source = Source::Sorted(rows.into_iter());
            // The residual has already been applied to every row.
            cursor.residual = Arc::new(Expr::True);
        }
        Ok(cursor)
    }

    /// The `window` rows that sort first, in order.
    ///
    /// A bounded max-heap: push, and once it is over capacity drop the worst.
    /// What is left is the best `window`, which pop out worst-first and are
    /// reversed.
    async fn top_n(&mut self, window: usize, keys: Arc<[SortKey]>) -> Result<Vec<Row>> {
        let mut heap: BinaryHeap<Ranked> = BinaryHeap::with_capacity(window + 1);
        while let Some(row) = self.next_admitted().await? {
            heap.push(Ranked {
                row,
                keys: Arc::clone(&keys),
            });
            if heap.len() > window {
                heap.pop();
            }
        }
        let mut rows: Vec<Row> = Vec::with_capacity(heap.len());
        while let Some(ranked) = heap.pop() {
            rows.push(ranked.row);
        }
        rows.reverse();
        Ok(rows)
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
    ///
    /// A known window also caps the prefetch. Overlapping reads is only free
    /// while every read is one the caller will use: fetching sixteen rows to
    /// return ten spends six round trips on rows that are discarded, which on
    /// a small limit is most of the query. The cap is the whole window, since
    /// an offset still has to fetch the rows it skips.
    #[must_use]
    pub(crate) const fn with_window(mut self, limit: Option<usize>, offset: usize) -> Self {
        self.limit = limit;
        self.offset = offset;
        if let Some(limit) = limit {
            let window = limit.saturating_add(offset);
            if window > 0 && window < self.prefetch {
                self.prefetch = window;
            }
        }
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
            decode_columns,
            needs_second_phase,
            transient,
            from_entry,
            prefetch,
            compute,
            ..
        } = self;

        match source {
            Source::Rows(cursor) => {
                while let Some(raw) = cursor.next_raw().await? {
                    if let Some(row) = materialise(
                        table,
                        residual,
                        filter_columns,
                        decode_columns,
                        *needs_second_phase,
                        compute,
                        transient,
                        &raw,
                    )? {
                        return Ok(Some(row));
                    }
                }
                Ok(None)
            }
            Source::Points {
                keys,
                inflight,
                exhausted,
            } => {
                loop {
                    while !*exhausted && inflight.len() < *prefetch {
                        match keys.next() {
                            Some(primary_key) => {
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
                                decode_columns,
                                *needs_second_phase,
                                compute,
                                transient,
                                &raw,
                            )? {
                                return Ok(Some(row));
                            }
                        }
                        Some(Err(error)) => return Err(error),
                        // A key with no row. Unlike an index entry pointing at
                        // nothing, this is ordinary: the caller named a key,
                        // and nothing says it has to exist.
                        Some(Ok(None)) => {}
                        None => return Ok(None),
                    }
                }
            }
            Source::Index {
                cursor,
                index,
                rest,
                order,
                inflight,
                exhausted,
            } => {
                loop {
                    // Keep the pipeline full. Reading the next key from the
                    // index is a scan step and cheap; the row read it implies is
                    // a round trip, so the reads are issued together and
                    // collected in order.
                    while !*exhausted && inflight.len() < *prefetch {
                        match next_index_entry(*snapshot, table, index, *order, cursor, rest)
                            .await?
                        {
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
                                decode_columns,
                                *needs_second_phase,
                                compute,
                                transient,
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
            // An arm of its own rather than a case of `next_candidate`,
            // because the entry has to reach `extend`: the value an expression
            // index keys on is in the entry and in no column, and a `Row` on
            // its way out of `next_candidate` has nowhere to carry it.
            Source::CoveringIndex {
                cursor,
                index,
                rest,
                order,
            } => {
                loop {
                    // The entry already holds everything this query reads, so
                    // there is nothing to fetch. This is the whole point of a
                    // covering index: no read per matching row.
                    let Some((indexed, primary_key)) =
                        next_index_entry(*snapshot, table, index, *order, cursor, rest).await?
                    else {
                        return Ok(None);
                    };
                    let row =
                        row_from_index_entry(table, index, &indexed, &primary_key, decode_columns);
                    // An expression index keys on exactly one value, so it is
                    // the first and only one in the entry. `None` here for an
                    // ordinary covering index, whose scalars are evaluated from
                    // the columns the entry does carry.
                    let held = from_entry.and_then(|position| Some((position, indexed.first()?)));
                    let row = extend(row, compute, transient, held);
                    if residual.admits(&row) {
                        return Ok(Some(row));
                    }
                }
            }
            _ => {
                while let Some(row) = self.next_candidate().await? {
                    // Already-sorted rows were extended and filtered on the
                    // way in; anything else is extended here, before the
                    // residual, for the same reason `materialise` does it.
                    let row = if matches!(self.source, Source::Sorted(_)) {
                        row
                    } else {
                        extend(row, &self.compute, &self.transient, None)
                    };
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
            Source::CoveringIndex { .. } | Source::Index { .. } | Source::Points { .. } => {
                unreachable!("handled by next_admitted")
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
