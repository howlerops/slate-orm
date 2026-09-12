//! Predicates over rows.
//!
//! Expressions are the common currency between a caller's filter and a security
//! policy's mandatory filter: both are [`Expr`], both are conjoined by the
//! planner, and both are evaluated by the same code. That is deliberate — a
//! policy that were merely "checked somewhere else" could be skipped, whereas a
//! policy that *is* part of the predicate cannot be.
//!
//! # Null handling
//!
//! Evaluation is three-valued, as in SQL. A comparison against a null is
//! [`Truth::Unknown`], not false, and a row is only returned when the predicate
//! evaluates to [`Truth::True`]. This matters for security: under two-valued
//! logic, `NOT (owner = :caller)` would be *true* for a row whose owner is null,
//! and a deny-style policy would leak exactly the rows nobody owns.

use slate_schema::{Ordinal, Row};
use slate_tuple::Value;
use std::collections::BTreeSet;

/// Somewhere a predicate can look a column up.
///
/// A [`Row`] is the obvious one. The other is a row of a join, which has
/// columns from several tables and so needs an ordinal space of its own; see
/// [`crate::join::JoinSchema`].
///
/// This exists so there is exactly one evaluator. The three-valued logic below
/// is a *security* property — `NOT (owner = :caller)` must not admit a row
/// whose owner is null — and a second copy of it written for joined rows would
/// be a second place for that to drift.
pub trait Columns {
    /// The value at `ordinal`, or `None` if there is no such column here.
    ///
    /// `None` and a stored null are deliberately not the same: a missing
    /// column makes a comparison unknown, which is also what a null does, but
    /// `IS NULL` distinguishes nothing between them and should not.
    fn value(&self, ordinal: Ordinal) -> Option<&Value>;
}

impl Columns for Row {
    fn value(&self, ordinal: Ordinal) -> Option<&Value> {
        self.get(ordinal)
    }
}

impl<T: Columns + ?Sized> Columns for &T {
    fn value(&self, ordinal: Ordinal) -> Option<&Value> {
        (**self).value(ordinal)
    }
}

/// A bare run of values, for evaluating against a row being built.
///
/// Appending computed values to a row means reading the row as it stands
/// while still growing it. Without this the only way to hand the evaluator
/// something was to build a `Row`, which meant cloning every value once per
/// computed column — quadratic, and on ClickBench's ninety-sum query that was
/// ninety seconds.
impl Columns for [Value] {
    fn value(&self, ordinal: Ordinal) -> Option<&Value> {
        self.get(ordinal.0)
    }
}

/// The result of evaluating a predicate under SQL's three-valued logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    /// Definitely true. Only this admits a row.
    True,
    /// Definitely false.
    False,
    /// Undetermined, because a null was involved.
    Unknown,
}

impl Truth {
    /// Whether a row with this result passes the filter.
    #[must_use]
    pub const fn admits(self) -> bool {
        matches!(self, Self::True)
    }

