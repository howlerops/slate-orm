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
use slate_kernel::{Expr, RecordTransaction, ScanOrder, SecurityContext};
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
        let rows = self
            .query(context, R::table(), filter, order)
            .await?
            .collect()
            .await?;
        rows.iter().map(R::from_row).map(|r| Ok(r?)).collect()
    }
}
