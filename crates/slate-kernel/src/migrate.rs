//! Migrations: what the code declares, what the keyspace actually holds, and
//! the one gap between them that has to be closed by touching data.
//!
//! # The defect this exists for
//!
//! Adding an index to a table that already holds rows does not make queries
//! slower. It makes them **wrong**. Measured, before any of this existed, on a
//! three-row table with a filter on the indexed column:
//!
//! ```text
//! rows found through the new index = 0   (a scan finds 1)
//! ```
//!
//! The planner sees an index whose columns cover the predicate, costs it as the
//! cheap option, scans a key range that holds nothing because no write ever
//! wrote into it, and returns an empty result. Every layer behaves correctly
//! and the answer is wrong, which is the worst shape a bug can have: there is
//! no error, no warning, and the rows are still there.
//!
//! Nothing in the stored bytes could have caught it, because a row carries no
//! record of which indexes existed when it was written. So the fix has to be a
//! thing written down on purpose: one key per table, in its own keyspace,
//! saying what the last migration left behind.
//!
//! # What a migration is here
//!
//! Almost nothing, which is the point. The schema layer is deliberately
//! rewrite-free: a new column with a `DEFAULT` is read back by the decoder and
//! never written to old rows, a dropped column keeps its ordinal for ever and
//! is skipped, and a rename moves no bytes because no name appears on disk.
//! Those are declarations, and they take effect the moment the new binary
//! starts.
//!
//! **Building an index is the exception**, and as far as this module is
//! concerned it is the only one. An index entry is a key that must exist, and
//! no amount of reading the schema differently will conjure it. So the runner
//! has exactly one step that costs anything.
//!
//! # Why a fingerprint and not a diff
//!
//! What is stored is a hash of the layout, not the layout. A stored copy of
//! every column would let the runner say "the type of `price` changed from
//! `i64` to `f64`", which is a better message than "the layout changed" — and
//! it would mean the schema format itself is on disk, in a second encoding that
//! has to keep step with the first for ever. The fingerprint detects the same
//! set of changes, is eight bytes, and cannot drift from the thing it describes
//! because it is computed from it.
//!
//! The cost is a worse error message, and it is a real cost. It is paid down by
//! naming, in that message, the specific things the fingerprint covers.
//!
//! # What the fingerprint covers, and what it deliberately does not
//!
//! It covers what decides how bytes are read: the number of columns, each
//! one's type, whether it is nullable, whether it has been dropped, the primary
//! key, and the tenant column. A change to any of those reads old rows as
//! something they are not.
//!
//! It does **not** cover names. A name appears nowhere on disk — that is the
//! whole reason `renamed_column` is free — so a rename is not a migration and
//! must not look like one. Nor does it cover `CHECK` constraints or foreign
//! keys, which are rules about future writes rather than about existing bytes.
//! Neither claim is obvious, so both are tested.

use crate::error::{KernelError, Result};
use crate::keys;
use crate::store::{KeyRange, KvSnapshot, KvStore, ScanOrder};
use slate_schema::{Catalog, IndexDef, IndexId, TableDef, TableId, decode_row};
use slate_tuple::{Value, ValueType};

/// Format version of the stored state record.
///
/// Stored, rather than assumed, so a future change to this record is a refusal
/// naming the version it found instead of a misparse. It is the same reason
/// `encode_body` leads with one — and that one exists because of a bug this
/// project would rather not repeat.
const STATE_FORMAT_V1: u64 = 1;

/// The state record format that also carries the table's stored schema.
///
/// **Read both, write this one.** A flag day was the obvious alternative and
/// is unacceptable for a store that already holds rows: `decode_state` refuses
/// a format it does not know, so bumping the only version would make every
/// existing table unreadable and every deployment a dump and reload. A V1
/// record decodes as it always did and yields a state with no stored schema; a
/// table is rewritten as V2 the next time it is migrated, which is already a
/// transaction that writes the record. No separate step, no downtime, and the
/// upgrade arrives per table rather than all at once.
///
/// **A downgrade is one-way per table.** An older binary meeting a V2 record
/// gets `UnknownFormat` and refuses to start — fail-closed and correct, and
/// worth a release note rather than a discovery.
const STATE_FORMAT_V2: u64 = 2;

/// Version byte of the *nested* schema blob, inside a V2 state record.
///
/// Its own version rather than leaning on the outer one, which
/// `docs/persisting-the-schema.md` left as "probably yes". Yes: the outer
/// format says how the record is laid out and the inner says what a column
/// record holds, and those change for different reasons. Adding a per-column
/// property with only an outer version would need `STATE_FORMAT_V3` and a
/// third branch in `decode_state`, where with this it is one branch here.
const SCHEMA_FORMAT_V1: u64 = 1;

/// How many rows one backfill transaction writes before committing.
///
/// A backfill of a large table cannot be one transaction: the write set is
/// bounded by memory, and a failure at ninety per cent throws away ninety per
/// cent. Batching makes it resumable — the state key is written only at the
/// end, so an interrupted backfill simply runs again, and re-writing an index
/// entry that already exists is a no-op with the same bytes.
///
/// The number is a guess and is deliberately not tuned: the step is
/// once-per-deploy and bounded by the table, so the difference between 1,000
/// and 10,000 is not worth the measurement it would take to justify. Named
/// rather than inline so that is visible.
const BACKFILL_BATCH: usize = 1_000;

/// What the keyspace records about one table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableState {
    /// The schema version the last migration recorded.
    pub schema_version: u32,
    /// The layout fingerprint at that point. See the module docs.
    pub fingerprint: u64,
    /// Indexes whose entries have actually been written, in id order.
    ///
    /// An index in the schema and not in here is one whose key range is empty
    /// or partial, and querying through it is the defect this module opens
    /// with.
    pub built: Vec<IndexId>,
    /// The layout the fingerprint above was computed from, when the record
    /// carries one.
    ///
    /// `None` is **permanent, not a transition**: a table registered by an
    /// older binary, a restored backup, a table nobody has migrated since the
    /// upgrade. Every reader has to handle "I do not know this table's
    /// previous shape" for ever, so it is a first-class answer rather than a
    /// case to tolerate — [`FingerprintChange::describe`] falls back to the
    /// old hash-only refusal and says which of the two situations the reader
    /// is in.
    pub schema: Option<StoredSchema>,
}

