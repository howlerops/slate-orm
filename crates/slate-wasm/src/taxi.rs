//! The real dataset: New York City yellow taxi trips, January 2024.
//!
//! # Why this dataset
//!
//! The books fixture makes the planner's choices visible at 4,824 rows. It
//! cannot make them *convincing*: everybody knows a toy when they see one, and
//! a cost model that behaves on a table you generated is not evidence.
//!
//! This is the same corpus ClickHouse, DuckDB and half the analytics industry
//! benchmark on, published monthly by the NYC Taxi & Limousine Commission. The
//! trips are real, the zones are the TLC's own reference table, and an answer
//! the workbench gives about January 2024 is a true statement about January
//! 2024.
//!
//! # What is here and what is not
//!
//! **100,000 trips of the month's 2,964,619**, sampled across all 31 days.
//! That is not a limit of the record layer — the whole month loads into this
//! same in-memory store in 17.8 s — it is a limit of a browser tab. The store
//! costs about 1.3 KB a row, so the month wants roughly 3.8 GB, and a
//! `wasm32` tab has 4 GB of address space in theory and around 2 GB in
//! practice. See `docs/performance.md` for the measurement.
//!
//! The sample keeps the data's warts: fares of zero, trips stamped as lasting
//! eighteen hours, and 4.7% of rows with **no passenger count at all**. The
//! nulls are the reason `count(*)` and `count(passengers)` are different
//! numbers here, which is a distinction no generated fixture was drawing.
//!
//! # Why the trips arrive separately
//!
//! The rows are fetched as a compressed binary rather than compiled into the
//! wasm. Two reasons: the module stays cacheable across data changes and the
//! data stays cacheable across kernel changes, and a 1.2 MB download that
//! begins in parallel with the 0.7 MB module is faster than a 1.9 MB one.

use crate::literal;
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

/// `trips`: one real yellow-taxi trip.
pub const TRIPS: TableId = TableId(3);
/// `zones`: the TLC's own zone lookup, all 265 of them.
pub const ZONES: TableId = TableId(4);

/// How many bytes one packed trip takes. See `site/data/make-trips.py`.
const RECORD: usize = 24;

/// `255` in the packed byte means the source row had no passenger count.
const NO_PASSENGERS: u8 = 255;

#[must_use]
pub fn trips() -> TableDef {
    TableDef::builder("trips", TRIPS)
        .column("id", ValueType::U64)
        .column("pickup_zone", ValueType::U64)
        .column("dropoff_zone", ValueType::U64)
        // Seconds since the epoch. Not a date type: the kernel has no date,
        // and inventing one in the browser binding would be a type the SDKs
        // do not have. Queries about time are queries about integers here,
        // which is worse to write and honest about what is implemented.
        .column("pickup_time", ValueType::I64)
        .column("duration", ValueType::I64)
        // The one nullable column, because the source really is missing it on
        // 4.7% of rows. Filling it in would have made `count(passengers)` a
        // synonym for `count(*)`.
        .nullable_column("passengers", ValueType::I64)
        .column("distance", ValueType::F64)
        .column("fare", ValueType::F64)
        .column("tip", ValueType::F64)
        .column("total", ValueType::F64)
        .column("payment", ValueType::Str)
        .primary_key(["id"])
        // On `pickup_zone` rather than on anything else: it is the column a
        // reader is most likely to filter, it joins to `zones`, and at 100,000
        // rows with 226 distinct values it is selective enough that the
        // planner's choice between index and scan is a real decision rather
        // than a foregone one.
        .index(IndexDef::builder("by_pickup_zone", IndexId(30)).column("pickup_zone"))
        .build()
        .expect("the trips schema is valid")
}

