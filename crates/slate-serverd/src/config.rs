//! The configuration file: what it says, and what it deliberately cannot.
//!
//! # Why TOML
//!
//! The file has to carry typed literals, be edited by hand, be reviewed in a
//! diff, and — most of all — be *commented*, because half the decisions in it
//! (which authentication mode, which durability, whether a replica is pinned)
//! are decisions somebody will want to explain to the next reader.
//!
//! - **JSON** cannot hold a comment at all. That alone disqualifies it for a
//!   file whose `[auth]` section is a security decision.
//! - **YAML** can, and pays for it with a type system that guesses. `no` is
//!   `false`, `1.0` is a float, `08` is an error, and a version number is a
//!   string only if it is quoted. Every one of those is a silent wrong value
//!   in a file whose whole job is to state values exactly. YAML also has
//!   anchors and merge keys, which turn a config file into a program.
//! - **TOML** has comments, one unambiguous scalar syntax, no anchors, and is
//!   the format every Rust operator is already reading in this repository. Its
//!   real weakness — deep nesting is painful — does not bite here, because the
//!   two deeply nested things (predicates and index expressions) are strings
//!   in a little language rather than nested tables. See [`crate::lang`] for
//!   why that was the right split.
//!
//! Every table in this file is `deny_unknown_fields`. A mistyped key is a
//! refusal to start, not a setting silently at its default — which for
//! `[auth]` is the difference between a typo and an open server.
//!
//! # What a configuration file cannot express
//!
//! Recorded here rather than discovered later. In each case the honest answer
//! is that the deployment writes Rust and links `slate-server` directly, which
//! this binary makes unnecessary in the common case and does not pretend to
//! make unnecessary in every case.
//!
//! **A policy that is not a function of the caller's identity.** A
//! [`Policy`](slate_kernel::Policy) is a Rust closure over a
//! [`SecurityContext`](slate_kernel::SecurityContext) and can compute anything
//! at all — call out to a service, read a cached group membership, vary by
//! time of day. A configured policy is one [`Expr`](slate_kernel::Expr) with
//! two typed holes, `:principal` and `:tenant`. That is strictly less, and it
//! is the largest gap in this file.
//!
//! It is worth naming what the restriction *buys*, since the repository's own
//! argument for Rust policies is that a template language brings a
//! parameter-substitution bug class. The hole here is not textual: it is
//! filled with a [`Value`](slate_tuple::Value) taken straight off the
//! principal, after parsing, into a position the parser has already typed. No
//! string is ever concatenated, so the injection shape that argument is about
//! cannot occur. What is genuinely lost is expressiveness, not safety.
//!
//! **A policy whose predicate depends on a role.** `Policy::for_role` narrows
//! *which* callers a policy applies to, and that is expressible; a predicate
//! that reads the role set is not, because a role set is not a `Value`.
//!
//! **`CASE`, `EXTRACT`, `DATE_TRUNC` and vector distance in an index
//! expression.** See [`crate::lang::scalar`].
//!
//! **Statistics.** `HeadConfig::with_statistics` takes measurements, and
//! measurements belong to data rather than to a file. `analyze_on_start` runs
//! the real `analyze` instead, which is the only honest way to produce them.
//!
//! **Anything about the wire itself.** No TLS, no connection limits, no
//! keepalive. `slate-server`'s own documentation declines those on the grounds
//! that they belong to whatever fronts the server and that a second place to
//! configure them makes a deployment worse; a binary that added them here
//! would be that second place.
//!
//! **A schema that changes while the server runs.** There is no dynamic DDL in
//! this project by design, and a file read once at startup is exactly as
//! static as the Rust it replaces.

use crate::error::{Fault, Started};
use core::time::Duration;
use serde::Deserialize;
use std::path::Path;

