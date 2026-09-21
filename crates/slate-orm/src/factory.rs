//! Generate rows for a table, from the table.
//!
//! The other half of seeding. `insert_records` under
//! `SecurityContext::superuser()` is the entry point — `tests/seeding.rs` is
//! that written down — and everything it needs is rows, which until now every
//! caller wrote out by hand. A thousand-row fixture was a thousand lines of
//! TOML or a thousand struct literals.
//!
//! A [`Factory`] borrows a [`TableDef`] and produces [`Row`]s from it. The
//! table already says everything the generator needs: each column's type, its
//! nullability, its default, whether the store writes it, which columns are
//! the primary key, which participate in a unique index, and what the `CHECK`
//! constraints are.
//!
//! ```no_run
//! # use slate_orm::{Factory, TableDef, seeding_context};
//! # use slate_kernel::RecordTransaction;
//! # async fn seed(
//! #     txn: &RecordTransaction<'_>,
//! #     table: &TableDef,
//! # ) -> Result<(), Box<dyn std::error::Error>> {
//! let rows = Factory::new(table).seed(7).rows(1_000)?;
//! txn.insert_many(&seeding_context(), table, &rows).await?;
//! # Ok(()) }
//! ```
//!
//! # A value is a function of (seed, column, row index), not a stream
//!
//! The obvious implementation is a seeded PRNG drawn from left to right, and
//! it was rejected. Under a stream, `rows(10)` and `rows(1_000)` agree on
//! nothing, because generating row 0 of the second run consumes the same
//! draws but the run continues; worse, adding a column to the table shifts
//! every later value in every later row, so a fixture changes wholesale on a
//! change that has nothing to do with it.
//!
//! Here each value is a pure function of the seed, the column's ordinal and
//! the row index. So `rows(1_000)[7]` and `row(7)` are the same row — asserted
//! in `tests/factory.rs` rather than claimed — the first ten rows of a
//! thousand are the first ten rows of ten, and a new column changes only its
//! own column.
//!
//! The mixing function is written out here rather than taken from `rand`.
//! `rand`'s `StdRng` documents that its algorithm may change between minor
//! versions, which for a *fixture* is the whole problem: a test asserting
//! against generated data would break on a dependency bump with no change in
//! this repository. Splitmix64 is eleven lines, is fixed forever because it is
//! written down here, and needs no dependency.
//!
//! # What it refuses rather than guesses
//!
//! A factory that quietly produces rows the store will reject is worse than
//! one that will not produce them: the caller sees a `CheckViolation` from
//! deep in a batch insert and has no idea which row or which generator caused
//! it. So the refusals happen at generation time, with the column named and
//! the call that fixes it named too:
//!
//! - **A `CHECK` the generated rows fail.** The factory cannot read a
//!   predicate, and it does not pretend to — it runs every check over every
//!   row it made and reports the first failure with the check's name.
//! - **A vector column.** A vector's dimension is not in the schema, so a
//!   generated one would be whatever width the factory picked, and a table of
//!   vectors that are all the wrong width is a fixture that looks fine until
//!   the first similarity search.
//! - **A `bool` in the primary key or a unique index.** Two rows exhaust it.
//!
//! Each is [`Factory::set`] or [`Factory::cycle`] away from working.
//!
//! # The three columns it fills by rule rather than by draw
//!
//! - **A managed column** ([`Managed`]) gets a placeholder, because the store
//!   overwrites it on the way in. Filling it with a plausible timestamp would
//!   be work whose result is discarded, and a caller who saw the placeholder
//!   in a debug print might think the write kept it.
//! - **A soft-delete column** is left null. It is nullable, so the ordinary
//!   rule below would fill it — and then every generated row would arrive
//!   already retired, the whole fixture would read back empty, and the seed
//!   would look like it had silently done nothing. That is the single worst
//!   failure this module could have, so it is a rule rather than a draw.
//! - **A dropped column** is null, which is the only value
//!   [`Row::validate`](slate_schema::Row::validate) accepts for one.
//!
//! Otherwise: an explicit override wins, then the column's `DEFAULT` if it has
//! one — the schema's own answer to "what is a plausible value here" beats any
//! guess — then a draw.
//!
//! **Except in the primary key and in a unique index**, where the default is
//! deliberately ignored and the value is generated *injectively in the row
//! index*. A default cannot serve two rows of a unique column, so honouring it
//! would make every batch of more than one row collide; and a drawn value
//! collides by birthday long before a fixture gets large. Sequencing is the
//! only rule that produces a thousand insertable rows, which is what a factory
//! is for.
//!
//! # A nullable column is filled, and a null is asked for
//!
//! Nullable columns get values. A fixture of mostly-null rows exercises very
//! little, and a caller who wants nulls says [`Factory::null_for`]. The
//! soft-delete column above is the one exception and it goes the other way for
//! a reason stated there.

