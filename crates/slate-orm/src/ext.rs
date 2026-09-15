//! Typed operations on a record-layer transaction.
//!
//! These are a thin translation layer over the kernel: they convert a Rust
//! value to a [`Row`](slate_schema::Row) and hand it to the same call an
//! untyped caller would make. No check lives here — in particular, no policy
//! check — because anything enforced at this level could be skipped by using
//! the kernel directly.

use crate::error::{OrmError, Result};
use crate::record::Record;
use async_trait::async_trait;
use slate_kernel::{
    Aggregate, Chain, Explanation, Expr, Group, Join, JoinExplanation, KernelError, Projection,
    Query, RecordTransaction, ScanOrder, SecurityContext, TableStats,
};
use slate_schema::Ordinal;
use slate_tuple::Value;

/// One page of rows, and where to resume.
///
/// # Why `next` is `Some` on the last full page
///
/// A page that comes back full might be the last one, and the only way to know
/// is to ask again. This returns a cursor anyway rather than reading one row
/// further to find out: that extra read is paid on *every* page to save one
/// empty request at the end of a sequence most callers never finish. The cost
/// of the choice is that a caller looping until `next` is `None` makes one
/// final request that returns nothing, which is the ordinary shape of every
/// cursor API and is documented rather than optimised away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<R> {
    /// The rows, at most `query.limit` of them.
    pub rows: Vec<R>,
    /// The cursor for the next page, or `None` when this page was short and
    /// there is provably nothing after it.
    pub next: Option<Vec<Value>>,
}

impl<R> Page<R> {
    /// Whether there is provably nothing after this page.
    #[must_use]
    pub const fn is_last(&self) -> bool {
        self.next.is_none()
    }
}

/// Typed reads and writes over a [`RecordTransaction`].
///
/// Methods are suffixed rather than sharing names with the kernel's own
/// methods, so a call site says plainly which layer it is using.
#[async_trait]
pub trait Records {
    /// Read one record by primary key.
    async fn get_record<R: Record>(
        &self,
        context: &SecurityContext,
        primary_key: &[Value],
    ) -> Result<Option<R>>;

    /// Insert a record, failing if its primary key is taken.
    async fn insert_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()>;

    /// Replace an existing record.
    async fn update_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()>;

    /// Insert or replace a record.
    async fn upsert_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()>;