/// A table's layout, as the keyspace remembers it.
///
/// **Exactly the fingerprint's inputs, plus names.** Stopping precisely there
/// is the decision, and `docs/persisting-the-schema.md` argues both halves:
/// anything omitted is a difference the fingerprint can detect and this cannot
/// explain, and anything added is a difference this can report that is *not* a
/// migration — a `CHECK` or a foreign key moved, announced at startup, in a
/// refusal path, about something needing no action.
///
/// Names are the one addition to the hash's inputs and are deliberately **not**
/// hashed. The fingerprint covers a column's type and position and not its
/// name, because a rename moves no bytes and must not look like a migration —
/// but a diff that cannot say `email` is a diff that says "column 3". That
/// asymmetry is the whole point of storing rather than widening the hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSchema {
    /// Every column, live and dropped, in ordinal order.
    pub columns: Vec<StoredColumn>,
    /// The primary key, as ordinals.
    pub primary_key: Vec<usize>,
    /// The tenant column's ordinal, if the table is tenant-scoped.
    pub tenant_column: Option<usize>,
}

/// One column, as the keyspace remembers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredColumn {
    /// The column's name. Not hashed; see [`StoredSchema`].
    pub name: String,
    /// [`type_code`] of what it holds. A code rather than a name, for the same
    /// reason the fingerprint hashes one: renaming a `ValueType` variant
    /// upstream must not invalidate a stored record.
    pub type_code: u64,
    /// Whether it accepts null.
    pub nullable: bool,
    /// Whether a migration has retired it.
    pub dropped: bool,
    /// A decimal's scale, and `0` for every other type.
    pub scale: u8,
    /// [`type_code`] of an array's elements, and `0` for every other type.
    pub element_code: u64,
}

impl StoredSchema {
    /// What the keyspace would remember about `table` if it were written now.
    #[must_use]
    pub fn of(table: &TableDef) -> Self {
        Self {
            columns: table
                .columns()
                .iter()
                .map(|column| StoredColumn {
                    name: column.name().to_owned(),
                    type_code: type_code(column.value_type()),
                    nullable: column.is_nullable(),
                    dropped: column.is_dropped(),
                    scale: column.scale().unwrap_or(0),
                    element_code: column.element_type().map_or(0, type_code),
                })
                .collect(),
            primary_key: table.primary_key().iter().map(|o| o.0).collect(),
            tenant_column: table.tenant_column().map(|o| o.0),
        }
    }
}

/// One thing a migration would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A table the keyspace has never heard of. Costs nothing: it records the
    /// state, and its indexes are built by `BuildIndex` steps beside it.
    Register {
        /// The table.
        table: TableId,
        /// Its name, for the report.
        name: String,
    },
    /// Write every existing row's entry for an index that has none.
    ///
    /// The only step that reads or writes row data.
    BuildIndex {
        /// The table.
        table: TableId,
        /// The index to build.
        index: IndexId,
        /// Its name, for the report.
        name: String,
    },
    /// Delete the entries of an index the schema no longer declares.
    ///
    /// Not required for correctness — nothing reads an index the catalog does
    /// not know about — but the keys are dead weight that no row ever reclaims,
    /// because the write path only deletes entries for indexes it can see.
    DropIndex {
        /// The table.
        table: TableId,
        /// The index whose entries are dead.
        index: IndexId,
    },
    /// Record a new schema version against a table whose layout is compatible.
    NoteVersion {
        /// The table.
        table: TableId,
        /// The version the keyspace recorded.
        from: u32,
        /// The version the code declares.
        to: u32,
    },
    /// Adopt a wider layout over rows that were written under a narrower one.
    ///
    /// **Nothing is rewritten and nothing is read.** `decode_row` already
    /// reads a row's own schema version and skips a column that had not been
    /// added when it was written, taking its default — so an old row is
    /// readable under the wider schema and always was. All that changes is the
    /// recorded fingerprint and layout, which is one key.
    ///
    /// This step could not exist before the schema was stored: the fingerprint
    /// alone cannot tell an appended column from a retyped one, and this is
    /// the half of that pair that is safe.
    WidenSchema {
        /// The table.
        table: TableId,
        /// Its name, for the report.
        name: String,
        /// Columns appended since, by name.
        added: Vec<String>,
        /// Columns retired since, by name.
        retired: Vec<String>,
    },
    /// Rewrite a record that predates stored schemas, so the next layout
    /// refusal can name the column rather than the hash.
    ///
    /// **This step exists because the lazy upgrade did not happen without it.**
    /// The design said a table's record is rewritten "the next time it is
    /// migrated, which is already a transaction that writes the record" — true
    /// of a table that has something to do, and false of every other one:
    /// `apply` skips a table with no steps, so a table nobody ever changes
    /// would keep a version 1 record for ever and its refusals would keep
    /// being two hex numbers. A test caught it.
    ///
    /// A step rather than a silent rewrite in `apply`, so `--plan` shows it.
    /// Everything this module does is a step a reader can see beforehand, and
    /// a storage format upgrade is exactly the sort of thing an operator would
    /// rather be told about than have happen.
    RecordSchema {
        /// The table.
        table: TableId,
        /// Its name, for the report.
        name: String,
    },
}

/// One way a stored layout differs from the declared one.
///
/// Every variant is a difference the *fingerprint* detects, and every
/// difference the fingerprint detects is one of these. That correspondence is
/// the point of storing exactly the hash's inputs, and
/// `every_layout_change_moves_the_fingerprint` holds it in both directions
/// rather than leaving it as a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayoutChange {
    /// The table has a different number of columns.
    ColumnCount {
        /// How many the keyspace recorded.
        stored: usize,
        /// How many the code declares.
        current: usize,
    },
    /// A column holds something else, or its nullability, droppedness, scale
    /// or element type moved.
    Column {
        /// Its ordinal, which is what the fingerprint is keyed by.
        ordinal: usize,
        /// The stored name. Not the current one: a rename is not a layout
        /// change, so the two can differ here while nothing is wrong, and the
        /// name a *reader* is looking for is the one the database has.
        name: String,
        /// What it was.
        was: String,
        /// What it is now.
        now: String,
    },
    /// The primary key is over different columns, or in a different order.
    PrimaryKey {
        /// The ordinals recorded.
        stored: Vec<usize>,
        /// The ordinals declared.
        current: Vec<usize>,
    },
    /// The table gained, lost or moved its tenant column.
    TenantColumn {
        /// The ordinal recorded.
        stored: Option<usize>,
        /// The ordinal declared.
        current: Option<usize>,
    },
}

impl core::fmt::Display for LayoutChange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ColumnCount { stored, current } => {
                write!(f, "had {stored} columns and now has {current}")
            }
            Self::Column {
                ordinal,
                name,
                was,
                now,
            } => write!(f, "column {ordinal} `{name}` was {was} and is now {now}"),
            Self::PrimaryKey { stored, current } => {
                write!(f, "the primary key was {stored:?} and is now {current:?}")
            }
            Self::TenantColumn { stored, current } => match (stored, current) {
                (None, Some(at)) => write!(f, "the table gained a tenant column at {at}"),
                (Some(at), None) => write!(f, "the table lost its tenant column at {at}"),
                (a, b) => write!(f, "the tenant column moved from {a:?} to {b:?}"),
            },
        }
    }
}

