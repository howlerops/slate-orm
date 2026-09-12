//! Keyspace layout.
//!
//! Everything lives in one ordered key-value space, laid out so that the
//! queries the kernel needs to answer are all contiguous ranges.
//!
//! ```text
//! row entry     0x01 <table id : u32 BE> <primary key tuple>
//! index entry   0x02 <index id : u32 BE> <tenant?> <indexed tuple> <primary key tuple>
//! ```
//!
//! The leading byte separates the two spaces, and the fixed-width id that
//! follows keeps table and index prefixes from containing one another. Because
//! ids are fixed width and big-endian, a prefix of the key is always a prefix of
//! the logical tuple — which is what makes "all rows of table T", "all rows of
//! tenant X in table T" and "index I where the first column is v" each a single
//! range scan.
//!
//! # Tenant scoping
//!
//! When a table declares a tenant column, the schema layer has already
//! guaranteed it is the first primary key column, so a tenant restriction on
//! rows is a key prefix for free. Index keys do not automatically contain it, so
//! the tenant value is prepended to every index key on such a table. Both spaces
//! are then physically partitioned by tenant, and a tenant restriction becomes a
//! narrower scan rather than a filter applied to someone else's data.
//!
//! # Unique indexes
//!
//! A unique index whose indexed values are all non-null stores the primary key
//! in the *value* and leaves it out of the key. Two rows with the same indexed
//! values then write the same key, so the store's own write-write conflict
//! detection rejects one — no read-check, and no dependence on the isolation
//! level. If any indexed value is null the primary key goes back into the key,
//! which gives the SQL behaviour that nulls do not collide with each other.

use crate::error::{KernelError, Result};
use slate_schema::{IndexDef, TableDef};
use slate_tuple::{Direction, Value, encode, encode_value_into, encode_with, prefix_successor};

/// Keyspace discriminator for row entries.
const ROW_SPACE: u8 = 0x01;
/// Keyspace discriminator for secondary index entries.
const INDEX_SPACE: u8 = 0x02;

/// Length of a `<space byte><id : u32 BE>` header.
const HEADER_LEN: usize = 1 + 4;

fn header(space: u8, id: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.push(space);
    out.extend_from_slice(&id.to_be_bytes());
    out
}

/// The prefix covering every row of `table`.
#[must_use]
pub fn table_prefix(table: &TableDef) -> Vec<u8> {
    header(ROW_SPACE, table.id().0)
}

/// The prefix covering every row of `table` belonging to `tenant`.
///
/// Returns `None` when the table is not tenant-scoped, in which case there is
/// no physical partition to narrow to and the caller must filter instead.
#[must_use]
pub fn table_tenant_prefix(table: &TableDef, tenant: &Value) -> Option<Vec<u8>> {
    table.tenant_column()?;
    let mut out = table_prefix(table);
    encode_value_into(&mut out, tenant, Direction::Asc);
    Some(out)
}

/// The key of one row.
#[must_use]
pub fn row_key(table: &TableDef, primary_key: &[Value]) -> Vec<u8> {
    let mut out = table_prefix(table);
    out.extend_from_slice(&encode(primary_key));
    out
}

/// Recover a row's primary key values from its key.
pub fn decode_row_key(table: &TableDef, key: &[u8]) -> Result<Vec<Value>> {
    let body = key.get(HEADER_LEN..).ok_or(KernelError::KeyDecode(
        slate_tuple::TupleError::Truncated {
            offset: key.len(),
            needed: HEADER_LEN - key.len().min(HEADER_LEN),
        },
    ))?;
    Ok(slate_tuple::decode(body, &table.primary_key_types())?)
}