/// The whole file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document {
    /// Where to accept connections.
    pub(crate) listen: Listen,
    /// Who may ask. Optional in the *file* so that its absence is this
    /// crate's refusal with this crate's message, rather than serde's.
    pub(crate) auth: Option<Auth>,
    /// Where the data lives.
    pub(crate) storage: Storage,
    /// Read replicas, in addition to the writer.
    #[serde(default)]
    pub(crate) replicas: Vec<Replica>,
    /// The writer lease.
    #[serde(default)]
    pub(crate) lease: LeaseSettings,
    /// How reads are spread over replicas.
    #[serde(default)]
    pub(crate) routing: Routing,
    /// What the node will spend on open transactions and stream batches.
    #[serde(default)]
    pub(crate) limits: LimitSettings,
    /// What the planner is told about the data.
    #[serde(default)]
    pub(crate) planner: Planner,
    /// What happens to the schema on the way up.
    #[serde(default)]
    pub(crate) schema: Schema,
    /// How long a shutdown may take.
    #[serde(default)]
    pub(crate) shutdown: Shutdown,
    /// What the node says about the requests it serves.
    #[serde(default)]
    pub(crate) observability: Observability,
    /// The tables this node serves.
    #[serde(default)]
    pub(crate) tables: Vec<Table>,
    /// Roles, grants and policies.
    #[serde(default)]
    pub(crate) security: Security,
}

impl Document {
    /// Read and parse a file.
    pub(crate) fn read(path: &Path) -> Started<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|why| Fault::new(format!("cannot read `{}`: {why}", path.display())))?;
        // TOML's own error carries a line and column and a snippet, which is
        // better than anything worth writing here, so it is passed through
        // rather than summarised.
        toml::from_str(&text).map_err(|why| {
            Fault::new(format!(
                "`{}` is not valid configuration:\n{why}",
                path.display()
            ))
        })
    }
}

// ── listening ───────────────────────────────────────────────────────────────

/// Where to accept connections.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Listen {
    /// A socket address. Port 0 binds an arbitrary free port and the banner
    /// reports which, which is what a test harness needs and what nothing else
    /// should use.
    pub(crate) address: String,
}

// ── authentication ──────────────────────────────────────────────────────────

/// Who may ask.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Auth {
    /// `deny-all`, `trusted-header` or `token`. A string rather than an enum
    /// so an unknown mode is refused with a message listing the three, which
    /// is worth more here than anywhere else in the file.
    pub(crate) mode: String,
    /// `trusted-header`: refuse a request that names no tenant.
    ///
    /// An `Option` rather than a `bool` so that "not written" is
    /// distinguishable from "written as false": the mode checks below refuse a
    /// field that belongs to a different mode, and a default `false` would
    /// make `require_tenant` invisible to that check.
    #[serde(default)]
    pub(crate) require_tenant: Option<bool>,
    /// `trusted-header`: the operator's statement that something in front of
    /// this process sets the identity headers and strips the client's.
    /// Required to bind anything but loopback.
    #[serde(default)]
    pub(crate) header_source: Option<String>,
    /// `token`: the operator's statement that the connection is encrypted by
    /// the time it reaches this process. Required to bind anything but
    /// loopback.
    #[serde(default)]
    pub(crate) transport: Option<String>,
    /// `token`: the bearer tokens this server accepts.
    #[serde(default)]
    pub(crate) tokens: Vec<Token>,
}

/// One bearer token, and the identity it stands for.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Token {
    /// A label, for the error message when two tokens collide and for nothing
    /// else. Never compared against anything a client sends.
    pub(crate) name: String,
    /// The environment variable holding the secret.
    #[serde(default)]
    pub(crate) secret_env: Option<String>,
    /// A file holding the secret, trailing newline trimmed.
    #[serde(default)]
    pub(crate) secret_file: Option<String>,
    /// The principal id, tagged: `u64:7`, `str:ada`, `uuid:…`.
    pub(crate) principal: String,
    /// The tenant, tagged the same way.
    #[serde(default)]
    pub(crate) tenant: Option<String>,
    /// The roles this token holds.
    #[serde(default)]
    pub(crate) roles: Vec<String>,
}

