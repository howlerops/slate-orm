//! Rows and the row body codec.
//!
//! A stored row is split across its key and its value. The primary key columns
//! live in the key and are *not* repeated in the body: they are recovered by
//! decoding the key, which costs less than storing them twice on every row.

use crate::columns::ColumnSet;
use crate::error::{Result, SchemaError};
use crate::table::{IndexDef, Ordinal, TableDef};
use slate_tuple::{TupleReader, Value, encode_value_into};

/// Container format of a row body. Bumped only if the framing itself changes.
const ROW_FORMAT_V1: u8 = 1;

/// A full-width row: one [`Value`] per column of its table, in ordinal order,
/// including the primary key columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    values: Vec<Value>,
}

impl Row {
    /// Wrap a vector of values as a row. Use [`Row::validate`] to check it
    /// against a table before storing it.
    #[must_use]
    pub const fn new(values: Vec<Value>) -> Self {
        Self { values }
    }

    /// The row's values, in ordinal order.
    #[must_use]
    pub fn values(&self) -> &[Value] {
        &self.values
    }

    /// Consume the row, yielding its values.
    #[must_use]
    pub fn into_values(self) -> Vec<Value> {
        self.values
    }

    /// The value at `ordinal`, if the row is wide enough.
    #[must_use]
    pub fn get(&self, ordinal: Ordinal) -> Option<&Value> {
        self.values.get(ordinal.0)
    }

    /// Check the row's width, column types and nullability against `table`.
    ///
    /// The record store calls this before every write, so a row that reaches
    /// storage is always one the schema permits.
    pub fn validate(&self, table: &TableDef) -> Result<()> {
        if self.values.len() != table.columns().len() {
            return Err(SchemaError::ColumnCountMismatch {
                table: table.name().to_owned(),
                expected: table.columns().len(),
                actual: self.values.len(),
            });
        }
        for (column, value) in table.columns().iter().zip(&self.values) {
            // A dropped column keeps its ordinal so nothing after it shifts,
            // but nothing is written for it. Refusing a value here rather than
            // discarding it is the difference between a caller finding out and
            // a caller watching a field vanish between write and read.
            if column.is_dropped() {
                if value.is_null() {
                    continue;
                }
                return Err(SchemaError::DroppedColumnValue {
                    table: table.name().to_owned(),
                    column: column.name().to_owned(),
                });
            }
            match value.value_type() {
                None => {
                    if !column.is_nullable() {
                        return Err(SchemaError::UnexpectedNull {
                            table: table.name().to_owned(),
                            column: column.name().to_owned(),
                        });
                    }
                }
                Some(actual) if actual != column.value_type() => {
                    return Err(SchemaError::ValueTypeMismatch {
                        table: table.name().to_owned(),
                        column: column.name().to_owned(),
                        expected: column.value_type(),
                        actual: value.type_name(),
                    });
                }
                Some(_) => {}
            }
        }

        // An expression index declares the type it produces, because the
        // decoder needs it before it has a row to run the expression on. This
        // is where the declaration is held to: an entry encoded as one type and
        // decoded as another is a row that reads back as a corrupt index, at
        // some later scan, with nothing pointing at the write that caused it.
        // Refuse it here instead.
        for index in table.indexes() {
            let Some(expression) = index.expression() else {
                continue;
            };
            let value = expression.value(self);
            // Null is not a type, and an expression index holds nulls for the
            // same reason a nullable column does: `lower(null)` is null, and an
            // index that refused it would be an index missing rows.
            if let Some(actual) = value.value_type()
                && actual != expression.produces()
            {
                return Err(SchemaError::IndexValueTypeMismatch {
                    table: table.name().to_owned(),
                    index: index.name().to_owned(),
                    expected: expression.produces(),
                    actual: value.type_name(),
                });
            }
        }
        Ok(())
    }

    /// The row's primary key values, in key order.
    #[must_use]
    pub fn primary_key_values(&self, table: &TableDef) -> Vec<Value> {
        self.project(table.primary_key())
    }

