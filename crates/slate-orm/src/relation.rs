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

/// Every parent's related rows *through a join table*, from two reads.
///
/// `Article → ArticleTag → Tag`: entry `i` is the tags of article `i`, in the
/// order its join rows appear. A parent with none gets an empty vector, the
/// same shape [`load_related`] returns.
///
/// # This needed no new declaration, and that is the finding
///
/// The plan for this called for `#[record(has_many(Tag, through =
/// ArticleTag))]` — a third kind of relationship beside `has_many` and
/// `belongs_to`. It is not one. A many-to-many is exactly the composition of
/// the two that already exist: `P: Related<J>` is the has-many onto the join
/// table, `J: Related<C>` is the join table's belongs-to onto the far side,
/// and this function is those two bounds and nothing else. No new trait, no
/// new attribute, no change to the derive.
///
/// [`Through`] exists anyway, and is *only* a name: it lets a caller write
/// `load_through::<_, Article, Tag>` instead of naming `ArticleTag` at the
/// call site. It adds no capability, which is why it is a blanket impl over
/// the same two bounds rather than something the derive emits.
///
/// # Two reads, not one and not N
///
/// One read for the join rows of every parent, one for the far rows of every
/// join row. Both go through [`load_related`], so both deduplicate their `IN`
/// values: a thousand articles sharing one tag send that tag's id once.
///
/// Not one read, because that would be a join, and a join returns the product
/// — every article's row repeated once per tag — which is more bytes than the
/// two reads and has to be regrouped anyway. Not three: the join rows are the
/// intermediate key set and nothing else needs reading.
///
/// # Duplicates are returned, not removed
///
/// Two join rows pointing at the same far row give that row twice. Removing
/// them would need `C: Ord` or `C: Hash`, which [`Record`] does not require,
/// so the choice is between narrowing the bound for every caller and returning
/// what the join table says. SQL's `has_many through` has the same behaviour
/// without a `DISTINCT`, and a join table with a uniqueness constraint — which
/// is what a join table should have — cannot produce them.
///
/// # The wrong join table does not compile
///
/// `Through::Join` naming the far type rather than the middle one is the
/// mistake the attribute makes available, and it is caught by the type system
/// rather than by a test: `load_through` requires `Join: Related<C>`, and
/// `Tag: Related<Tag>` does not exist. Checked by making that mutation, which
/// failed to build rather than returning a wrong answer.
///
/// # Errors
/// If a parent's or a join row's column is missing, or either read fails.
pub async fn load_related_through<S, P, J, C>(
    store: &S,
    context: &SecurityContext,
    parents: &[P],
) -> Result<Vec<Vec<C>>>
where
    S: Records + Sync + ?Sized,
    P: Record + Related<J>,
    J: Record + Related<C>,
    C: Record,
{
    // The far rows of [`load_nested`], with the join rows dropped. Expressed
    // on top of it rather than beside it because the two differ only in
    // whether the middle is kept, and two copies of the same regrouping is two
    // places for an off-by-one to live.
    Ok(load_nested::<S, P, J, C>(store, context, parents)
        .await?
        .into_iter()
        .map(|per_parent| {
            per_parent
                .into_iter()
                .flat_map(|(_join, far)| far)
                .collect()
        })
        .collect())
}