/// How a column reads in a refusal: its type, and whatever qualifies it.
///
/// Built from codes rather than from `ValueType`, because the stored side has
/// only codes — and a code this binary has no name for is printed as a code
/// rather than guessed at, which is the case a downgrade or a corrupt record
/// produces.
fn describe_column(column: &StoredColumn) -> String {
    let named = |code: u64| {
        ValueType::ALL
            .iter()
            .find(|ty| type_code(**ty) == code)
            .map_or_else(|| format!("type code {code}"), |ty| ty.name().to_owned())
    };
    let mut out = named(column.type_code);
    if column.element_code != 0 {
        out = format!("{out} of {}", named(column.element_code));
    }
    if column.scale != 0 {
        out = format!("{out} at scale {}", column.scale);
    }
    if column.nullable {
        out = format!("nullable {out}");
    }
    if column.dropped {
        out = format!("a dropped {out}");
    }
    out
}

/// Every way `stored` differs from `current`, in the order a reader scans.
///
/// Column count first, because a count difference makes every per-column
/// comparison after it meaningless — and reporting "column 4 changed" when the
/// real answer is "there is no column 4 any more" is worse than reporting
/// nothing.
#[must_use]
pub fn layout_changes(stored: &StoredSchema, current: &StoredSchema) -> Vec<LayoutChange> {
    let mut out = Vec::new();
    if stored.columns.len() != current.columns.len() {
        out.push(LayoutChange::ColumnCount {
            stored: stored.columns.len(),
            current: current.columns.len(),
        });
    }
    {
        // The common prefix is compared even when the counts differ, which the
        // first version of this function did not do — it reported the count
        // and stopped, on the argument that a count difference makes every
        // later comparison meaningless. True of a *general* diff and wrong
        // here: distinguishing "two columns were appended" from "two columns
        // were appended and column 1 was retyped" is the whole question
        // `evolution` asks, and `zip` already stops at the shorter side, so
        // the comparison it does make is between columns that are genuinely at
        // the same ordinal.
        for (ordinal, (was, now)) in stored.columns.iter().zip(&current.columns).enumerate() {
            // Everything *except* the name, which is the asymmetry
            // `StoredSchema` exists for: a rename moves no bytes and must not
            // read as a migration.
            let moved = was.type_code != now.type_code
                || was.nullable != now.nullable
                || was.dropped != now.dropped
                || was.scale != now.scale
                || was.element_code != now.element_code;
            if moved {
                out.push(LayoutChange::Column {
                    ordinal,
                    name: was.name.clone(),
                    was: describe_column(was),
                    now: describe_column(now),
                });
            }
        }
    }
    if stored.primary_key != current.primary_key {
        out.push(LayoutChange::PrimaryKey {
            stored: stored.primary_key.clone(),
            current: current.primary_key.clone(),
        });
    }
    if stored.tenant_column != current.tenant_column {
        out.push(LayoutChange::TenantColumn {
            stored: stored.tenant_column,
            current: current.tenant_column,
        });
    }
    out
}

/// What a layout difference means for rows that are already stored.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Evolution {
    /// Every stored row still reads back correctly. Record the new layout and
    /// move on; nothing is rewritten.
    Compatible {
        /// Columns appended since, by name, for the plan.
        added: Vec<String>,
        /// Columns retired since, by name, for the plan.
        retired: Vec<String>,
    },
    /// At least one stored row would read back as something other than what
    /// was written.
    Incompatible(Vec<LayoutChange>),
}

/// Whether the declared layout can be adopted over rows written at
/// `stored_version`.
///
/// # Why this can exist now and could not before
///
/// `adding_a_nullable_column_is_not_a_migration_but_it_is_a_new_layout` refused
/// an appended column and its comment said why: *"the runner cannot tell an
/// appended column from a retyped one and the safe answer to 'I cannot tell'
/// is no"*, calling it the sharpest limitation of the fingerprint. The stored
/// schema removes the "cannot tell" — an append is a `ColumnCount` with an
/// unchanged prefix and a retype is a `Column`, and those are different
/// answers now rather than the same hash.
///
/// # What makes an append safe
///
/// Nothing in the row format changes. `decode_row` already reads a row's own
/// schema version and skips a column `present_at` says had not been added
/// when it was written, taking its default — so an old row is *already*
/// readable under the wider schema, and always was. The fingerprint was the
/// only thing saying no.
///
/// The conditions are therefore about what the declaration promises, not about
/// the bytes:
///
/// - the common prefix is unchanged, so no existing column is reinterpreted;
/// - every new column is appended, never inserted, so no ordinal shifts;
/// - every new column's `added_in` is **after** the version the stored rows
///   were written at, so `present_at` skips it for all of them;
/// - the same for a retired column's `dropped_in`;
/// - the primary key and the tenant column are untouched, because both decide
///   how a key is laid out rather than how a body is read.
///
/// A column added at or before `stored_version` is refused even though it
/// looks additive: `present_at` would say it *was* present when those rows
/// were written, and the decoder would read a value that is not there.
///
/// That a new column is nullable or defaulted is not checked here, because it
/// is unrepresentable — `added_column` makes it nullable and
/// `added_column_with_default` gives it a default, and there is no third way
/// to declare one. `TableBuilder::check_evolution` says so.
#[must_use]
pub fn evolution(stored: &StoredSchema, table: &TableDef, stored_version: u32) -> Evolution {
    let current = StoredSchema::of(table);
    let changes = layout_changes(stored, &current);
    let widened = current.columns.len() > stored.columns.len();

    // Anything that is not the column count is a reinterpretation of a column
    // that already exists, or of the key. Either is a no.
    if changes
        .iter()
        .any(|change| !matches!(change, LayoutChange::ColumnCount { .. }))
        || (!changes.is_empty() && !widened)
    {
        return Evolution::Incompatible(changes);
    }

    let mut added = Vec::new();
    for column in table.columns().iter().skip(stored.columns.len()) {
        if column.added_in() <= stored_version {
            // Looks additive and is not: `present_at` would say this column
            // was already there when those rows were written, and the decoder
            // would read a value nobody wrote.
            return Evolution::Incompatible(changes);
        }
        added.push(column.name().to_owned());
    }

    // A retirement is the other half of the same mechanism, and reaches here
    // only when the prefix is otherwise unchanged — `is_dropped` is part of
    // the per-column comparison, so a drop shows up as a `Column` change and
    // is refused above. Collected for the plan's message rather than to decide
    // anything, which is why this loop cannot make the answer incompatible.
    let retired = table
        .columns()
        .iter()
        .take(stored.columns.len())
        .zip(&stored.columns)
        .filter(|(now, was)| now.is_dropped() && !was.dropped)
        .map(|(now, _)| now.name().to_owned())
        .collect();

    Evolution::Compatible { added, retired }
}

