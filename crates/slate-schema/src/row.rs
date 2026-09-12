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
    for ordinal in table.body_columns() {
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
/// # Schema evolution
///
/// A body written at version `v` carries only the columns whose
/// [`ColumnDef::added_in`](crate::ColumnDef::added_in) is at most `v`, in
/// ordinal order. Columns added after `v` read back as null, which is why the
/// builder only allows a later-added column to be nullable — and why a row from
/// a *newer* schema than this build is an error rather than a guess.
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
        if column.added_in() > written_version {
            // Not present in this row; the builder guaranteed it is nullable.
            if !column.is_nullable() {
                return Err(SchemaError::IncompatibleSchemaEvolution {
                    table: table.name().to_owned(),
                    column: column.name().to_owned(),
                    written: written_version,
                });
            }
            continue;
        }
        decoded += 1;

        if wanted.is_some_and(|wanted| !wanted.contains(ordinal)) {
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
