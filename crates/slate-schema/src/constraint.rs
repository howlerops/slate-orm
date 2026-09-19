//! Constraints a row must satisfy: `CHECK` predicates and foreign keys.
//!
//! Both are declared on the table and enforced by the record store's write
//! path. That is the only place they *can* be enforced: a constraint checked by
//! a wrapper is a constraint a caller can step around, which is the same
//! argument the security layer makes for living in the kernel rather than in
//! the ORM.
//!
//! # Why a trait rather than an expression
//!
//! A `CHECK` should be an ordinary predicate — the same language a filter and a
//! row policy are written in — rather than a second one that has to learn SQL's
//! null rules all over again. That language is `slate_kernel::Expr`, and the
//! kernel depends on this crate rather than the other way round, so a
//! [`TableDef`](crate::TableDef) cannot name it.
//!
//! [`Predicate`] is the seam. This crate says what a check needs of a
//! predicate; the kernel implements it for `Expr`. There is still one
//! expression language, one evaluator and one set of null rules.
//!
//! Moving `Expr` down into this crate was the obvious alternative and was
//! rejected: it drags the regex engine and the pattern cache into the schema
//! crate to buy nothing that the trait does not already buy.

use crate::row::Row;
use crate::table::{Ordinal, TableId};
use std::sync::Arc;

/// Something that can judge a row.
///
/// Implemented in the kernel for `Expr`; see the module docs for why this is a
/// trait and not a stored expression.
pub trait Predicate: Send + Sync + 'static {
    /// The predicate's truth for `row`, three-valued as in SQL: `None` means a
    /// null made the answer unknown.
    ///
    /// Deliberately not a `bool`. Which way "unknown" falls is *not* the same
    /// in every context — a `WHERE` withholds the row, a `CHECK` accepts it —
    /// so collapsing it here would hand that decision to each implementor and
    /// give the two rules two places to drift apart.
    fn truth(&self, row: &Row) -> Option<bool>;

    /// The predicate itself, for a caller that needs to *read* it rather than
    /// run it. `None` by default, and by default is the honest answer.
    ///
    /// One caller needs this: the planner, deciding whether it may read a
    /// partial index. That decision is an implication between two predicates —
    /// does everything this query admits fall inside what the index holds — and
    /// no amount of running either of them on rows answers it. It needs the
    /// tree.
    ///
    /// Downcasting is the price of the seam. The alternative was to state a
    /// partial index's predicate twice, once as something the write path can
    /// run and once as something the planner can read, and two statements of
    /// one rule drift. A predicate that declines to answer here is simply an
    /// index the planner never chooses, which is the safe direction.
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        None
    }
}

/// A `CHECK` constraint: a predicate every stored row must satisfy.
#[derive(Clone)]
pub struct CheckDef {
    name: String,
    predicate: Arc<dyn Predicate>,
    column: Option<String>,
    message: Option<String>,
}

impl core::fmt::Debug for CheckDef {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CheckDef")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Two checks are the same check when they have the same name.
///
/// A predicate is a Rust function, and functions do not compare. Names are
/// unique within a table, which is what makes this a usable identity rather
/// than a convenient fiction — but it does mean [`TableDef`](crate::TableDef)
/// equality ignores what a check actually tests.
impl PartialEq for CheckDef {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for CheckDef {}

impl CheckDef {
    /// Define a check called `name` over `predicate`.
    pub fn new<P: Predicate>(name: impl Into<String>, predicate: P) -> Self {
        Self {
            name: name.into(),
            predicate: Arc::new(predicate),
            column: None,
            message: None,
        }
    }

    /// Say which column this check is about.
    ///
    /// Optional, and optional is not laziness: `discount <= price` is about two
    /// columns and naming one of them would be a lie a form would render next
    /// to the wrong field. A check that *is* about one column should say so,
    /// and one that is not should stay silent rather than guess.
    ///
    /// Nothing here verifies the column exists — this crate does not hold the
    /// table. The daemon's schema loader does, and refuses a `column` that
    /// names nothing, because an error pointing at a field the form does not
    /// have is worse than an error pointing at no field at all.
    #[must_use]
    pub fn with_column(mut self, column: impl Into<String>) -> Self {
        self.column = Some(column.into());
        self
    }

    /// The sentence to show a person, in place of the default.
    ///
    /// A plain string, not a translation key. Both were defensible and the
    /// note that proposed this said so; a string wins here because the catalog
    /// is the only place the rule exists, and a key would put the rule in the
    /// catalog and its meaning somewhere this system cannot see. A deployment
    /// that needs two languages can put a key *in* the string — nothing stops
    /// it — and it keeps the default case readable.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// The check's name, unique within its table.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The column this check is about, when it is about one.
    #[must_use]
    pub fn column(&self) -> Option<&str> {
        self.column.as_deref()
    }

