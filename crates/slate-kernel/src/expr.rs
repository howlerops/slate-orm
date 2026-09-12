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
    /// `column IS NULL`, or `IS NOT NULL` when negated.
    IsNull {
        /// The column being tested.
        column: Ordinal,
        /// Whether the test is inverted.
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
        match self {
            Self::True => Truth::True,
            Self::False => Truth::False,
            Self::Compare { column, op, value } => {
                let Some(actual) = row.get(*column) else {
                    return Truth::Unknown;
                };
                // A comparison touching a null is unknown, never false.
                if actual.is_null() || value.is_null() {
                    return Truth::Unknown;
                }
                Truth::from(op.apply(actual.cmp(value)))
            }
            Self::IsNull { column, negated } => {
                // `IS NULL` is the one test that is never unknown.
                let is_null = row.get(*column).is_none_or(Value::is_null);
                Truth::from(is_null != *negated)
            }
            Self::In { column, values } => {
                let Some(actual) = row.get(*column) else {
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
                    match part.evaluate(row) {
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
                    match part.evaluate(row) {
                        Truth::True => return Truth::True,
                        Truth::Unknown => result = Truth::Unknown,
                        Truth::False => {}
                    }
                }
                result
            }
            Self::Not(inner) => inner.evaluate(row).negate(),
        }
    }

    /// Whether `row` passes this predicate.
    #[must_use]
    pub fn admits(&self, row: &Row) -> bool {
        self.evaluate(row).admits()
    }
}
