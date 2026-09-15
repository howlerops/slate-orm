//! The load-bearing property of the record layer: byte order equals value order.
//!
//! Everything above this crate — range scans, index bounds, tenant prefix
//! isolation — is only correct if these properties hold, so they are tested as
//! properties rather than as examples.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use bytes::Bytes;
use core::cmp::Ordering;
use proptest::prelude::*;
use slate_tuple::{
    Direction, TupleReader, Value, ValueType, decode, decode_prefix, encode, encode_with,
};
use uuid::Uuid;

/// Generate a value together with the type tag needed to decode it.
fn any_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        // Byte strings deliberately favour 0x00 and 0xFF: the escaping rules
        // are where an order-preserving codec goes wrong.
        proptest::collection::vec(prop_oneof![Just(0u8), Just(0xFFu8), any::<u8>()], 0..12)
            .prop_map(|v| Value::Bytes(Bytes::from(v))),
        // Same idea for strings, which may hold NUL as a code point.
        proptest::collection::vec(
            prop_oneof![Just('\0'), Just('a'), Just('\u{10FFFF}'), any::<char>()],
            0..8
        )
        .prop_map(|cs| Value::Str(cs.into_iter().collect())),
        any::<i64>().prop_map(Value::I64),
        any::<u64>().prop_map(Value::U64),
        // A decimal rides the integer encoding behind one extra byte, so it
        // belongs in this generator rather than in a suite of its own: every
        // property in this file — round trip, byte order equals value order,
        // prefix-freeness, descending — then covers it, and covers it against
        // the other types rather than only against itself.
        //
        // The boundaries are named because the decode narrows through `i128`:
        // `i64::MIN`'s magnitude does not fit in an `i64`, which is exactly
        // the value a careless implementation loses.
        prop_oneof![
            Just(i64::MIN),
            Just(i64::MAX),
            Just(0i64),
            Just(-1i64),
            any::<i64>()
        ]
        .prop_map(Value::Decimal),
        prop_oneof![
            Just(f64::NAN),
            Just(f64::INFINITY),
            Just(f64::NEG_INFINITY),
            Just(0.0f64),
            Just(-0.0f64),
            any::<f64>()
        ]
        .prop_map(Value::F64),
        any::<[u8; 16]>().prop_map(|b| Value::Uuid(Uuid::from_bytes(b))),
        // Vectors are length-prefixed rather than terminated, so they exercise
        // a different path through every property here — including the
        // prefix-freeness one, where a length prefix is the whole argument for
        // the encoding being safe.
        proptest::collection::vec(
            prop_oneof![
                Just(f32::NAN),
                Just(f32::INFINITY),
                Just(f32::NEG_INFINITY),
                Just(0.0f32),
                Just(-0.0f32),
                any::<f32>()
            ],
            0..6
        )
        .prop_map(Value::Vector),
    ]
}

/// The type tag a value must be decoded with. Nulls are untyped, so pick one.
fn type_of(v: &Value) -> ValueType {
    v.value_type().unwrap_or(ValueType::I64)
}

fn any_direction() -> impl Strategy<Value = Direction> {
    prop_oneof![Just(Direction::Asc), Just(Direction::Desc)]
}

/// A tuple of values paired with a direction per element.
fn any_tuple(max_len: usize) -> impl Strategy<Value = (Vec<Value>, Vec<Direction>)> {
    proptest::collection::vec((any_value(), any_direction()), 0..max_len)
        .prop_map(|pairs| pairs.into_iter().unzip())
}

