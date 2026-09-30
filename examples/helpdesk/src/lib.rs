//! A small multi-tenant helpdesk, written against `slate-orm` the way an
//! application would be.
//!
//! # Why this exists
//!
//! Everything else in this repository exercises the record layer as a *test*
//! does: a fixture built for the property under test, torn down after it. The
//! demo goes through the head node and three SDKs; the deployed harness loads
//! a hundred thousand taxi trips and folds them. None of them is somebody
//! sitting down to write an application against the Rust surface and finding
//! out what that is like.
//!
//! This is that. It is an ordinary domain — tenants, agents, tickets,
//! comments — with the parts of an application that are usually where an ORM
//! is awkward: a status column that must be one of four words, a soft delete
//! that a reopen has to undo, a list page that must not shift under a write,
//! a search box, a per-assignee roll-up, and a tenant boundary that has to
//! hold whichever access path the planner picks.
//!
//! The service methods below are the application. They take a
//! [`SecurityContext`] — the caller — and return domain types. Nothing here
//! reaches past `slate_orm`'s public surface, which is the point: if
//! something is hard to say, that is a finding about the surface rather than
//! about this file.
//!
//! # What the storage is
//!
//! [`Helpdesk::open`] takes any [`RecordStore`], so the same service *should*
//! run over `MemoryStore` in the tests and over `SlateStore` — SlateDB on an
//! object store — in a binary. Only the first has been run. The generic
//! parameter is a claim about the surface, and until something drives this
//! against a bucket it stays one; the binary that would settle it is named as
//! unwritten in
//! `ledger/2026-09-30-an-application-written-against-the-rust-surface.md`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use slate_orm::{
    Action, Catalog, Expr, Grant, Policy, Principal, Record, RecordStore, Records, SecurityCatalog,
    SecurityContext, TableId, Value,
};
use uuid::Uuid;