use crate::error::{OrmError, Result};
use slate_kernel::SecurityContext;
use slate_schema::{Managed, Ordinal, Row, TableDef};
use slate_tuple::{Value, ValueType};
use std::collections::BTreeMap;
use uuid::Uuid;

/// What a caller pinned for a column, in place of a draw.
#[derive(Debug)]
enum Override {
    /// The same value in every row.
    Fixed(Value),
    /// Drawn from a list by row index, wrapping. A foreign key is what this is
    /// for: the parent keys are in hand and the child rows point at them.
    Cycle(Vec<Value>),
    /// Null in every row, for a nullable column.
    Null,
}

/// Rows for a table, generated from the table.
///
/// See the [module documentation](self) for what is drawn, what is filled by
/// rule, and what is refused.
#[derive(Debug)]
pub struct Factory<'a> {
    table: &'a TableDef,
    seed: u64,
    start: u64,
    overrides: BTreeMap<usize, Override>,
}

impl<'a> Factory<'a> {
    /// A factory for `table`, with seed 0 and ids from 1.
    #[must_use]
    pub const fn new(table: &'a TableDef) -> Self {
        Self {
            table,
            seed: 0,
            start: 1,
            overrides: BTreeMap::new(),
        }
    }

    /// Change the seed, which changes every drawn value and nothing else.
    #[must_use]
    pub const fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// The row index the first row is generated at.
    ///
    /// Sequenced columns count from here, so two factories over one table with
    /// `starting_at(1)` and `starting_at(1_001)` produce batches that do not
    /// collide. That is how a fixture grows without being regenerated.
    #[must_use]
    pub const fn starting_at(mut self, start: u64) -> Self {
        self.start = start;
        self
    }

    /// Give every row the same value for `column`.
    ///
    /// # Errors
    /// If `column` is not a column of the table.
    pub fn set(self, column: &str, value: Value) -> Result<Self> {
        self.pin(column, Override::Fixed(value))
    }

    /// Draw `column` from `values` by row index, wrapping.
    ///
    /// The foreign-key tool: generate the parents, collect their keys, and
    /// cycle the children over them. The factory deliberately does not read
    /// the store to find parents itself — that would make generation `async`
    /// and tie it to a transaction, and the caller generating the parents
    /// already has the keys in hand.
    ///
    /// # Errors
    /// If `column` is not a column of the table, or `values` is empty — which
    /// is refused rather than treated as "no override", because an empty list
    /// is what a caller passes when the parent query came back empty, and
    /// silently generating unrelated values there produces a fixture whose
    /// foreign keys all dangle.
    pub fn cycle(self, column: &str, values: Vec<Value>) -> Result<Self> {
        if values.is_empty() {
            return Err(OrmError::Factory(FactoryError::EmptyCycle {
                table: self.table.name().to_owned(),
                column: column.to_owned(),
            }));
        }
        self.pin(column, Override::Cycle(values))
    }

    /// Leave `column` null in every row.
    ///
    /// # Errors
    /// If `column` is not a column of the table, or is not nullable.
    pub fn null_for(self, column: &str) -> Result<Self> {
        let ordinal = self.ordinal(column)?;
        let def = self
            .table
            .column(ordinal)
            .ok_or_else(|| self.no_such_column(column))?;
        if !def.is_nullable() {
            return Err(OrmError::Factory(FactoryError::NullForNonNullable {
                table: self.table.name().to_owned(),
                column: column.to_owned(),
            }));
        }
        self.pin(column, Override::Null)
    }

