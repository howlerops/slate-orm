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
    Action, Aggregate, Catalog, CheckDef, Direction, Explanation, Expr, Grant, Grouped, IndexDef,
    IndexId, Managed, MigrationReport, Ordinal, Policy, Principal, Query, Record, RecordError,
    RecordStore, Records, Row, ScanOrder, SecurityCatalog, SecurityContext, SortKey, TableDef,
    TableId, Value, ValueType, migrate,
};
use std::sync::LazyLock;
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
    /// The catalog itself is invalid, which is a bug in this crate.
    #[error(transparent)]
    Schema(#[from] slate_schema::SchemaError),
    /// No ticket with that reference, in this tenant, that this caller may see.
    #[error("no ticket {0} here")]
    NoSuchTicket(String),
    /// A status that is not one of the four the schema admits.
    #[error("{0:?} is not a status: open, pending, solved or closed")]
    NotAStatus(String),
    /// A grouped read came back in a shape the roll-up cannot read.
    ///
    /// Cannot happen while the aggregate list and `read_workload` agree, and
    /// it is an error rather than a `panic!` or a zero because a grouped
    /// read's results are positional: the only thing tying position 1 to
    /// `hours_logged` is that the same list was passed a few lines earlier,
    /// and a silent zero in a billing roll-up is worse than a refusal.
    #[error("a grouped read came back in an unexpected shape: {0}")]
    UnexpectedGroup(String),
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
    ///
    /// **This is the last of the three gaps that entry found.** The other two
    /// — a `CHECK` and a foreign key — are attributes now, and `status` below
    /// and [`Comment`] use them. `text` is the one left, and it is the reason
    /// [`TICKETS_TABLE`] still exists.
    pub body: String,
    /// `open`, `pending`, `solved` or `closed`, and the **database** says so.
    ///
    /// This doc comment used to say the four words were this crate's rather
    /// than the schema's, because `#[derive(Record)]` could declare no
    /// `CHECK` and the constraint lived in [`Helpdesk::open_ticket`] — so a
    /// caller reaching the store directly could write `"opne"`. The
    /// attribute exists now
    /// (`ledger/2026-10-01-a-check-the-derive-could-not-declare.md`) and the
    /// rule is where it belongs.
    ///
    /// The `message` is the sentence a form shows and the `column` defaults
    /// to this field, which is what a form puts it beside. The application
    /// check in `open_ticket` stays: it refuses before the write with a typed
    /// error naming the four, which is a better experience than a round trip,
    /// and `the_statuses_and_the_check_agree` holds the two to each other so
    /// the list and the pattern cannot drift.
    #[record(check(
        name = "ticket_status",
        predicate(Expr::matches(status, STATUS_PATTERN)),
        message = "A ticket is open, pending, solved or closed."
    ))]
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
///
/// Both references are declared, and both are composite: `tickets` and
/// `agents` are tenant-scoped, so their primary key begins with `tenant_id`
/// and a key into either names that column **first**. That is what confines
/// the reference to the caller's own tenant by the key encoding rather than
/// by a check somebody has to remember to write.
///
/// `on_delete = cascade` on the ticket edge, because a comment on a ticket
/// nobody can see is a row nobody can reach. The agent edge is the default,
/// `restrict`: deleting an agent who has written comments should fail loudly
/// rather than erase what they said.
#[record(foreign_key(
    name = "comments_ticket",
    parent = Ticket,
    column = "tenant_id",
    column = "ticket_id",
    on_delete = cascade,
))]
#[record(foreign_key(
    name = "comments_author",
    parent = Agent,
    column = "tenant_id",
    column = "author_id",
))]
pub struct Comment {
    /// The tenant.
    #[record(pk)]
    pub tenant_id: Uuid,
    /// The comment.
    #[record(pk)]
    pub id: Uuid,
    /// Which ticket this is on. Half of `comments_ticket`.
    pub ticket_id: Uuid,
    /// Who wrote it. Half of `comments_author`.
    pub author_id: Uuid,
    /// What they said.
    pub body: String,
    /// Stamped on insert.
    #[record(created_at)]
    pub written_at: i64,
}

/// The four statuses, so the application and the `CHECK` cannot drift apart.
pub const STATUSES: [&str; 4] = ["open", "pending", "solved", "closed"];