// ── storage ─────────────────────────────────────────────────────────────────

/// Where the data lives.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Storage {
    /// `memory`, `local` or `s3`.
    pub(crate) backend: String,
    /// The database's path within the object store.
    #[serde(default = "default_database_path")]
    pub(crate) path: String,
    /// `local`: the directory the object store is rooted at.
    #[serde(default)]
    pub(crate) directory: Option<String>,
    /// `s3`: the bucket and how to reach it.
    #[serde(default)]
    pub(crate) s3: Option<S3>,
    /// Whether a commit waits for object storage: `durable` or `visible`.
    #[serde(default = "default_durability")]
    pub(crate) durability: String,
    /// `serializable` or `snapshot`.
    #[serde(default = "default_isolation")]
    pub(crate) isolation: String,
}

fn default_database_path() -> String {
    "/records".to_owned()
}

fn default_durability() -> String {
    "durable".to_owned()
}

/// The safer level is the default and stepping down is explicit, matching the
/// record layer's own choice.
fn default_isolation() -> String {
    "serializable".to_owned()
}

/// An S3-compatible bucket.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct S3 {
    /// Take the whole configuration from `SLATE_S3_*` instead of this table.
    #[serde(default)]
    pub(crate) from_env: bool,
    /// Bucket name. Required unless `from_env`.
    #[serde(default)]
    pub(crate) bucket: Option<String>,
    /// Endpoint URL, for anything that is not AWS.
    #[serde(default)]
    pub(crate) endpoint: Option<String>,
    /// Region. Anything self-hosted ignores the value but still needs one.
    #[serde(default)]
    pub(crate) region: Option<String>,
    /// The environment variable holding the access key id.
    ///
    /// A variable rather than the key itself, for the same reason a bearer
    /// token's secret is: this file is the thing that gets committed, copied
    /// into a ticket and pasted into a chat window.
    #[serde(default)]
    pub(crate) access_key_id_env: Option<String>,
    /// The environment variable holding the secret access key.
    #[serde(default)]
    pub(crate) secret_access_key_env: Option<String>,
    /// The environment variable holding a session token.
    #[serde(default)]
    pub(crate) session_token_env: Option<String>,
    /// Permit plain HTTP. Needed by a local MinIO and by nothing in
    /// production.
    #[serde(default)]
    pub(crate) allow_http: bool,
    /// `bucket.host/key` rather than `host/bucket/key`.
    #[serde(default)]
    pub(crate) virtual_hosted_style: bool,
}

/// A read replica.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Replica {
    /// The name reported in `served_by`, and the one tenant affinity hashes.
    pub(crate) name: String,
    /// `following`, `latest` or `pinned`.
    #[serde(default = "default_replica_mode")]
    pub(crate) mode: String,
    /// The checkpoint id, for `pinned`.
    #[serde(default)]
    pub(crate) checkpoint: Option<String>,
    /// How often this replica re-reads the manifest.
    ///
    /// Most of the replica's lag, and the number that decides whether a read
    /// carrying a sequence can be served here at all: the pool waits
    /// `[routing] catch_up` for a replica to reach that sequence, and a
    /// replica can only reach it on a poll. Unset derives it from `catch_up`
    /// rather than repeating a constant that has to stay below one — see
    /// `storage::poll_interval`.
    #[serde(default)]
    pub(crate) poll_interval: Option<String>,
}

fn default_replica_mode() -> String {
    "following".to_owned()
}

// ── leadership ──────────────────────────────────────────────────────────────

/// The writer lease.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LeaseSettings {
    /// The lease object's path, in the same store the database is in.
    #[serde(default = "default_lease_path")]
    pub(crate) path: String,
    /// This node's name in the lease. Defaults to `hostname/pid`, which is
    /// what an operator reading the lease object with `cat` wants to see.
    #[serde(default)]
    pub(crate) holder: Option<String>,
    /// How long a term lasts.
    #[serde(default)]
    pub(crate) term: Option<String>,
}

