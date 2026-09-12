//! Reading ClickBench's parquet into the record layer.

use arrow::array::{Array, BinaryArray, Int16Array, Int32Array, Int64Array, UInt16Array};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::{RecordStore, SecurityContext, memory::MemoryStore};
use slate_schema::Row;
use slate_tuple::Value;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::schema::{COLUMNS, table};

/// Rows written per transaction. Large enough to amortise the commit, small
/// enough that a batch's rows and their index entries fit comfortably.
const BATCH: usize = 8_192;

/// One column of a batch, converted to values.
///
/// ClickBench's integers arrive as three different widths and one unsigned;
/// all of them fit in `I64`. Its strings arrive as `binary` and are not
/// guaranteed to be UTF-8, so they are converted lossily — this is a
/// benchmark, and a replacement character in a URL changes no query's shape.
fn column_values(batch: &RecordBatch, index: usize) -> Vec<Value> {
    let array = batch.column(index);
    let rows = array.len();
    let mut out = Vec::with_capacity(rows);

    macro_rules! ints {
        ($ty:ty) => {{
            let typed = array
                .as_any()
                .downcast_ref::<$ty>()
                .expect("an integer column");
            for i in 0..rows {
                out.push(if typed.is_null(i) {
                    Value::I64(0)
                } else {
                    Value::I64(i64::from(typed.value(i)))
                });
            }
        }};
    }

    match array.data_type() {
        arrow::datatypes::DataType::Int16 => ints!(Int16Array),
        arrow::datatypes::DataType::Int32 => ints!(Int32Array),
        arrow::datatypes::DataType::UInt16 => ints!(UInt16Array),
        arrow::datatypes::DataType::Int64 => {
            let typed = array
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("an i64 column");
            for i in 0..rows {
                out.push(Value::I64(if typed.is_null(i) {
                    0
                } else {
                    typed.value(i)
                }));
            }
        }
        arrow::datatypes::DataType::Binary => {
            let typed = array
                .as_any()
                .downcast_ref::<BinaryArray>()
                .expect("a binary column");
            for i in 0..rows {
                let bytes = if typed.is_null(i) {
                    b""
                } else {
                    typed.value(i)
                };
                out.push(Value::Str(String::from_utf8_lossy(bytes).into_owned()));
            }
        }
        other => panic!("unhandled parquet type {other:?}"),
    }
    out
}

/// Load `path` into a fresh in-memory store.
///
/// In memory on purpose. ClickBench measures a query engine, and running this
/// over object storage would measure SlateDB's block fetching instead — which
/// [`slate-slatedb`'s `scan_tuning` example](../../slate-slatedb) already does,
/// separately and honestly.
pub(crate) type Store = RecordStore<LatencyStore<MemoryStore>>;

pub(crate) async fn load(path: &Path, limit: Option<usize>) -> (Store, Arc<IoCounters>, usize) {
    let table = table();
    let catalog = slate_schema::Catalog::from_tables([table.clone()]).expect("catalog");
    // Wrapped in a `LatencyStore` with no latency, purely for its counters:
    // "this query scanned 1,200 rows of a million" says whether the key design
    // is doing anything, where a wall-clock number does not.
    let backing = LatencyStore::new(MemoryStore::new(), LatencyProfile::free());
    let counters = backing.counters();
    let store = RecordStore::new(backing, catalog, slate_kernel::SecurityCatalog::new());
    let root = SecurityContext::superuser();

    let file = File::open(path).expect("open the parquet file");
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .expect("read the parquet metadata")
        .with_batch_size(BATCH)
        .build()
        .expect("build the reader");

    let started = Instant::now();
    let mut ordinal = 0u64;
    let mut loaded = 0usize;
    for batch in reader {
        let batch = batch.expect("read a batch");
        let height = batch.num_rows();
        // Converted column by column, then transposed: the arrow arrays are
        // columnar and downcasting once per column beats once per cell.
        let columns: Vec<Vec<Value>> = (0..COLUMNS.len())
            .map(|i| column_values(&batch, i))
            .collect();

        let mut rows = Vec::with_capacity(height);
        for r in 0..height {
            let mut values: Vec<Value> = Vec::with_capacity(COLUMNS.len() + 1);
            for column in &columns {
                values.push(column.get(r).cloned().unwrap_or(Value::Null));
            }
            values.push(Value::U64(ordinal));
            ordinal += 1;
            rows.push(Row::new(values));
        }

        let txn = store.begin().await.expect("begin");
        txn.insert_many(&root, &table, &rows)
            .await
            .expect("load a batch");
        txn.commit().await.expect("commit");

        loaded += height;
        if limit.is_some_and(|l| loaded >= l) {
            break;
        }
        if loaded.is_multiple_of(BATCH * 25) {
            let rate = loaded as f64 / started.elapsed().as_secs_f64();
            eprintln!("  loaded {loaded} rows ({rate:.0}/s)");
        }
    }
    eprintln!(
        "  loaded {loaded} rows in {:.1}s",
        started.elapsed().as_secs_f64()
    );
    (store, counters, loaded)
}