/// The same four, as the `CHECK` on [`Ticket::status`] spells them.
///
/// Two spellings of one list, which is the thing this repository usually
/// refuses. It is kept because they are read by different things — a Rust
/// `contains` and a regular expression the kernel compiles — and deriving
/// either from the other means either building a pattern at runtime, so the
/// attribute cannot take it, or parsing one, so the error is at the wrong
/// layer. `the_statuses_and_the_check_agree` is what makes the duplication a
/// cost rather than a hazard: it runs every word of `STATUSES` through the
/// check and one that is not in it.
pub const STATUS_PATTERN: &str = "^(open|pending|solved|closed)$";

/// The tickets table as this application actually declares it: everything
/// `#[derive(Record)]` produced, plus two **text** indexes.
///
/// # Why this exists, and why it is the whole cost of the finding
///
/// The search box needs an inverted index, and `#[record(index(...))]` takes
/// `name`, `id`, `unique`, `desc`, `columns` and `only_where` — no `text`.
/// The kernel has one, `IndexBuilder::text()` declares it and the TOML schema
/// takes `text = true`; the derive is the one door it is not behind.
///
/// The workaround is *not* "call `IndexBuilder::text()` and hand it to the
/// derive's table", because a `TableDef` is immutable once built and
/// [`Record::table`] is what every `Records` method passes to the store. An
/// index the store never sees is an index no write maintains, so a search
/// over it returns nothing — the silent-empty-index failure
/// `ledger/2026-09-15-a-new-index-returns-nothing.md` is about. The table the
/// writes use has to be this one.
///
/// So the whole table is written out again. What the derive still does is the
/// part worth keeping: `to_row`, `from_row` and the `COLUMNS` ordinals, which
/// [`Indexed`] borrows by wrapping `Ticket`. Only the *schema* is duplicated,
/// and `the_hand_written_tickets_table_matches_the_derived_one` fails if the
/// two drift — which is the guard a duplicated declaration needs and the
/// reason this is a cost rather than a hazard.
///
/// # Two indexes, not one
///
/// A text index is one entry per term of **one** string column
/// (`IndexDef::is_text`), so covering the subject and the body is two indexes
/// and an `Expr::Or`. That is not a limitation of the derive; it is what an
/// inverted index over a row is.
// `expect` in a `LazyLock`, which the workspace lints deny. There is nothing
// to return: `Record::table()` gives a `&'static TableDef` and every caller
// is a `Records` method that has already decided which table it is using.
// The alternative — `LazyLock<Result<TableDef, _>>` and an unwrap at each of
// the dozen call sites — moves the panic rather than removing it, and hides
// it in twelve places instead of one. A table that does not build is a bug
// in this file and nothing else, and it is caught by
// `the_hand_written_tickets_table_matches_the_derived_one`, which builds it.
#[allow(clippy::expect_used)]
static TICKETS_TABLE: LazyLock<TableDef> = LazyLock::new(|| {
    TableDef::builder("tickets", TICKETS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::Uuid)
        .column("reference", ValueType::Str)
        .column("subject", ValueType::Str)
        .column("body", ValueType::Str)
        .column("status", ValueType::Str)
        .column("priority", ValueType::I64)
        .nullable_column("assignee_id", ValueType::Uuid)
        .column("hours_logged", ValueType::I64)
        .column("opened_at", ValueType::I64)
        .column("updated_at", ValueType::I64)
        .nullable_column("closed_at", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .soft_delete("closed_at")
        .managed_for("opened_at", Managed::CreatedAt)
        .managed_for("updated_at", Managed::UpdatedAt)
        // The same check the derive declares on `Ticket::status`. Written out
        // here for the same reason every other line in this builder is: the
        // table the writes use is this one, so a constraint only the derived
        // table carries is a constraint no write enforces — the silent-empty
        // failure one field over, with a rule instead of an index.
        // `the_hand_written_tickets_table_matches_the_derived_one` is what
        // notices if the two drift.
        .check(
            CheckDef::new("ticket_status", Expr::matches(Ordinal(5), STATUS_PATTERN))
                .with_column("status")
                .with_message("A ticket is open, pending, solved or closed."),
        )
        .schema_version(1)
        .index(
            IndexDef::builder("tickets_by_reference", IndexId(30))
                .column("reference")
                .unique(),
        )
        .index(
            IndexDef::builder("tickets_by_priority", IndexId(32))
                .column("priority")
                .column_with("opened_at", Direction::Desc),
        )
        .index(
            IndexDef::builder("tickets_by_subject_term", IndexId(33))
                .column("subject")
                .text(),
        )
        .index(
            IndexDef::builder("tickets_by_body_term", IndexId(34))
                .column("body")
                .text(),
        )
        .build()
        .expect("the hand-written tickets table is valid")
});

/// A [`Ticket`] stored against [`TICKETS_TABLE`] rather than the derive's.
///
/// Every service method below reads and writes this rather than `Ticket`, so
/// the text indexes are the ones the store maintains. The row codec is still
/// the derive's — this forwards to it — which is why the duplication above is
/// the schema and nothing else.
#[derive(Debug, Clone, PartialEq)]
pub struct Indexed(pub Ticket);

impl Record for Indexed {
    fn table() -> &'static TableDef {
        &TICKETS_TABLE
    }

    fn to_row(&self) -> Row {
        self.0.to_row()
    }

    fn from_row(row: &Row) -> std::result::Result<Self, RecordError> {
        Ticket::from_row(row).map(Self)
    }
}

