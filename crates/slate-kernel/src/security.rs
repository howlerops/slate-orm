//! Row-level security and role-based access control.
//!
//! This is the part a record layer has to get structurally right rather than
//! merely implement. Policies are not checked by a wrapper that a caller could
//! step around: they are compiled into the predicate the executor evaluates and
//! into the bounds it scans, inside the kernel. Every read and write on
//! [`RecordTransaction`](crate::RecordTransaction) takes a [`SecurityContext`],
//! so there is no unauthenticated door to find — bypassing is possible, but only
//! by naming [`SecurityContext::superuser`], which is one grep away.
//!
//! Three things happen before any row is touched:
//!
//! 1. **RBAC**, from the catalog: does one of the principal's roles grant this
//!    action on this table? This is a cheap check against metadata and runs
//!    before a plan is built.
//! 2. **Tenant restriction**, if the table is tenant-scoped: the principal's
//!    tenant is forced into the predicate. Because the schema layer guarantees
//!    the tenant column leads the key, the planner turns that into a key prefix,
//!    so the scan physically cannot reach another tenant's bytes. A context with
//!    no tenant is refused outright rather than shown everything.
//! 3. **RLS**, from the policies: the matching policies for this action are
//!    combined and conjoined onto the predicate.
//!
//! Writes are checked from both directions, as in Postgres: an update or delete
//! may only touch a row the principal can already see (`USING`), and may only
//! leave behind a row the principal is allowed to have written (`WITH CHECK`).
//!
//! # What a write discloses, exactly
//!
//! An `update` or a `delete` aimed at a row the policy hides reports the row as
//! **missing**, not as forbidden, so those two errors cannot be used to probe
//! for existence.
//!
//! An `insert` or an `upsert` cannot make that promise, and does not. Both
//! succeed on a free primary key and fail on one held by a row the caller
//! cannot see, and the difference between those two outcomes is exactly the bit
//! "is this key taken". That is inherent rather than an oversight: a unique key
//! is a resource shared by everyone who can write the table, and the only ways
//! to withhold the bit are to overwrite the hidden row or to accept a write
//! that cannot be stored. Postgres RLS has the same oracle for the same reason.
//!
//! This claim was previously written as covering every write, and it did not:
//! `security_probe_cascade.rs` demonstrates both the insert and the upsert
//! version. The upsert case is worth naming rather than folding into "writes",
//! because a caller reaching for an upsert *in order to avoid* the insert
//! oracle would be choosing it for a property it does not have.
//!
//! Two things bound the exposure, and neither is a fix:
//!
//! - It is same-tenant only. A tenant-scoped table puts the tenant in the key
//!   prefix, so a key in another tenant is a different key and there is no
//!   collision to observe. (Finding 2 in `docs/security-review.md` was a
//!   separate path where the same bit *did* cross tenants; that one is fixed.)
//! - It requires the attacker to be able to name the key. A primary key that is
//!   a UUID, or drawn from a sequence the attacker cannot read, leaves nothing
//!   to probe for — the oracle answers a question the attacker cannot ask.
//!   Where a table's key is attacker-chosen and its existence is a secret, that
//!   is the case to design around.

use crate::error::KernelError;
use crate::expr::Expr;
use slate_schema::{Row, TableDef, TableId};
use slate_tuple::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// An operation subject to authorisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    /// Reading rows.
    Read,
    /// Adding rows.
    Insert,
    /// Modifying existing rows.
    Update,
    /// Removing rows.
    Delete,
    /// Asking for a plan without running it.
    ///
    /// Separate from [`Action::Read`] because it is not a weaker version of
    /// one. A plan is costed against statistics gathered over the *whole*
    /// table, by design — per-policy histograms would make the planner
    /// optimise for a table nobody is querying — so a plan's estimated
    /// cardinality describes rows the caller may not read, and a caller who
    /// can vary a literal and watch the estimate move can recover the
    /// histogram bounds themselves, which are sampled values.
    ///
    /// A caller who can already read every row of a table learns nothing from
    /// this. The grant exists for the case that is not true: a table-level
    /// grant plus a row policy, which is the ordinary tenant arrangement.
    Explain,
}

impl Action {
    /// Every action on *rows*, for granting blanket data access.
    ///
    /// Deliberately excludes [`Action::Explain`], which is not a data action:
    /// it reads statistics describing rows the grantee's row policy may hide.
    /// A deployment that wants its readers to keep `EXPLAIN` grants it
    /// alongside this, which is the reopening knob and is per-role and
    /// per-table rather than global. Including it here would mean every
    /// existing blanket grant silently kept the disclosure, and a blanket
    /// grant plus a row policy is precisely the arrangement that has it.
    pub const ALL: [Self; 4] = [Self::Read, Self::Insert, Self::Update, Self::Delete];

