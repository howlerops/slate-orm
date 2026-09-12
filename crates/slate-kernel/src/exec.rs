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
use crate::read::{self, IndexCursor, RowCursor};
use crate::store::KvSnapshot;
use slate_schema::{IndexDef, Row, TableDef};

/// Where a cursor's candidate rows come from.
enum Source<'a> {
    /// Rows read straight out of the table's key range.
    Rows(RowCursor<'a>),
    /// Primary keys read from an index, each fetched from the table.
    Index {
        cursor: IndexCursor<'a>,
        index: &'a IndexDef,
    },
    /// Rows assembled from index entries, with no table read at all.
    CoveringIndex {
        cursor: IndexCursor<'a>,
        index: &'a IndexDef,
    },
    /// A single row, fetched by primary key.
    Point(Option<Row>),
    /// The plan proved there is nothing to read.
    Empty,
}

/// A cursor over the rows a plan admits.
pub struct QueryCursor<'a> {
    snapshot: &'a dyn KvSnapshot,
    table: &'a TableDef,
    source: Source<'a>,
    residual: Expr,
    limit: Option<usize>,
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
                    Source::Index { cursor, index }
                }
            }
        };
        Ok(Self {
            snapshot,
            table,
            source,
            residual: plan.residual,
            limit: None,
            yielded: 0,
        })
    }

    /// Stop after `limit` rows.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// The next admitted row.
    pub async fn next(&mut self) -> Result<Option<Row>> {
        if self.limit.is_some_and(|l| self.yielded >= l) {
            return Ok(None);
        }
        while let Some(row) = self.next_candidate().await? {
            if self.residual.admits(&row) {
                self.yielded += 1;
                return Ok(Some(row));
            }
        }
        Ok(None)
    }

    async fn next_candidate(&mut self) -> Result<Option<Row>> {
        match &mut self.source {
            Source::Empty => Ok(None),
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
            Source::Index { cursor, index } => {
                while let Some((_, primary_key)) = cursor.next().await? {
                    if let Some(row) =
                        read::read_row_unchecked(self.snapshot, self.table, &primary_key).await?
                    {
                        return Ok(Some(row));
                    }
                    // Index entries and rows are written in one transaction, so
                    // on a point-in-time view an entry without a row means the
                    // two have diverged on disk. On a replica that follows the
                    // manifest it means only that the view advanced between the
                    // two reads, and skipping is correct.
                    if self.snapshot.is_point_in_time() {
                        return Err(KernelError::CorruptIndexEntry {
                            table: self.table.name().to_owned(),
                            index: index.name().to_owned(),
                        });
                    }
                }
                Ok(None)
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