/// What went wrong, in the application's own words.
///
/// An application does not want to hand a `slate_orm::OrmError` to whatever is
/// above it: half its variants are about a layer the caller has never heard
/// of. This is the translation, and writing it is the first thing that tells
/// you whether the errors underneath carry enough to translate *from*.
#[derive(Debug, thiserror::Error)]
pub enum HelpdeskError {
    /// The record layer refused: a policy, a duplicate key, a moved row.
    #[error(transparent)]
    Store(#[from] slate_orm::OrmError),
    /// The kernel refused, below the ORM.
    ///
    /// Two variants rather than one because the two layers report separately
    /// and an application has to carry both: `begin` and `commit` raise a
    /// `KernelError`, every `Records` method raises an `OrmError`, and there
    /// is no single error type covering a transaction from open to close.
    /// Writing this enum is how that is found out.
    #[error(transparent)]
    Kernel(#[from] slate_kernel::KernelError),
    /// No ticket with that reference, in this tenant, that this caller may see.
    #[error("no ticket {0} here")]
    NoSuchTicket(String),
    /// A status that is not one of the four the schema admits.
    #[error("{0:?} is not a status: open, pending, solved or closed")]
    NotAStatus(String),
}

/// The result an application method returns.
pub type Result<T> = std::result::Result<T, HelpdeskError>;

/// Table ids. Stated as constants because the security catalog names tables by
/// id and a literal in two places is a literal that drifts.
pub const TENANTS: TableId = TableId(1);
/// See [`TENANTS`].
pub const AGENTS: TableId = TableId(2);
/// See [`TENANTS`].
pub const TICKETS: TableId = TableId(3);
/// See [`TENANTS`].
pub const COMMENTS: TableId = TableId(4);

/// One customer of the helpdesk. Not tenant-scoped: it *is* the tenant.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "tenants", id = 1, version = 1)]
pub struct Tenant {
    /// The tenant's id, which every other table carries as its key prefix.
    #[record(pk)]
    pub id: Uuid,
    /// What to call them.
    #[record(index(name = "tenants_by_name", id = 10, unique))]
    pub name: String,
}

/// Somebody who answers tickets.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "agents", id = 2, version = 1, tenant = "tenant_id")]
pub struct Agent {
    /// The tenant, leading the key so scoping is a prefix and not a filter.
    #[record(pk)]
    pub tenant_id: Uuid,
    /// The agent.
    #[record(pk)]
    pub id: Uuid,
    /// Unique within the tenant, which is what the index's prefix gives.
    #[record(index(name = "agents_by_email", id = 20, unique))]
    pub email: String,
    /// Display name.
    pub name: String,
}

/// A request from a customer.
///
/// The interesting columns are the last four. `status` is a string a `CHECK`
/// narrows to four words, so a typo is refused by the database rather than by
/// whichever caller remembered to validate. `hours_logged` is money-shaped:
/// an integer count of hundredths, because an application that adds up
/// billable time in a float is an application that has to explain a missing
/// cent. `opened_at` and `updated_at` are managed — the server stamps them —
/// and `closed_at` is the soft-delete column, which is what makes a reopen
/// possible at all.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "tickets", id = 3, version = 1, tenant = "tenant_id")]
#[record(index(
    name = "tickets_by_priority",
    id = 32,
    columns("priority", desc("opened_at"))
))]
pub struct Ticket {
    /// The tenant.
    #[record(pk)]
    pub tenant_id: Uuid,
    /// The ticket.
    #[record(pk)]
    pub id: Uuid,
    /// What a customer quotes on the phone. Unique within the tenant.
    #[record(index(name = "tickets_by_reference", id = 30, unique))]
    pub reference: String,
    /// One line.
    pub subject: String,
    /// The whole request.
    ///
    /// **Not searchable from here.** The database has a text index — the TOML
    /// schema takes `text = true` and the kernel walks an inverted index — and
    /// `#[derive(Record)]` has no way to ask for one: its index options are
    /// `name`, `id`, `unique`, `desc`, `columns` and `only_where`. An
    /// application that wants full-text over a column has to declare that
    /// table with `slate_schema::TableBuilder` instead, which means writing
    /// the column list a second time and keeping the two in step. Written up
    /// in `ledger/2026-09-30-an-application-written-against-the-rust-surface.md`.
    pub body: String,
    /// `open`, `pending`, `solved` or `closed`.
    ///
    /// **The four words are this crate's, not the schema's.** A `CHECK` is
    /// what should narrow them, and `#[derive(Record)]` cannot declare one —
    /// its field options are `pk`, `rename`, `added_in`, `scale`,
    /// `created_at`, `updated_at`, `soft_delete` and `index`. So the
    /// constraint lives in [`Helpdesk::open_ticket`] and a caller reaching
    /// the store directly can write `"opne"`. Same finding as `body` above
    /// and the same workaround.
    pub status: String,
    /// 1 is most urgent. Indexed with `opened_at` so the inbox is a range
    /// walk rather than a sort.
    pub priority: i64,
    /// Whoever holds it, if anybody does.
    pub assignee_id: Option<Uuid>,
    /// Billable time, in hundredths of an hour.
    pub hours_logged: i64,
    /// Stamped on insert.
    #[record(created_at)]
    pub opened_at: i64,
    /// Stamped on every write.
    #[record(updated_at)]
    pub updated_at: i64,
    /// Set when the ticket is closed; `None` while it is live.
    #[record(soft_delete)]
    pub closed_at: Option<i64>,
}

/// A message on a ticket.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "comments", id = 4, version = 1, tenant = "tenant_id")]
#[record(index(
    name = "comments_by_ticket",
    id = 40,
    columns("ticket_id", "written_at")
))]
pub struct Comment {
    /// The tenant.
    #[record(pk)]
    pub tenant_id: Uuid,
    /// The comment.
    #[record(pk)]
    pub id: Uuid,
    /// Which ticket this is on.
    pub ticket_id: Uuid,
    /// Who wrote it.
    pub author_id: Uuid,
    /// What they said.
    pub body: String,
    /// Stamped on insert.
    #[record(created_at)]
    pub written_at: i64,
}

/// The four statuses, so the application and the `CHECK` cannot drift apart.
pub const STATUSES: [&str; 4] = ["open", "pending", "solved", "closed"];

/// The catalog this application needs.
pub fn catalog() -> std::result::Result<Catalog, slate_schema::SchemaError> {
    Catalog::from_tables([
        Tenant::table().clone(),
        Agent::table().clone(),
        Ticket::table().clone(),
        Comment::table().clone(),
    ])
}