    /// The sentence to show a person, when one was written.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Whether `row` satisfies the constraint.
    ///
    /// **A check passes when its predicate is unknown.** That is SQL, and it is
    /// the opposite of a `WHERE`: `CHECK (age > 0)` accepts a row whose age is
    /// null, because the constraint has not been shown to be violated. Anything
    /// else would make every `CHECK` an implicit `NOT NULL`, which is a
    /// different constraint that the schema already has a way to say.
    #[must_use]
    pub fn satisfied_by(&self, row: &Row) -> bool {
        self.predicate.truth(row) != Some(false)
    }
}

/// What happens to referencing rows when the row they reference is deleted.
///
/// Exhaustive on purpose: the record store matches on this, so adding an action
/// should fail to compile there rather than fall through a wildcard into
/// silently doing nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReferentialAction {
    /// Refuse the delete while any row still references the one being deleted.
    Restrict,
    /// Delete the referencing rows too, in the same transaction.
    Cascade,
}

/// A foreign key: this table's `columns` reference another table's primary key.
///
/// The referenced columns are always the parent's **primary key**, in key
/// order, rather than any unique index. That is a restriction, and it is the
/// one that makes the check a point read instead of an index probe followed by
/// a row read — which matters when every lookup is a network round trip.
///
/// It has a second consequence worth having. A tenant-scoped parent's primary
/// key begins with its tenant column, so a child of one must carry the tenant
/// too and name it here. The reference is then confined to the caller's own
/// tenant by the key encoding rather than by a check somebody has to remember
/// to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyDef {
    name: String,
    columns: Vec<Ordinal>,
    parent: TableId,
    on_delete: ReferentialAction,
}

impl ForeignKeyDef {
    /// Start defining a foreign key into `parent`.
    #[must_use]
    pub fn builder(name: impl Into<String>, parent: TableId) -> ForeignKeyBuilder {
        ForeignKeyBuilder {
            name: name.into(),
            columns: Vec::new(),
            parent,
            on_delete: ReferentialAction::Restrict,
        }
    }

    /// The constraint's name, unique within its table.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The referencing columns, in the parent's primary key order.
    #[must_use]
    pub fn columns(&self) -> &[Ordinal] {
        &self.columns
    }

    /// The table referenced.
    #[must_use]
    pub const fn parent(&self) -> TableId {
        self.parent
    }

    /// What a delete of a referenced row does to the rows referencing it.
    #[must_use]
    pub const fn on_delete(&self) -> ReferentialAction {
        self.on_delete
    }

    /// The parent primary key `row` references, or `None` if it references
    /// nothing.
    ///
    /// A null in any referencing column means the row references nothing and
    /// the constraint is satisfied without a lookup — SQL's `MATCH SIMPLE`,
    /// which is what every dialect does by default. Treating a partial key as a
    /// reference would mean inventing a value for the null half.
    #[must_use]
    pub fn parent_key(&self, row: &Row) -> Option<Vec<slate_tuple::Value>> {
        let mut key = Vec::with_capacity(self.columns.len());
        for ordinal in &self.columns {
            let value = row.get(*ordinal)?;
            if value.is_null() {
                return None;
            }
            key.push(value.clone());
        }
        Some(key)
    }
}

/// Builder for [`ForeignKeyDef`]. Column names are resolved when the table is
/// built; the parent's shape is checked when the catalog is.
#[derive(Debug, Clone)]
pub struct ForeignKeyBuilder {
    pub(crate) name: String,
    pub(crate) columns: Vec<String>,
    pub(crate) parent: TableId,
    pub(crate) on_delete: ReferentialAction,
}

impl ForeignKeyBuilder {
    /// Append a referencing column.
    ///
    /// Columns are given in the parent's primary key order, so a reference to a
    /// tenant-scoped parent names the tenant column first.
    #[must_use]
    pub fn column(mut self, name: impl Into<String>) -> Self {
        self.columns.push(name.into());
        self
    }

    /// Set what a delete of the referenced row does. Defaults to
    /// [`ReferentialAction::Restrict`].
    #[must_use]
    pub const fn on_delete(mut self, action: ReferentialAction) -> Self {
        self.on_delete = action;
        self
    }

    /// Freeze the definition against already-resolved column ordinals.
    pub(crate) fn finish(&self, columns: Vec<Ordinal>) -> ForeignKeyDef {
        ForeignKeyDef {
            name: self.name.clone(),
            columns,
            parent: self.parent,
            on_delete: self.on_delete,
        }
    }
}
