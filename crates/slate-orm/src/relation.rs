//! Relationships: declaring them, and loading them without N+1.
//!
//! # Why this is explicit
//!
//! The feature an ORM is judged on is `author.books()`. The feature an ORM is
//! *blamed* for is that `author.books()` ran a query, once per author, inside a
//! loop nobody noticed writing. Those are the same feature: a lazy accessor
//! cannot know whether it is being called once or ten thousand times, so it
//! issues one query either way and the cost lands somewhere the code does not
//! mention.
//!
//! So there is no lazy accessor here. A relationship is *declared* on the type
//! and *loaded* by an explicit call that takes the whole set of parents at
//! once:
//!
//! ```text
//! let authors = store.find_records::<Author>(&ctx, filter, order).await?;
//! let books = store.load_related::<Author, Book>(&ctx, &authors).await?;
//! ```
//!
//! Two reads for any number of authors, and the second one is visible in the
//! code that pays for it. The cost of the design is that the caller has to say
//! what they want loaded; the benefit is that they can see what it cost.
//!
//! # Why the loader does not join
//!
//! A join would also answer this in one read, and the kernel has good ones. It
//! is the wrong shape for *this* question: a join returns the parent's columns
//! repeated once per child, so an author with forty books arrives forty times
//! and the caller reassembles them anyway — after paying to decode the author
//! forty times. `IN` over the child table reads each row once and each parent
//! zero times, because the parents are already in hand.
//!
//! It also lands on machinery that is already there: `Expr::In` is what task
//! #32 taught the planner to turn into point gets and index ranges, so a
//! relationship keyed on an indexed column is a range read rather than a scan,
//! with no work here at all.

use crate::error::Result;
use crate::ext::Records;
use crate::record::Record;
use slate_kernel::{Expr, ScanOrder, SecurityContext};
use slate_schema::Ordinal;
use slate_tuple::Value;
use std::collections::BTreeMap;

/// A declared relationship from `Self` to `Other`.
///
/// One direction per impl: `Author: Related<Book>` says how to find an author's
/// books, and `Book: Related<Author>` how to find a book's author. They are
/// separate impls rather than one bidirectional declaration because the two
/// ordinals swap roles and a single declaration would have to be read
/// differently depending on which way it was being used — which is exactly the
/// kind of thing that is right in the test and wrong in the caller.
pub trait Related<Other: Record>: Record {
    /// The column of `Self` whose value identifies the related rows.
    ///
    /// Usually the primary key for a has-many, and the foreign key column for a
    /// belongs-to.
    fn local() -> Ordinal;

    /// The column of `Other` that holds that value.
    fn foreign() -> Ordinal;
}

/// The filter that selects every parent's related rows.
///
/// [`load_related`] runs this and decodes the result; it is public separately
/// because the two useful things to do with a relationship are *fetch it* and
/// *narrow something else by it*, and the second one wants the `Expr` to combine
/// with its own conditions rather than a `Vec` of rows it has to filter again.
///
/// Also because it is what makes the deduplication observable. The values are
/// sorted and deduplicated, and that has no effect on the rows returned or on
/// the number of reads — only on the size of the request. A claim nothing can
/// see is a claim nothing keeps true, so the thing being claimed is returned
/// rather than buried.
///
/// Deduplication matters because the alternative degrades into the cost this
/// module exists to avoid: ten thousand books by one author would send ten
/// thousand copies of the same id, and the planner turns each value of an `IN`
/// on an indexed column into its own point get — so the single read would do
/// the work of the N+1.
///
/// With no parents the filter is an `IN` over no values, which matches nothing.
/// That is the right answer and not one worth asking for: [`load_related`]
/// returns before issuing it, because a read that cannot match is still a read.
///
/// # Errors
/// If a parent's local column is missing (a row narrower than its table).
pub fn related_filter<P, C>(parents: &[P]) -> Result<Expr>
where
    P: Record + Related<C>,
    C: Record,
{
    let local = <P as Related<C>>::local();
    let mut values: Vec<Value> = Vec::with_capacity(parents.len());
    for parent in parents {
        values.push(value_at(parent, local)?);
    }
    values.sort();
    values.dedup();
    Ok(Expr::In {
        column: <P as Related<C>>::foreign(),
        values,
    })
}