impl Default for LeaseSettings {
    fn default() -> Self {
        Self {
            path: default_lease_path(),
            holder: None,
            term: None,
        }
    }
}

fn default_lease_path() -> String {
    "leases/writer".to_owned()
}

// ── tuning ──────────────────────────────────────────────────────────────────

/// How reads are spread over replicas.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Routing {
    /// How long to wait for a replica to catch up before giving up on it.
    ///
    /// Also, and not obviously, the setting that decides how often every
    /// replica polls: a replica reaches a sequence on a poll, so a poll longer
    /// than this budget means no read carrying a sequence can be served by a
    /// replica at all. `[[replicas]] poll_interval` derives from this unless it
    /// is set, and is refused if it is set at or above it. See
    /// `storage::poll_interval`.
    #[serde(default)]
    pub(crate) catch_up: Option<String>,
    /// Route by tenant, rather than round-robin.
    #[serde(default)]
    pub(crate) tenant_affinity: Option<bool>,
}

/// What the node will spend on open transactions and stream batches.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LimitSettings {
    /// How many transactions may be open at once.
    #[serde(default)]
    pub(crate) max_transactions: Option<usize>,
    /// How long a transaction may sit idle before it is rolled back.
    #[serde(default)]
    pub(crate) idle_timeout: Option<String>,
    /// How many rows go in one message of a query stream.
    #[serde(default)]
    pub(crate) rows_per_message: Option<usize>,
    /// How many operations one `Batch` may carry.
    ///
    /// `0` is refused rather than read as "no limit", the same as every other
    /// limit in this table — a zero here is a typo far more often than an
    /// intention, and the way to mean "no limit" is to say so.
    pub(crate) max_batch_operations: Option<usize>,
    /// How many rows a predicate write may hand back when `RETURNING` is asked
    /// for.
    ///
    /// `0` is refused, the same as the others. Raising it past what a client
    /// will decode trades one failure for another: the write is then allowed
    /// and the response is refused at the client, which is the defect this
    /// limit exists to prevent.
    #[serde(default)]
    pub(crate) max_returned_rows: Option<usize>,
    /// How many requests may be in flight at once across all connections.
    ///
    /// Unset means unbounded, which is what shipped: a caller could open as
    /// many concurrent requests as they had sockets.
    #[serde(default)]
    pub(crate) max_concurrent_requests: Option<usize>,
    /// How long one request may run before it is cancelled.
    #[serde(default)]
    pub(crate) request_timeout: Option<String>,
    /// Distinct `GROUP BY` keys one request may hold.
    #[serde(default)]
    pub(crate) max_groups: Option<usize>,
    /// Distinct values one `COUNT(DISTINCT)` may hold.
    #[serde(default)]
    pub(crate) max_distinct: Option<usize>,
    /// Rows an `ORDER BY` with no `LIMIT` may materialise.
    #[serde(default)]
    pub(crate) max_sort_rows: Option<usize>,
}

/// What the planner is told about the data.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Planner {
    /// Read every table at startup and gather statistics.
    ///
    /// Off by default because it costs a full read of every table before the
    /// socket opens, which on real object storage is not something to do
    /// without asking.
    #[serde(default)]
    pub(crate) analyze_on_start: bool,
}

/// What happens to the schema on the way up.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Schema {
    /// Build any index the keyspace does not already hold, before serving.
    ///
    /// **On by default**, which is the opposite of `analyze_on_start` above,
    /// and for the opposite reason. Statistics are an optimisation: a node that
    /// skips them answers correctly and more slowly. An unbuilt index is not —
    /// a query the planner routes through it returns *no rows*, with no error,
    /// and the rows are still on disk. The expensive case and the case you need
    /// are therefore the same case, and defaulting to off would make the safe
    /// configuration the one nobody writes down.
    ///
    /// Turning it off does not mean ignoring the problem. The node then
    /// *verifies* instead and refuses to start if anything is outstanding,
    /// which is the setting for a deployment that wants to run a large backfill
    /// deliberately rather than inside a rolling restart.
    #[serde(default = "yes")]
    pub(crate) migrate_on_start: bool,
}