    /// Insert many records, failing if any primary key is taken.
    ///
    /// The duplicate-key and unique-index reads are overlapped rather than
    /// done one row at a time, so the batch costs a handful of round trips
    /// instead of one per row. Detection is unchanged: a taken key still
    /// reports `DuplicatePrimaryKey`, and a collision inside the batch itself
    /// is caught before anything is written.
    async fn insert_records<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        records: &[R],
    ) -> Result<()>;

    /// Insert or replace many records, overlapping the reads as
    /// [`insert_records`](Records::insert_records) does.
    async fn upsert_records<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        records: &[R],
    ) -> Result<()>;

    /// Delete by primary key, reporting whether anything was removed.
    async fn delete_record<R: Record>(
        &self,
        context: &SecurityContext,
        primary_key: &[Value],
    ) -> Result<bool>;

    /// Run a query and decode the results.
    async fn find_records<R: Record>(
        &self,
        context: &SecurityContext,
        filter: Expr,
        order: ScanOrder,
    ) -> Result<Vec<R>>;

    /// Run a full query — filter, order, limit, offset — and decode the results.
    ///
    /// The projection is forced to every column: a decoded record needs all of
    /// its fields, and a narrowed projection would fill the rest with nulls
    /// that are indistinguishable from stored ones. Use
    /// [`Records::aggregate_records`] or the kernel's projected query when the
    /// point is to read less.
    async fn query_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<Vec<R>>;

    /// One page of a keyset-paged read, with the cursor for the next one.
    ///
    /// The whole point is that the caller does not extract the key. Paging by
    /// hand means knowing which columns are the primary key and in what order,
    /// on every call site, for every type — and getting it subtly wrong is
    /// invisible, because a cursor built from the wrong column still pages, just
    /// through the wrong sequence.
    ///
    /// `query.limit` must be set: a page with no size is not a page. Pass the
    /// returned [`Page::next`] to [`Query::after`] for the page after this one.
    ///
    /// # Errors
    /// If the limit is unset, if the query cannot be resumed from a key (see
    /// [`Query::after`]), or if the read fails.
    async fn page_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<Page<R>>;

    /// Count the rows a query matches, without decoding any.
    async fn count_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<u64>;

    /// Join two record types and decode both sides.
    ///
    /// The join condition names columns, and the derive generates a `COLUMNS`
    /// constant for each field, so a call site reads
    /// `Join::equating(Author::COLUMNS.id, Book::COLUMNS.author_id)`.
    ///
    /// For an inner or left join, where the left side is always present. A
    /// right or full outer join can return a row with no left side at all, so
    /// this refuses one rather than inventing a record to put there — use
    /// [`Records::outer_join_records`] for those.
    async fn join_records<L: Record, R: Record>(
        &self,
        context: &SecurityContext,
        join: &Join,
    ) -> Result<Vec<(L, Option<R>)>>;

    /// [`Records::join_records`] for any join type, including the two that can
    /// drop the left side.
    async fn outer_join_records<L: Record, R: Record>(
        &self,
        context: &SecurityContext,
        join: &Join,
    ) -> Result<Vec<(Option<L>, Option<R>)>>;

    /// The plan a join would run under, without running it.
    fn explain_join_records<L: Record, R: Record>(
        &self,
        context: &SecurityContext,
        join: &Join,
    ) -> Result<JoinExplanation>;

    /// Join three record types in a chain, decoding each side.
    ///
    /// The chain's ordinal space comes from
    /// `JoinSchema::over([A::table(), B::table(), C::table()])`, and each
    /// step's condition is written in it. A table that an outer step left
    /// absent decodes to `None`.
    async fn chain_records<A: Record, B: Record, C: Record>(
        &self,
        context: &SecurityContext,
        chain: &Chain,
    ) -> Result<Vec<(Option<A>, Option<B>, Option<C>)>>;

    /// Compute aggregates over the rows a query matches.
    async fn aggregate_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
        aggregates: &[Aggregate],
    ) -> Result<Vec<Value>>;

    /// Compute aggregates per distinct combination of `group`.
    async fn group_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
    ) -> Result<Vec<Group>>;

    /// The plan a query would run under, without running it.
    fn explain_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<Explanation>;

    /// Gather statistics for a record's table. See the kernel's `analyze`.
    async fn analyze_records<R: Record>(&self, context: &SecurityContext) -> Result<TableStats>;
}

