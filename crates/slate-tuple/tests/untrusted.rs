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

/// Scalars only: a vector's encoding carries a length that random generation
/// would rarely make interesting, and it is covered by the hostile-byte cases.
/// `Uuid` is left out for the same reason — sixteen fixed bytes with no
/// structure for a corruption to interact with.
///
/// `Decimal` was left out by omission rather than by that argument, and is in
/// now: it is an `i64` on the wire but a *distinct tag*, so the truncation and
/// single-byte-corruption cases below never produced one.
fn any_scalar() -> impl Strategy<Value = Value> {
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

/// An empty buffer is not a panic, at every entry point.
#[test]
fn nothing_at_all_is_an_ordinary_error() {
    assert!(decode(&[], &[ValueType::I64]).is_err());
    assert_eq!(decode(&[], &[]).unwrap(), Vec::new());
    assert!(decode_dynamic(&[]).unwrap().is_empty());
    assert!(TupleReader::new(&[]).skip(Direction::Asc).is_err());
}