impl Default for Schema {
    fn default() -> Self {
        Self {
            migrate_on_start: true,
        }
    }
}

const fn yes() -> bool {
    true
}

/// What the node says about the requests it serves.
///
/// Both off by default. A node that starts logging differently because it was
/// upgraded is a surprise, and a line per request on a busy node is a hundred
/// megabytes an hour of stderr nobody asked for — the failure mode of logging
/// by default is a full disk, not a missing log.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observability {
    /// One line per request: method, gRPC status, and time to the response
    /// head. A debugging tool rather than something to leave on.
    #[serde(default)]
    pub(crate) request_log: bool,
    /// How often to print per-method counters, as a duration like `"60s"`.
    ///
    /// Separate from `request_log` because it is cheap at any request rate,
    /// and is the one a production node should have on.
    #[serde(default)]
    pub(crate) summary_interval: Option<String>,
    /// Where to serve `/metrics` over HTTP, or nowhere when unset.
    ///
    /// A second address rather than a path on the gRPC one: the two want
    /// different firewall rules, because one carries the data and the other
    /// carries the shape of the traffic. Unauthenticated — see `metrics.rs`
    /// for why a token was left out rather than added — so a value that is not
    /// loopback is warned about at startup.
    #[serde(default)]
    pub(crate) metrics_address: Option<String>,
}

/// How long a shutdown may take.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Shutdown {
    /// How long in-flight requests have to finish after a signal.
    #[serde(default)]
    pub(crate) grace: Option<String>,
}

// ── schema ──────────────────────────────────────────────────────────────────

/// A table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Table {
    /// The name clients use.
    pub(crate) name: String,
    /// The table id, which is the key prefix. Stable for the life of the data.
    pub(crate) id: u32,
    /// Columns, in ordinal order.
    pub(crate) columns: Vec<Column>,
    /// The primary key, in key order.
    pub(crate) primary_key: Vec<String>,
    /// The tenant discriminator, which must be the first key column.
    #[serde(default)]
    pub(crate) tenant_column: Option<String>,
    /// The version stamped onto rows as they are written.
    #[serde(default)]
    pub(crate) schema_version: Option<u32>,
    /// Secondary indexes.
    #[serde(default)]
    pub(crate) indexes: Vec<Index>,
    /// `CHECK` constraints.
    #[serde(default)]
    pub(crate) checks: Vec<Check>,
    /// Foreign keys into another table's primary key.
    #[serde(default)]
    pub(crate) foreign_keys: Vec<ForeignKey>,
}

/// A column.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Column {
    /// The column's name.
    pub(crate) name: String,
    /// One of `bool`, `bytes`, `str`, `i64`, `u64`, `f64`, `uuid`, `vector`,
    /// `decimal`.
    #[serde(rename = "type")]
    pub(crate) value_type: String,
    /// Digits after the decimal point, for a `decimal` column.
    ///
    /// Required there and refused anywhere else, rather than defaulted to 0: a
    /// decimal column whose scale was left out would silently store units at
    /// scale 0, so every value in it would be a hundred times the number
    /// somebody meant, and nothing would ever say so. A scale on a column that
    /// is not a decimal is a different mistake with the same cause — somebody
    /// believes this column holds a fixed-point number — and is refused for
    /// the same reason.
    #[serde(default)]
    pub(crate) scale: Option<u8>,
    /// `"created_at"` or `"updated_at"`: the store writes this column.
    ///
    /// A string rather than two booleans, because the two are exclusive and a
    /// column with both set would have no meaning — `created_at = true,
    /// updated_at = true` is a configuration somebody can write and nobody can
    /// explain. One field with two spellings makes that unsayable rather than
    /// refused.
    #[serde(default)]
    pub(crate) managed: Option<String>,
    /// Whether the column may hold null.
    #[serde(default)]
    pub(crate) nullable: bool,
    /// The schema version the column was added in. Rows older than this carry
    /// no bytes for it, so it must be nullable or have a default.
    #[serde(default)]
    pub(crate) added_in: Option<u32>,
    /// The schema version the column was dropped in.
    #[serde(default)]
    pub(crate) dropped_in: Option<u32>,
    /// The value an unset column is stored as, and the value a row older than
    /// `added_in` reads back as. A TOML value, converted against `type`.
    #[serde(default)]
    pub(crate) default: Option<toml::Value>,
    /// Names this column used to answer to.
    #[serde(default)]
    pub(crate) previous_names: Vec<String>,
}