    /// Generate `count` rows.
    ///
    /// # Errors
    /// [`FactoryError`] for a column the factory will not guess at, and
    /// [`FactoryError::CheckViolation`] for a generated row the table's own
    /// `CHECK` constraints reject.
    pub fn rows(&self, count: usize) -> Result<Vec<Row>> {
        (0..count).map(|i| self.row(i as u64)).collect()
    }

    /// Generate the row at `index`, on its own.
    ///
    /// Equal to `rows(n)[index]` for any `n > index`, which is the point of
    /// generating from a function rather than a stream.
    ///
    /// # Errors
    /// As [`Factory::rows`].
    pub fn row(&self, index: u64) -> Result<Row> {
        let index = self.start.wrapping_add(index);
        let mut values = Vec::with_capacity(self.table.columns().len());
        for (position, column) in self.table.columns().iter().enumerate() {
            let ordinal = Ordinal(position);
            let value = match self.overrides.get(&position) {
                Some(Override::Fixed(value)) => value.clone(),
                Some(Override::Cycle(choices)) => {
                    // `index` counts from `start`, so the wrap is over the
                    // absolute row number: two batches of a growing fixture
                    // continue the cycle rather than both restarting at 0.
                    let at = usize::try_from(index % choices.len() as u64).unwrap_or(0);
                    choices.get(at).cloned().unwrap_or(Value::Null)
                }
                Some(Override::Null) => Value::Null,
                None => self.draw(column, ordinal, index)?,
            };
            values.push(value);
        }
        let row = Row::new(values);
        self.check(&row)?;
        Ok(row)
    }

    /// Every `CHECK` on the table, over one generated row.
    ///
    /// Run here rather than left to the insert because a violation surfacing
    /// from inside a batch write names the table and the check but not which
    /// row produced it, and a caller looking at a thousand generated rows
    /// cannot tell whether one is wrong or all of them are.
    fn check(&self, row: &Row) -> Result<()> {
        for check in self.table.checks() {
            if !check.satisfied_by(row) {
                return Err(OrmError::Factory(FactoryError::CheckViolation {
                    table: self.table.name().to_owned(),
                    check: check.name().to_owned(),
                    column: check.column().map(ToOwned::to_owned),
                }));
            }
        }
        Ok(())
    }