/// Compare two tuples the way the encoding must: element-wise, honouring each
/// element's direction, then shorter-is-less.
fn expected_cmp(a: &[Value], b: &[Value], dirs: &[Direction]) -> Ordering {
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let ord = match dirs.get(i).copied().unwrap_or_default() {
            Direction::Asc => x.cmp(y),
            Direction::Desc => y.cmp(x),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.len().cmp(&b.len())
}

proptest! {
    // The ordering guarantee is the crate's whole reason to exist; spend more
    // cases on it than proptest's default.
    #![proptest_config(ProptestConfig::with_cases(4096))]

    /// Schema-directed decode recovers exactly what was encoded.
    #[test]
    fn round_trips((values, dirs) in any_tuple(6)) {
        let types: Vec<ValueType> = values.iter().map(type_of).collect();
        let encoded = encode_with(&values, &dirs);

        let mut reader = TupleReader::new(&encoded);
        let mut decoded = Vec::new();
        for (i, ty) in types.iter().enumerate() {
            let dir = dirs.get(i).copied().unwrap_or_default();
            decoded.push(reader.read(*ty, dir).expect("decode"));
        }
        prop_assert!(
            reader.is_empty(),
            "reader left {} trailing byte(s)",
            reader.remainder().len()
        );
        prop_assert_eq!(decoded, values);
    }

    /// The whole point: comparing encodings is comparing values.
    ///
    /// Both tuples share one direction vector, which is how a real index works
    /// — the direction is a property of the index column, not of the row.
    #[test]
    fn byte_order_matches_value_order(
        (a, dirs_a) in any_tuple(5),
        (b, _dirs_b) in any_tuple(5),
    ) {
        let dirs = dirs_a;
        let ea = encode_with(&a, &dirs);
        let eb = encode_with(&b, &dirs);
        prop_assert_eq!(
            ea.cmp(&eb),
            expected_cmp(&a, &b, &dirs),
            "a={:?} b={:?} dirs={:?} ea={:02x?} eb={:02x?}", a, b, dirs, ea, eb
        );
    }

    /// Concatenation is composition: a tuple's encoding starts with the
    /// encoding of its prefix. This is what makes a prefix scan a range scan.
    #[test]
    fn encoding_is_prefix_closed(
        (head, dh) in any_tuple(3),
        (tail, dt) in any_tuple(3),
    ) {
        let head_enc = encode_with(&head, &dh);
        let mut joined: Vec<Value> = head.clone();
        joined.extend(tail);
        let mut dirs = dh.clone();
        dirs.extend(dt);
        let joined_enc = encode_with(&joined, &dirs);
        prop_assert!(joined_enc.starts_with(&head_enc));
    }

    /// An index key is `<indexed columns><primary key>`; decoding the first
    /// part must hand back the second part byte-exact.
    ///
    /// The suffix is itself an encoded tuple, which is the documented
    /// precondition: a trailing byte string that began with `0xFF` would be
    /// read as the escaped NUL of a preceding string element.
    #[test]
    fn decode_prefix_returns_the_suffix(
        idx in proptest::collection::vec(any_value(), 0..4),
        pk in proptest::collection::vec(any_value(), 0..3),
    ) {
        let types: Vec<ValueType> = idx.iter().map(type_of).collect();
        let suffix = encode(&pk);
        let mut buf = encode(&idx);
        buf.extend_from_slice(&suffix);

        let (decoded, rest) = decode_prefix(&buf, &types).expect("decode prefix");
        prop_assert_eq!(decoded, idx);
        prop_assert_eq!(rest, &suffix[..]);
    }

    /// Descending elements sort exactly backwards from ascending ones.
    #[test]
    fn desc_reverses(a in any_value(), b in any_value()) {
        let asc = encode_with(core::slice::from_ref(&a), &[Direction::Asc])
            .cmp(&encode_with(core::slice::from_ref(&b), &[Direction::Asc]));
        let desc = encode_with(&[a], &[Direction::Desc])
            .cmp(&encode_with(&[b], &[Direction::Desc]));
        prop_assert_eq!(asc, desc.reverse());
    }

    /// No encoded element is a proper prefix of another.
    ///
    /// This is the invariant every other guarantee rests on: it is what lets
    /// elements be concatenated, and what makes complementing an element an
    /// exact order reversal rather than an approximate one.
    #[test]
    fn element_encodings_are_prefix_free(a in any_value(), b in any_value()) {
        // Stated as an implication rather than with `prop_assume!(a != b)` so
        // that duplicate pairs cost nothing against proptest's reject budget.
        let ea = encode(core::slice::from_ref(&a));
        let eb = encode(core::slice::from_ref(&b));
        if ea.starts_with(&eb) || eb.starts_with(&ea) {
            prop_assert_eq!(&a, &b, "distinct values, but {:02x?} prefixes {:02x?}", ea, eb);
        }
    }

    /// Value equality and encoding equality are the same relation.
    ///
    /// The forward direction makes a unique index actually unique; the reverse
    /// makes it not over-reject. Stated as an iff so neither can regress alone.
    #[test]
    fn equality_matches_encoding_equality(a in any_value(), b in any_value()) {
        let same_value = a == b;
        let same_bytes = encode(core::slice::from_ref(&a)) == encode(core::slice::from_ref(&b));
        prop_assert_eq!(same_value, same_bytes, "a={:?} b={:?}", a, b);
    }
}

/// Two shapes that broke earlier versions of the codec.
///
/// Both come down to prefix-freeness. A byte string element used to end in a
/// single `0x00`, which made `enc("\xff")` a proper prefix of `enc("\xff\0")`
/// — harmless ascending, but descending then failed to reverse them, and in a
/// mixed-direction tuple the following element's bytes could decide the
/// comparison outright.
#[test]
fn prefix_free_regressions() {
    // Descending must reverse even when one encoding used to prefix the other.
    let short = Value::Bytes(Bytes::from_static(b"\xff"));
    let long = Value::Bytes(Bytes::from_static(b"\xff\0"));
    assert!(short < long, "premise");
    assert!(
        encode_with(core::slice::from_ref(&short), &[Direction::Desc])
            > encode_with(core::slice::from_ref(&long), &[Direction::Desc]),
        "descending must reverse a prefix relationship"
    );

    // A NUL inside a string, followed by a descending element and then an
    // ascending one.
    let dirs = [Direction::Asc, Direction::Desc, Direction::Asc];
    let a = [Value::Str("a".into()), Value::Null, Value::Bool(false)];
    let b = [Value::Str("a\0".into()), Value::Null, Value::Bool(false)];
    assert!(a[0] < b[0], "premise");
    assert!(
        encode_with(&a, &dirs) < encode_with(&b, &dirs),
        "encoded order must follow value order"
    );
}

#[test]
fn cross_type_order_is_the_documented_rank() {
    let ascending = [
        Value::Null,
        Value::Bool(false),
        Value::Bool(true),
        Value::Bytes(Bytes::from_static(b"z")),
        Value::Str("a".into()),
        Value::I64(i64::MIN),
        Value::I64(-1),
        Value::I64(0),
        Value::U64(u64::MAX),
        Value::F64(f64::NEG_INFINITY),
        Value::F64(f64::NAN),
        Value::Uuid(Uuid::nil()),
        Value::Vector(Vec::new()),
        Value::Vector(vec![f32::NEG_INFINITY]),
    ];
    for pair in ascending.windows(2) {
        let (lo, hi) = (&pair[0], &pair[1]);
        assert!(lo < hi, "{lo:?} should sort below {hi:?}");
        assert!(
            encode(core::slice::from_ref(lo)) < encode(core::slice::from_ref(hi)),
            "encoding of {lo:?} should sort below {hi:?}"
        );
    }
}

#[test]
fn signed_and_unsigned_interleave_numerically() {
    // They share an encoding, so a u64 column and an i64 column compare sanely.
    assert!(encode(&[Value::I64(-1)]) < encode(&[Value::U64(0)]));
    assert!(encode(&[Value::U64(5)]) < encode(&[Value::I64(6)]));
    assert!(encode(&[Value::I64(i64::MAX)]) < encode(&[Value::U64(u64::MAX)]));
}

#[test]
fn integer_encoding_is_minimal_and_canonical() {
    assert_eq!(encode(&[Value::I64(0)]), vec![0x15]);
    assert_eq!(encode(&[Value::I64(1)]), vec![0x16, 0x01]);
    assert_eq!(encode(&[Value::I64(-1)]), vec![0x14, 0xFE]);
    assert_eq!(encode(&[Value::I64(255)]), vec![0x16, 0xFF]);
    assert_eq!(encode(&[Value::I64(256)]), vec![0x17, 0x01, 0x00]);

    // A non-minimal length (0x16 says one byte, but the value is zero) must be
    // rejected rather than silently accepted as a second spelling of `0`.
    let err = decode(&[0x16, 0x00], &[ValueType::I64]).unwrap_err();
    assert!(
        matches!(err, slate_tuple::TupleError::NonCanonicalInteger { .. }),
        "got {err:?}"
    );
}

#[test]
fn float_edge_cases_round_trip_and_order() {
    let vals = [
        f64::NEG_INFINITY,
        -1.0,
        -0.0,
        0.0,
        1.0,
        f64::INFINITY,
        f64::NAN,
    ];
    for w in vals.windows(2) {
        let (lo, hi) = (Value::F64(w[0]), Value::F64(w[1]));
        assert!(
            encode(core::slice::from_ref(&lo)) < encode(core::slice::from_ref(&hi)),
            "{lo:?} should encode below {hi:?}"
        );
    }
    // -0.0 and 0.0 are distinct keys, and NaN is canonical.
    assert_ne!(encode(&[Value::F64(-0.0)]), encode(&[Value::F64(0.0)]));
    assert_eq!(
        encode(&[Value::F64(f64::NAN)]),
        encode(&[Value::F64(-f64::NAN)])
    );
}

#[test]
fn strings_with_nul_sort_correctly() {
    let mut keys = [
        Value::Str("a".into()),
        Value::Str("a\0".into()),
        Value::Str("a\0b".into()),
        Value::Str("a\u{1}".into()),
        Value::Str("ab".into()),
        Value::Str("b".into()),
    ]
    .map(|v| (encode(core::slice::from_ref(&v)), v));
    keys.sort_by(|a, b| a.0.cmp(&b.0));
    let order: Vec<&Value> = keys.iter().map(|(_, v)| v).collect();
    let expected = ["a", "a\0", "a\0b", "a\u{1}", "ab", "b"];
    for (got, want) in order.iter().zip(expected) {
        assert_eq!(**got, Value::Str(want.into()));
    }
}

#[test]
fn decode_rejects_malformed_input() {
    use slate_tuple::TupleError;

    // Truncated string: no terminator.
    assert!(matches!(
        decode(&[0x05, b'a'], &[ValueType::Str]).unwrap_err(),
        TupleError::Truncated { .. }
    ));
    // Wrong type for the column.
    assert!(matches!(
        decode(&encode(&[Value::Bool(true)]), &[ValueType::Str]).unwrap_err(),
        TupleError::TypeMismatch { .. }
    ));
    // Undefined type code.
    assert!(matches!(
        decode(&[0x7F], &[ValueType::Str]).unwrap_err(),
        TupleError::UnknownTypeCode { .. }
    ));
    // Integer that does not fit the declared column width.
    assert!(matches!(
        decode(&encode(&[Value::U64(u64::MAX)]), &[ValueType::I64]).unwrap_err(),
        TupleError::IntegerOutOfRange { .. }
    ));
    // Leftover bytes.
    assert!(matches!(
        decode(&encode(&[Value::I64(1), Value::I64(2)]), &[ValueType::I64]).unwrap_err(),
        TupleError::TrailingBytes { .. }
    ));
}

/// Skipping must land in exactly the same place as decoding, for every type.
///
/// A skip that drifts by a byte does not fail loudly; it decodes the *next*
/// column as garbage, which is why this is checked against the decoder rather
/// than against hand-counted offsets.
#[test]
fn skipping_agrees_with_decoding() {
    let values = [
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        Value::I64(0),
        Value::I64(-1),
        Value::I64(i64::MIN),
        Value::U64(u64::MAX),
        Value::F64(-0.0),
        Value::F64(f64::NAN),
        Value::Str(String::new()),
        Value::Str("with\0embedded\0nulls".into()),
        Value::Bytes(Bytes::from_static(b"\xff\x00\xff")),
        Value::Uuid(Uuid::from_u128(u128::MAX)),
    ];

    for direction in [Direction::Asc, Direction::Desc] {
        for value in &values {
            let encoded = encode_with(core::slice::from_ref(value), &[direction]);

            let mut reading = TupleReader::new(&encoded);
            reading.read_dynamic(direction).expect("decode");
            let decoded_to = reading.position();

            let mut skipping = TupleReader::new(&encoded);
            skipping.skip(direction).expect("skip");
            assert_eq!(
                skipping.position(),
                decoded_to,
                "skip disagreed with decode for {value:?} ({direction:?})"
            );
            assert!(skipping.is_empty());
        }
    }
}

proptest! {
    /// The same, over arbitrary tuples: skipping some elements and decoding
    /// others must leave the reader wherever a full decode would have.
    #[test]
    fn skipping_and_decoding_can_be_mixed((values, dirs) in any_tuple(6)) {
        let encoded = encode_with(&values, &dirs);
        let mut reader = TupleReader::new(&encoded);
        for (i, value) in values.iter().enumerate() {
            let dir = dirs.get(i).copied().unwrap_or_default();
            // Skip every other element, decode the rest.
            if i % 2 == 0 {
                reader.skip(dir)?;
            } else {
                let ty = value.value_type().unwrap_or(ValueType::I64);
                prop_assert_eq!(&reader.read(ty, dir)?, value);
            }
        }
        prop_assert!(reader.is_empty());
    }
}