/// A secondary index.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Index {
    /// The index's name, used by a query hint and by `EXPLAIN`.
    pub(crate) name: String,
    /// The index id, which is the key prefix. Stable for the life of the data.
    pub(crate) id: u32,
    /// Columns, in key order. Each is either a name or `{ column, direction }`.
    #[serde(default)]
    pub(crate) columns: Vec<IndexColumn>,
    /// A key computed from the row instead of read out of it. Mutually
    /// exclusive with `columns`.
    #[serde(default)]
    pub(crate) expression: Option<String>,
    /// What `expression` produces. Required with it.
    #[serde(default)]
    pub(crate) produces: Option<String>,
    /// The direction an expression index is stored in.
    #[serde(default)]
    pub(crate) direction: Option<String>,
    /// Whether the index is unique.
    #[serde(default)]
    pub(crate) unique: bool,
    /// The rows the index holds. Absent means every row.
    #[serde(default, rename = "where")]
    pub(crate) predicate: Option<String>,
}

/// One column of an index: a bare name, or a name and a direction.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum IndexColumn {
    /// `columns = ["kind"]`
    Named(String),
    /// `columns = [{ column = "kind", direction = "desc" }]`
    Directed {
        /// The column's name.
        column: String,
        /// `asc` or `desc`.
        direction: String,
    },
}

/// A `CHECK` constraint.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Check {
    /// The constraint's name, unique within its table.
    pub(crate) name: String,
    /// The predicate a stored row must not violate.
    pub(crate) predicate: String,
}

/// A foreign key into another table's primary key.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForeignKey {
    /// The constraint's name.
    pub(crate) name: String,
    /// The table referenced, by name. Resolved to its id, so a renamed table
    /// is a startup error rather than a dangling number.
    pub(crate) parent: String,
    /// The referencing columns, in the parent's primary key order.
    pub(crate) columns: Vec<String>,
    /// `restrict` or `cascade`.
    #[serde(default = "default_on_delete")]
    pub(crate) on_delete: String,
}

fn default_on_delete() -> String {
    "restrict".to_owned()
}

// ── security ────────────────────────────────────────────────────────────────

/// Roles, grants and policies.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Security {
    /// What each role may do.
    #[serde(default)]
    pub(crate) grants: Vec<GrantSpec>,
    /// Row-level policies. Adding one enables row-level security on its table.
    #[serde(default)]
    pub(crate) policies: Vec<PolicySpec>,
    /// Tables where row-level security is on with no policy, which denies
    /// every row to every non-superuser.
    #[serde(default)]
    pub(crate) rls_enabled: Vec<String>,
}

/// A role's permission on some tables.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrantSpec {
    /// The role granted.
    pub(crate) role: String,
    /// The tables, by name.
    pub(crate) tables: Vec<String>,
    /// `read`, `insert`, `update`, `delete`, or `all`.
    pub(crate) actions: Vec<String>,
}