/// Who may do what.
///
/// Two roles, which is the smallest number that makes the policy interesting:
/// an `agent` may work tickets in their own tenant, a `viewer` may read them
/// and nothing else. The tenant boundary is *not* here — it is the key prefix,
/// enforced below this layer — and that separation is the thing worth seeing:
/// forgetting a policy loses a rule, forgetting the tenant loses the wall.
pub fn security() -> SecurityCatalog {
    let read = [Action::Read];
    SecurityCatalog::new()
        .grant(Grant::new("agent", TICKETS, Action::ALL))
        .grant(Grant::new("agent", COMMENTS, Action::ALL))
        .grant(Grant::new("agent", AGENTS, read))
        .grant(Grant::new("agent", TENANTS, read))
        .grant(Grant::new("viewer", TICKETS, read))
        .grant(Grant::new("viewer", COMMENTS, read))
        .grant(Grant::new("viewer", AGENTS, read))
        // An agent works their own tenant's tickets. The tenant *prefix*
        // already makes another tenant's rows unreachable; this is the rule
        // inside one — and it is written as `True` because there is no
        // narrower rule to write yet. Recorded as a place a real deployment
        // would put "only tickets on your team".
        .policy(Policy::new(
            "agents_work_their_tenant",
            TICKETS,
            Action::ALL,
            |_: &_| Expr::True,
        ))
        .policy(Policy::new(
            "agents_read_comments",
            COMMENTS,
            Action::ALL,
            |_: &_| Expr::True,
        ))
}

/// A caller.
pub fn caller(agent: Uuid, tenant: Uuid, role: &str) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::Uuid(agent))
            .with_tenant(Value::Uuid(tenant))
            .with_role(role),
    )
}

/// One page of an inbox, and where to resume.
#[derive(Debug, Clone)]
pub struct Inbox {
    /// The tickets, most urgent first and newest first within a priority.
    pub tickets: Vec<Ticket>,
    /// Pass this back to get the next page, or `None` at the end.
    pub next: Option<Vec<Value>>,
}

/// How much work one agent is holding.
#[derive(Debug, Clone, PartialEq)]
pub struct Workload {
    /// The agent, or `None` for the unassigned pile.
    pub assignee_id: Option<Uuid>,
    /// How many live tickets.
    pub tickets: i64,
    /// Billable hundredths of an hour across them.
    pub hundredths: i64,
}

/// The application.
#[derive(Debug)]
pub struct Helpdesk<S> {
    store: RecordStore<S>,
}

impl<S: slate_kernel::KvStore> Helpdesk<S> {
    /// Open the helpdesk over a store.
    ///
    /// # Errors
    /// If the four tables do not make a valid catalog, which is a bug here.
    pub fn open(store: S) -> std::result::Result<Self, slate_schema::SchemaError> {
        Ok(Self {
            store: RecordStore::new(store, catalog()?, security()),
        })
    }

    /// The store underneath, for the parts of an application that need one.
    #[must_use]
    pub const fn store(&self) -> &RecordStore<S> {
        &self.store
    }

    /// Register a tenant and its first agent.
    ///
    /// # Errors
    /// If either write is refused.
    pub async fn found(
        &self,
        root: &SecurityContext,
        tenant: &Tenant,
        agent: &Agent,
    ) -> Result<()> {
        let txn = self.store.begin().await?;
        txn.insert_record(root, tenant).await?;
        txn.insert_record(root, agent).await?;
        txn.commit().await?;
        Ok(())
    }

    /// Raise a ticket.
    ///
    /// The status check is *here*, and it should not be: see the note on
    /// [`Ticket::status`]. Anything reaching the store without going through
    /// this method can write a status the application does not know.
    ///
    /// # Errors
    /// [`HelpdeskError::NotAStatus`] for a status outside [`STATUSES`], and
    /// whatever the insert raises — a duplicate `reference` among them.
    pub async fn open_ticket(&self, who: &SecurityContext, ticket: Ticket) -> Result<Ticket> {
        if !STATUSES.contains(&ticket.status.as_str()) {
            return Err(HelpdeskError::NotAStatus(ticket.status));
        }
        let txn = self.store.begin().await?;
        txn.insert_record(who, &ticket).await?;
        txn.commit().await?;
        Ok(ticket)
    }