/// Something the runner will not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The layout changed under existing rows.
    LayoutChanged {
        /// The table.
        table: String,
        /// What the keyspace recorded.
        stored: u64,
        /// What the code declares.
        current: u64,
        /// Which columns moved, when the record was written by a binary that
        /// stored the schema.
        ///
        /// Empty means one of two different things and the message says
        /// which: either the record predates stored schemas — permanently
        /// possible, see [`TableState::schema`] — or the two schemas agree
        /// and only the hash does not, which is a contradiction worth naming
        /// rather than rendering as silence.
        changes: Vec<LayoutChange>,
    },
    /// The state record is in a format this binary does not know.
    UnknownFormat {
        /// The table.
        table: String,
        /// The format version found.
        format: u64,
    },
    /// The state record did not decode.
    Corrupt {
        /// The table.
        table: String,
        /// What went wrong.
        detail: String,
    },
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LayoutChanged {
                table,
                stored,
                current,
                changes,
            } if !changes.is_empty() => {
                // The whole point of storing the schema: two hex numbers and a
                // list of six things one of which moved, replaced by the one
                // that did. Every line here is a difference the stored schema
                // actually holds, so the message cannot claim more than it
                // knows.
                write!(
                    f,
                    "`{table}`: the column layout changed under rows that are already stored"
                )?;
                for change in changes {
                    write!(f, "; {change}")?;
                }
                write!(
                    f,
                    ". Renames, CHECKs and foreign keys are not covered and are not this."
                )
            }
            Self::LayoutChanged {
                table,
                stored,
                current,
                ..
            } => write!(
                f,
                "`{table}`: the column layout changed under rows that are already stored \
                 (recorded {stored:#x}, code declares {current:#x}). This record predates stored \
                 schemas — or holds one that agrees with the code, which would mean the record \
                 is inconsistent with its own hash — so all that can be said is that the \
                 fingerprint covers the number of columns, each one's type and nullability, \
                 which columns are dropped, the primary key and the tenant column, and one of \
                 those is not what it was. Migrating this table once rewrites the record and \
                 the next such refusal will name the column. Renames, CHECKs and foreign keys \
                 are not covered and are not this."
            ),
            Self::UnknownFormat { table, format } => write!(
                f,
                "`{table}`: schema state is in format {format}, which this binary cannot read. \
                 It was written by a newer version"
            ),
            Self::Corrupt { table, detail } => {
                write!(f, "`{table}`: schema state did not decode: {detail}")
            }
        }
    }
}

/// What a migration would do, and what it refuses to.
///
/// Separate from applying it so a deployment can look first. A plan with any
/// refusal is not applied at all, rather than applied up to the refusal: half a
/// migration is a state no one designed.
#[derive(Debug, Clone, Default)]
pub struct MigrationPlan {
    /// What would happen, in the order it would happen.
    pub steps: Vec<Step>,
    /// What will not, and why.
    pub refusals: Vec<Refusal>,
}

impl MigrationPlan {
    /// Whether there is anything to do and nothing in the way.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.refusals.is_empty()
    }

    /// Whether applying this plan would refuse.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        !self.refusals.is_empty()
    }

    /// The refusals, joined into one message.
    #[must_use]
    pub fn why_blocked(&self) -> String {
        self.refusals
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// What a migration did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The steps that ran.
    pub steps: Vec<Step>,
    /// How many index entries were written, per `BuildIndex` step, in order.
    pub entries_written: Vec<usize>,
}

/// The layout fingerprint of a table. See the module docs for what it covers.
#[must_use]
pub fn fingerprint(table: &TableDef) -> u64 {
    // FNV-1a, 64-bit. Not a cryptographic choice: nothing here defends against
    // a chosen collision, because whoever could choose one could edit the
    // schema. What it needs is to change when any input byte does, which every
    // reasonable hash gives, and to be stable across builds and platforms —
    // which `DefaultHasher` explicitly does not promise and this does.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let byte = |b: u8, hash: &mut u64| {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    let number = |n: u64, hash: &mut u64| {
        for b in n.to_be_bytes() {
            byte(b, hash);
        }
    };

    number(table.columns().len() as u64, &mut hash);
    for column in table.columns() {
        // The type discriminant rather than the name: a rename of the *variant*
        // upstream would change a name-based hash and invalidate every deployed
        // database for no reason on disk.
        number(type_code(column.value_type()), &mut hash);
        byte(u8::from(column.is_nullable()), &mut hash);
        byte(u8::from(column.is_dropped()), &mut hash);
        // A decimal's scale decides what its stored units *mean*, so changing
        // it reinterprets every row already written — units 1250 read as 12.50
        // at scale 2 and as 1.250 at scale 3. That is the same kind of change
        // as retyping a column, and belongs in the fingerprint for the same
        // reason. `unwrap_or(0)` covers every other type, which has no scale.
        byte(column.scale().unwrap_or(0), &mut hash);
        // An array's element type decides how every element of every value in
        // the column is read, so changing it reinterprets stored rows exactly
        // as a decimal's scale does — and for exactly that reason it is hashed
        // here rather than smuggled into the column's own type code. `0` is
        // the "no element type" case and cannot collide with a real one,
        // because `type_code` gives every type a non-zero code and
        // `every_type_has_its_own_non_zero_code` holds it to that.
        number(column.element_type().map_or(0, type_code), &mut hash);
    }
    number(table.primary_key().len() as u64, &mut hash);
    for ordinal in table.primary_key() {
        number(ordinal.0 as u64, &mut hash);
    }
    // Tenant scoping decides whether index keys carry a tenant prefix, so two
    // tables with identical columns lay their index out differently depending
    // on it.
    //
    // The `byte(0)`/`byte(1)` tag is redundant *today* and is kept on purpose.
    // A mutation replacing it with nothing survives the suite, because `number`
    // always writes eight bytes and so `Some(0)` can never hash like `None`
    // anyway. It is here against the day `number` becomes a varint and drops
    // leading zeros, at which point `Some(Ordinal(0))` would write nothing and
    // collide with `None` — a collision that would silently accept a table
    // gaining or losing tenant scoping. Two bytes to remove a dependency
    // between two functions that have no other reason to know about each other.
    match table.tenant_column() {
        None => byte(0, &mut hash),
        Some(ordinal) => {
            byte(1, &mut hash);
            number(ordinal.0 as u64, &mut hash);
        }
    }
    hash
}

