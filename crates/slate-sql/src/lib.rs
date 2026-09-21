//! The SQL subset, and the query spec it compiles to.
//!
//! # Why this is its own crate
//!
//! It was `slate-wasm`'s for as long as the browser was the only caller, and
//! that reading of it was wrong in a way `docs/views.md` had written down as
//! an architectural constraint: a `[[views]]` block holding SQL "would make
//! the daemon's schema loader depend on the SQL front end, which lives in
//! `slate-wasm` — a dependency direction nothing has needed yet".
//!
//! Nothing here is browser code. The parser mentions no `wasm_bindgen`; the
//! specs are `serde` types; the lowering takes a `TableDef` and returns a
//! `slate_kernel::Query`. The whole of `slate-wasm` was 7,384 lines of which
//! five mentioned the binding, and this is most of the rest.
//!
//! So `slate-wasm` and `slate-serverd` are peers over this crate rather than
//! one reaching into the other, and a view can be written as the SQL it is.
//!
//! # What a spec is
//!
//! The **query spec** is the shape an SDK sends: a table, filters, sorts,
//! computes, a join or a chain, aggregates, windows. SQL is a front end onto
//! it, not a second query language — `sql::parse` produces one of these and
//! the workbench shows it beside the answer, which is what keeps "the editor
//! is a front end" checkable rather than asserted.

#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod sql;

use serde::{Deserialize, Serialize};

/// What the UI sends. Every field optional, because the panel builds it up.
///
/// `Serialize` as well as `Deserialize` because the workbench shows the
/// compiled spec beside the results: SQL typed in the editor is lowered onto
/// this and the reader is shown what it became, which is the only honest way
/// to offer SQL for a database that does not have any.
///
/// The `skip_serializing_if` attributes are for that panel rather than for
/// the wire — a spec printed with six empty fields buries the two that the
/// reader's query actually set.
#[derive(Deserialize, Serialize, Default, Debug, Clone, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct QuerySpec {
    pub table: String,
    /// One condition, kept for the shape the panel started with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<FilterSpec>,
    /// Several, ANDed. Where this gets interesting is that the planner does
    /// not treat them alike: one conjunct may become a scan bound and the
    /// rest stay a residual predicate evaluated per row, and the plan says
    /// which is which.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FilterSpec>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "is_zero")]
    pub offset: u64,
    /// Column ordinals to return. Empty means every column — which is also
    /// what makes an index-only scan impossible, so the UI exposes it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<u32>,
    /// Group by these columns. Empty means return rows rather than groups.
    ///
    /// Grouping is not a filter applied after the fact: the kernel narrows the
    /// projection to the group keys and the aggregates' columns, which is what
    /// lets `count(*) per zone` be answered from an index without reading a
    /// row. The plan says whether it managed to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<u32>,
    /// What to compute per group. Empty is not a shorthand for `count(*)`:
    /// a grouping with keys and no aggregates is `SELECT DISTINCT` over those
    /// keys, which is how the SQL front end lowers it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aggregates: Vec<AggregateSpec>,
    /// Values computed per row and appended after the table's own columns, so
    /// the `i`th sits at ordinal `columns().len() + i`. Everything downstream
    /// — a group key, a sort key, a HAVING — addresses one the ordinary way.
    ///
    /// This is how `hour(pickup_time)` becomes something the spec can hold:
    /// the query spec has no expression language and is not getting one, and a
    /// computed column is the kernel's own answer to that.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub compute: Vec<ComputeSpec>,
    /// Which groups survive, ANDed. Ordinals are in *group* space —
    /// `[keys..., aggregates...]` — the same space `sort` uses when there is a
    /// grouping, and not the table's. `HAVING count(*) > 100` names ordinal
    /// `group_by.len()`, not a column.
    ///
    /// A separate field from `filters` rather than a flag on it, because the
    /// two are evaluated against different things at different times: a filter
    /// can become a scan bound and skip rows before they are read, and a
    /// having cannot — the group it tests does not exist until every row is in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub having: Vec<FilterSpec>,
    /// Values computed over a partition, one per input row, appended after the
    /// computed ones — so the `i`th sits at ordinal
    /// `columns().len() + compute.len() + i` and everything downstream, a sort
    /// key included, addresses it the ordinary way.
    ///
    /// **After** the computed values rather than before, because a window may
    /// partition by or order on one and a computed value may not read a
    /// window. The dependency runs one way, so the layout does too.
    ///
    /// Mutually exclusive with a grouping: `GROUP BY` folds rows away and a
    /// window answers per row, so a spec carrying both is two answers to one
    /// question. [`build`] refuses it rather than dropping one, which is what
    /// the kernel's own `narrowed` does for the grouped path and is correct
    /// there and silent here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub window: Vec<WindowSpec>,
}