    /// The row's values for an index's columns, in key order.
    #[must_use]
    pub fn index_values(&self, index: &IndexDef) -> Vec<Value> {
        index
            .columns()
            .iter()
            .map(|c| self.value_or_null(c.ordinal))
            .collect()
    }

    fn project(&self, ordinals: &[Ordinal]) -> Vec<Value> {
        ordinals.iter().map(|o| self.value_or_null(*o)).collect()
    }

    /// Ordinals are schema-validated, so the fallback is unreachable for a row
    /// that passed [`Row::validate`]; it keeps the accessor total.
    fn value_or_null(&self, ordinal: Ordinal) -> Value {
        self.values.get(ordinal.0).cloned().unwrap_or(Value::Null)
    }
}

/// Encode a row's body: everything except the primary key columns.
///
/// The body is `<format byte><schema version><tuple of body columns>`. The
/// version is what lets a later build tell which columns a stored row actually
/// carries.
#[must_use]
pub fn encode_body(table: &TableDef, row: &Row) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ROW_FORMAT_V1);
    out.extend_from_slice(&table.schema_version().to_be_bytes());
    // `stored_columns` rather than `body_columns`: a dropped column is not
    // written any more, though rows written before the drop still carry it and
    // the decoder still has to walk past those bytes.
    for ordinal in table.stored_columns() {
        encode_value_into(
            &mut out,
            &row.value_or_null(*ordinal),
            slate_tuple::Direction::Asc,
        );
    }
    out
}

/// Rebuild a full row from its decoded primary key and its stored body.
///
// The decoder walks columns in ordinal order, so a version predicate has to be
// evaluated per column rather than folded into the list once.
/// Whether a body written at `version` carries bytes for `column`.
///
/// The two edges are independent: a column added later is not there yet, and a
/// column dropped earlier is there no longer. A column both added and dropped
/// before `version` is not there — the drop is the later fact.
fn present_at(column: &crate::ColumnDef, version: u32) -> bool {
    column.added_in() <= version && column.dropped_in().is_none_or(|at| version < at)
}

/// # Schema evolution
///
/// A body written at version `v` carries exactly the columns present at `v`:
/// added at or before it, and not dropped at or before it. Everything else is
/// reconstructed rather than read.
///
/// - A column added *after* `v` reads back as its
///   [default](crate::ColumnDef::default_value), or as null if it has none —
///   which is why the builder requires a later-added column to be one or the
///   other.
/// - A column dropped at or before `v` was never written, and one dropped after
///   it was, so its bytes are skipped. Either way it reads back as null: a
///   dropped column holds nothing.
/// - A row from a *newer* schema than this build is an error rather than a
///   guess.
pub fn decode_row(table: &TableDef, primary_key: &[Value], body: &[u8]) -> Result<Row> {
    decode_row_columns(table, primary_key, body, None)
}