/// A stable code per value type.
///
/// Written out rather than `as u64` on the enum, so that inserting a variant
/// upstream does not renumber the ones after it and invalidate every stored
/// fingerprint. That is exactly the failure this whole module is about.
const fn type_code(ty: ValueType) -> u64 {
    match ty {
        ValueType::Bool => 1,
        ValueType::Bytes => 2,
        ValueType::Str => 3,
        ValueType::I64 => 4,
        ValueType::U64 => 5,
        ValueType::F64 => 6,
        ValueType::Uuid => 7,
        ValueType::Vector => 8,
        // Added when `Value::Decimal` was. The `_` arm below is why this line
        // is here rather than forgotten: a new type silently taking code 0
        // would make a decimal column fingerprint identically to a vector one
        // — the arm keeps that from being a *wrong* answer, and this line
        // keeps it from being the answer at all.
        ValueType::Decimal => 9,
        // Added when `Value::Array` was, for the reason above.
        ValueType::Array => 10,
        // `ValueType` is `#[non_exhaustive]`, so a new variant compiles here
        // rather than failing. It must not silently take an existing code: a
        // column of the new type would then fingerprint identically to one of
        // whatever type shares the code, which is the exact class of mistake
        // this function is written out by hand to avoid. 0 is reserved for
        // "unknown", so every unrecognised type collides with every other one
        // and the fingerprint is at worst too *quick* to say a layout changed.
        _ => 0,
    }
}

/// The bytes of a state record.
///
/// `pub` for one reason, stated so it is not mistaken for an API: a test has
/// to be able to write a record *this binary would not write* — specifically a
/// version 1 one — to exercise the path where an older binary's record is read
/// by this one. Building those bytes by hand in the test would be a second
/// copy of the format, which is the thing most likely to drift and the least
/// likely to be noticed when it does.
#[must_use]
pub fn encode_state(state: &TableState) -> Vec<u8> {
    let mut ids = Vec::with_capacity(state.built.len() * 4);
    for id in &state.built {
        ids.extend_from_slice(&id.0.to_be_bytes());
    }
    // The V1 prefix, always, followed by the schema blob when there is one.
    // Appending an encoded tuple to an encoded tuple is the composition
    // `decode_prefix` documents as safe, and it is what lets `decode_state`
    // read the format *before* deciding how much more there is — rather than
    // guessing a shape and falling back, which would report a V2 record's
    // error for a corrupt V1 one.
    let mut out = slate_tuple::encode(&[
        Value::U64(if state.schema.is_some() {
            STATE_FORMAT_V2
        } else {
            STATE_FORMAT_V1
        }),
        Value::U64(u64::from(state.schema_version)),
        Value::U64(state.fingerprint),
        Value::Bytes(ids.into()),
    ]);
    if let Some(schema) = &state.schema {
        out.extend_from_slice(&encode_schema(schema));
    }
    out
}

/// The nested schema blob: a version, a column count, then the columns.
///
/// Nested rather than flattened into the outer record because the outer decode
/// is a fixed type list and a variable column count does not fit one. One
/// record rather than a second key beside it: both would be written inside the
/// migration's transaction so atomicity is not the argument — the argument is
/// that two records are two things that can disagree, silently.
fn encode_schema(schema: &StoredSchema) -> Vec<u8> {
    let mut out = slate_tuple::encode(&[
        Value::U64(SCHEMA_FORMAT_V1),
        Value::U64(schema.columns.len() as u64),
    ]);
    for column in &schema.columns {
        out.extend_from_slice(&slate_tuple::encode(&[
            Value::Str(column.name.clone()),
            Value::U64(column.type_code),
            // One packed byte rather than two booleans, because `Value` has no
            // bool-sized encoding and two `U64`s would be sixteen bytes to say
            // two bits.
            Value::U64(u64::from(column.nullable) | (u64::from(column.dropped) << 1)),
            Value::U64(u64::from(column.scale)),
            Value::U64(column.element_code),
        ]));
    }
    out.extend_from_slice(&slate_tuple::encode(&[Value::U64(
        schema.primary_key.len() as u64,
    )]));
    for ordinal in &schema.primary_key {
        out.extend_from_slice(&slate_tuple::encode(&[Value::U64(*ordinal as u64)]));
    }
    // Tagged rather than a sentinel ordinal, for the reason the fingerprint
    // tags it: there is no ordinal that cannot be a real one.
    match schema.tenant_column {
        None => out.extend_from_slice(&slate_tuple::encode(&[Value::U64(0)])),
        Some(ordinal) => out.extend_from_slice(&slate_tuple::encode(&[
            Value::U64(1),
            Value::U64(ordinal as u64),
        ])),
    }
    out
}

/// Read a schema blob, or say what is wrong with it.
fn decode_schema(bytes: &[u8]) -> core::result::Result<StoredSchema, String> {
    let read = |buf: &[u8],
                types: &[ValueType]|
     -> core::result::Result<(Vec<Value>, usize), String> {
        let (values, rest) = slate_tuple::decode_prefix(buf, types).map_err(|e| e.to_string())?;
        Ok((values, buf.len() - rest.len()))
    };
    let mut at = 0usize;
    let slice = |at: usize| bytes.get(at..).ok_or_else(|| "truncated".to_owned());

    let (header, used) = read(slice(at)?, &[ValueType::U64, ValueType::U64])?;
    at += used;
    let [Value::U64(version), Value::U64(count)] = &header[..] else {
        return Err("the schema header is not two numbers".to_owned());
    };
    if *version != SCHEMA_FORMAT_V1 {
        return Err(format!(
            "schema blob version {version} is not one this binary reads"
        ));
    }

    let mut columns = Vec::new();
    for index in 0..*count {
        let (values, used) = read(
            slice(at)?,
            &[
                ValueType::Str,
                ValueType::U64,
                ValueType::U64,
                ValueType::U64,
                ValueType::U64,
            ],
        )?;
        at += used;
        let [
            Value::Str(name),
            Value::U64(type_code),
            Value::U64(flags),
            Value::U64(scale),
            Value::U64(element_code),
        ] = &values[..]
        else {
            return Err(format!("column {index} is not of the expected shape"));
        };
        columns.push(StoredColumn {
            name: name.clone(),
            type_code: *type_code,
            nullable: flags & 1 == 1,
            dropped: flags & 2 == 2,
            scale: u8::try_from(*scale)
                .map_err(|_| format!("column {index} scale out of range"))?,
            element_code: *element_code,
        });
    }

    let (key_count, used) = read(slice(at)?, &[ValueType::U64])?;
    at += used;
    let [Value::U64(key_len)] = &key_count[..] else {
        return Err("the primary key length is not a number".to_owned());
    };
    let mut primary_key = Vec::new();
    for _ in 0..*key_len {
        let (ordinal, used) = read(slice(at)?, &[ValueType::U64])?;
        at += used;
        let [Value::U64(o)] = &ordinal[..] else {
            return Err("a primary key ordinal is not a number".to_owned());
        };
        primary_key.push(usize::try_from(*o).map_err(|_| "ordinal out of range".to_owned())?);
    }

    let (tag, used) = read(slice(at)?, &[ValueType::U64])?;
    at += used;
    let [Value::U64(tagged)] = &tag[..] else {
        return Err("the tenant tag is not a number".to_owned());
    };
    let tenant_column = if *tagged == 0 {
        None
    } else {
        let (ordinal, used) = read(slice(at)?, &[ValueType::U64])?;
        at += used;
        let [Value::U64(o)] = &ordinal[..] else {
            return Err("the tenant ordinal is not a number".to_owned());
        };
        Some(usize::try_from(*o).map_err(|_| "ordinal out of range".to_owned())?)
    };

    if at != bytes.len() {
        return Err(format!("{} bytes after the schema", bytes.len() - at));
    }
    Ok(StoredSchema {
        columns,
        primary_key,
        tenant_column,
    })
}