/// The catalog this application needs.
///
/// # Errors
/// If the four tables do not make a valid catalog, which is a bug here.
pub fn catalog() -> std::result::Result<Catalog, slate_schema::SchemaError> {
    Catalog::from_tables([
        Tenant::table().clone(),
        Agent::table().clone(),
        // `Indexed::table()`, not `Ticket::table()`: the catalog must agree
        // with what the writes use, or a migration creates one set of indexes
        // and the write path maintains another.
        Indexed::table().clone(),
        Comment::table().clone(),
    ])
}

/// Bring a store's schema up to what [`catalog`] declares.
///
/// An application does this at boot, before it serves anything. Separate from
/// [`Helpdesk::open`] because migrating is a decision — a process that is one
/// of six replicas should not quietly create indexes on startup — and because
/// [`slate_orm::migrate::plan`] exists for showing the change first.
///
/// # Errors
/// If the migration is refused (see [`slate_orm::Refusal`]) or a write fails.
pub async fn apply_schema<S: slate_kernel::KvStore>(
    store: &S,
) -> std::result::Result<MigrationReport, HelpdeskError> {
    Ok(migrate::migrate(store, &catalog()?).await?)
}

/// Who may do what.
///
/// Three roles, because two were not enough to make the policies say
/// anything: an `agent` works tickets, a `viewer` reads them, and a
/// `supervisor` moves work between agents.
///
/// # Where the tenant boundary is *not*
///
/// It is not here. It is the key prefix — `#[record(tenant = "tenant_id")]`
/// — and it is enforced below this layer. That separation is the thing worth
/// seeing: forgetting a policy loses a rule, forgetting the tenant loses the
/// wall, and only one of those two mistakes is visible in this function.
///
/// # Why the write rule is two policies and not one `Or`
///
/// "an agent may write a ticket that is unassigned **or** assigned to them"
/// is one sentence and two [`Policy`] values. Policies are permissive and
/// combine with `OR`, as in PostgreSQL, so the pair *is* the disjunction —
/// and `Expr::Or` would have worked too. The pair is preferred because each
/// half is separately named, so a refusal can say which rule was expected to
/// admit the row, and because a fourth rule is an addition rather than an
/// edit to an expression somebody has to re-read.
///
/// The predicate is checked against the row **as it will be** as well as the
/// row as it was, which is what makes `agents_work_their_own_tickets` a real
/// rule rather than a decoration: an agent can take an unassigned ticket and
/// cannot hand one to a colleague, because the resulting row is not theirs.
/// That is what a `supervisor` is for, and
/// `an_agent_cannot_push_work_onto_a_colleague` is the demonstration.
pub fn security() -> SecurityCatalog {
    let read = [Action::Read];
    let write = [Action::Insert, Action::Update, Action::Delete];
    SecurityCatalog::new()
        .grant(Grant::new("agent", TICKETS, Action::ALL))
        .grant(Grant::new("agent", COMMENTS, Action::ALL))
        .grant(Grant::new("agent", AGENTS, read))
        .grant(Grant::new("agent", TENANTS, read))
        .grant(Grant::new("supervisor", TICKETS, Action::ALL))
        // `Action::ALL` is four actions and deliberately not five: `Explain`
        // is excluded, because a plan is costed against whole-table
        // statistics and so describes rows the caller's row policy hides. An
        // application therefore has to *decide* who may see one, and this is
        // the decision: a supervisor sizing the queue may, an agent working
        // it may not. `an_agent_may_not_ask_for_a_plan` is the other half.
        .grant(Grant::new("supervisor", TICKETS, [Action::Explain]))
        .grant(Grant::new("supervisor", COMMENTS, Action::ALL))
        .grant(Grant::new("supervisor", AGENTS, read))
        .grant(Grant::new("supervisor", TENANTS, read))
        .grant(Grant::new("viewer", TICKETS, read))
        .grant(Grant::new("viewer", COMMENTS, read))
        .grant(Grant::new("viewer", AGENTS, read))
        // Reads are open within the tenant: an agent triaging a queue has to
        // see tickets nobody has picked up, and a rule narrower than the
        // tenant would make the inbox lie about what is waiting.
        .policy(
            Policy::new("agents_read_the_queue", TICKETS, read, |_: &_| Expr::True)
                .for_role("agent"),
        )
        .policy(
            Policy::new("viewers_read_the_queue", TICKETS, read, |_: &_| Expr::True)
                .for_role("viewer"),
        )
        .policy(
            Policy::new("supervisors_read_the_queue", TICKETS, read, |_: &_| {
                Expr::True
            })
            .for_role("supervisor"),
        )
        // And the two halves of the write rule.
        .policy(
            Policy::new("agents_take_unassigned_tickets", TICKETS, write, |_: &_| {
                Expr::is_null(Ticket::COLUMNS.assignee_id)
            })
            .for_role("agent"),
        )
        .policy(
            Policy::new(
                "agents_work_their_own_tickets",
                TICKETS,
                write,
                |who: &SecurityContext| {
                    Expr::eq(Ticket::COLUMNS.assignee_id, who.principal().id.clone())
                },
            )
            .for_role("agent"),
        )
        .policy(
            Policy::new("supervisors_move_work", TICKETS, Action::ALL, |_: &_| {
                Expr::True
            })
            .for_role("supervisor"),
        )
        // Comments are the tenant's, with no per-agent rule: a thread nobody
        // but the assignee could add to is not a thread. The tenant prefix is
        // still the wall.
        .policy(
            Policy::new("agents_join_the_thread", COMMENTS, Action::ALL, |_: &_| {
                Expr::True
            })
            .for_role("agent"),
        )
        .policy(
            Policy::new(
                "supervisors_join_the_thread",
                COMMENTS,
                Action::ALL,
                |_: &_| Expr::True,
            )
            .for_role("supervisor"),
        )
        .policy(
            Policy::new("viewers_read_the_thread", COMMENTS, read, |_: &_| {
                Expr::True
            })
            .for_role("viewer"),
        )
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

impl Inbox {
    /// Whether there is provably nothing after this page.
    ///
    /// A full page that happens to be the last one still carries a cursor:
    /// the only way to find out is to ask again, and reading one row further
    /// on *every* page to save one empty request at the end of a sequence
    /// most callers never finish is the wrong trade. See [`slate_orm::Page`],
    /// which this forwards the shape of.
    #[must_use]
    pub const fn is_last(&self) -> bool {
        self.next.is_none()
    }
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
        txn.insert_record(who, &Indexed(ticket.clone())).await?;
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
        let found: Vec<Indexed> = txn
            .find_records(
                who,
                Expr::eq(Ticket::COLUMNS.reference, Value::Str(reference.to_owned())),
                ScanOrder::Ascending,
            )
            .await?;
        found
            .into_iter()
            .next()
            .map(|Indexed(ticket)| ticket)
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
        txn.replace_record(who, &Indexed(before), &Indexed(after.clone()))
            .await?;
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
        txn.replace_record(who, &Indexed(before), &Indexed(after.clone()))
            .await?;
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
                ScanOrder::Ascending,
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
        txn.delete_record::<Indexed>(
            who,
            &[Value::Uuid(ticket.tenant_id), Value::Uuid(ticket.id)],
        )
        .await?;
        txn.commit().await?;
        Ok(())
    }

    /// One page of the live queue, and where to resume.
    ///
    /// Pass `after = None` for the first page and [`Inbox::next`] for the
    /// one after it. A short page proves there is nothing behind it; a full
    /// one proves nothing either way, so a caller loops until `next` is
    /// `None` and makes one empty request at the end. That is
    /// [`slate_orm::Page`]'s documented shape and not a bug here.
    ///
    /// # The order is the primary key, and it is not a choice
    ///
    /// An inbox wants *most urgent first*, and this pages in `(tenant_id,
    /// id)` order instead. [`Query::after`] pins the access path to the
    /// table's own key range — deliberately, because a page boundary has to
    /// be somewhere the cursor can name, and an index scan yields an order
    /// the primary key does not describe. So a paged read cannot also be a
    /// sorted one: [`Helpdesk::most_urgent`] is the sorted read and it is
    /// bounded by a limit rather than paged.
    ///
    /// `a_sorted_inbox_page_is_refused_rather_than_silently_misordered` is
    /// the demonstration, and it is a *good* refusal — the alternative is a
    /// second page that silently skips or repeats rows.
    ///
    /// # Errors
    /// If the read is refused, and if `page` is zero: a page with no size is
    /// the whole table.
    pub async fn inbox(
        &self,
        who: &SecurityContext,
        after: Option<Vec<Value>>,
        page: usize,
    ) -> Result<Inbox> {
        let mut query = Query::all().filter(live()).limit(page);
        if let Some(cursor) = after {
            query = query.after(cursor);
        }
        let txn = self.store.begin().await?;
        let found = txn.page_records::<Indexed>(who, &query).await?;
        Ok(Inbox {
            tickets: found.rows.into_iter().map(|Indexed(t)| t).collect(),
            next: found.next,
        })
    }

    /// The most urgent live tickets, worst first, at most `limit` of them.
    ///
    /// Sorted rather than paged, for the reason [`Helpdesk::inbox`] gives.
    ///
    /// # What the index actually buys, measured
    ///
    /// `tickets_by_priority` is `(priority, opened_at DESC)`, which is
    /// exactly this order — and an earlier version of this comment went on
    /// to say the planner "can serve it without a sort". **That was a claim
    /// nobody had checked, and checking it made it smaller.** Measured in
    /// `the_priority_index_wins_somewhere_between_12k_and_13k_rows`:
    ///
    /// | tickets | plan |
    /// |---|---|
    /// | any, before [`Helpdesk::analyze`] | `Sort -> Table Scan`, `rows=0` |
    /// | 30 | `Sort -> Table Scan`, cost 1.01 |
    /// | 3,000 | `Sort -> Table Scan`, cost 2.07 |
    /// | 12,000 | `Sort -> Table Scan`, cost 5.75 |
    /// | 13,000 | `Index Scan using tickets_by_priority`, cost 6.00 |
    ///
    /// The index plan's cost is flat because a `LIMIT 5` walk stops after
    /// five entries however large the table is; the scan-and-sort grows with
    /// it. Below the crossover the planner sorts, and it is *right* to —
    /// a point read per row costs more than sorting thirty rows.
    ///
    /// So the honest version: the index earns its keep on a queue of tens of
    /// thousands, and on a small one it is maintained on every write and
    /// never read. Which of those a deployment has is not something this
    /// crate knows.
    ///
    /// # Errors
    /// If the read is refused.
    pub async fn most_urgent(&self, who: &SecurityContext, limit: usize) -> Result<Vec<Ticket>> {
        let txn = self.store.begin().await?;
        let found: Vec<Indexed> = txn.query_records(who, &urgent(limit)).await?;
        Ok(found.into_iter().map(|Indexed(t)| t).collect())
    }

    /// Gather statistics for all four tables and hand them to the planner.
    ///
    /// # This is a step an application has to take, and nothing does it for
    /// you
    ///
    /// Until it runs, the planner costs every query against
    /// [`slate_orm::TableStats::assumed`] — *no rows*. Measured here, on
    /// thirty tickets: `most_urgent` planned as `Sort -> Table Scan on
    /// tickets (rows=0 cost=1.00)`, materialising and sorting an order
    /// `tickets_by_priority` already stores, because a scan of a table the
    /// planner believes is empty is free and an index walk is not.
    ///
    /// The kernel is not hiding this — `analyze_records` and
    /// `set_statistics` are both public and documented. What is missing is
    /// anybody *saying* that a fresh store plans every query wrong until an
    /// application calls them, which is why this method exists and why
    /// `most_urgent_is_served_from_the_priority_index` asserts the plan on
    /// both sides of it.
    ///
    /// `&mut self` because the statistics live on the `RecordStore`, which
    /// is what makes this a decision an application schedules rather than
    /// something a read does for itself: re-analysing on every query would
    /// scan the table to plan the scan.
    ///
    /// # Errors
    /// If the caller may not read a table. A superuser is the usual caller —
    /// statistics are whole-table by design, so gathering them under a row
    /// policy would describe the policy rather than the table.
    pub async fn analyze(&mut self, who: &SecurityContext) -> Result<()> {
        let mut stats = slate_orm::Statistics::new();
        let txn = self.store.begin().await?;
        stats.set(TENANTS, txn.analyze_records::<Tenant>(who).await?);
        stats.set(AGENTS, txn.analyze_records::<Agent>(who).await?);
        stats.set(TICKETS, txn.analyze_records::<Indexed>(who).await?);
        stats.set(COMMENTS, txn.analyze_records::<Comment>(who).await?);
        drop(txn);
        self.store.set_statistics(stats);
        Ok(())
    }

    /// The plan [`Helpdesk::most_urgent`] would run under, without running it.
    ///
    /// Exposed so a test can assert the plan against the *same* query the
    /// method issues. A test that rebuilds the query itself asserts about a
    /// query nobody runs, and drifts the moment the method changes — which is
    /// the failure mode that kept "the planner should serve this from the
    /// index" a claim rather than a result
    /// (`ledger/2026-09-30-the-helpdesk-on-slatedb.md` recorded it as one).
    ///
    /// # Errors
    /// If the caller may not ask for a plan: [`Action::Explain`] is a
    /// separate grant from [`Action::Read`], deliberately, because a plan is
    /// costed against whole-table statistics and so describes rows the caller
    /// may not read.
    pub async fn explain_most_urgent(
        &self,
        who: &SecurityContext,
        limit: usize,
    ) -> Result<Explanation> {
        let txn = self.store.begin().await?;
        Ok(txn.explain_records::<Indexed>(who, &urgent(limit))?)
    }

    /// How much live work each agent is holding, and the unassigned pile.
    ///
    /// One grouped read rather than a scan and a fold in the application,
    /// which is the difference between decoding every ticket and decoding
    /// none: the aggregates are computed where the rows are.
    ///
    /// The unassigned pile arrives as a group whose key is `Value::Null`,
    /// because `GROUP BY` puts the nulls together rather than dropping them.
    /// That is worth knowing: an application that maps the key straight to a
    /// `Uuid` loses the row it most wants to see.
    ///
    /// # Reading a group by name, not by position
    ///
    /// [`Records::grouped_records`] rather than `group_records`. The
    /// difference is the whole of what this crate's first roll-up found
    /// wrong: `group_records` returns `Vec<Value>` in request order, so the
    /// only thing tying `values[1]` to `Sum(hours_logged)` is that the same
    /// list was passed four lines earlier, and reordering the list silently
    /// changes what every call site reads. `Grouped::get` takes the
    /// aggregate, so a mismatch is `None` rather than a wrong number.
    /// Written up in `ledger/2026-09-30-a-group-read-by-name.md`.
    ///
    /// # Errors
    /// If the read is refused, or if a group comes back in a shape this does
    /// not recognise — which is now only a *type* mismatch, since the
    /// positions can no longer drift.
    pub async fn workload(&self, who: &SecurityContext) -> Result<Vec<Workload>> {
        let txn = self.store.begin().await?;
        let groups: Vec<Grouped> = txn
            .grouped_records::<Indexed>(
                who,
                &Query::all().filter(live()),
                &[Ticket::COLUMNS.assignee_id],
                &[
                    Aggregate::Count,
                    Aggregate::Sum(Ticket::COLUMNS.hours_logged),
                ],
            )
            .await?;
        groups.iter().map(read_workload).collect()
    }

    /// Every live ticket whose subject or body holds all of `terms`.
    ///
    /// Two `contains` predicates under an `Or`, one per text index, because
    /// an inverted index covers one column. Terms are lowercased and
    /// deduplicated by [`Expr::contains`], and an empty term list matches
    /// nothing rather than everything — so a blank search box returns no
    /// rows, which is the answer a blank search box should give.
    ///
    /// # Errors
    /// If the read is refused.
    pub async fn search(&self, who: &SecurityContext, terms: &str) -> Result<Vec<Ticket>> {
        let anywhere = Expr::Or(vec![
            Expr::contains(Ticket::COLUMNS.subject, terms),
            Expr::contains(Ticket::COLUMNS.body, terms),
        ]);
        let txn = self.store.begin().await?;
        let found: Vec<Indexed> = txn
            .find_records(who, live().and(anywhere), ScanOrder::Ascending)
            .await?;
        Ok(found.into_iter().map(|Indexed(t)| t).collect())
    }
}