    const fn negate(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

impl From<bool> for Truth {
    fn from(b: bool) -> Self {
        if b { Self::True } else { Self::False }
    }
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    /// `=`
    Eq,
    /// `<>`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl CmpOp {
    /// The operator that means the same thing when the column's stored order is
    /// reversed. Used to derive index bounds on a descending column.
    #[must_use]
    pub const fn mirrored(self) -> Self {
        match self {
            Self::Eq => Self::Eq,
            Self::Ne => Self::Ne,
            Self::Lt => Self::Gt,
            Self::Le => Self::Ge,
            Self::Gt => Self::Lt,
            Self::Ge => Self::Le,
        }
    }

    fn apply(self, ordering: core::cmp::Ordering) -> bool {
        use core::cmp::Ordering::{Equal, Greater, Less};
        match self {
            Self::Eq => ordering == Equal,
            Self::Ne => ordering != Equal,
            Self::Lt => ordering == Less,
            Self::Le => matches!(ordering, Less | Equal),
            Self::Gt => ordering == Greater,
            Self::Ge => matches!(ordering, Greater | Equal),
        }
    }
}

/// A predicate over one table's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Expr {
    /// Matches every row. The identity of [`Expr::And`].
    True,
    /// Matches no row. The identity of [`Expr::Or`].
    False,
    /// `column <op> value`.
    Compare {
        /// The column being compared.
        column: Ordinal,
        /// The operator.
        op: CmpOp,
        /// The literal to compare against.
        value: Value,
    },
    /// `left <op> right`, comparing two columns of the same row.
    ///
    /// Distinct from [`Expr::Compare`] because there is no literal: the
    /// planner cannot turn this into a scan bound, since a bound needs a value
    /// known before the row is read. It stays a residual filter, always.
    ///
    /// A join's cross-side condition is this, over a joined row's ordinal
    /// space; within one table it is the ordinary `WHERE started < finished`.
    ///
    /// Both columns must have the same type. [`Value`]'s order is type-first —
    /// that is what makes the key encoding sortable — so comparing an integer
    /// column to a float one would compare the *types* and quietly answer the
    /// same way for every row. Rather than coerce (which would put value
    /// comparison and encoding order out of step, and scan bounds are derived
    /// from encoding order) the mismatch is refused; see
    /// [`Expr::column_type_conflict`].
    CompareColumns {
        /// The column on the left of the operator.
        left: Ordinal,
        /// The operator.
        op: CmpOp,
        /// The column on the right of the operator.
        right: Ordinal,
    },
    /// `column IS NULL`, or `IS NOT NULL` when negated.
    IsNull {
        /// The column being tested.
        column: Ordinal,
        /// Whether the test is inverted.
        negated: bool,
    },
    /// `column LIKE pattern`, with SQL's wildcards: `%` matches any run of
    /// characters and `_` matches exactly one.
    ///
    /// A pattern anchored at the front — `'abc%'` — is a key range rather than
    /// a filter, and the planner turns it into one. Anything else stays a
    /// residual, because a pattern that can start anywhere says nothing about
    /// where in the keyspace its matches are.
    Like {
        /// The column being matched.
        column: Ordinal,
        /// The pattern.
        pattern: String,
        /// Whether the test is inverted, for `NOT LIKE`.
        negated: bool,
    },
    /// `column IN (values)`.
    In {
        /// The column being tested.
        column: Ordinal,
        /// The candidate values.
        values: Vec<Value>,
    },
    /// Conjunction.
    And(Vec<Expr>),
    /// Disjunction.
    Or(Vec<Expr>),
    /// Negation.
    Not(Box<Expr>),
}

impl Expr {
    /// `column = value`.
    #[must_use]
    pub const fn eq(column: Ordinal, value: Value) -> Self {
        Self::Compare {
            column,
            op: CmpOp::Eq,
            value,
        }
    }

    /// `column <op> value`.
    #[must_use]
    pub const fn compare(column: Ordinal, op: CmpOp, value: Value) -> Self {
        Self::Compare { column, op, value }
    }

    /// `left <op> right`, comparing two columns rather than a column and a
    /// literal. See [`Expr::CompareColumns`].
    #[must_use]
    pub const fn compare_columns(left: Ordinal, op: CmpOp, right: Ordinal) -> Self {
        Self::CompareColumns { left, op, right }
    }

    /// `column LIKE pattern`. See [`Expr::Like`].
    #[must_use]
    pub fn like(column: Ordinal, pattern: impl Into<String>) -> Self {
        Self::Like {
            column,
            pattern: pattern.into(),
            negated: false,
        }
    }

    /// `column NOT LIKE pattern`.
    #[must_use]
    pub fn not_like(column: Ordinal, pattern: impl Into<String>) -> Self {
        Self::Like {
            column,
            pattern: pattern.into(),
            negated: true,
        }
    }