/// Decode a state record, or say why not.
///
/// Returns `Err(Refusal)` rather than a kernel error because every way this
/// fails is a thing the plan should report beside the others, not an
/// exception that hides the rest of the tables.
fn decode_state(table: &TableDef, bytes: &[u8]) -> core::result::Result<TableState, Refusal> {
    // `decode_prefix` rather than `decode`: a V2 record is the V1 four
    // followed by the schema blob, and reading the prefix lets the *format*
    // decide how much more to expect. Trying one shape and falling back to the
    // other would work and would report a V2 parse error for a corrupt V1
    // record, which is a worse message for the more likely failure.
    let (values, rest) = slate_tuple::decode_prefix(
        bytes,
        &[
            ValueType::U64,
            ValueType::U64,
            ValueType::U64,
            ValueType::Bytes,
        ],
    )
    .map_err(|error| Refusal::Corrupt {
        table: table.name().to_owned(),
        detail: error.to_string(),
    })?;

    let bad = |detail: &str| Refusal::Corrupt {
        table: table.name().to_owned(),
        detail: detail.to_owned(),
    };
    let [
        Value::U64(format),
        Value::U64(version),
        Value::U64(fingerprint),
        Value::Bytes(ids),
    ] = &values[..]
    else {
        return Err(bad("not four values of the expected types"));
    };
    let schema = match *format {
        STATE_FORMAT_V1 => {
            if !rest.is_empty() {
                return Err(bad("a version 1 record has bytes after it"));
            }
            None
        }
        STATE_FORMAT_V2 => Some(decode_schema(rest).map_err(|detail| Refusal::Corrupt {
            table: table.name().to_owned(),
            detail,
        })?),
        other => {
            return Err(Refusal::UnknownFormat {
                table: table.name().to_owned(),
                format: other,
            });
        }
    };
    // `split_first_chunk` rather than `chunks_exact`: it hands back a `[u8; 4]`
    // by value, so there is no copy into a scratch array and no slice index,
    // and what is left over at the end *is* the remainder — which turns the
    // trailing-bytes check into `is_empty` instead of arithmetic that has to
    // agree with the loop. (`chunks_exact` here is also the lint CI's clippy
    // has and this container's does not; see the note in CLAUDE.md.)
    let mut built = Vec::with_capacity(ids.len() / 4);
    let mut rest: &[u8] = ids;
    while let Some((four, tail)) = rest.split_first_chunk::<4>() {
        built.push(IndexId(u32::from_be_bytes(*four)));
        rest = tail;
    }
    if !rest.is_empty() {
        return Err(bad("the built-index list is not a whole number of ids"));
    }
    Ok(TableState {
        schema_version: u32::try_from(*version).map_err(|_| bad("schema version out of range"))?,
        fingerprint: *fingerprint,
        built,
        schema,
    })
}

/// Read the recorded state of every table in `catalog`.
///
/// A table with no record is absent from the result rather than defaulted, so a
/// caller can tell "never migrated" from "migrated, with nothing built".
///
/// # Errors
/// If the store cannot be read.
pub async fn stored_state<S: KvStore + ?Sized>(
    store: &S,
    catalog: &Catalog,
) -> Result<Vec<(TableId, core::result::Result<TableState, Refusal>)>> {
    let txn = store.begin().await?;
    let out = stored_state_of(txn.as_ref(), catalog).await;
    txn.rollback();
    out
}

/// [`stored_state`] over a snapshot rather than a store.
///
/// Reading the state needs `get` and nothing else, so the read half of this
/// module works on anything that can be read — including a **read replica**,
/// which has a `KvSnapshot` and no way to open a transaction at all. That
/// matters because a replica serves queries through the same indexes: a node
/// that holds no writer still has to be able to tell that what it is reading
/// has not been migrated, even though it can do nothing about it.
///
/// # Errors
/// If the snapshot cannot be read.
pub async fn stored_state_of(
    snapshot: &(impl KvSnapshot + ?Sized),
    catalog: &Catalog,
) -> Result<Vec<(TableId, core::result::Result<TableState, Refusal>)>> {
    let mut out = Vec::new();
    for table in catalog.tables() {
        if let Some(bytes) = snapshot.get(&keys::meta_key(table.id())).await? {
            out.push((table.id(), decode_state(table, &bytes)));
        }
    }
    Ok(out)
}

/// What migrating `catalog` against `store` would do.
///
/// # Errors
/// If the store cannot be read.
pub async fn plan<S: KvStore + ?Sized>(store: &S, catalog: &Catalog) -> Result<MigrationPlan> {
    let txn = store.begin().await?;
    let out = plan_of(txn.as_ref(), catalog).await;
    txn.rollback();
    out
}