/// A row-level policy.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicySpec {
    /// The policy's name, which appears in nothing a client sees and
    /// everything an operator reads.
    pub(crate) name: String,
    /// The table it applies to.
    pub(crate) table: String,
    /// The actions it applies to.
    pub(crate) actions: Vec<String>,
    /// The roles it applies to. Empty means every caller.
    #[serde(default)]
    pub(crate) roles: Vec<String>,
    /// The rows it admits. May use `:principal` and `:tenant`.
    pub(crate) using: String,
}

// ── shared parsing ──────────────────────────────────────────────────────────

/// Parse a duration written as `250ms`, `15s`, `2m` or `1h`.
///
/// Hand-written rather than taken from a crate because the error is the point:
/// "`15` has no unit; write `15s`" is worth more than a parser that accepts a
/// bare number and picks a unit, and worth more than pulling in a dependency
/// whose message names its own grammar.
pub(crate) fn duration(text: &str, field: &str) -> Started<Duration> {
    let trimmed = text.trim();
    let split = trimmed.find(|c: char| !c.is_ascii_digit()).ok_or_else(|| {
        Fault::new(format!(
            "`{field} = \"{trimmed}\"` has no unit; write one of ms, s, m or h"
        ))
    })?;
    let (number, unit) = (
        trimmed.get(..split).unwrap_or_default(),
        trimmed.get(split..).unwrap_or_default().trim(),
    );
    let count: u64 = number.parse().map_err(|_| {
        Fault::new(format!(
            "`{field} = \"{trimmed}\"` does not start with a whole number"
        ))
    })?;
    let scale = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        other => {
            return Err(Fault::new(format!(
                "`{field} = \"{trimmed}\"` has the unit `{other}`, which is not one of ms, s, m or h"
            )));
        }
    };
    count
        .checked_mul(scale)
        .map(Duration::from_millis)
        .ok_or_else(|| {
            Fault::new(format!(
                "`{field} = \"{trimmed}\"` is too long to represent"
            ))
        })
}

/// Parse a duration if it is present.
pub(crate) fn optional_duration(text: Option<&String>, field: &str) -> Started<Option<Duration>> {
    text.map(|t| duration(t, field)).transpose()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn durations_carry_their_unit() {
        assert_eq!(duration("250ms", "x").unwrap(), Duration::from_millis(250));
        assert_eq!(duration("15s", "x").unwrap(), Duration::from_secs(15));
        assert_eq!(duration("2m", "x").unwrap(), Duration::from_secs(120));
        assert_eq!(duration("1h", "x").unwrap(), Duration::from_secs(3600));
    }

    #[test]
    fn a_bare_number_is_refused_rather_than_given_a_unit() {
        let error = duration("15", "lease.term").unwrap_err().to_string();
        assert!(error.contains("no unit"), "{error}");
        assert!(error.contains("lease.term"), "{error}");
    }

    #[test]
    fn an_unknown_unit_names_the_ones_that_exist() {
        let error = duration("15 fortnights", "x").unwrap_err().to_string();
        assert!(error.contains("ms, s, m or h"), "{error}");
    }

    #[test]
    fn a_mistyped_key_is_refused_rather_than_defaulted() {
        // The property that matters most in this file: `[auth] moed = "token"`
        // must not leave `mode` at some default.
        let error = toml::from_str::<Document>(
            r#"
            [listen]
            address = "127.0.0.1:0"
            [storage]
            backend = "memory"
            [auth]
            moed = "deny-all"
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("moed"), "{error}");
    }

    #[test]
    fn an_index_column_may_be_a_name_or_a_direction() {
        let table: Table = toml::from_str(
            r#"
            name = "docs"
            id = 1
            columns = [{ name = "id", type = "u64" }]
            primary_key = ["id"]
            [[indexes]]
            name = "by_id"
            id = 1
            columns = ["id", { column = "id", direction = "desc" }]
            "#,
        )
        .unwrap();
        let index = table.indexes.first().unwrap();
        assert!(matches!(index.columns.first(), Some(IndexColumn::Named(_))));
        assert!(matches!(
            index.columns.get(1),
            Some(IndexColumn::Directed { .. })
        ));
    }
}