    /// The value for one column of one row, by the rules in the module docs.
    fn draw(
        &self,
        column: &slate_schema::ColumnDef,
        ordinal: Ordinal,
        index: u64,
    ) -> Result<Value> {
        if column.is_dropped() {
            return Ok(Value::Null);
        }
        if self.table.soft_delete() == Some(ordinal) {
            return Ok(Value::Null);
        }
        if column.managed().is_some() {
            // Overwritten by the store on the way in; see `Managed`. Zero
            // rather than a plausible timestamp so that a placeholder that
            // *did* survive is obvious in a debug print instead of blending in.
            return Ok(match column.managed() {
                Some(Managed::CreatedAt | Managed::UpdatedAt) | None => Value::I64(0),
            });
        }

        let sequenced = self.must_be_unique(ordinal);
        if !sequenced && let Some(default) = column.default_value() {
            return Ok(default.clone());
        }

        let word = mix(self.seed ^ mix(ordinal.0 as u64) ^ mix(index));
        let ty = column.value_type();
        Ok(match ty {
            ValueType::Bool if sequenced => {
                return Err(OrmError::Factory(FactoryError::CannotSequence {
                    table: self.table.name().to_owned(),
                    column: column.name().to_owned(),
                }));
            }
            ValueType::Vector => {
                return Err(OrmError::Factory(FactoryError::CannotGenerate {
                    table: self.table.name().to_owned(),
                    column: column.name().to_owned(),
                    ty,
                    reason: NO_DIMENSION,
                }));
            }
            ValueType::Bool => Value::Bool(word & 1 == 1),
            ValueType::I64 if sequenced => Value::I64(index as i64),
            ValueType::U64 if sequenced => Value::U64(index),
            ValueType::I64 => Value::I64((word % 10_000) as i64),
            ValueType::U64 => Value::U64(word % 10_000),
            // Two decimal places of a number under a thousand: large enough to
            // sort interestingly, small enough to read in a failure message.
            ValueType::F64 => Value::F64((word % 100_000) as f64 / 100.0),
            // A count of the column's smallest unit, which is what a decimal
            // is here — the scale lives on the column and does not change what
            // a plausible count looks like.
            ValueType::Decimal if sequenced => Value::Decimal(index as i64),
            ValueType::Decimal => Value::Decimal((word % 1_000_000) as i64),
            ValueType::Str if sequenced => Value::Str(format!("{} {index}", phrase(word))),
            ValueType::Str => Value::Str(phrase(word)),
            ValueType::Uuid if sequenced => Value::Uuid(uuid_from(index, mix(self.seed))),
            ValueType::Uuid => Value::Uuid(uuid_from(word, mix(word))),
            ValueType::Bytes if sequenced => Value::Bytes(index.to_be_bytes().to_vec().into()),
            ValueType::Bytes => Value::Bytes(word.to_be_bytes().to_vec().into()),
            // **`ValueType` is `#[non_exhaustive]` and this is a downstream
            // crate**, so the compiler cannot make this match complete the way
            // it does inside `slate-tuple`. A wildcard is therefore forced,
            // and a wildcard is exactly how a new type gets no generator and
            // nobody finds out. `every_value_type_is_generated_or_refused` in
            // `tests/factory.rs` holds the arms below to `ValueType::ALL` and
            // names the refusals, so adding a type fails a test here rather
            // than producing rows with a hole in them somewhere else.
            ValueType::Array => {
                // **Unreachable through a built table, and kept anyway.**
                // `TableBuilder::build` refuses an array column with no
                // element type, so `element_type()` is `Some` for every
                // `Array` column that can exist — verified by building one and
                // watching it refuse, not assumed. A mutation swapping this
                // reason for another therefore survives, which is recorded in
                // the ledger rather than papered over with a test that would
                // have to construct a `TableDef` no constructor produces.
                //
                // It is a refusal rather than an `expect` because the schema
                // layer's guarantee is one edit away from being weakened, and
                // the failure then would be a fixture full of arrays of the
                // wrong type rather than a panic anybody could read.
                let element = column.element_type().ok_or_else(|| {
                    OrmError::Factory(FactoryError::CannotGenerate {
                        table: self.table.name().to_owned(),
                        column: column.name().to_owned(),
                        ty,
                        reason: NO_ELEMENT_TYPE,
                    })
                })?;
                // One to three elements, and for a unique column the first
                // element carries the row index — which makes the whole list
                // distinct, since two lists differing in their first element
                // differ.
                let length = (word % 3) + 1;
                let elements = (0..length)
                    .map(|slot| {
                        let at = mix(word ^ mix(slot));
                        if sequenced && slot == 0 {
                            element_value(element, index, true)
                        } else {
                            element_value(element, at, false)
                        }
                    })
                    .collect::<Option<Vec<Value>>>()
                    .ok_or_else(|| {
                        OrmError::Factory(FactoryError::CannotGenerate {
                            table: self.table.name().to_owned(),
                            column: column.name().to_owned(),
                            ty: element,
                            reason: NO_GENERATOR,
                        })
                    })?;
                Value::Array(elements)
            }
            _ => {
                return Err(OrmError::Factory(FactoryError::CannotGenerate {
                    table: self.table.name().to_owned(),
                    column: column.name().to_owned(),
                    ty,
                    reason: NO_GENERATOR,
                }));
            }
        })
    }

    /// Whether a column has to hold a different value in every row.
    ///
    /// The primary key or any column of a unique index. A non-unique index
    /// deliberately does not count: duplicates there are the normal case and
    /// sequencing them would produce a fixture with no repeated values to
    /// group by, which is the opposite of what a fixture is for.
    ///
    /// **Every** column of a composite key counts, which is more than
    /// distinctness strictly needs — one injective column makes the tuple
    /// distinct, so a mutation sequencing only the first one survives. The
    /// redundancy is the point: a caller pinning the tenant of a
    /// tenant-scoped key with `cycle` has to leave the key distinct through
    /// whichever column they did *not* pin, and a rule that sequenced one
    /// column would work only when it happened to pick the other one.
    /// `a_composite_key_is_distinct_as_a_whole` covers both halves.
    fn must_be_unique(&self, ordinal: Ordinal) -> bool {
        self.table.is_primary_key_column(ordinal)
            || self
                .table
                .indexes()
                .iter()
                .filter(|index| index.is_unique())
                .flat_map(slate_schema::IndexDef::columns)
                .any(|column| column.ordinal == ordinal)
    }

