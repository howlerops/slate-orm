//! Codec throughput.
//!
//! Every key the record layer touches goes through here, so this is the floor
//! under every other measurement: a row write encodes one primary key and one
//! key per index, and a scan decodes one key per row.

// A benchmark that cannot set itself up should stop, loudly.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use bytes::Bytes;
use criterion::{BatchSize, Criterion, criterion_group};
use slate_tuple::{Direction, Value, ValueType, decode, encode, encode_with};
use std::hint::black_box;
use uuid::Uuid;

/// The shape of a realistic composite key: tenant, id, and a couple of columns.
fn wide_tuple() -> Vec<Value> {
    vec![
        Value::Uuid(Uuid::from_u128(0x1234_5678_9abc_def0)),
        Value::U64(9_876_543),
        Value::Str("a-reasonably-typical-string-value".to_owned()),
        Value::I64(-4_242),
        Value::F64(1.5),
        Value::Bytes(Bytes::from_static(b"\x00\xff binary \x00")),
    ]
}

fn wide_types() -> Vec<ValueType> {
    vec![
        ValueType::Uuid,
        ValueType::U64,
        ValueType::Str,
        ValueType::I64,
        ValueType::F64,
        ValueType::Bytes,
    ]
}

fn codec(c: &mut Criterion) {
    let mut group = c.benchmark_group("tuple");

    // A two-column primary key is the commonest thing encoded.
    let pk = vec![Value::Uuid(Uuid::from_u128(7)), Value::U64(42)];
    let pk_types = [ValueType::Uuid, ValueType::U64];
    let pk_bytes = encode(&pk);

    group.bench_function("encode/primary_key", |b| {
        b.iter(|| encode(black_box(&pk)));
    });
    group.bench_function("decode/primary_key", |b| {
        b.iter(|| decode(black_box(&pk_bytes), black_box(&pk_types)).expect("decode"));
    });

    let wide = wide_tuple();
    let types = wide_types();
    let wide_bytes = encode(&wide);

    group.bench_function("encode/six_columns", |b| {
        b.iter(|| encode(black_box(&wide)));
    });
    group.bench_function("decode/six_columns", |b| {
        b.iter(|| decode(black_box(&wide_bytes), black_box(&types)).expect("decode"));
    });

    // Descending columns complement every byte, so they are worth measuring
    // separately from ascending ones.
    let directions = [Direction::Desc; 6];
    group.bench_function("encode/six_columns_descending", |b| {
        b.iter(|| encode_with(black_box(&wide), black_box(&directions)));
    });

    // Encoding into a reused buffer is what the key builders should be doing.
    group.bench_function("encode/into_reused_buffer", |b| {
        b.iter_batched(
            || Vec::with_capacity(128),
            |mut buffer: Vec<u8>| {
                for value in &pk {
                    slate_tuple::encode_value_into(&mut buffer, value, Direction::Asc);
                }
                black_box(buffer)
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, codec);
/// Spelled out rather than `criterion_main!(benches)`, which is what this
/// was: the generated main runs the group and prints criterion's summary,
/// with nowhere to say what build produced the numbers. A bench whose output
/// cannot be told apart from a debug run's is the defect #280 is about.
fn main() {
    slate_kernel::build::announce();
    benches();
    Criterion::default().configure_from_args().final_summary();
}