/// [`plan`] over a snapshot rather than a store. See [`stored_state_of`].
///
/// # Errors
/// If the snapshot cannot be read.
pub async fn plan_of(
    snapshot: &(impl KvSnapshot + ?Sized),
    catalog: &Catalog,
) -> Result<MigrationPlan> {
    let mut out = MigrationPlan::default();
    let recorded = stored_state_of(snapshot, catalog).await?;

    for table in catalog.tables() {
        let current = fingerprint(table);
        let state = recorded
            .iter()
            .find(|(id, _)| *id == table.id())
            .map(|(_, state)| state);

        let Some(state) = state else {
            // Never migrated. Every index needs building — which for an empty
            // table writes nothing, so the first deploy of a new table is
            // exactly as cheap as it should be.
            out.steps.push(Step::Register {
                table: table.id(),
                name: table.name().to_owned(),
            });
            for index in table.indexes() {
                out.steps.push(Step::BuildIndex {
                    table: table.id(),
                    index: index.id(),
                    name: index.name().to_owned(),
                });
            }
            continue;
        };

        let state = match state {
            Err(refusal) => {
                out.refusals.push(refusal.clone());
                continue;
            }
            Ok(state) => state,
        };

        if state.fingerprint != current {
            // An appended column is now told apart from a retyped one, which
            // is what the stored schema bought. Before it, this was a refusal
            // in every case and the test that pinned that said so: "the runner
            // cannot tell an appended column from a retyped one and the safe
            // answer to 'I cannot tell' is no". The answer is only no when it
            // is still cannot-tell — a record with no stored schema — or when
            // the difference genuinely reinterprets a stored row.
            let verdict = state
                .schema
                .as_ref()
                .map(|schema| evolution(schema, table, state.schema_version));
            match verdict {
                Some(Evolution::Compatible { added, retired }) => {
                    out.steps.push(Step::WidenSchema {
                        table: table.id(),
                        name: table.name().to_owned(),
                        added,
                        retired,
                    });
                }
                Some(Evolution::Incompatible(changes)) => {
                    out.refusals.push(Refusal::LayoutChanged {
                        table: table.name().to_owned(),
                        stored: state.fingerprint,
                        current,
                        changes,
                    });
                    continue;
                }
                None => {
                    out.refusals.push(Refusal::LayoutChanged {
                        table: table.name().to_owned(),
                        stored: state.fingerprint,
                        current,
                        changes: Vec::new(),
                    });
                    continue;
                }
            }
        }

        for index in table.indexes() {
            if !state.built.contains(&index.id()) {
                out.steps.push(Step::BuildIndex {
                    table: table.id(),
                    index: index.id(),
                    name: index.name().to_owned(),
                });
            }
        }
        for built in &state.built {
            if table.index(*built).is_none() {
                out.steps.push(Step::DropIndex {
                    table: table.id(),
                    index: *built,
                });
            }
        }
        if state.schema_version != table.schema_version() {
            out.steps.push(Step::NoteVersion {
                table: table.id(),
                from: state.schema_version,
                to: table.schema_version(),
            });
        }
        if state.schema.is_none() {
            out.steps.push(Step::RecordSchema {
                table: table.id(),
                name: table.name().to_owned(),
            });
        }
    }
    Ok(out)
}

/// Apply a plan.
///
/// Refuses the whole plan if it has any refusal, rather than running the steps
/// before it: a migration that stops half way leaves a state nobody designed,
/// and the operator who has to reason about it has less information than the
/// one who was simply told no.
///
/// # Errors
/// If the plan is blocked, if the store fails, or if building a unique index
/// finds two rows that collide.
pub async fn apply<S: KvStore + ?Sized>(
    store: &S,
    catalog: &Catalog,
    plan: &MigrationPlan,
) -> Result<Report> {
    if plan.is_blocked() {
        return Err(KernelError::MigrationRefused {
            reason: plan.why_blocked(),
        });
    }

    let mut report = Report::default();
    for step in &plan.steps {
        match step {
            // None of the three does work of its own: the state write at the
            // end of `apply` is what records a registration, a version and a
            // schema, and it happens for every table the plan touches.
            Step::Register { .. }
            | Step::NoteVersion { .. }
            | Step::RecordSchema { .. }
            | Step::WidenSchema { .. } => {}
            Step::BuildIndex { table, index, .. } => {
                let (table, index) = resolve(catalog, *table, *index)?;
                report
                    .entries_written
                    .push(build(store, table, index).await?);
            }
            Step::DropIndex { index, .. } => {
                drop_entries(store, *index).await?;
            }
        }
        report.steps.push(step.clone());
    }

    // State is written last, per table, and only once every step for that table
    // has committed. An interrupted migration therefore leaves the state
    // unchanged and the next run repeats the work — which is safe because
    // writing an index entry that already exists writes the same bytes.
    let txn = store.begin().await?;
    for table in catalog.tables() {
        if !plan.steps.iter().any(|step| touches(step, table.id())) {
            continue;
        }
        txn.put(
            keys::meta_key(table.id()),
            encode_state(&TableState {
                schema_version: table.schema_version(),
                fingerprint: fingerprint(table),
                built: table.indexes().iter().map(IndexDef::id).collect(),
                // Every write is a version 2 record. This is the whole of the
                // lazy upgrade: a table gets its stored schema the next time
                // it is migrated, which is already the transaction that
                // rewrites this key.
                schema: Some(StoredSchema::of(table)),
            }),
        )?;
    }
    txn.commit().await?;
    Ok(report)
}

/// Plan and apply in one call.
///
/// # Errors
/// As [`plan`] and [`apply`].
pub async fn migrate<S: KvStore + ?Sized>(store: &S, catalog: &Catalog) -> Result<Report> {
    let plan = plan(store, catalog).await?;
    apply(store, catalog, &plan).await
}

/// Refuse unless every table in `catalog` is migrated.
///
/// This is the guard, and it belongs at startup rather than in the query path.
/// A per-query check would have to read the state key on every read — or cache
/// it and be wrong after a migration — to defend against a condition that can
/// only be introduced by deploying a binary. Refusing to start says the same
/// thing once, before a single wrong answer is served.
///
/// # Errors
/// If the store cannot be read, or if anything is outstanding.
pub async fn verify<S: KvStore + ?Sized>(store: &S, catalog: &Catalog) -> Result<()> {
    let txn = store.begin().await?;
    let out = verify_of(txn.as_ref(), catalog).await;
    txn.rollback();
    out
}

/// [`verify`] over a snapshot rather than a store. See [`stored_state_of`].
///
/// # Errors
/// If the snapshot cannot be read, or if anything is outstanding.
pub async fn verify_of(snapshot: &(impl KvSnapshot + ?Sized), catalog: &Catalog) -> Result<()> {
    let plan = plan_of(snapshot, catalog).await?;
    if plan.is_blocked() {
        return Err(KernelError::MigrationRefused {
            reason: plan.why_blocked(),
        });
    }
    let pending: Vec<String> = plan
        .steps
        .iter()
        .filter_map(|step| match step {
            Step::BuildIndex { name, .. } => Some(format!("index `{name}` is not built")),
            Step::Register { name, .. } => Some(format!("table `{name}` is not registered")),
            _ => None,
        })
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    Err(KernelError::MigrationRefused {
        reason: format!(
            "the schema has unapplied migrations, and querying through an index that was never \
             built returns no rows rather than an error: {}. Run `migrate` first",
            pending.join(", ")
        ),
    })
}

