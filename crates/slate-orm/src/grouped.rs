//! A grouped read whose aggregates are looked up by what they compute.
//!
//! # Why this exists
//!
//! [`Records::group_records`] hands back [`Group`], whose `key` and `values`
//! are `Vec<Value>` **in the order they were requested**. That is the right
//! shape for the kernel — a group is a row, and a row is positional — and it
//! is the wrong shape for an application, because the only thing tying
//! `values[1]` to `Sum(hours_logged)` is that the same list was passed a few
//! lines earlier. Reorder the aggregate list and every call site silently
//! reads a different number.
//!
//! `examples/helpdesk` met this on its first roll-up and wrote a
//! `read_workload` that maps positions to fields by hand, erroring on every
//! shape it does not expect because a silent zero in a billing total is worse
//! than a refusal. That function is the cost, and
//! `ledger/2026-09-30-the-helpdesk-on-slatedb.md` recorded it as a gap.
//!
//! [`Grouped`] closes it by carrying the request alongside the answer: a call
//! site asks for the aggregate it wants rather than the position it landed
//! in, and a mismatch is `None` rather than a wrong column.
//!
//! # Why not a derive, or a typed tuple
//!
//! The obvious alternative is a generic `group_records_as::<R, K, A>` where
//! `A` is a tuple or a struct, decoding positionally under the hood with the
//! types checked at compile time. That is a better API and a much larger one:
//! it needs a trait implemented for every arity, or a second derive macro
//! over a struct whose fields are aggregates — and the positional decode does
//! not go away, it moves into generated code where a reordered list is
//! *still* wrong and now invisible. `Aggregate` is `Copy + Eq`, so a lookup
//! by value costs a linear scan of at most a handful of entries and removes
//! the coupling outright. When there is a reason to want the tuple, this is
//! the layer it would be built on.
//!
//! # Why duplicates are refused rather than resolved
//!
//! `[Count, Count]` makes `get(Count)` ambiguous, and every way of resolving
//! it — first wins, last wins, an error at lookup — hides the mistake at a
//! point further from where it was made. The list is rejected when the read
//! is issued, which is the only place the caller can see both entries.

use crate::error::{OrmError, Result};
use slate_kernel::{Aggregate, Group};
use slate_schema::Ordinal;
use slate_tuple::Value;

/// One grouped row, with the request it answers.
///
/// Built by [`crate::Records::grouped_records`]. The `Vec`s are the kernel's
/// own, and [`Grouped::into_group`] hands them back untouched for a caller
/// that wants the positional form after all.
#[derive(Debug, Clone, PartialEq)]
pub struct Grouped {
    group: Group,
    /// The grouping columns, in the order they were requested. Parallel to
    /// `group.key`.
    columns: Vec<Ordinal>,
    /// The aggregates, in the order they were requested. Parallel to
    /// `group.values`.
    aggregates: Vec<Aggregate>,
}

impl Grouped {
    /// Pair a kernel [`Group`] with the request that produced it.
    ///
    /// # Errors
    /// If either list's length does not match the group's — which would mean
    /// the kernel and this layer disagreed about what was asked for — or if
    /// a column or aggregate is requested twice, which makes a lookup
    /// ambiguous. See the module docs for why that is refused rather than
    /// resolved.
    pub fn new(group: Group, columns: &[Ordinal], aggregates: &[Aggregate]) -> Result<Self> {
        let mismatch = |what: &str, asked: usize, got: usize| {
            OrmError::Grouping(format!(
                "a grouped read asked for {asked} {what} and got {got} back; the \
                 kernel and the typed layer disagree about the request"
            ))
        };
        if columns.len() != group.key.len() {
            return Err(mismatch(
                "grouping column(s)",
                columns.len(),
                group.key.len(),
            ));
        }
        if aggregates.len() != group.values.len() {
            return Err(mismatch(
                "aggregate(s)",
                aggregates.len(),
                group.values.len(),
            ));
        }
        for (at, one) in columns.iter().enumerate() {
            // `get(..at)` rather than `[..at]`: `at` is in range by
            // construction, and the lint does not know that. Cheaper to
            // satisfy than to exempt.
            if columns.get(..at).is_some_and(|seen| seen.contains(one)) {
                return Err(OrmError::Grouping(format!(
                    "grouping column {one:?} is requested twice, which makes \
                     `Grouped::key` ambiguous"
                )));
            }
        }
        for (at, one) in aggregates.iter().enumerate() {
            if aggregates.get(..at).is_some_and(|seen| seen.contains(one)) {
                return Err(OrmError::Grouping(format!(
                    "aggregate {one:?} is requested twice, which makes \
                     `Grouped::get` ambiguous"
                )));
            }
        }
        Ok(Self {
            group,
            columns: columns.to_vec(),
            aggregates: aggregates.to_vec(),
        })
    }

    /// This group's value for a grouping column, or `None` if the read did
    /// not group by it.
    ///
    /// A `GROUP BY` puts the nulls together rather than dropping them, so a
    /// present column can still be [`Value::Null`] — that is the "unassigned"
    /// pile, and it is usually the row an application most wants.
    #[must_use]
    pub fn key(&self, column: Ordinal) -> Option<&Value> {
        let at = self.columns.iter().position(|one| *one == column)?;
        self.group.key.get(at)
    }

    /// This group's value for an aggregate, or `None` if the read did not ask
    /// for it.
    ///
    /// `None` means *not requested*, never *no rows*: a `SUM` over no rows is
    /// `Some(Value::Null)`, which cannot happen for a group that exists,
    /// because a group exists because a row landed in it.
    #[must_use]
    pub fn get(&self, aggregate: Aggregate) -> Option<&Value> {
        let at = self.aggregates.iter().position(|one| *one == aggregate)?;
        self.group.values.get(at)
    }

    /// The grouping columns this read asked for, in order.
    #[must_use]
    pub fn columns(&self) -> &[Ordinal] {
        &self.columns
    }

    /// The aggregates this read asked for, in order.
    #[must_use]
    pub fn aggregates(&self) -> &[Aggregate] {
        &self.aggregates
    }

    /// The positional form, for a caller that wants it.
    ///
    /// Kept rather than hidden: a caller folding every aggregate the same way
    /// wants the `Vec`, and making them go through `get` once per entry would
    /// be slower and no safer.
    #[must_use]
    pub fn into_group(self) -> Group {
        self.group
    }
}