/// Every parent's children, each paired with its own children, from two reads.
///
/// `Article → Comment → Author`: entry `i` is article `i`'s comments, and each
/// comment carries the authors of *that* comment. One level of nesting, the
/// intermediate kept — which is the only difference from
/// [`load_related_through`], and why that function is one line of this one.
///
/// # There is no depth limit, because there is no depth
///
/// The plan for this asked for "a depth limit with a named refusal rather than
/// unbounded recursion". There is nothing to bound. Each level of nesting is a
/// *type parameter*, so the depth of a call is fixed when it compiles: two
/// levels is `load_nested`, three would be a function with four parameters,
/// and a caller cannot ask for a thousand without writing a thousand types.
/// Unbounded recursion needs a depth that arrives at runtime.
///
/// That form does exist and is not this one: a path on the wire is a list
/// whose depth a request chooses, and it needs exactly the refusal the plan
/// describes. **It now exists** — `RelatedRequest.path` — and so does the
/// refusal: `Limits::max_relation_depth`, checked in the head node before any
/// step resolves, because one step is one read. The reasoning above still
/// holds for *this* function, whose depth is fixed when it compiles; it is the
/// wire form that needed bounding, and the two are bounded differently because
/// they are bounded by different things.
///
/// # Errors
/// If a column a relationship names is missing, or either read fails.
pub async fn load_nested<S, P, C, G>(
    store: &S,
    context: &SecurityContext,
    parents: &[P],
) -> Result<Vec<Vec<(C, Vec<G>)>>>
where
    S: Records + Sync + ?Sized,
    P: Record + Related<C>,
    C: Record + Related<G>,
    G: Record,
{
    // No guard for an empty `parents`, although `load_related` has one.
    // Written with one first, and a mutation removing it changed no answer:
    // `load_related` returns early itself, so the counts are empty, the
    // flattened children are empty, and the early return below produces the
    // same empty vector. Two guards for one condition, the second
    // unobservable — a comment claiming a saving already made a line deeper.

    // Read one: every parent's children, grouped by parent.
    let per_parent: Vec<Vec<C>> = load_related::<S, P, C>(store, context, parents).await?;

    // How many children each parent had, kept before the grouping is flattened
    // away. Counts rather than clones: `Record` does not require `Clone`, so
    // flattening has to move, and moving loses the boundaries unless they are
    // recorded first.
    let counts: Vec<usize> = per_parent.iter().map(Vec::len).collect();

    // Read two: the grandchildren of every child, in one flat batch. Reading
    // per parent group instead would be the N+1 this exists to avoid, with N
    // the parent count rather than the row count.
    let flat: Vec<C> = per_parent.into_iter().flatten().collect();
    if flat.is_empty() {
        return Ok(counts.iter().map(|_| Vec::new()).collect());
    }
    let per_child: Vec<Vec<G>> = load_related::<S, C, G>(store, context, &flat).await?;

    // Regroup by walking the children in the order they were flattened, so the
    // cursor tracks without a lookup table. `zip` truncates to the shorter
    // side, which cannot bite here — `load_related` returns one entry per
    // input — and is the reason a mismatch would be silent rather than a
    // panic, so it is `zip` on purpose and not by habit.
    let mut pairs = flat.into_iter().zip(per_child);
    let mut out: Vec<Vec<(C, Vec<G>)>> = Vec::with_capacity(counts.len());
    for count in counts {
        let mut mine: Vec<(C, Vec<G>)> = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(pair) = pairs.next() {
                mine.push(pair);
            }
        }
        out.push(mine);
    }
    Ok(out)
}

/// A many-to-many: which join table stands between `Self` and `C`.
///
/// Emitted by `#[record(has_many(Tag, through = ArticleTag))]`, and this is
/// the *only* thing that attribute produces — the capability is already there
/// without it, in [`load_related_through`], which needs no declaration at all
/// because a many-to-many is the composition of a has-many and a belongs-to.
///
/// So why does the attribute exist? Because the composition cannot be
/// *inferred*. A blanket impl over `P: Related<J>, J: Related<C>` is the
/// obvious way to derive this for free, and the compiler refuses it: `J` is
/// not constrained by the trait, the self type or the predicates, so nothing
/// determines which join table to pick if two would do. Written that way
/// first, and `E0207` is the reason it is not written that way now. Which
/// table is the join table is a fact about the schema, and somebody has to
/// say it.
pub trait Through<C: Record>: Record {
    /// The join table between `Self` and `C`.
    type Join: Record;
}

/// [`load_related_through`] with the join table inferred from [`Through`].
///
/// # Errors
/// As [`load_related_through`].
pub async fn load_through<S, P, C>(
    store: &S,
    context: &SecurityContext,
    parents: &[P],
) -> Result<Vec<Vec<C>>>
where
    S: Records + Sync + ?Sized,
    P: Record + Through<C>,
    P: Related<<P as Through<C>>::Join>,
    <P as Through<C>>::Join: Related<C>,
    C: Record,
{
    load_related_through::<S, P, <P as Through<C>>::Join, C>(store, context, parents).await
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