/// Every parent's related rows, in parent order, from one read.
///
/// Returns a vector the same length as `parents`: entry `i` is the rows of
/// `Other` whose [`Related::foreign`] column equals parent `i`'s
/// [`Related::local`] column. A parent with no related rows gets an empty
/// vector, not a missing entry — the result is indexable by the caller's own
/// loop counter, which is the shape that makes it hard to misuse.
///
/// Parents that share a key share their children, and the key is read once:
/// two books by the same author send one value in the `IN`, and both get the
/// same author back.
///
/// # Errors
/// If a parent's local column is missing (a row narrower than its table), or
/// the read fails.
pub async fn load_related<S, P, C>(
    store: &S,
    context: &SecurityContext,
    parents: &[P],
) -> Result<Vec<Vec<C>>>
where
    S: Records + Sync + ?Sized,
    P: Record + Related<C>,
    C: Record,
{
    // No parents, no query. Worth its own line: the alternative is an `IN ()`,
    // which is a filter that matches nothing and still pays for a read.
    if parents.is_empty() {
        return Ok(Vec::new());
    }

    let local = <P as Related<C>>::local();
    let mut keys: Vec<Value> = Vec::with_capacity(parents.len());
    for parent in parents {
        keys.push(value_at(parent, local)?);
    }

    let children: Vec<C> = store
        .find_records::<C>(
            context,
            related_filter::<P, C>(parents)?,
            ScanOrder::Ascending,
        )
        .await?;

    // Grouped by the child's own foreign value rather than by position, because
    // the read returns them in the table's order and not the parents'.
    let foreign = <P as Related<C>>::foreign();
    let mut by_key: BTreeMap<Value, Vec<C>> = BTreeMap::new();
    for child in children {
        let key = value_at(&child, foreign)?;
        by_key.entry(key).or_default().push(child);
    }

    // Cloned out per parent rather than moved, because two parents may share a
    // key and both are entitled to the rows. `C: Record` does not imply
    // `Clone`, so this re-decodes from the row instead — which is also what
    // keeps the shared case from being a special one.
    let mut out = Vec::with_capacity(parents.len());
    for key in keys {
        match by_key.get(&key) {
            None => out.push(Vec::new()),
            Some(found) => {
                let mut mine = Vec::with_capacity(found.len());
                for child in found {
                    mine.push(C::from_row(&child.to_row())?);
                }
                out.push(mine);
            }
        }
    }
    Ok(out)
}

/// One related row per parent, for a relationship that has at most one.
///
/// The belongs-to shape: every book has one author, so a `Vec<Vec<Author>>` is
/// a shape the caller would immediately flatten and have to decide what to do
/// with the impossible case. This decides it once — the first, and `None` when
/// there is none — so the caller's code says `if let Some(author)`.
///
/// It does not *check* that there is at most one. A uniqueness claim belongs to
/// the schema, where a unique index can enforce it; asserting it here would
/// turn a data problem into a panic at a call site that cannot fix it.
///
/// # Errors
/// As [`load_related`].
pub async fn load_one_related<S, P, C>(
    store: &S,
    context: &SecurityContext,
    parents: &[P],
) -> Result<Vec<Option<C>>>
where
    S: Records + Sync + ?Sized,
    P: Record + Related<C>,
    C: Record,
{
    let loaded = load_related::<S, P, C>(store, context, parents).await?;
    Ok(loaded
        .into_iter()
        .map(|mut rows| {
            if rows.is_empty() {
                None
            } else {
                Some(rows.swap_remove(0))
            }
        })
        .collect())
}

/// One record's value at an ordinal.
///
/// Goes through `to_row` rather than asking the type for a field, because
/// `Record` has no field accessor and adding one for this would mean every
/// implementor writing a match over its own columns. `to_row` is what the
/// derive already generates.
fn value_at<R: Record>(record: &R, column: Ordinal) -> Result<Value> {
    let row = record.to_row();
    row.values().get(column.0).cloned().ok_or_else(|| {
        // `ColumnCount` rather than a new variant: a row too short to hold the
        // ordinal a relationship names *is* a row with the wrong number of
        // columns, and the existing message says so in the words the rest of
        // the crate uses.
        crate::error::OrmError::Record(crate::record::RecordError::ColumnCount {
            table: R::table().name(),
            expected: column.0 + 1,
            actual: row.values().len(),
        })
    })
}