#[must_use]
pub fn zones() -> TableDef {
    TableDef::builder("zones", ZONES)
        .column("id", ValueType::U64)
        .column("borough", ValueType::Str)
        .column("zone", ValueType::Str)
        .column("service_zone", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("the zones schema is valid")
}

#[must_use]
pub fn catalog() -> Catalog {
    Catalog::from_tables([
        crate::fixture::authors(),
        crate::fixture::books(),
        trips(),
        zones(),
    ])
    .expect("the catalog is valid")
}

/// The TLC's zone table, verbatim.
///
/// Compiled in rather than fetched: 12 KB is smaller than the request that
/// would collect it, and the join has no meaning without it.
const ZONE_CSV: &str = include_str!("taxi_zones.csv");

/// Parse the zone lookup.
///
/// A real CSV parser would be a dependency for one 265-line file whose shape
/// is fixed and whose quoting this handles: `"1","EWR","Newark Airport","EWR"`.
/// Commas inside quotes do occur — `"Governor's Island/Ellis Island/Liberty
/// Island"` has none but `"Queens"` neighbours do — so quotes are honoured.
#[must_use]
pub fn zone_rows() -> Vec<Row> {
    let mut out = Vec::with_capacity(265);
    for line in ZONE_CSV.lines().skip(1) {
        let mut fields = Vec::with_capacity(4);
        let mut current = String::new();
        let mut quoted = false;
        for c in line.chars() {
            match c {
                '"' => quoted = !quoted,
                ',' if !quoted => fields.push(std::mem::take(&mut current)),
                _ => current.push(c),
            }
        }
        fields.push(current);
        let Some(id) = fields.first().and_then(|f| f.trim().parse::<u64>().ok()) else {
            continue;
        };
        let field = |i: usize| {
            fields
                .get(i)
                .map(|f| f.trim().to_owned())
                .unwrap_or_default()
        };
        out.push(Row::new(vec![
            Value::U64(id),
            Value::Str(field(1)),
            Value::Str(field(2)),
            Value::Str(field(3)),
        ]));
    }
    out
}

/// Decode the packed trip file.
///
/// # Errors
///
/// A file whose length is not a whole number of records, which is the only
/// corruption this format can detect. There is no header and no checksum: the
/// file is served from the same origin as the code that reads it, and a
/// truncated transfer shows up as a length that does not divide.
pub fn decode(bytes: &[u8]) -> Result<Vec<Row>, String> {
    if !bytes.len().is_multiple_of(RECORD) {
        return Err(format!(
            "the trip file is {} bytes, which is not a whole number of {RECORD}-byte records",
            bytes.len()
        ));
    }
    // 2024-01-01 00:00:00 UTC, the offset every packed pickup is relative to.
    const BASE: i64 = 1_704_067_200;
    const PAYMENTS: [&str; 6] = [
        "unknown",
        "credit card",
        "cash",
        "no charge",
        "dispute",
        "voided",
    ];

    // `as_chunks` rather than `chunks_exact`: the record size is a constant, so
    // this hands back `&[u8; RECORD]` and the length is known to the type
    // system rather than assumed. The remainder is empty by the check above.
    let (records, _) = bytes.as_chunks::<RECORD>();
    let mut out = Vec::with_capacity(records.len());
    for (i, record) in records.iter().enumerate() {
        let u16_at = |o: usize| -> u64 {
            u64::from(u16::from_le_bytes([
                record.get(o).copied().unwrap_or(0),
                record.get(o + 1).copied().unwrap_or(0),
            ]))
        };
        let i32_at = |o: usize| -> i64 {
            i64::from(i32::from_le_bytes([
                record.get(o).copied().unwrap_or(0),
                record.get(o + 1).copied().unwrap_or(0),
                record.get(o + 2).copied().unwrap_or(0),
                record.get(o + 3).copied().unwrap_or(0),
            ]))
        };
        let byte = |o: usize| record.get(o).copied().unwrap_or(0);

        let passengers = byte(10);
        out.push(Row::new(vec![
            Value::U64(i as u64 + 1),
            Value::U64(u16_at(6)),
            Value::U64(u16_at(8)),
            Value::I64(BASE + i32_at(0)),
            Value::I64(i64::try_from(u16_at(4)).unwrap_or(0)),
            if passengers == NO_PASSENGERS {
                Value::Null
            } else {
                Value::I64(i64::from(passengers))
            },
            Value::F64(u16_at(11) as f64 / 100.0),
            Value::F64(i32_at(13) as f64 / 100.0),
            Value::F64(u16_at(17) as f64 / 100.0),
            Value::F64(i32_at(19) as f64 / 100.0),
            Value::Str(
                PAYMENTS
                    .get(byte(23) as usize)
                    .copied()
                    .unwrap_or("unknown")
                    .to_owned(),
            ),
        ]));
    }
    Ok(out)
}

/// Parse one text value against a trips column, for the write path.
///
/// Exists so the binding's `literal` stays the only place a string becomes a
/// typed value, including for a nullable column where the empty string and the
/// word `null` both mean "no value".
///
/// `scale` is `ColumnDef::scale`, which is `None` for every column `trips`
/// actually has. It is a parameter rather than a hard-coded `None` because a
/// hard-coded one would be right today and silently wrong the day a decimal
/// column is added — the fare columns are `f64` and are the obvious candidates.
///
/// # Errors
///
/// Whatever [`crate::literal`] refuses.
pub fn trip_literal(
    text: &str,
    kind: ValueType,
    scale: Option<u8>,
    nullable: bool,
) -> Result<Value, String> {
    if nullable && (text.trim().is_empty() || text.trim().eq_ignore_ascii_case("null")) {
        return Ok(Value::Null);
    }
    literal(text, kind, scale)
}

/// A stable summary of the two tables, for the committed bucket listing.
///
/// `site/data/bucket.json` is a real listing of a real load, and it can never
/// be compared object for object against a fresh one: the SST names are ULIDs
/// minted at write time and the byte counts move with SlateDB's block packing.
/// So the objects are not what gets checked — *what they are a listing of* is.
/// This is that, and `bucket_provenance.rs` compares it against the committed
/// copy in milliseconds without going near SlateDB or an object store.
///
/// Every column's name, type and nullability, and every index, in declaration
/// order. Deliberately not a hash: a fingerprint that says *what* changed is
/// worth more than one that says only that something did, and both are one
/// line in a JSON file. A reader who breaks this test sees `duration:I64` next
/// to `duration:F64` rather than two hex strings.
#[must_use]
pub fn schema_fingerprint() -> String {
    [trips(), zones()]
        .iter()
        .map(|table| {
            let columns: Vec<String> = table
                .columns()
                .iter()
                .map(|c| {
                    format!(
                        "{}:{:?}{}",
                        c.name(),
                        c.value_type(),
                        if c.is_nullable() { "?" } else { "" }
                    )
                })
                .collect();
            let indexes: Vec<String> = table
                .indexes()
                .iter()
                .map(|i| i.name().to_owned())
                .collect();
            format!(
                "{}({})[{}]",
                table.name(),
                columns.join(","),
                indexes.join(",")
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}