/// One window function and the `OVER (…)` clause it is computed under.
///
/// The shape mirrors the gRPC `Window` rather than the kernel's enum, for the
/// reason every other spec in this file mirrors the wire: this is what a UI
/// sends over JSON, and a tagged union is not something a hand-written panel
/// or a JSON literal produces comfortably. The kernel's own refusals —
/// an unordered `RANK`, a running `COUNT(DISTINCT)`, an offset of zero — are
/// left to `Window::new`, so there is one statement of each rule.
#[derive(Deserialize, Serialize, Default, Debug, Clone, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowSpec {
    /// `row_number`, `rank`, `dense_rank`, `lag`, `lead`, or `aggregate`.
    pub function: String,
    /// What to aggregate, when `function` is `aggregate`. Its `input` is
    /// ignored: a window here is over one table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<AggregateSpec>,
    /// The column `lag` and `lead` read. Ignored by every other function,
    /// and *not* skipped when zero, because ordinal 0 is a real column.
    pub column: u32,
    /// How many rows back (`lag`) or forward (`lead`). Zero is refused by the
    /// kernel — it is the current row spelled obscurely.
    pub offset: u64,
    /// `PARTITION BY`. Empty is one partition over the whole result, which is
    /// what SQL means by omitting the clause — not one partition per row.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub partition_by: Vec<u32>,
    /// The window's own `ORDER BY`, which is not the query's: it decides peer
    /// groups and turns an aggregate's frame into a running one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<SortSpec>,
}

/// For `skip_serializing_if`, which needs a path rather than a closure.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FilterSpec {
    pub column: u32,
    pub op: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// The candidate list, when `op` is `in`.
    ///
    /// A separate field rather than `value` holding a comma-joined string,
    /// because a string value can itself contain a comma and splitting one
    /// would make `WHERE payment IN ('cash, tip')` two candidates instead of
    /// one — silently, and only for the data that has a comma in it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Where the candidate list comes from, when it comes from a query.
    ///
    /// `WHERE pickup_zone IN (SELECT id FROM zones WHERE borough = 'Queens')`
    /// is *two* reads: the inner one runs first, its single column becomes
    /// `values`, and the outer one is an ordinary `Expr::In` the planner can
    /// already turn into point gets or an index range. That is the same
    /// lowering `load_related` uses in the Rust layer, and the reason a
    /// subquery needed no new kernel operator.
    ///
    /// Only an *uncorrelated* subquery fits: the inner query is run once,
    /// before the outer one, so it cannot mention a column of the outer table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subquery: Option<Box<QuerySpec>>,
}

/// One computed column: a function of one of the table's own columns.
///
/// A named function rather than a nested expression tree. The kernel's
/// `Scalar` is a tree and could carry `hour(x) + 1`, but the SQL front end has
/// no expression grammar to produce one — `WHERE` takes `col <op> literal` and
/// nothing else — so a spec that could express more than the parser can parse
/// would be a shape nobody produces and nobody tests.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ComputeSpec {
    /// `hour`, `year`, `day_of_week`, and the rest of `compute_scalar`.
    pub function: String,
    /// Which side of the join the column is on: 0 for the left table, 1 for
    /// the right. Zero on a single-table query, where there is only one.
    ///
    /// This used to be implicit and left-only. `Join::compute` is evaluated
    /// over the *joined* row and has always been able to read either side; the
    /// restriction was in this spec, which resolved every computed column's
    /// name against the left table. So `hour(pickup_time)` worked on `trips`
    /// and `hour(zones.updated_at)` was "not a column of trips".
    #[serde(default)]
    pub input: u32,
    /// The column it reads, an ordinal within the table `input` names.
    pub column: u32,
    /// Seconds to add before reading the calendar out, so a timestamp stored
    /// in UTC can be asked about at a fixed offset. Zero is UTC.
    ///
    /// This needs no kernel and no wire support, because it is already
    /// expressible: shifting a timestamp is adding to it, and `Scalar::Add`
    /// has been there since scalars arrived. `compute_scalar` wraps the column
    /// rather than passing an offset down, so every path that already handles
    /// `Add` — the planner, the wire, the covering scan — handles this one
    /// with no change at all.
    ///
    /// Mutually exclusive with [`ComputeSpec::zone`]: a fixed offset and a
    /// named zone are two answers to one question, and the parser produces at
    /// most one of them. `compute_scalar` refuses both rather than picking.
    #[serde(default)]
    pub offset: i64,
    /// An IANA zone name — `America/New_York` — whose offset *at each row's
    /// instant* is added before the calendar is read out. Empty is "no zone".
    ///
    /// This used to be refused, with a reason that was true of a timezone
    /// *database* and not of a timezone *table*: the kernel now ships the
    /// transitions for a curated list of zones, a few kilobytes of sorted
    /// integers, so daylight saving is looked up rather than guessed at. A
    /// name outside the list is still refused, and the refusal names the ones
    /// that are there.
    ///
    /// Unlike `offset`, this does need a kernel variant: the shift is not a
    /// constant, so `Scalar::Add` cannot express it. `Scalar::ZoneShift`
    /// composes the same way, so the same argument applies one level down —
    /// the planner and the wire handle a nested scalar already.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub zone: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub column: u32,
    #[serde(default)]
    pub descending: bool,
}

