//! What one request may spend before it is refused.
//!
//! A join has had a budget since it was written: it materialises one side, so
//! left unbounded a mistyped join key is a way to turn a query into an
//! out-of-memory kill. The same argument applies to everything else that holds
//! state proportional to the data rather than to the answer, and until now
//! nothing else had one.
//!
//! Three things do:
//!
//! - `GROUP BY` holds one entry per distinct key. Grouping a large table by a
//!   unique column is a request-sized copy of that table.
//! - `COUNT(DISTINCT)` holds every distinct encoded value, and is exact by
//!   design — an approximate counter is a different feature, not a fix.
//! - `ORDER BY` with no `LIMIT` materialises the whole result before the first
//!   row. With a limit it uses a bounded heap, so the mitigation exists and
//!   simply is not reachable without one.
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

/// Per-request ceilings on the operators that hold unbounded state.
///
/// [`Default`] is the three constants in this module. Every field is a hard
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
        }
    }
}