/// Rebuild a row, decoding only the columns in `wanted`.
///
/// Columns outside the set are skipped rather than decoded, which for a string
/// or a byte column is the difference between an allocation and advancing a
/// cursor. Passing `None` decodes everything.
///
/// Skipped columns come back null, indistinguishable from a stored null — so
/// this is for a caller that knows what it asked for. See
/// [`Projection`](../slate_kernel/plan/enum.Projection.html).
pub fn decode_row_columns(
    table: &TableDef,
    primary_key: &[Value],
    body: &[u8],
    wanted: Option<&ColumnSet>,
) -> Result<Row> {
    let mut cursor = body.iter().copied();
    let format = cursor.next().ok_or_else(|| SchemaError::RowDecode {
        table: table.name().to_owned(),
        source: slate_tuple::TupleError::Truncated {
            offset: 0,
            needed: 1,
        },
    })?;
    if format != ROW_FORMAT_V1 {
        return Err(SchemaError::UnsupportedRowFormat {
            table: table.name().to_owned(),
            format,
        });
    }

    let mut version_bytes = [0u8; 4];
    for (slot, byte) in version_bytes.iter_mut().zip(&mut cursor) {
        *slot = byte;
    }
    let header_len = 1 + version_bytes.len();
    if body.len() < header_len {
        return Err(SchemaError::RowDecode {
            table: table.name().to_owned(),
            source: slate_tuple::TupleError::Truncated {
                offset: body.len(),
                needed: header_len - body.len(),
            },
        });
    }
    let written_version = u32::from_be_bytes(version_bytes);
    if written_version > table.schema_version() {
        return Err(SchemaError::RowFromFutureSchema {
            table: table.name().to_owned(),
            found: written_version,
            known: table.schema_version(),
        });
    }

    // Built by repetition rather than `vec![Value::Null; n]`, which fills by
    // *cloning* the null once per column. That was free while `Value::clone`
    // still inlined, and stopped being free the moment the enum grew a variant
    // big enough that it did not: a hundred and five out-of-line clone calls
    // per row, which callgrind put at 10% of the whole benchmark.
    let mut values: Vec<Value> = core::iter::repeat_with(|| Value::Null)
        .take(table.columns().len())
        .collect();

    // Nothing wanted from the body: do not walk it at all. This is the shape of
    // the filtering pass when a predicate only touches key columns, and walking
    // the body to skip every field of it would be the whole cost of that pass.
    //
    // Sound only because the columns it skips are ones the caller said it does
    // not want. A column reconstructed rather than read — one added after this
    // row was written, which takes its default — is skipped on the same
    // grounds and comes back null like every other unwanted column, which is
    // the same answer the walk below gives.
    if wanted.is_some_and(|wanted| {
        !table
            .body_columns()
            .iter()
            .any(|ordinal| wanted.contains(*ordinal))
    }) {
        for (ordinal, value) in table.primary_key().iter().zip(primary_key) {
            if let Some(slot) = values.get_mut(ordinal.0) {
                *slot = value.clone();
            }
        }
        return Ok(Row::new(values));
    }

    let payload = body.get(header_len..).unwrap_or_default();
    let mut reader = TupleReader::new(payload);
    let mut decoded = 0;

    for &ordinal in table.body_columns() {
        let Some(column) = table.column(ordinal) else {
            continue;
        };
        if !present_at(column, written_version) {
            // Nothing to read. A column dropped by now leaves its slot null; a
            // column not yet added takes its default, and must have one or be
            // nullable — checked at build time, re-checked here because a
            // stored row is the one input the builder never saw.
            if column.dropped_in().is_none_or(|at| written_version < at) {
                match column.default_value() {
                    // Only when the caller asked for it, so a default behaves
                    // exactly like a stored value: outside the projection both
                    // come back null, and the fast path above stays sound.
                    Some(default) => {
                        if wanted.is_none_or(|wanted| wanted.contains(ordinal))
                            && let Some(slot) = values.get_mut(ordinal.0)
                        {
                            *slot = default.clone();
                        }
                    }
                    None if !column.is_nullable() => {
                        return Err(SchemaError::IncompatibleSchemaEvolution {
                            table: table.name().to_owned(),
                            column: column.name().to_owned(),
                            written: written_version,
                        });
                    }
                    None => {}
                }
            }
            continue;
        }
        decoded += 1;

        // A column dropped since this row was written is still in its bytes.
        // Skipping is not optional: the next column's value starts where this
        // one ends, so leaving the cursor put would decode every following
        // column out of the wrong bytes.
        if column.is_dropped() || wanted.is_some_and(|wanted| !wanted.contains(ordinal)) {
            reader
                .skip(slate_tuple::Direction::Asc)
                .map_err(|source| SchemaError::RowDecode {
                    table: table.name().to_owned(),
                    source,
                })?;
            continue;
        }

        let value = reader
            .read(column.value_type(), slate_tuple::Direction::Asc)
            .map_err(|source| SchemaError::RowDecode {
                table: table.name().to_owned(),
                source,
            })?;
        if let Some(slot) = values.get_mut(ordinal.0) {
            *slot = value;
        }
    }

    if !reader.is_empty() {
        return Err(SchemaError::RowDecode {
            table: table.name().to_owned(),
            source: slate_tuple::TupleError::TrailingBytes {
                decoded,
                remaining: reader.remainder().len(),
            },
        });
    }

    for (ordinal, value) in table.primary_key().iter().zip(primary_key) {
        if let Some(slot) = values.get_mut(ordinal.0) {
            *slot = value.clone();
        }
    }

    Ok(Row::new(values))
}