fn touches(step: &Step, table: TableId) -> bool {
    match step {
        Step::Register { table: t, .. }
        | Step::BuildIndex { table: t, .. }
        | Step::DropIndex { table: t, .. }
        | Step::NoteVersion { table: t, .. }
        | Step::RecordSchema { table: t, .. }
        | Step::WidenSchema { table: t, .. } => *t == table,
    }
}

fn resolve(catalog: &Catalog, table: TableId, index: IndexId) -> Result<(&TableDef, &IndexDef)> {
    let table = catalog
        .table(table)
        .ok_or_else(|| KernelError::MigrationRefused {
            reason: format!("no table {} in the catalog", table.0),
        })?;
    let index = table
        .index(index)
        .ok_or_else(|| KernelError::MigrationRefused {
            reason: format!("no index {} on `{}`", index.0, table.name()),
        })?;
    Ok((table, index))
}

/// Write every existing row's entry for `index`.
///
/// Returns the number of entries written, which is not the number of rows: a
/// partial index admits only some of them.
async fn build<S: KvStore + ?Sized>(
    store: &S,
    table: &TableDef,
    index: &IndexDef,
) -> Result<usize> {
    let prefix = keys::table_prefix(table);
    let mut written = 0usize;
    // The key to resume after. A backfill of a large table is several
    // transactions, and each one picks up where the last committed.
    let mut after: Option<Vec<u8>> = None;

    loop {
        let txn = store.begin().await?;
        let scanned;
        let mut range = KeyRange::prefix(&prefix);
        if let Some(after) = &after {
            // Exclusive, so the batch boundary does not re-read the row it
            // stopped on. Re-reading it would be harmless — the same entry gets
            // the same bytes — but a batch that always starts where it ended
            // never advances.
            range = range.intersect(KeyRange::new(
                core::ops::Bound::Excluded(after.clone()),
                core::ops::Bound::Unbounded,
            ));
        }
        // Collected first, written after the cursor closes. The unique check
        // below is a read on the same transaction, and a read while a cursor
        // over it is open is a borrow that outlives the commit — so the scan
        // and the writes are two phases rather than one interleaved loop.
        let mut batch: Vec<(Vec<Value>, keys::IndexEntry)> = Vec::new();
        let mut last: Option<Vec<u8>> = None;
        {
            let mut cursor = txn.scan(range, ScanOrder::Ascending).await?;
            let mut seen = 0usize;
            while let Some(pair) = cursor.next().await? {
                let primary_key = keys::decode_row_key(table, &pair.key)?;
                let row = decode_row(table, &primary_key, &pair.value)?;
                last = Some(pair.key.to_vec());
                seen += 1;
                // A partial index admits only some rows; the rest are walked
                // past and counted, because the batch bound is about how much
                // of the table one transaction reads, not how much it writes.
                if index.admits(&row) {
                    let entry =
                        keys::index_entry(table, index, &index.key_values(&row), &primary_key);
                    batch.push((primary_key, entry));
                }
                if seen >= BACKFILL_BATCH {
                    break;
                }
            }
            scanned = seen;
        }

        for (primary_key, entry) in batch {
            // A unique index being built over data that is not unique is the
            // one thing a backfill can be asked for that is impossible. Caught
            // rather than resolved: a unique entry's key omits the primary key,
            // so the second row writes the *same* key and would silently
            // replace the first row's pointer — leaving an index that names one
            // row and hides another, which is worse than the missing index this
            // whole module exists to fix.
            if entry.enforces_uniqueness
                && let Some(existing) = txn.get(&entry.key).await?
                && slate_tuple::decode(&existing, &table.primary_key_types())? != primary_key
            {
                txn.rollback();
                return Err(KernelError::UniqueViolation {
                    table: table.name().to_owned(),
                    index: index.name().to_owned(),
                });
            }
            txn.put(entry.key, entry.value)?;
            written += 1;
        }
        txn.commit().await?;

        // The scan ran out before the batch filled, so there is nothing left.
        if scanned < BACKFILL_BATCH {
            return Ok(written);
        }
        after = last;
    }
}

/// Delete every entry of an index the catalog no longer declares.
async fn drop_entries<S: KvStore + ?Sized>(store: &S, index: IndexId) -> Result<()> {
    let prefix = keys::index_prefix_of(index);
    loop {
        let txn = store.begin().await?;
        let mut dead: Vec<Vec<u8>> = Vec::new();
        {
            let mut cursor = txn
                .scan(KeyRange::prefix(&prefix), ScanOrder::Ascending)
                .await?;
            while let Some(pair) = cursor.next().await? {
                dead.push(pair.key.to_vec());
                if dead.len() >= BACKFILL_BATCH {
                    break;
                }
            }
        }
        let batch = dead.len();
        for key in dead {
            txn.delete(key)?;
        }
        txn.commit().await?;
        if batch < BACKFILL_BATCH {
            return Ok(());
        }
    }
}

#[cfg(test)]
// A guard test that reports a collision by name, which reads better as a
// `panic!` than as an `assert!` over an `Option`.
#[allow(clippy::panic)]
mod type_code_tests {
    use super::type_code;
    use slate_tuple::ValueType;

    /// Every type has a code of its own, and none of them is the unknown one.
    ///
    /// The distinctness half is also checked through `fingerprint` by
    /// `every_value_type_fingerprints_apart` in `tests/migrations.rs`, which is
    /// the property a caller sees. This one exists because that test cannot see
    /// the other half: with a single type falling through to `_ => 0`, code 0
    /// is *unique* and no two fingerprints collide, so the integration test
    /// passes and the trap is armed — the harm arrives with the second
    /// fallthrough, by which time the first is deployed. A mutation deleting
    /// `ValueType::Array`'s arm survived it, which is how this was found.
    ///
    /// `type_code` is private, so this has to live beside it rather than with
    /// the other fingerprint tests.
    #[test]
    fn every_type_has_its_own_non_zero_code() {
        let mut seen: std::collections::BTreeMap<u64, ValueType> =
            std::collections::BTreeMap::new();
        for kind in ValueType::ALL {
            let code = type_code(kind);
            assert_ne!(
                code, 0,
                "{kind} falls through to the unknown code; give it an arm in \
                 type_code, and pick a number no other type uses"
            );
            if let Some(clash) = seen.insert(code, kind) {
                panic!("{kind} and {clash} share fingerprint code {code}");
            }
        }
    }
}