    fn pin(mut self, column: &str, value: Override) -> Result<Self> {
        let ordinal = self.ordinal(column)?;
        self.overrides.insert(ordinal.0, value);
        Ok(self)
    }

    fn ordinal(&self, column: &str) -> Result<Ordinal> {
        self.table
            .ordinal_of(column)
            .ok_or_else(|| self.no_such_column(column))
    }

    fn no_such_column(&self, column: &str) -> OrmError {
        OrmError::Factory(FactoryError::NoSuchColumn {
            table: self.table.name().to_owned(),
            column: column.to_owned(),
        })
    }
}

/// A row the factory will not make, and why.
///
/// Every variant names the column, because the caller's next move is a
/// [`Factory::set`] or [`Factory::cycle`] on exactly that column and a message
/// that does not name it makes them go and find it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FactoryError {
    /// A column that is not on the table.
    NoSuchColumn {
        /// The table.
        table: String,
        /// The name asked for.
        column: String,
    },
    /// [`Factory::null_for`] on a column the schema says cannot be null.
    NullForNonNullable {
        /// The table.
        table: String,
        /// The column.
        column: String,
    },
    /// [`Factory::cycle`] with nothing to cycle over.
    EmptyCycle {
        /// The table.
        table: String,
        /// The column.
        column: String,
    },
    /// A type the factory will not invent a value for. See the module docs.
    CannotGenerate {
        /// The table.
        table: String,
        /// The column.
        column: String,
        /// The type it holds.
        ty: ValueType,
        /// Which refusal this is. The two are different situations and a
        /// caller can act on only one of them, so they do not share a
        /// sentence: [`NO_DIMENSION`] is a decision with a reason and `set` is
        /// the answer, while [`NO_GENERATOR`] means a type was added to
        /// `ValueType` and nothing here was taught to draw it, which is a bug
        /// in this module rather than anything the caller did.
        reason: &'static str,
    },
    /// A key or unique column whose type cannot hold a distinct value per row.
    CannotSequence {
        /// The table.
        table: String,
        /// The column.
        column: String,
    },
    /// A generated row the table's own `CHECK` constraints reject.
    CheckViolation {
        /// The table.
        table: String,
        /// The check that failed.
        check: String,
        /// The column it names, if it names one.
        column: Option<String>,
    },
}

impl core::fmt::Display for FactoryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoSuchColumn { table, column } => {
                write!(f, "`{table}` has no column `{column}`")
            }
            Self::NullForNonNullable { table, column } => write!(
                f,
                "`{table}.{column}` is not nullable, so it cannot be left null; use `set` to give it a value"
            ),
            Self::EmptyCycle { table, column } => write!(
                f,
                "`{table}.{column}` was given an empty list to cycle over; if the parent rows are missing, generate them first"
            ),
            Self::CannotGenerate {
                table,
                column,
                ty,
                reason,
            } => write!(
                f,
                "the factory will not invent a {ty} for `{table}.{column}`: {reason}"
            ),
            Self::CannotSequence { table, column } => write!(
                f,
                "`{table}.{column}` is a key or unique column holding a type with too few values to give every row its own; give it one with `set` or `cycle`"
            ),
            Self::CheckViolation {
                table,
                check,
                column,
            } => match column {
                Some(column) => write!(
                    f,
                    "a generated row fails `{table}`'s check `{check}` on `{column}`; the factory cannot read a predicate, so set `{column}` yourself"
                ),
                None => write!(
                    f,
                    "a generated row fails `{table}`'s check `{check}`; the factory cannot read a predicate, so set the columns it covers yourself"
                ),
            },
        }
    }
}

impl core::error::Error for FactoryError {}

/// The superuser context a seed writes under, named for what it is.
///
/// Sugar over [`SecurityContext::superuser`], and deliberately nothing more.
/// The whole argument in `slate_kernel::security` is that a privileged call
/// site should be findable with one grep, so this is a second name for the
/// same thing rather than a wrapper that hides it — it is `pub` here so that
/// `grep -r seeding_context` finds every seed, and it still contains the word
/// the other grep looks for.
#[must_use]
pub fn seeding_context() -> SecurityContext {
    SecurityContext::superuser()
}

