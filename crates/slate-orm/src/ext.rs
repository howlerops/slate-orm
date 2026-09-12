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
    Aggregate, Explanation, Expr, Group, Projection, Query, RecordTransaction, ScanOrder,
    SecurityContext, TableStats,
};
use slate_schema::Ordinal;
use slate_tuple::Value;

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

    /// Count the rows a query matches, without decoding any.
    async fn count_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<u64>;

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