/// The prefix covering every entry of `index`.
///
/// When the table is tenant-scoped, `tenant` narrows the prefix to that
/// tenant's slice of the index; passing `None` for a tenant-scoped table
/// covers every tenant.
#[must_use]
pub fn index_prefix(table: &TableDef, index: &IndexDef, tenant: Option<&Value>) -> Vec<u8> {
    let mut out = header(INDEX_SPACE, index.id().0);
    if table.tenant_column().is_some()
        && let Some(tenant) = tenant
    {
        encode_value_into(&mut out, tenant, Direction::Asc);
    }
    out
}

/// A stored secondary index entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// The key to write.
    pub key: Vec<u8>,
    /// The value to write. Empty unless the key omits the primary key.
    pub value: Vec<u8>,
    /// Whether writing this key twice is itself a uniqueness violation.
    ///
    /// True only for a unique index with no null in the indexed values; see the
    /// module docs.
    pub enforces_uniqueness: bool,
}

/// Build the index entry for one row.
///
/// `index_values` are the row's values for the index's columns in key order,
/// and `primary_key` its primary key values in key order.
#[must_use]
pub fn index_entry(
    table: &TableDef,
    index: &IndexDef,
    index_values: &[Value],
    primary_key: &[Value],
) -> IndexEntry {
    let tenant = tenant_value(table, primary_key);
    let mut key = index_prefix(table, index, tenant.as_ref());
    key.extend_from_slice(&encode_with(index_values, &index.directions()));

    // A unique index only collides on duplicates if the primary key is out of
    // the key. Nulls never collide, so a null in the indexed values puts it
    // back and the entry becomes an ordinary non-unique one.
    let enforces_uniqueness = index.is_unique() && !index_values.iter().any(Value::is_null);
    if enforces_uniqueness {
        IndexEntry {
            key,
            value: encode(primary_key),
            enforces_uniqueness,
        }
    } else {
        key.extend_from_slice(&encode(primary_key));
        IndexEntry {
            key,
            value: Vec::new(),
            enforces_uniqueness,
        }
    }
}

/// Recover the indexed values and primary key from a stored index entry.
pub fn decode_index_entry(
    table: &TableDef,
    index: &IndexDef,
    key: &[u8],
    value: &[u8],
) -> Result<(Vec<Value>, Vec<Value>)> {
    let corrupt = || KernelError::CorruptIndexEntry {
        table: table.name().to_owned(),
        index: index.name().to_owned(),
    };

    let mut body = key.get(HEADER_LEN..).ok_or_else(corrupt)?;

    // A tenant-scoped table prepends the tenant value; skip it. Its type is the
    // type of the first primary key column, which is the tenant column.
    if table.tenant_column().is_some() {
        let tenant_type = *table.primary_key_types().first().ok_or_else(corrupt)?;
        let (_, rest) = slate_tuple::decode_prefix(body, &[tenant_type])?;
        body = rest;
    }

    let (index_values, rest) =
        slate_tuple::decode_prefix_with(body, &table.index_key_types(index), &index.directions())?;

    // Where the primary key lives is decided by the same rule that built the
    // entry, so this stays in step with `index_entry`.
    let all_present = index.is_unique() && !index_values.iter().any(Value::is_null);
    let primary_key = if all_present {
        if !rest.is_empty() {
            return Err(corrupt());
        }
        slate_tuple::decode(value, &table.primary_key_types())?
    } else {
        if !value.is_empty() {
            return Err(corrupt());
        }
        slate_tuple::decode(rest, &table.primary_key_types())?
    };

    Ok((index_values, primary_key))
}

/// The tenant value of a row, taken from its primary key.
///
/// The schema layer guarantees the tenant column is primary key column zero, so
/// this is always the first key value.
#[must_use]
pub fn tenant_value(table: &TableDef, primary_key: &[Value]) -> Option<Value> {
    table.tenant_column()?;
    primary_key.first().cloned()
}

/// The exclusive upper bound of a prefix scan, or `None` when unbounded.
#[must_use]
pub fn prefix_end(prefix: &[u8]) -> Option<Vec<u8>> {
    prefix_successor(prefix)
}