/// What the UI sends for a join or a grouped join.
///
/// This used to hard-code `authors.id = books.author_id`, which was defensible
/// while that was the only join in the database. With `trips` joining `zones`
/// there are two, and a spec that can only express one of them would have made
/// the zone names — the entire reason to carry a lookup table — unreachable.
///
/// The keys are still named explicitly rather than discovered: the parser
/// checks the `ON` clause against the schema, so a pair of columns that would
/// join to nothing is refused with a reason instead of returning an empty
/// result.
#[derive(Deserialize, Serialize, Default, Debug, Clone, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct JoinSpec {
    /// The left table's name. Its columns come first in a joined row.
    pub left: String,
    /// The right table's name.
    pub right: String,
    /// What this query calls the left table, when that is not its own name.
    /// Empty means the table's name. See [`ChainInputSpec::alias`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub left_alias: String,
    /// And the right. Two tables need aliases for the same reason n do:
    /// `trips AS a JOIN trips AS b ON a.dropoff_zone = b.pickup_zone` is one
    /// table twice, and the kernel joins it without complaint — this was
    /// checked before the field was added rather than assumed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub right_alias: String,
    /// The join key's ordinal in the left table.
    pub left_key: u32,
    /// And in the right table.
    pub right_key: u32,
    /// Conditions on the left table, ANDed.
    pub left_where: Vec<FilterSpec>,
    /// Conditions on the right table, ANDed.
    pub right_where: Vec<FilterSpec>,
    /// Computed per joined row, appended after *both* tables' columns. See
    /// `Join::compute`.
    ///
    /// Each names its side with `ComputeSpec::input`, so an expression may read
    /// either table. It used to be able to read only the left, which was a
    /// limitation of this spec rather than of the kernel.
    pub compute: Vec<ComputeSpec>,
    /// Group by these ordinals of the **joined row**: a left column keeps its
    /// own ordinal, a right column sits at `left.columns() + n`, and a computed
    /// column at `left.columns() + right.columns() + i`. Empty means return
    /// joined rows rather than groups.
    ///
    /// A list rather than one key, matching [`QuerySpec::group_by`], which has
    /// been a list since grouping arrived. The asymmetry was not a decision:
    /// the single-table path grew a second key and the joined path was never
    /// revisited, so `GROUP BY payment, passengers` worked on `trips` and was a
    /// refusal the moment a join appeared — and the workbench's own "kitchen
    /// sink" example said so in prose, splitting itself into two statements to
    /// work around it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<u32>,
    /// Computed per group. Each names its side with `AggregateSpec::input`.
    pub aggregates: Vec<AggregateSpec>,
    /// Conditions over the **groups**, ANDed, applied after grouping.
    ///
    /// `column` is a group-space ordinal — `[keys..., aggregates...]` — not a
    /// joined-row one, because that is what a HAVING term names: `HAVING
    /// count(*) > 300` is about the count this query computes, and a group
    /// carries nothing else. The parser resolves it, which is the only place
    /// that knows which aggregate the reader meant.
    ///
    /// Only meaningful with `group_by`, for the reason `sort` is: without
    /// groups there is nothing to filter, and the parser says so rather than
    /// accepting it into a field nothing reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub having: Vec<FilterSpec>,
    /// How to order the **groups** of a grouped join. Empty leaves them in the
    /// order the encoded key sorts them.
    ///
    /// A group is `[key, aggregates...]`, so `column: 0` is the key and
    /// `column: 1` is the first aggregate — a space of its own with nothing to
    /// do with either table's ordinals. That is `Grouping::sort`, and it is
    /// what `ORDER BY count(*) DESC` on a grouped join lowers onto.
    ///
    /// Only meaningful with `group_by`. An *ungrouped* join has no sort,
    /// because `Join` has no sort: the kernel orders groups and not joined
    /// rows, and the parser refuses `ORDER BY` there with that reason rather
    /// than accepting it into a field nothing reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortSpec>,
    pub limit: Option<u64>,
    /// Rows, or groups, to discard first.
    ///
    /// On a grouped join this is `Grouping::offset` and on an ungrouped one
    /// `Join::offset`, both of which the kernel has always had. This spec did
    /// not, so `LIMIT 3 OFFSET 5` was a parse error on a join and worked on a
    /// single table — a difference in the *front end* that read as a
    /// difference in the engine.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: u64,
}