/// Why a vector is refused: the width would be a guess, and a wrong one.
pub const NO_DIMENSION: &str = "a vector's dimension is not in the schema, so a generated one would be whatever \
     width the factory picked and every similarity search over it would be wrong; \
     give it one with `set`";

/// Why an array column with no element type is refused.
pub const NO_ELEMENT_TYPE: &str = "an array column declares its element type and this one does not, so there is \
     nothing to generate elements of; give it a value with `set`";

/// Why a type nothing here draws is refused.
///
/// Distinct from [`NO_DIMENSION`] because it is not a decision: it means
/// `ValueType` grew a variant and this module did not, which
/// `every_value_type_is_generated_or_refused` exists to catch first.
pub const NO_GENERATOR: &str = "the factory has no generator for this type, which is a hole in the factory \
     rather than a decision about your schema; `set` is the way through today";

/// Splitmix64's finalizer. See the module docs for why it is written out.
const fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Two words of a small vocabulary, which reads as a name in a failure message.
fn phrase(word: u64) -> String {
    const ADJECTIVES: [&str; 16] = [
        "amber", "brisk", "copper", "dusky", "eager", "fallow", "gilded", "hollow", "ivory",
        "jagged", "keen", "lucid", "murmur", "nimble", "opal", "quiet",
    ];
    const NOUNS: [&str; 16] = [
        "anchor", "bramble", "cinder", "delta", "ember", "fathom", "grove", "harbor", "inlet",
        "juniper", "kestrel", "lantern", "meadow", "north", "orchard", "quarry",
    ];
    format!("{} {}", pick(&ADJECTIVES, word), pick(&NOUNS, word >> 8))
}

/// One word of a list, chosen by a hash.
///
/// `get` rather than `[]` because `clippy::indexing_slicing` cannot see that
/// `% len` is in range and CI runs with `-D warnings`. The fallback is
/// unreachable and is a word rather than an `unwrap` on purpose: if the
/// reasoning above is ever wrong, a dull fixture is a better outcome than a
/// panic from inside somebody's seed.
fn pick<'a>(words: &[&'a str], word: u64) -> &'a str {
    let len = words.len() as u64;
    let at = if len == 0 { 0 } else { (word % len) as usize };
    words.get(at).copied().unwrap_or("slate")
}

/// A uuid from two words, so it is a function of the inputs like everything else.
fn uuid_from(high: u64, low: u64) -> Uuid {
    Uuid::from_u128((u128::from(high) << 64) | u128::from(low))
}

/// One element of a generated array.
///
/// `None` for a type that cannot be an element here, which is the nesting
/// refusal and the vector refusal arriving through a second door — the schema
/// layer refuses both at build time, so this is unreachable through a built
/// table and says so by returning rather than panicking.
fn element_value(ty: ValueType, word: u64, sequenced: bool) -> Option<Value> {
    Some(match ty {
        ValueType::Bool => Value::Bool(word & 1 == 1),
        ValueType::I64 if sequenced => Value::I64(word as i64),
        ValueType::U64 if sequenced => Value::U64(word),
        ValueType::I64 => Value::I64((word % 10_000) as i64),
        ValueType::U64 => Value::U64(word % 10_000),
        ValueType::F64 => Value::F64((word % 100_000) as f64 / 100.0),
        ValueType::Decimal => Value::Decimal((word % 1_000_000) as i64),
        ValueType::Str if sequenced => Value::Str(format!("{} {word}", phrase(word))),
        ValueType::Str => Value::Str(phrase(word)),
        ValueType::Uuid => Value::Uuid(uuid_from(word, mix(word))),
        ValueType::Bytes => Value::Bytes(word.to_be_bytes().to_vec().into()),
        // A vector or an array as an *element* is the nesting refusal arriving
        // through a second door, and the wildcard is the `#[non_exhaustive]`
        // one from `draw` — same reason, same test holding it honest.
        ValueType::Vector | ValueType::Array => return None,
        _ => return None,
    })
}
