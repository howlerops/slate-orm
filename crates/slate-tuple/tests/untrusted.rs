//! The decoder against bytes it did not write.
//!
//! Every other test in this crate feeds the decoder something the encoder
//! produced. That is the easy half. A decoder reads whatever storage hands
//! back, and storage can hand back a truncated write, a page from a different
//! table, a value written by an older schema, or — if an attacker ever gets
//! near the object store — anything at all.
//!
//! The contract being tested is deliberately weak, because a weak contract is
//! the one that can actually hold: **for any input, decoding returns `Ok` or
//! `Err`.** It must not panic, must not run away, and must not read out of
//! bounds. Nothing is claimed about *what* it decodes from nonsense; only that
//! nonsense is rejected rather than fatal.
//!
//! Panicking on malformed input is not a cosmetic failure in a database. A
//! single corrupt block would take down every reader that touches it, and
//! `panic = "abort"` or a poisoned lock turns that into an outage rather than
//! an error path.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_tuple::{Direction, TupleReader, Value, ValueType, decode, decode_dynamic, encode};

/// Every `ValueType`, from the enum itself.
///
/// This was a hand-written `prop_oneof!` of eight `Just`s, and the enum has
/// nine: `Decimal` arrived later and nothing said so, so the adversarial
/// decoder suite never once told the decoder to expect one. A roster nothing
/// forces you to edit is a roster that goes stale, which is the failure this
/// repository met in four other places today.
///
/// `ValueType::ALL` is the fix, and it lives in `slate-tuple` rather than here
/// because the enum is `#[non_exhaustive]`: the exhaustive, wildcard-free
/// match that catches a tenth variant can only be written inside the defining
/// crate. `value.rs`'s `all_lists_every_variant` is that match.
fn any_type() -> impl Strategy<Value = ValueType> {
    proptest::sample::select(ValueType::ALL.to_vec())
}

/// Bytes biased towards the ones that mean something to the codec: type tags,
/// the NUL that terminates a string, and the 0xFF that escapes it. Uniform
/// random bytes almost always fail on the first tag and never reach the
/// interesting code.
fn hostile_bytes() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(
        prop_oneof![
            2 => Just(0x00u8),      // NUL: string terminator
            2 => Just(0xFFu8),      // escape, and the top of every range
            2 => Just(0x01u8),      // the escape's partner
            1 => Just(0x23u8),      // VECTOR: a length-prefixed type
            // ARRAY: the only tag whose body is other elements, so a run of
            // them is the recursion bomb. The decoder refuses a nested array
            // rather than bounding it, and this is what tries to catch it
            // not doing so — see also
            // `a_deeply_nested_array_is_refused_rather_than_recursed`.
            2 => Just(0x24u8),
            1 => Just(0x0Cu8),      // BYTES
            1 => Just(0x02u8),      // STR
            1 => Just(0x21u8),      // F64
            1 => 0x00u8..=0x40u8,   // the tag space generally
            1 => any::<u8>(),
        ],
        0..64,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    /// Schema-directed decoding terminates and does not panic, whatever the
    /// bytes and whatever types it is told to expect.
    #[test]
    fn decoding_arbitrary_bytes_never_panics(
        bytes in hostile_bytes(),
        types in proptest::collection::vec(any_type(), 0..6),
    ) {
        // Only that it returns. A panic or a hang fails the test by happening.
        let _ = decode(&bytes, &types);
    }

    /// The same for the self-describing decoder, which has no schema to bound
    /// what it will try to read and so has more ways to go wrong.
    #[test]
    fn dynamic_decoding_never_panics(bytes in hostile_bytes()) {
        let _ = decode_dynamic(&bytes);
    }

    /// Skipping is the path a projection takes over columns it does not want,
    /// and it advances by lengths read out of the buffer — so a bad length is
    /// exactly the input that would walk off the end.
    #[test]
    fn skipping_arbitrary_bytes_never_panics(
        bytes in hostile_bytes(),
        steps in 0..8usize,
        descending in any::<bool>(),
    ) {
        let direction = if descending { Direction::Desc } else { Direction::Asc };
        let mut reader = TupleReader::new(&bytes);
        for _ in 0..steps {
            if reader.skip(direction).is_err() {
                break;
            }
        }
    }

    /// Reading and skipping interleaved, which is what a real projection does
    /// and which no single-operation test covers.
    #[test]
    fn mixed_reads_and_skips_never_panic(
        bytes in hostile_bytes(),
        plan in proptest::collection::vec((any_type(), any::<bool>()), 0..8),
    ) {
        let mut reader = TupleReader::new(&bytes);
        for (ty, skip) in plan {
            let outcome = if skip {
                reader.skip(Direction::Asc)
            } else {
                reader.read(ty, Direction::Asc).map(|_| ())
            };
            if outcome.is_err() {
                break;
            }
        }
    }

    /// Truncating a *valid* encoding at every length is the realistic
    /// corruption: a write that did not finish. None of the prefixes may panic,
    /// and the full length must still decode to what went in.
    #[test]
    fn every_truncation_of_a_valid_encoding_is_handled(
        values in proptest::collection::vec(any_scalar(), 1..5),
    ) {
        let types: Vec<ValueType> = values
            .iter()
            .map(|v| v.value_type().unwrap_or(ValueType::I64))
            .collect();
        let bytes = encode(&values);

        for cut in 0..bytes.len() {
            let _ = decode(&bytes[..cut], &types);
        }
        prop_assert_eq!(decode(&bytes, &types).unwrap(), values);
    }

    /// Flipping one byte of a valid encoding, at every position, over every
    /// value. Corruption in storage is a bit flip far more often than it is a
    /// truncation.
    #[test]
    fn a_single_corrupted_byte_is_handled(
        values in proptest::collection::vec(any_scalar(), 1..4),
        position in any::<prop::sample::Index>(),
        replacement in any::<u8>(),
    ) {
        let types: Vec<ValueType> = values
            .iter()
            .map(|v| v.value_type().unwrap_or(ValueType::I64))
            .collect();
        let mut bytes = encode(&values);
        if bytes.is_empty() {
            return Ok(());
        }
        let at = position.index(bytes.len());
        bytes[at] = replacement;
        let _ = decode(&bytes, &types);
    }
}