/// Three or more tables, chained.
///
/// Separate from [`JoinSpec`] rather than replacing it, and the split is the
/// kernel's own: `Join` and `Chain` are two types with two entry points, and
/// `group_by_join` narrows each side's projection in a way `group_by_chain`
/// cannot. `slate-server`'s `MultiRead` and `GroupedSource` make the same
/// split, with a comment saying that folding them would mean choosing at the
/// call site anyway, one level further from the reason.
///
/// So: exactly two tables is a [`JoinSpec`] and runs through `Join`; three or
/// more is this and runs through `Chain`. The parser decides, and never
/// produces a two-input `ChainSpec` — that would be a second spec for a query
/// the first already expresses, which is the drift this front end exists to
/// avoid.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChainSpec {
    /// The tables, in the order the chain reads them. At least three.
    pub inputs: Vec<ChainInputSpec>,
    /// Computed per chain row, appended after *every* table's columns. See
    /// `Chain::compute`, and `ComputeSpec::input` for how each names its
    /// table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compute: Vec<ComputeSpec>,
    /// Group by these ordinals of the **chain row**: input `n`'s column `c` is
    /// at the sum of the widths before `n`, plus `c`; a computed value is
    /// past every table. Empty means return rows. See [`JoinSpec::group_by`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<u32>,
    /// Computed per group. Each names its input with `AggregateSpec::input`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aggregates: Vec<AggregateSpec>,
    /// Conditions over the **groups**, ANDed, applied after grouping.
    ///
    /// `column` is a group-space ordinal — `[keys..., aggregates...]` — not a
    /// joined-row one, because that is what a HAVING term names: `HAVING
    /// count(*) > 300` is about the count this query computes, and a group
    /// carries nothing else. The parser resolves it, which is the only place
    /// that knows which aggregate the reader meant.
    ///
    /// Only meaningful with `group_by`, for the reason `sort` is: without
    /// groups there is nothing to filter, and the parser says so rather than
    /// accepting it into a field nothing reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub having: Vec<FilterSpec>,
    /// How to order the groups. `[keys..., aggregates...]`, as on [`JoinSpec`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: u64,
}

/// One table of a chain.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChainInputSpec {
    /// The table's name, as the catalog spells it.
    pub table: String,
    /// What this query calls the table, when that is not the table's own name.
    ///
    /// Empty means "the table's name", which is what makes a chain that uses no
    /// alias serialise exactly as it did before aliases existed. The alias is
    /// what a *qualifier* resolves against and what the header shows; the
    /// binding still looks the table up by `table`, because an alias renames an
    /// input and not a table.
    ///
    /// This is the field that lets one table appear twice. `trips` reaches
    /// `zones` through both `pickup_zone` and `dropoff_zone`, and "the borough
    /// it started in and the borough it ended in" is the three-table question
    /// this dataset is for — inexpressible while every input was identified by
    /// its table's name, because two `zones` made every column reference
    /// ambiguous with no way to say which was meant.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alias: String,
    /// What this joins to. Absent on input 0, which joins to nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<ChainOnSpec>,
    /// Conditions on this table alone, ANDed, so the planner can push each
    /// into this table's own scan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FilterSpec>,
}

/// Which earlier table a step joins to.
///
/// `input` and `column` name a column of an *earlier* input — any earlier one,
/// not just the previous — and `own` is this table's own ordinal. That is
/// exactly `JoinKey` in the joined space, which is what lets `a JOIN b JOIN c
/// ON a.x = c.y` work rather than only a straight line.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChainOnSpec {
    /// Which earlier input.
    pub input: u32,
    /// Its column, in that table's own ordinals.
    pub column: u32,
    /// This table's column.
    pub own: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AggregateSpec {
    pub kind: String,
    /// Which side of the join the column is on: 0 for the left table, 1 for
    /// the right. Zero on a single-table query.
    ///
    /// Like `ComputeSpec::input`, this used to be implicit -- and implicitly
    /// the *opposite* side: an aggregate's ordinal was shifted past every left
    /// column unconditionally, so it could only name the right table. Between
    /// the two defaults, "group by the hour and average the fare" over `trips
    /// JOIN zones` was not expressible: the group key had to come from the left
    /// and the aggregate from the right, and both of those columns are on
    /// `trips`.
    #[serde(default)]
    pub input: u32,
    /// Ordinal within the table `input` names, ignored by `count`.
    #[serde(default)]
    pub column: u32,
}