/// A row with a column left *unset* rather than null.
///
/// `Row` is full width by construction, so it has no way to say "I did not
/// supply this" — and `DEFAULT` is precisely a rule about columns nobody
/// supplied. Reading a null as "unset" was the alternative and was rejected: it
/// takes away the ability to store a null in a defaulted nullable column, which
/// is a thing SQL lets you do and a thing an application does mean sometimes.
///
/// ```
/// # use slate_schema::{PartialRow, TableDef, TableId};
/// # use slate_tuple::{Value, ValueType};
/// let items = TableDef::builder("items", TableId(1))
///     .column("id", ValueType::U64)
///     .column("status", ValueType::Str)
///     .primary_key(["id"])
///     .default_for("status", Value::Str("new".into()))
///     .build()?;
///
/// let row = PartialRow::for_table(&items)
///     .set(items.ordinal_of("id").unwrap(), Value::U64(1))
///     .into_row(&items)?;
///
/// assert_eq!(row.values()[1], Value::Str("new".into()));
/// # Ok::<(), slate_schema::SchemaError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialRow {
    values: Vec<Option<Value>>,
}

impl PartialRow {
    /// A row of `table`'s width with every column unset.
    #[must_use]
    pub fn for_table(table: &TableDef) -> Self {
        Self {
            values: vec![None; table.columns().len()],
        }
    }

    /// Supply a column's value.
    ///
    /// Setting the same column twice keeps the last value, as an assignment
    /// does. An ordinal outside the table is ignored here and caught by
    /// [`PartialRow::into_row`], which knows the table's width.
    #[must_use]
    pub fn set(mut self, ordinal: Ordinal, value: Value) -> Self {
        if let Some(slot) = self.values.get_mut(ordinal.0) {
            *slot = Some(value);
        }
        self
    }

    /// Whether a value has been supplied for `ordinal`.
    #[must_use]
    pub fn is_set(&self, ordinal: Ordinal) -> bool {
        self.values.get(ordinal.0).is_some_and(Option::is_some)
    }

    /// Fill the unset columns and validate the result.
    ///
    /// An unset column takes its [default](crate::ColumnDef::default_value), or
    /// null if it has none — so an unset column that is neither nullable nor
    /// defaulted fails as [`SchemaError::UnexpectedNull`], which is the same
    /// error the same row would get if it had been written out in full.
    pub fn into_row(self, table: &TableDef) -> Result<Row> {
        if self.values.len() != table.columns().len() {
            return Err(SchemaError::ColumnCountMismatch {
                table: table.name().to_owned(),
                expected: table.columns().len(),
                actual: self.values.len(),
            });
        }
        let values = table
            .columns()
            .iter()
            .zip(self.values)
            .map(|(column, supplied)| match supplied {
                Some(value) => value,
                // A managed column left unset gets a placeholder, not a null.
                // The store overwrites it before the write — that is what
                // "managed" means — but `validate` runs first and would refuse
                // a null in a non-nullable column, so a caller using
                // `insert_partial` would have to name the one column the whole
                // feature exists to let them ignore.
                //
                // Zero rather than anything cleverer because nothing ever
                // reads it: `RecordTransaction::stamp` replaces every managed
                // slot on every write, so the only way to observe this value
                // is to bypass the store entirely.
                None if column.managed().is_some() => Value::I64(0),
                None => column.default_value().cloned().unwrap_or(Value::Null),
            })
            .collect();
        let row = Row::new(values);
        row.validate(table)?;
        Ok(row)
    }
}