/// The types `any_scalar` deliberately does not generate, and why.
///
/// An exclusion with no reason is indistinguishable from an omission, which is
/// exactly how `Decimal` survived in `any_type` for as long as it did. These
/// two are arguments; `Decimal` was an accident.
const NOT_GENERATED: [(ValueType, &str); 2] = [
    (
        ValueType::Vector,
        "a length prefix random generation would rarely make interesting; the \
         hostile-byte cases reach it, and `a_huge_declared_vector_length_does_not_allocate` \
         is the case that matters",
    ),
    (
        ValueType::Uuid,
        "sixteen fixed bytes with no structure for a truncation or a \
         single-byte corruption to interact with",
    ),
];

/// `any_scalar` generates every type but those, held to the enum.
///
/// The same defect as `any_type`'s, one level down, and the previous entry
/// left it open: `Value` has no `ALL` to derive from, because its variants
/// carry data. `Value::value_type` is the bridge — it is an exhaustive
/// wildcard-free match inside the crate, so a new variant fails to compile
/// there, and `ValueType::ALL` grows, and this fails until somebody either
/// generates the new type or writes down why not.
///
/// Sampled rather than introspected, because a `prop_oneof!` cannot be asked
/// what it can produce. 500 draws over 9 equally weighted outcomes leaves a
/// miss at 9·(8/9)^500 ≈ 10^-25, and the runner is seeded deterministically,
/// so this is not a flake waiting to happen.
#[test]
fn any_scalar_generates_every_type_it_does_not_exclude() {
    use proptest::strategy::ValueTree as _;
    use proptest::test_runner::TestRunner;

    let strategy = any_scalar();
    let mut runner = TestRunner::deterministic();
    let mut seen: std::collections::BTreeSet<ValueType> = std::collections::BTreeSet::new();
    let mut nulls = 0;
    for _ in 0..500 {
        match strategy.new_tree(&mut runner).expect("a value").current() {
            Value::Null => nulls += 1,
            other => {
                seen.insert(other.value_type().expect("a typed value has a type"));
            }
        }
    }

    let excluded: std::collections::BTreeSet<ValueType> =
        NOT_GENERATED.iter().map(|(kind, _)| *kind).collect();
    let wanted: std::collections::BTreeSet<ValueType> = ValueType::ALL
        .iter()
        .copied()
        .filter(|kind| !excluded.contains(kind))
        .collect();

    assert_eq!(
        seen, wanted,
        "any_scalar and ValueType::ALL disagree; add the type or add it to \
         NOT_GENERATED with a reason"
    );
    // `Null` has no `ValueType`, so the comparison above cannot see it, and a
    // strategy that stopped producing it would lose the one value whose
    // encoding is a bare tag.
    assert!(nulls > 0, "any_scalar stopped generating Null");
}

/// Scalars, minus the two `NOT_GENERATED` argues for.
///
/// `Decimal` was missing by omission rather than by an argument, and is in
/// now: it is an `i64` on the wire but a *distinct tag*, so the truncation and
/// single-byte-corruption cases below never produced one.
///
/// `Array` is here for a sharper reason than coverage. Its encoding ends in a
/// *single* byte with no length to cross-check it against, so a one-byte
/// corruption at the terminator is the cheapest way to turn a valid array into
/// one that runs off the end of the buffer — and a truncation that removes the
/// terminator is the same input arriving by accident. Those two cases are the
/// ones that would find it.
fn any_scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        // Uniform over nine outcomes: the eight element kinds share weight 8
        // between them, and an array takes the ninth. Weighting matters here
        // because an array whose elements are themselves drawn would otherwise
        // crowd out the flat values these cases were written for.
        8 => any_element(),
        1 => proptest::collection::vec(any_element(), 0..4).prop_map(Value::Array),
    ]
}