#[async_trait]
impl Records for RecordTransaction<'_> {
    async fn get_record<R: Record>(
        &self,
        context: &SecurityContext,
        primary_key: &[Value],
    ) -> Result<Option<R>> {
        match self.get(context, R::table(), primary_key).await? {
            None => Ok(None),
            Some(row) => Ok(Some(R::from_row(&row)?)),
        }
    }

    async fn insert_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()> {
        self.insert(context, R::table(), &record.to_row())
            .await
            .map_err(OrmError::from)
    }

    async fn update_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()> {
        self.update(context, R::table(), &record.to_row())
            .await
            .map_err(OrmError::from)
    }

    async fn upsert_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()> {
        self.upsert(context, R::table(), &record.to_row())
            .await
            .map_err(OrmError::from)
    }

    async fn insert_records<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        records: &[R],
    ) -> Result<()> {
        let rows: Vec<_> = records.iter().map(Record::to_row).collect();
        self.insert_many(context, R::table(), &rows)
            .await
            .map_err(OrmError::from)
    }

    async fn upsert_records<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        records: &[R],
    ) -> Result<()> {
        let rows: Vec<_> = records.iter().map(Record::to_row).collect();
        self.upsert_many(context, R::table(), &rows)
            .await
            .map_err(OrmError::from)
    }

    async fn delete_record<R: Record>(
        &self,
        context: &SecurityContext,
        primary_key: &[Value],
    ) -> Result<bool> {
        self.delete(context, R::table(), primary_key)
            .await
            .map_err(OrmError::from)
    }

    async fn page_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<Page<R>> {
        let Some(limit) = query.limit else {
            return Err(OrmError::Kernel(slate_kernel::KernelError::InvalidCursor {
                table: R::table().name().to_owned(),
                reason: "a paged read needs `limit`: a page with no size is the whole table, and \
                         the cursor it would return names its last row"
                    .to_owned(),
            }));
        };
        let rows: Vec<R> = self.query_records(context, query).await?;
        // A short page proves there is nothing after it; a full one proves
        // nothing either way. See `Page`.
        let next = if rows.len() < limit {
            None
        } else {
            rows.last().map(Record::primary_key)
        };
        Ok(Page { rows, next })
    }

    async fn find_records<R: Record>(
        &self,
        context: &SecurityContext,
        filter: Expr,
        order: ScanOrder,
    ) -> Result<Vec<R>> {
        self.query_records(context, &Query::all().filter(filter).order(order))
            .await
    }

    async fn query_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<Vec<R>> {
        let mut query = query.clone();
        query.projection = Projection::All;
        let rows = self
            .execute(context, R::table(), &query)
            .await?
            .collect()
            .await?;
        rows.iter().map(R::from_row).map(|r| Ok(r?)).collect()
    }

    async fn join_records<L: Record, R: Record>(
        &self,
        context: &SecurityContext,
        join: &Join,
    ) -> Result<Vec<(L, Option<R>)>> {
        if join.join_type.may_drop_left() {
            return Err(OrmError::Kernel(KernelError::JoinNotSupported {
                reason: "this join can return a row with no left side; \
                         use outer_join_records"
                    .to_owned(),
            }));
        }
        // Collected before decoding, not decoded as they arrive: a `Record` is
        // not required to be `Send`, so holding one across the next await
        // would make the whole future unsendable. Same reason as
        // [`Records::query_records`].
        let joined = self
            .join(context, L::table(), R::table(), join)
            .await?
            .collect()
            .await?;
        joined
            .iter()
            .map(|row| {
                let left = row.left.as_ref().ok_or_else(|| {
                    OrmError::Kernel(KernelError::JoinNotSupported {
                        reason: "join returned a row with no left side".to_owned(),
                    })
                })?;
                Ok((
                    L::from_row(left)?,
                    row.right.as_ref().map(R::from_row).transpose()?,
                ))
            })
            .collect()
    }

    async fn outer_join_records<L: Record, R: Record>(
        &self,
        context: &SecurityContext,
        join: &Join,
    ) -> Result<Vec<(Option<L>, Option<R>)>> {
        let joined = self
            .join(context, L::table(), R::table(), join)
            .await?
            .collect()
            .await?;
        joined
            .iter()
            .map(|row| {
                Ok((
                    row.left.as_ref().map(L::from_row).transpose()?,
                    row.right.as_ref().map(R::from_row).transpose()?,
                ))
            })
            .collect()
    }

    fn explain_join_records<L: Record, R: Record>(
        &self,
        context: &SecurityContext,
        join: &Join,
    ) -> Result<JoinExplanation> {
        self.explain_join(context, L::table(), R::table(), join)
            .map_err(OrmError::from)
    }

    async fn chain_records<A: Record, B: Record, C: Record>(
        &self,
        context: &SecurityContext,
        chain: &Chain,
    ) -> Result<Vec<(Option<A>, Option<B>, Option<C>)>> {
        let rows = self
            .chain(context, &[A::table(), B::table(), C::table()], chain)
            .await?
            .collect()
            .await?;
        rows.iter()
            .map(|row| {
                Ok((
                    row.at(0).map(A::from_row).transpose()?,
                    row.at(1).map(B::from_row).transpose()?,
                    row.at(2).map(C::from_row).transpose()?,
                ))
            })
            .collect()
    }

    async fn count_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<u64> {
        Ok(self.count(context, R::table(), query).await?)
    }

    async fn aggregate_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
        aggregates: &[Aggregate],
    ) -> Result<Vec<Value>> {
        Ok(self
            .aggregate(context, R::table(), query, aggregates)
            .await?)
    }

    async fn group_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
    ) -> Result<Vec<Group>> {
        Ok(self
            .group_by(context, R::table(), query, group, aggregates)
            .await?)
    }

    fn explain_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<Explanation> {
        Ok(self.explain(context, R::table(), query)?)
    }

    async fn analyze_records<R: Record>(&self, context: &SecurityContext) -> Result<TableStats> {
        Ok(self.analyze(context, R::table()).await?)
    }
}