    /// The ticket a customer is quoting, if this caller may see it.
    ///
    /// # Errors
    /// [`HelpdeskError::NoSuchTicket`] when the reference is unknown *or* the
    /// caller may not see it — deliberately the same error, because telling
    /// them apart tells a stranger which references exist.
    pub async fn by_reference(&self, who: &SecurityContext, reference: &str) -> Result<Ticket> {
        let txn = self.store.begin().await?;
        let found: Vec<Ticket> = txn
            .find_records(
                who,
                Expr::eq(Ticket::COLUMNS.reference, Value::Str(reference.to_owned())),
                slate_kernel::ScanOrder::Ascending,
            )
            .await?;
        found
            .into_iter()
            .next()
            .ok_or_else(|| HelpdeskError::NoSuchTicket(reference.to_owned()))
    }

    /// Hand a ticket to an agent, refusing if somebody else moved it first.
    ///
    /// `replace_record` is the whole reason this is three lines rather than
    /// two: read, edit, write-if-unchanged. A plain update here would
    /// silently overwrite a colleague's reassignment with a decision taken
    /// before it happened.
    ///
    /// # Errors
    /// [`HelpdeskError::NoSuchTicket`], or `RowChanged` if the ticket moved
    /// between the read and the write.
    pub async fn assign(&self, who: &SecurityContext, reference: &str, to: Uuid) -> Result<Ticket> {
        let before = self.by_reference(who, reference).await?;
        let mut after = before.clone();
        after.assignee_id = Some(to);
        after.status = "pending".to_owned();
        let txn = self.store.begin().await?;
        txn.replace_record(who, &before, &after).await?;
        txn.commit().await?;
        Ok(after)
    }

    /// Log billable time against a ticket, in hundredths of an hour.
    ///
    /// # Errors
    /// As [`Helpdesk::assign`].
    pub async fn log_time(
        &self,
        who: &SecurityContext,
        reference: &str,
        hundredths: i64,
    ) -> Result<Ticket> {
        let before = self.by_reference(who, reference).await?;
        let mut after = before.clone();
        after.hours_logged += hundredths;
        let txn = self.store.begin().await?;
        txn.replace_record(who, &before, &after).await?;
        txn.commit().await?;
        Ok(after)
    }

    /// Add a message to a ticket.
    ///
    /// # Errors
    /// [`HelpdeskError::NoSuchTicket`] if the ticket is not the caller's to
    /// see, or whatever the insert raises.
    pub async fn comment(
        &self,
        who: &SecurityContext,
        reference: &str,
        author: Uuid,
        body: &str,
    ) -> Result<Comment> {
        let ticket = self.by_reference(who, reference).await?;
        let comment = Comment {
            tenant_id: ticket.tenant_id,
            id: Uuid::new_v4(),
            ticket_id: ticket.id,
            author_id: author,
            body: body.to_owned(),
            written_at: 0,
        };
        let txn = self.store.begin().await?;
        txn.insert_record(who, &comment).await?;
        txn.commit().await?;
        Ok(comment)
    }

    /// Everything said on a ticket, oldest first.
    ///
    /// # Errors
    /// As [`Helpdesk::by_reference`].
    pub async fn thread(&self, who: &SecurityContext, reference: &str) -> Result<Vec<Comment>> {
        let ticket = self.by_reference(who, reference).await?;
        let txn = self.store.begin().await?;
        Ok(txn
            .find_records(
                who,
                Expr::eq(Comment::COLUMNS.ticket_id, Value::Uuid(ticket.id)),
                slate_kernel::ScanOrder::Ascending,
            )
            .await?)
    }

    /// Close a ticket: the row is stamped rather than erased.
    ///
    /// **And that is as far as this goes.** `#[record(soft_delete)]`
    /// keeps the row, and the Rust surface offers no way to read it back
    /// or to restore it — the kernel's `Deleted::Visible` is private and
    /// the wire's `include_deleted` has no equivalent here. So this
    /// application can close a ticket and cannot reopen it, which
    /// `a_closed_ticket_is_hidden_and_cannot_be_reopened_from_here`
    /// asserts so that growing that read turns the test red.
    ///
    /// # Errors
    /// As [`Helpdesk::by_reference`].
    pub async fn close(&self, who: &SecurityContext, reference: &str) -> Result<()> {
        let ticket = self.by_reference(who, reference).await?;
        let txn = self.store.begin().await?;
        txn.delete_record::<Ticket>(
            who,
            &[Value::Uuid(ticket.tenant_id), Value::Uuid(ticket.id)],
        )
        .await?;
        txn.commit().await?;
        Ok(())
    }
}