/// What may appear *inside* an array, which is everything but an array.
///
/// Nesting is refused by the decoder rather than bounded, so generating it
/// here would generate values this crate cannot round-trip. The refusal has
/// its own case instead.
fn any_element() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        proptest::collection::vec(any::<u8>(), 0..8)
            .prop_map(|v| Value::Bytes(bytes::Bytes::from(v))),
        ".{0,8}".prop_map(Value::Str),
        any::<i64>().prop_map(Value::I64),
        any::<u64>().prop_map(Value::U64),
        any::<f64>().prop_map(Value::F64),
        any::<i64>().prop_map(Value::Decimal),
    ]
}

/// A length prefix is a promise the buffer does not have to keep.
///
/// A vector says how many elements follow. Claiming four billion of them must
/// be an error, not an allocation — this is the classic decompression-bomb
/// shape, and the one place in the codec where a number in the input decides
/// how much memory is reserved.
#[test]
fn a_huge_declared_vector_length_does_not_allocate() {
    // Tag, then a u32 count of 0xFFFFFFFF, then nothing.
    let bytes = [0x23, 0xFF, 0xFF, 0xFF, 0xFF];
    let decoded = decode(&bytes, &[ValueType::Vector]);
    assert!(
        decoded.is_err(),
        "a vector claiming four billion elements decoded from five bytes"
    );

    // And a merely large one, still far beyond the buffer.
    let bytes = [0x23, 0x00, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00];
    assert!(decode(&bytes, &[ValueType::Vector]).is_err());
}

/// Nesting is refused, so a run of array tags cannot become a run of frames.
///
/// The decoder recursing once per `0x24` would blow the stack long before this
/// many, and a stack overflow is an abort: no unwinding, no error path, the
/// whole process. That is why nesting is refused outright rather than given a
/// ceiling — a ceiling is a number somebody has to get right, and this is a
/// property that holds at every depth.
///
/// Both entry points, because `skip` writes the refusal out a second time
/// rather than sharing the decoder's.
#[test]
fn a_deeply_nested_array_is_refused_rather_than_recursed() {
    // One level in, first and deliberately: an implementation that permits
    // nesting fails *here*, as a named test with a readable message, rather
    // than on the hundred-thousand-deep case below — which would overflow the
    // stack and abort the process, and an aborted binary reports no test
    // failure at all. The order is the difference between a finding and an
    // empty log.
    let one_deep = [0x24u8, 0x24, 0x00, 0x00];
    assert!(decode(&one_deep, &[ValueType::Array]).is_err());
    assert!(decode_dynamic(&one_deep).is_err());
    assert!(TupleReader::new(&one_deep).skip(Direction::Asc).is_err());

    // And at depth, which is what catches a *bounded* recursion rather than
    // an outright refusal.
    let bytes = vec![0x24u8; 100_000];
    assert!(decode(&bytes, &[ValueType::Array]).is_err());
    assert!(decode_dynamic(&bytes).is_err());
    assert!(TupleReader::new(&bytes).skip(Direction::Asc).is_err());
}

/// An array with no terminator is an error, not a read past the end.
///
/// The terminator is one byte and there is no length to disagree with it, so
/// losing it is the array's characteristic corruption: the decoder keeps
/// asking for elements and has to stop when the buffer does.
#[test]
fn an_unterminated_array_stops_at_the_end_of_the_buffer() {
    // Tag, one integer element, and then nothing where the terminator goes.
    let bytes = [0x24u8, 0x16, 0x07];
    assert!(decode(&bytes, &[ValueType::Array]).is_err());
    assert!(TupleReader::new(&bytes).skip(Direction::Asc).is_err());

    // The same bytes with the terminator do decode, so the case above is
    // about the missing byte and not about the rest being malformed.
    let bytes = [0x24u8, 0x16, 0x07, 0x00];
    assert_eq!(
        decode(&bytes, &[ValueType::Array]).unwrap(),
        vec![Value::Array(vec![Value::I64(7)])]
    );
}

/// An empty buffer is not a panic, at every entry point.
#[test]
fn nothing_at_all_is_an_ordinary_error() {
    assert!(decode(&[], &[ValueType::I64]).is_err());
    assert_eq!(decode(&[], &[]).unwrap(), Vec::new());
    assert!(decode_dynamic(&[]).unwrap().is_empty());
    assert!(TupleReader::new(&[]).skip(Direction::Asc).is_err());
}