    /// `column IS NULL`.
    #[must_use]
    pub const fn is_null(column: Ordinal) -> Self {
        Self::IsNull {
            column,
            negated: false,
        }
    }

    /// `column IS NOT NULL`.
    #[must_use]
    pub const fn is_not_null(column: Ordinal) -> Self {
        Self::IsNull {
            column,
            negated: true,
        }
    }

    /// Conjoin two predicates, dropping trivial operands.
    ///
    /// Used to fold a policy predicate into a caller's filter, so the common
    /// case of "no caller filter" does not leave an `AND true` behind for the
    /// planner to look through.
    #[must_use]
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, e) | (e, Self::True) => e,
            (Self::And(mut a), Self::And(b)) => {
                a.extend(b);
                Self::And(a)
            }
            (Self::And(mut a), e) | (e, Self::And(mut a)) => {
                a.push(e);
                Self::And(a)
            }
            (a, b) => Self::And(vec![a, b]),
        }
    }

    /// Conjoin a list, folding away the trivial cases.
    ///
    /// An empty list is [`Expr::True`]: nothing to require is satisfied by
    /// everything, which is what makes this composable.
    #[must_use]
    pub fn all<I: IntoIterator<Item = Self>>(parts: I) -> Self {
        parts.into_iter().fold(Self::True, Self::and)
    }

    /// The top-level conjuncts of this predicate.
    ///
    /// A predicate that is not a conjunction is its own single conjunct. The
    /// planner works over this list: each conjunct can independently become a
    /// scan bound or stay behind as a residual filter.
    #[must_use]
    pub fn conjuncts(&self) -> Vec<&Self> {
        match self {
            Self::True => Vec::new(),
            Self::And(parts) => parts.iter().flat_map(Self::conjuncts).collect(),
            other => vec![other],
        }
    }

    /// Evaluate against a row under three-valued logic.
    #[must_use]
    pub fn evaluate(&self, row: &Row) -> Truth {
        self.evaluate_over(row)
    }

    /// Evaluate against anything that can produce a column value.
    ///
    /// The same evaluator [`Expr::evaluate`] uses; that one is the `Row` case,
    /// kept as its own name because it is what nearly every call site wants.
    #[must_use]
    pub fn evaluate_over<C: Columns + ?Sized>(&self, row: &C) -> Truth {
        match self {
            Self::True => Truth::True,
            Self::False => Truth::False,
            Self::Compare { column, op, value } => {
                let Some(actual) = row.value(*column) else {
                    return Truth::Unknown;
                };
                // A comparison touching a null is unknown, never false.
                if actual.is_null() || value.is_null() {
                    return Truth::Unknown;
                }
                Truth::from(op.apply(actual.cmp(value)))
            }
            Self::CompareColumns { left, op, right } => {
                let (Some(a), Some(b)) = (row.value(*left), row.value(*right)) else {
                    return Truth::Unknown;
                };
                // Same rule as against a literal: a null on either side makes
                // the comparison unknown. On a joined row that is what an
                // outer join's missing side produces, so a condition touching
                // it does not admit — which is the `ON` semantics wanted.
                if a.is_null() || b.is_null() {
                    return Truth::Unknown;
                }
                Truth::from(op.apply(a.cmp(b)))
            }
            Self::Like {
                column,
                pattern,
                negated,
            } => {
                let Some(actual) = row.value(*column) else {
                    return Truth::Unknown;
                };
                // A null matches no pattern, and does not fail to match one
                // either: the same three-valued rule as a comparison.
                let Value::Str(text) = actual else {
                    return Truth::Unknown;
                };
                Truth::from(like_matches(text, pattern) != *negated)
            }
            Self::IsNull { column, negated } => {
                // `IS NULL` is the one test that is never unknown.
                let is_null = row.value(*column).is_none_or(Value::is_null);
                Truth::from(is_null != *negated)
            }
            Self::In { column, values } => {
                let Some(actual) = row.value(*column) else {
                    return Truth::Unknown;
                };
                if actual.is_null() {
                    return Truth::Unknown;
                }
                if values.iter().any(|v| !v.is_null() && v == actual) {
                    Truth::True
                } else if values.iter().any(Value::is_null) {
                    // A null candidate could have matched; we cannot say no.
                    Truth::Unknown
                } else {
                    Truth::False
                }
            }
            Self::And(parts) => {
                let mut result = Truth::True;
                for part in parts {
                    match part.evaluate_over(row) {
                        Truth::False => return Truth::False,
                        Truth::Unknown => result = Truth::Unknown,
                        Truth::True => {}
                    }
                }
                result
            }
            Self::Or(parts) => {
                let mut result = Truth::False;
                for part in parts {
                    match part.evaluate_over(row) {
                        Truth::True => return Truth::True,
                        Truth::Unknown => result = Truth::Unknown,
                        Truth::False => {}
                    }
                }
                result
            }
            Self::Not(inner) => inner.evaluate_over(row).negate(),
        }
    }

    /// Whether `row` passes this predicate.
    #[must_use]
    pub fn admits(&self, row: &Row) -> bool {
        self.evaluate(row).admits()
    }

    /// Whether anything with columns passes this predicate.
    #[must_use]
    pub fn admits_over<C: Columns + ?Sized>(&self, row: &C) -> bool {
        self.evaluate_over(row).admits()
    }

    /// The same predicate with every column ordinal put through `f`.
    ///
    /// A predicate written in one ordinal space, read in another. Used to move
    /// a join's cross-side condition back into a single table's ordinals so
    /// that table's statistics can be applied to it.
    #[must_use]
    pub fn map_columns(&self, f: &impl Fn(Ordinal) -> Ordinal) -> Self {
        match self {
            Self::True => Self::True,
            Self::False => Self::False,
            Self::Compare { column, op, value } => Self::Compare {
                column: f(*column),
                op: *op,
                value: value.clone(),
            },
            Self::CompareColumns { left, op, right } => Self::CompareColumns {
                left: f(*left),
                op: *op,
                right: f(*right),
            },
            Self::Like {
                column,
                pattern,
                negated,
            } => Self::Like {
                column: f(*column),
                pattern: pattern.clone(),
                negated: *negated,
            },
            Self::IsNull { column, negated } => Self::IsNull {
                column: f(*column),
                negated: *negated,
            },
            Self::In { column, values } => Self::In {
                column: f(*column),
                values: values.clone(),
            },
            Self::And(parts) => Self::And(parts.iter().map(|p| p.map_columns(f)).collect()),
            Self::Or(parts) => Self::Or(parts.iter().map(|p| p.map_columns(f)).collect()),
            Self::Not(inner) => Self::Not(Box::new(inner.map_columns(f))),
        }
    }

    /// The first column comparison whose two sides have different types.
    ///
    /// `types` answers what a column holds; a column it does not know is
    /// skipped rather than assumed to conflict. Only [`Expr::CompareColumns`]
    /// is checked — everything else compares against a literal the caller
    /// wrote next to the column, where a mismatch is visible at the call site.
    #[must_use]
    pub fn column_type_conflict(
        &self,
        types: &impl Fn(Ordinal) -> Option<slate_tuple::ValueType>,
    ) -> Option<(Ordinal, Ordinal)> {
        match self {
            Self::True
            | Self::False
            | Self::Compare { .. }
            | Self::IsNull { .. }
            | Self::In { .. }
            | Self::Like { .. } => None,
            Self::CompareColumns { left, right, .. } => match (types(*left), types(*right)) {
                (Some(a), Some(b)) if a != b => Some((*left, *right)),
                _ => None,
            },
            Self::And(parts) | Self::Or(parts) => {
                parts.iter().find_map(|p| p.column_type_conflict(types))
            }
            Self::Not(inner) => inner.column_type_conflict(types),
        }
    }

    /// Every column this predicate reads.
    ///
    /// An index-only scan is only sound if the predicate can be evaluated from
    /// the index entry alone, so the planner needs this to decide whether the
    /// row lookup can be skipped. Since the security filter is part of the
    /// predicate, a policy on an uncovered column correctly prevents the
    /// optimisation rather than being skipped by it.
    #[must_use]
    pub fn columns(&self) -> BTreeSet<Ordinal> {
        let mut out = BTreeSet::new();
        self.collect_columns(&mut out);
        out
    }

    fn collect_columns(&self, out: &mut BTreeSet<Ordinal>) {
        match self {
            Self::True | Self::False => {}
            Self::Compare { column, .. }
            | Self::IsNull { column, .. }
            | Self::In { column, .. }
            | Self::Like { column, .. } => {
                out.insert(*column);
            }
            Self::CompareColumns { left, right, .. } => {
                out.insert(*left);
                out.insert(*right);
            }
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_columns(out);
                }
            }
            Self::Not(inner) => inner.collect_columns(out),
        }
    }
}