    /// Every action, data and `EXPLAIN` alike.
    pub const EVERYTHING: [Self; 5] = [
        Self::Read,
        Self::Insert,
        Self::Update,
        Self::Delete,
        Self::Explain,
    ];

    /// A human-readable name, used in error messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Explain => "explain",
        }
    }
}

/// Who is asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// The principal's identity, available to policies.
    pub id: Value,
    /// The tenant the principal acts within, if any.
    pub tenant: Option<Value>,
    /// The roles the principal holds.
    pub roles: BTreeSet<String>,
}

impl Principal {
    /// A principal with an id and no roles.
    #[must_use]
    pub fn new(id: Value) -> Self {
        Self {
            id,
            tenant: None,
            roles: BTreeSet::new(),
        }
    }

    /// Set the principal's tenant.
    #[must_use]
    pub fn with_tenant(mut self, tenant: Value) -> Self {
        self.tenant = Some(tenant);
        self
    }

    /// Add a role.
    #[must_use]
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.roles.insert(role.into());
        self
    }
}

/// The identity and privilege a request runs under.
#[derive(Debug, Clone)]
pub struct SecurityContext {
    principal: Principal,
    bypass: bool,
}

impl SecurityContext {
    /// A context that is subject to RBAC and RLS.
    #[must_use]
    pub const fn new(principal: Principal) -> Self {
        Self {
            principal,
            bypass: false,
        }
    }

    /// A context that skips every check.
    ///
    /// Intended for migrations, backfills and administrative tooling. It is a
    /// named constructor rather than a flag so that every privileged call site
    /// is findable with a single search.
    #[must_use]
    pub fn superuser() -> Self {
        Self {
            principal: Principal::new(Value::Null),
            bypass: true,
        }
    }

    /// Who is asking.
    #[must_use]
    pub const fn principal(&self) -> &Principal {
        &self.principal
    }

    /// Whether this context skips authorisation entirely.
    #[must_use]
    pub const fn is_superuser(&self) -> bool {
        self.bypass
    }
}

/// Builds a policy's predicate for a specific caller.
///
/// A policy needs to talk about the principal (`owner_id = current_user`), so
/// it is a function of the context rather than a static expression. Keeping it
/// a Rust function means no template language and no parameter-substitution bug
/// class, at the cost of policies being code rather than data — which matches
/// the rest of the design, where schemas are code too.
pub trait PolicyPredicate: Send + Sync + 'static {
    /// The predicate this policy imposes on `context`.
    fn build(&self, context: &SecurityContext) -> Expr;
}

impl<F> PolicyPredicate for F
where
    F: Fn(&SecurityContext) -> Expr + Send + Sync + 'static,
{
    fn build(&self, context: &SecurityContext) -> Expr {
        self(context)
    }
}

/// A row-level security policy.
///
/// Policies are permissive and combine with `OR`, as in Postgres: a row is
/// visible if any policy that applies to the caller admits it.
#[derive(Clone)]
pub struct Policy {
    name: String,
    table: TableId,
    actions: BTreeSet<Action>,
    roles: BTreeSet<String>,
    predicate: Arc<dyn PolicyPredicate>,
}

impl core::fmt::Debug for Policy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Policy")
            .field("name", &self.name)
            .field("table", &self.table)
            .field("actions", &self.actions)
            .field("roles", &self.roles)
            .finish_non_exhaustive()
    }
}

impl Policy {
    /// Define a policy over `table` for `actions`.
    ///
    /// With no roles added it applies to every caller.
    pub fn new<P: PolicyPredicate>(
        name: impl Into<String>,
        table: TableId,
        actions: impl IntoIterator<Item = Action>,
        predicate: P,
    ) -> Self {
        Self {
            name: name.into(),
            table,
            actions: actions.into_iter().collect(),
            roles: BTreeSet::new(),
            predicate: Arc::new(predicate),
        }
    }

    /// Restrict the policy to callers holding `role`.
    #[must_use]
    pub fn for_role(mut self, role: impl Into<String>) -> Self {
        self.roles.insert(role.into());
        self
    }

    /// The policy's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    fn applies_to(&self, context: &SecurityContext, table: TableId, action: Action) -> bool {
        self.table == table
            && self.actions.contains(&action)
            && (self.roles.is_empty()
                || self
                    .roles
                    .iter()
                    .any(|r| context.principal.roles.contains(r)))
    }
}

