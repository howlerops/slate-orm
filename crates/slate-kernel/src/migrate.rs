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
            } => write!(
                f,
                "`{table}`: the column layout changed under rows that are already stored \
                 (recorded {stored:#x}, code declares {current:#x}). The fingerprint covers the \
                 number of columns, each one's type and nullability, which columns are dropped, \
                 the primary key and the tenant column — one of those is not what it was. \
                 Renames, CHECKs and foreign keys are not covered and are not this."
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

fn encode_state(state: &TableState) -> Vec<u8> {
    let mut ids = Vec::with_capacity(state.built.len() * 4);
    for id in &state.built {
        ids.extend_from_slice(&id.0.to_be_bytes());
    }
    slate_tuple::encode(&[
        Value::U64(STATE_FORMAT_V1),
        Value::U64(u64::from(state.schema_version)),
        Value::U64(state.fingerprint),
        Value::Bytes(ids.into()),
    ])
}

/// Decode a state record, or say why not.
///
/// Returns `Err(Refusal)` rather than a kernel error because every way this
/// fails is a thing the plan should report beside the others, not an
/// exception that hides the rest of the tables.
fn decode_state(table: &TableDef, bytes: &[u8]) -> core::result::Result<TableState, Refusal> {
    let values = slate_tuple::decode(
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
    if *format != STATE_FORMAT_V1 {
        return Err(Refusal::UnknownFormat {
            table: table.name().to_owned(),
            format: *format,
        });
    }
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
            out.refusals.push(Refusal::LayoutChanged {
                table: table.name().to_owned(),
                stored: state.fingerprint,
                current,
            });
            continue;
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
            Step::Register { .. } | Step::NoteVersion { .. } => {}
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
        | Step::NoteVersion { table: t, .. } => *t == table,
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