/// Whether `text` matches a SQL `LIKE` pattern.
///
/// `%` matches any run of characters, `_` matches exactly one. Both can be
/// escaped with a backslash, which is what most dialects do without an
/// explicit `ESCAPE` clause.
///
/// Iterative with backtracking rather than recursive: a pattern is caller
/// input, and a recursive matcher on `%a%a%a%…` is a stack overflow waiting to
/// be sent. This is O(text x pattern) in the worst case and linear in
/// practice.
#[must_use]
pub fn like_matches(text: &str, pattern: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();

    let (mut t, mut p) = (0usize, 0usize);
    // Where to resume if the current `%` turns out to have matched too little.
    let (mut star_p, mut star_t) = (None, 0usize);

    while t < text.len() {
        let literal = match pattern.get(p) {
            Some('%') => {
                star_p = Some(p);
                star_t = t;
                p += 1;
                continue;
            }
            Some('_') => {
                p += 1;
                t += 1;
                continue;
            }
            // An escape takes the next character literally, and a trailing
            // backslash is itself.
            Some('\\') => pattern.get(p + 1).copied().unwrap_or('\\'),
            Some(other) => *other,
            None => {
                // Pattern spent with text left over: only a `%` can absorb it.
                match star_p {
                    Some(star) => {
                        p = star + 1;
                        star_t += 1;
                        t = star_t;
                        continue;
                    }
                    None => return false,
                }
            }
        };
        let width = if pattern.get(p) == Some(&'\\') { 2 } else { 1 };

        if text.get(t) == Some(&literal) {
            p += width;
            t += 1;
            continue;
        }
        match star_p {
            Some(star) => {
                p = star + 1;
                star_t += 1;
                t = star_t;
            }
            None => return false,
        }
    }

    // Text spent: whatever is left of the pattern must match nothing. `p` can
    // run past the end when the pattern was exhausted first, which `get` reads
    // as "nothing left", the same answer.
    pattern
        .get(p..)
        .is_none_or(|rest| rest.iter().all(|c| *c == '%'))
}

/// The literal prefix a pattern requires, if it requires one.
///
/// `'abc%'` and `'abc%def'` both begin with `abc`, so every match sorts inside
/// that prefix and the scan can be bounded by it. `'%abc'` has none. Returns
/// `None` rather than an empty string when there is nothing to bound by, so a
/// caller cannot mistake "no constraint" for "matches the empty prefix".
#[must_use]
pub fn like_prefix(pattern: &str) -> Option<String> {
    let mut prefix = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' | '_' => break,
            '\\' => match chars.next() {
                Some(escaped) => prefix.push(escaped),
                None => prefix.push('\\'),
            },
            other => prefix.push(other),
        }
    }
    (!prefix.is_empty()).then_some(prefix)
}