/// A role's permission to perform actions on a table.
#[derive(Debug, Clone)]
pub struct Grant {
    role: String,
    table: TableId,
    actions: BTreeSet<Action>,
}

impl Grant {
    /// Grant `role` the given actions on `table`.
    pub fn new(
        role: impl Into<String>,
        table: TableId,
        actions: impl IntoIterator<Item = Action>,
    ) -> Self {
        Self {
            role: role.into(),
            table,
            actions: actions.into_iter().collect(),
        }
    }
}

/// The access rules a store enforces.
///
/// Empty by default, which denies everything to every non-superuser: a table
/// with no grant is unreachable rather than public.
#[derive(Debug, Clone, Default)]
pub struct SecurityCatalog {
    grants: Vec<Grant>,
    policies: Vec<Policy>,
    rls_enabled: BTreeMap<TableId, bool>,
}

impl SecurityCatalog {
    /// An empty catalog. Denies every non-superuser action.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a role grant.
    #[must_use]
    pub fn grant(mut self, grant: Grant) -> Self {
        self.grants.push(grant);
        self
    }

    /// Add a row-level policy. Adding one enables RLS on its table.
    #[must_use]
    pub fn policy(mut self, policy: Policy) -> Self {
        self.rls_enabled.insert(policy.table, true);
        self.policies.push(policy);
        self
    }

    /// Turn on row-level security for a table without adding a policy.
    ///
    /// The table then denies every row to non-superusers until a policy admits
    /// some, which is the safe direction for the default to point.
    #[must_use]
    pub fn enable_rls(mut self, table: TableId) -> Self {
        self.rls_enabled.entry(table).or_insert(true);
        self
    }

    /// Check that `context` may perform `action` on `table`.
    ///
    /// Runs before a plan is built, so an unauthorised request never reaches
    /// the storage layer at all.
    pub fn authorize(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        action: Action,
    ) -> crate::Result<()> {
        if context.is_superuser() {
            return Ok(());
        }
        let permitted = self.grants.iter().any(|g| {
            g.table == table.id()
                && g.actions.contains(&action)
                && context.principal.roles.contains(&g.role)
        });
        if permitted {
            Ok(())
        } else {
            Err(KernelError::AccessDenied {
                table: table.name().to_owned(),
                action: action.name(),
            })
        }
    }

    /// The mandatory predicate for `context` on `table` and `action`.
    ///
    /// This is conjoined onto whatever the caller asked for, and is what makes
    /// the policy unavoidable rather than advisory. It combines the tenant
    /// restriction with the OR of every applicable policy.
    pub fn row_filter(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        action: Action,
    ) -> crate::Result<Expr> {
        if context.is_superuser() {
            return Ok(Expr::True);
        }

        let mut filter = self.tenant_filter(context, table)?;

        if self.rls_enabled.get(&table.id()).copied().unwrap_or(false) {
            let applicable: Vec<&Policy> = self
                .policies
                .iter()
                .filter(|p| p.applies_to(context, table.id(), action))
                .collect();

            // RLS on with nothing admitting anything means no rows, not all
            // rows. Failing closed is the whole point.
            let policy_filter = if applicable.is_empty() {
                Expr::False
            } else {
                Expr::Or(
                    applicable
                        .iter()
                        .map(|p| p.predicate.build(context))
                        .collect(),
                )
            };
            filter = filter.and(policy_filter);
        }

        Ok(filter)
    }

    /// Force the principal's tenant onto a tenant-scoped table.
    ///
    /// Refusing a context with no tenant is deliberate: the alternative — no
    /// restriction — would silently make every tenant visible.
    fn tenant_filter(&self, context: &SecurityContext, table: &TableDef) -> crate::Result<Expr> {
        let Some(tenant_column) = table.tenant_column() else {
            return Ok(Expr::True);
        };
        let Some(tenant) = context.principal.tenant.clone() else {
            return Err(KernelError::TenantRequired {
                table: table.name().to_owned(),
            });
        };
        Ok(Expr::eq(tenant_column, tenant))
    }

    /// Whether `row` satisfies the policy for `action` — Postgres's
    /// `WITH CHECK` for writes, and `USING` for the row a write targets.
    pub fn permits_row(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        action: Action,
        row: &Row,
    ) -> crate::Result<bool> {
        Ok(self.row_filter(context, table, action)?.admits(row))
    }
}