/// The query behind [`Helpdesk::most_urgent`] and its plan.
///
/// One function, two callers, so the plan a test asserts is the plan the
/// service runs. The sort is `(priority ASC, opened_at DESC)`, which is the
/// order `tickets_by_priority` already stores — whether the planner *uses*
/// it is what `most_urgent_is_served_from_the_priority_index` measures
/// rather than assumes.
fn urgent(limit: usize) -> Query {
    Query::all()
        .filter(live())
        .sort_by([
            SortKey::asc(Ticket::COLUMNS.priority),
            SortKey::desc(Ticket::COLUMNS.opened_at),
        ])
        .limit(limit)
}

/// `status IN ('open', 'pending')` — a ticket somebody still has to do.
///
/// A helper rather than a constant because [`Expr`] holds owned `Value`s.
fn live() -> Expr {
    Expr::In {
        column: Ticket::COLUMNS.status,
        values: vec![
            Value::Str("open".to_owned()),
            Value::Str("pending".to_owned()),
        ],
    }
}

/// One grouped row as a [`Workload`].
///
/// Every arm names the column or the aggregate it reads, so this cannot
/// drift with the request the way the first version could — that one indexed
/// `values[0]` and `values[1]`, and it is the reason
/// [`Records::grouped_records`] exists at all.
///
/// What remains is the *type* mapping, which no lookup can do: a [`Value`]
/// is a closed enum and a `Workload` field is an `i64`. Every arm that
/// cannot happen is an error rather than a `panic!` or a zero, because a
/// silent zero in a billing roll-up is the worst of the three.
fn read_workload(group: &Grouped) -> Result<Workload> {
    let shape = |what: &str, got: Option<&Value>| {
        HelpdeskError::UnexpectedGroup(format!("{what} came back as {got:?}"))
    };
    let assignee = group.key(Ticket::COLUMNS.assignee_id);
    let assignee_id = match assignee {
        Some(Value::Uuid(id)) => Some(*id),
        // `Null` is the unassigned pile. `None` cannot happen — the same
        // column was requested four lines up — but is folded in rather than
        // refused, because both mean "no agent holds these".
        Some(Value::Null) | None => None,
        other => return Err(shape("assignee_id", other)),
    };
    let tickets = group.get(Aggregate::Count);
    let hundredths = group.get(Aggregate::Sum(Ticket::COLUMNS.hours_logged));
    Ok(Workload {
        assignee_id,
        tickets: match tickets {
            Some(Value::I64(n)) => *n,
            Some(Value::U64(n)) => i64::try_from(*n).map_err(|_| shape("COUNT(*)", tickets))?,
            other => return Err(shape("COUNT(*)", other)),
        },
        // `SUM` over no rows is null, which cannot happen for a group that
        // exists — a group exists because a row landed in it. Mapped to zero
        // anyway rather than to an error, because "this agent has logged no
        // time" is a real answer and a refusal would be a wrong one.
        hundredths: match hundredths {
            Some(Value::I64(n)) => *n,
            Some(Value::Null) => 0,
            other => return Err(shape("SUM(hours_logged)", other)),
        },
    })
}
