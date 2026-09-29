//! What one request may spend before it is refused.
//!
//! A join has had a budget since it was written: it materialises one side, so
//! left unbounded a mistyped join key is a way to turn a query into an
//! out-of-memory kill. The same argument applies to everything else that holds
//! state proportional to the data rather than to the answer, and until now
//! nothing else had one.
//!
//! Five things do:
//!
//! - `GROUP BY` holds one entry per distinct key. Grouping a large table by a
//!   unique column is a request-sized copy of that table.
//! - `COUNT(DISTINCT)` holds every distinct encoded value, and is exact by
//!   design — an approximate counter is a different feature, not a fix.
//! - `ORDER BY` with no `LIMIT` materialises the whole result before the first
//!   row. With a limit it uses a bounded heap, so the mitigation exists and
//!   simply is not reachable without one.
//! - A window function holds every selected row, and unlike the sort it holds
//!   them whether or not there is a `LIMIT`: the window is computed before the
//!   limit applies, because `ROW_NUMBER() OVER (…) … LIMIT 10` has to number
//!   the rows before it can know which ten. There is no bounded form to fall
//!   back to, which is why this one has its own ceiling rather than borrowing
//!   the sort's.
//! - An `IN` list is checked against every row the scan reaches, so its cost
//!   is the list's length times the rows scanned. Unlike the four above it
//!   holds no state — the memory is the request's own bytes, which the
//!   transport already bounds — so this ceiling is about *time*, and it is the
//!   one limit here that refuses before any row is read rather than partway
//!   through. Above [`IN_LOOKUP_THRESHOLD`](crate::expr::IN_LOOKUP_THRESHOLD)
//!   the list becomes a hash lookup, which flattens the per-row cost but not
//!   the cost of building the set.
//!
//! # Refused, not killed
//!
//! Every limit here reports. A request that exceeds one gets an error naming
//! the limit and the value it passed, which a caller can act on; the
//! alternative is the OOM killer choosing a victim among whatever else the
//! node was serving, which nobody can act on.
//!
//! # Why these are defaults rather than a policy
//!
//! The numbers are chosen to be far above any reasonable query and far below
//! anything that threatens a node — they are not tuned, and a deployment that
//! knows its own memory should say so with [`ExecutionLimits`] rather than
//! treat these as correct for it. Set them per store with
//! [`RecordStore::with_limits`](crate::RecordStore::with_limits).

/// Distinct `GROUP BY` keys one request may hold.
///
/// A million groups of a handful of values each is already hundreds of
/// megabytes, and a grouped query returning a million rows is not one anybody
/// reads.
pub const DEFAULT_GROUP_LIMIT: usize = 1_000_000;

/// Distinct values one `COUNT(DISTINCT)` may hold.
///
/// Lower than the group limit because it is per aggregate and a query may
/// carry several, where groups are shared across the whole request.
pub const DEFAULT_DISTINCT_LIMIT: usize = 1_000_000;

/// Rows an unlimited `ORDER BY` may materialise.
///
/// A sort *with* a limit uses a bounded heap and is not affected: this is the
/// ceiling on the case that has no other bound. It is the largest of the three
/// because a row here is the answer the caller asked for, not bookkeeping.
pub const DEFAULT_SORT_LIMIT: usize = 5_000_000;

/// Rows a window function may materialise.
///
/// The same order as [`DEFAULT_SORT_LIMIT`] and for the same reason — these
/// are the caller's own rows rather than bookkeeping — but a separate number,
/// because a deployment that raised the sort ceiling because its sorts are
/// bounded by a `LIMIT` has said nothing about windows, which no `LIMIT`
/// bounds.
pub const DEFAULT_WINDOW_LIMIT: usize = 5_000_000;

/// Values one `IN` list may carry.
///
/// Ten thousand rather than a million, and the reason is that this ceiling is
/// unlike the other four: they bound state the node accumulates while
/// answering, where this bounds an input the caller sends. A caller who needs
/// to match ten thousand keys is describing a join, and saying so gets a hash
/// join with a build side the planner can cost — where an `IN` list of the
/// same keys is a filter the planner can only apply.
///
/// The number is chosen against the *transport* rather than against memory: a
/// list long enough to matter arrives inside a request the node has already
/// accepted and decoded, so by the time this fires the bytes are spent. What
/// it prevents is the scan.
pub const DEFAULT_IN_LIST_LIMIT: usize = 10_000;

/// Per-request ceilings on the operators whose cost the answer does not bound.
///
/// [`Default`] is the five constants in this module. Every field is a hard
/// refusal rather than a hint: an operator that would exceed one stops and
/// reports instead of continuing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionLimits {
    /// Distinct `GROUP BY` keys. See [`DEFAULT_GROUP_LIMIT`].
    pub max_groups: usize,
    /// Distinct values per `COUNT(DISTINCT)`. See [`DEFAULT_DISTINCT_LIMIT`].
    pub max_distinct: usize,
    /// Rows an unlimited `ORDER BY` may hold. See [`DEFAULT_SORT_LIMIT`].
    pub max_sort_rows: usize,
    /// Rows a window function may hold. See [`DEFAULT_WINDOW_LIMIT`].
    pub max_window_rows: usize,
    /// Values one `IN` list may carry. See [`DEFAULT_IN_LIST_LIMIT`].
    pub max_in_values: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self::new_default()
    }
}

impl ExecutionLimits {
    /// The defaults, in a `const` so a `const fn` constructor can use them.
    ///
    /// `Default::default` is not callable from a `const fn`, and
    /// [`RecordStore::new`](crate::RecordStore::new) is one.
    #[must_use]
    pub const fn new_default() -> Self {
        Self {
            max_groups: DEFAULT_GROUP_LIMIT,
            max_distinct: DEFAULT_DISTINCT_LIMIT,
            max_sort_rows: DEFAULT_SORT_LIMIT,
            max_window_rows: DEFAULT_WINDOW_LIMIT,
            max_in_values: DEFAULT_IN_LIST_LIMIT,
        }
    }
}

impl ExecutionLimits {
    /// Limits that refuse nothing.
    ///
    /// For a caller that has its own bound on the data — a test over a handful
    /// of rows, or a batch job that has already sized its machine. Named so
    /// that reaching for it is a decision rather than a `None`.
    #[must_use]
    pub const fn unbounded() -> Self {
        Self {
            max_groups: usize::MAX,
            max_distinct: usize::MAX,
            max_sort_rows: usize::MAX,
            max_window_rows: usize::MAX,
            max_in_values: usize::MAX,
        }
    }
}
