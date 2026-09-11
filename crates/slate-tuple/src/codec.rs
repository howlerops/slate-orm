//! The order-preserving tuple codec.
//!
//! # Wire format
//!
//! Every element is `<type code> <payload>`, where the payload is
//! self-delimiting. Because elements are self-delimiting, a tuple needs no
//! length prefix and a reader can stop after `n` elements and hand the rest of
//! the buffer to another decoder — which is exactly what index keys need, since
//! they carry the primary key as a suffix.
//!
//! Type codes are chosen so that comparing the code bytes gives the cross-type
//! order `null < bool < bytes < string < integer < double < uuid`:
//!
//! | code(s)      | element                                       |
//! |--------------|-----------------------------------------------|
//! | `0x01`       | null                                          |
//! | `0x02`/`0x03`| `false` / `true`                              |
//! | `0x04`       | byte string, zero-escaped, `0x00 0x00`-ended   |
//! | `0x05`       | UTF-8 string, same escaping                   |
//! | `0x0D..=0x1D`| integer, `0x15` is zero (see below)           |
//! | `0x21`       | `f64`, 8 bytes, order-preserving bit flip     |
//! | `0x22`       | UUID, 16 raw bytes                            |
//!
//! **Invariant: every element's encoding is prefix-free** — no encoded element
//! is a proper prefix of another. This is what makes elements composable: two
//! tuples that first differ at element `i` are decided by bytes *inside*
//! element `i`, so nothing that follows can change the outcome, and a tuple
//! whose elements prefix another's encodes to a byte prefix and therefore sorts
//! below it. Any new type must preserve this.
//!
//! ## Integers
//!
//! An integer is encoded as `0x14 + len` for non-negatives and `0x14 - len` for
//! negatives, where `len` is the *minimal* number of big-endian magnitude bytes
//! (so zero is the bare code `0x14`). Negative magnitudes are stored
//! ones-complemented. Larger positives get a larger code, and larger negative
//! magnitudes a smaller one, so the code byte alone orders across magnitudes and
//! the magnitude bytes break ties. Non-minimal lengths are rejected on decode:
//! allowing them would give one value two encodings and silently break the
//! uniqueness of index keys.
//!
//! ## Byte strings
//!
//! `0x00` is escaped to `0x00 0xFF`, and the element ends with `0x00 0x00`. The
//! terminator sorts below every escaped zero and every ordinary byte, which is
//! what makes `"a" < "ab"` hold in the encoding too.
//!
//! The terminator is *two* bytes so that the encoding is prefix-free. With a
//! one-byte terminator, `enc("\xff")` = `04 ff 00` is a proper prefix of
//! `enc("\xff\0")` = `04 ff 00 ff 00`, because the shorter element's
//! terminator lines up with the longer one's escape marker. Ascending order
//! survives that (a prefix sorts below), but descending order does not:
//! complementing leaves the prefix relation intact, so both would still sort
//! the short one first instead of reversing.
//!
//! ## Descending order
//!
//! A [`Direction::Desc`] element is encoded exactly as above and then every byte
//! is complemented, which reverses its byte order while keeping it
//! self-delimiting. Because elements are prefix-free, complementing reverses
//! the order *exactly* — including for values of different encoded lengths.
//! Decoding complements on the way back in, so all the logic above is shared.

use crate::error::{Result, TupleError};
use crate::value::{Value, ValueType};
use bytes::Bytes;
use uuid::Uuid;

/// Sort direction of a single encoded element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Direction {
    /// Ascending: the encoding is emitted as-is.
    #[default]
    Asc,
    /// Descending: every byte of the encoding is complemented.
    Desc,
}

impl Direction {
    /// The XOR mask applied to every byte of an element in this direction.
    const fn mask(self) -> u8 {
        match self {
            Self::Asc => 0x00,
            Self::Desc => 0xFF,
        }
    }
}

mod codes {
    pub(super) const NULL: u8 = 0x01;
    pub(super) const FALSE: u8 = 0x02;
    pub(super) const TRUE: u8 = 0x03;
    pub(super) const BYTES: u8 = 0x04;
    pub(super) const STR: u8 = 0x05;
    pub(super) const INT_ZERO: u8 = 0x15;
    pub(super) const INT_MIN: u8 = INT_ZERO - 8;
    pub(super) const INT_MAX: u8 = INT_ZERO + 8;
    pub(super) const F64: u8 = 0x21;
    pub(super) const UUID: u8 = 0x22;

    pub(super) const NUL: u8 = 0x00;
    pub(super) const ESCAPE: u8 = 0xFF;
}

const SIGN_BIT: u64 = 1 << 63;

/// Map an `f64` onto a `u64` whose unsigned order matches float order.
///
/// NaN is canonicalised first so a value has exactly one encoding.
fn f64_to_ordered(v: f64) -> u64 {
    let bits = if v.is_nan() {
        f64::NAN.to_bits()
    } else {
        v.to_bits()
    };
    if bits & SIGN_BIT == 0 {
        bits ^ SIGN_BIT
    } else {
        !bits
    }
}

/// Inverse of [`f64_to_ordered`].
fn ordered_to_f64(ordered: u64) -> f64 {
    let bits = if ordered & SIGN_BIT == 0 {
        !ordered
    } else {
        ordered ^ SIGN_BIT
    };
    f64::from_bits(bits)
}

/// Minimal number of big-endian bytes needed to hold `magnitude`.
const fn magnitude_len(magnitude: u64) -> usize {
    8 - (magnitude.leading_zeros() / 8) as usize
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// A masked sink, so ascending and descending share one code path.
struct Sink<'a> {
    buf: &'a mut Vec<u8>,
    mask: u8,
}

impl Sink<'_> {
    fn push(&mut self, byte: u8) {
        self.buf.push(byte ^ self.mask);
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend(bytes.iter().map(|b| b ^ self.mask));
    }

    /// Write a byte string: `0x00` becomes `0x00 0xFF`, and the element ends
    /// with `0x00 0x00`. See the module docs for why the terminator is two
    /// bytes.
    fn push_escaped(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.push(b);
            if b == codes::NUL {
                self.push(codes::ESCAPE);
            }
        }
        self.push(codes::NUL);
        self.push(codes::NUL);
    }

    fn push_int(&mut self, negative: bool, magnitude: u64) {
        if magnitude == 0 {
            self.push(codes::INT_ZERO);
            return;
        }
        let len = magnitude_len(magnitude);
        // `len` is 1..=8 for a non-zero magnitude, so neither of these wraps.
        let code = if negative {
            codes::INT_ZERO - len as u8
        } else {
            codes::INT_ZERO + len as u8
        };
        self.push(code);
        let be = magnitude.to_be_bytes();
        for &b in be.iter().skip(8 - len) {
            self.push(if negative { !b } else { b });
        }
    }
}

/// Append one element's encoding to `out`.
pub fn encode_value_into(out: &mut Vec<u8>, value: &Value, direction: Direction) {
    let mut sink = Sink {
        buf: out,
        mask: direction.mask(),
    };
    match value {
        Value::Null => sink.push(codes::NULL),
        Value::Bool(false) => sink.push(codes::FALSE),
        Value::Bool(true) => sink.push(codes::TRUE),
        Value::Bytes(b) => {
            sink.push(codes::BYTES);
            sink.push_escaped(b);
        }
        Value::Str(s) => {
            sink.push(codes::STR);
            sink.push_escaped(s.as_bytes());
        }
        Value::I64(v) => {
            sink.push_int(*v < 0, v.unsigned_abs());
        }
        Value::U64(v) => {
            sink.push_int(false, *v);
        }
        Value::F64(v) => {
            sink.push(codes::F64);
            sink.extend(&f64_to_ordered(*v).to_be_bytes());
        }
        Value::Uuid(u) => {
            sink.push(codes::UUID);
            sink.extend(u.as_bytes());
        }
    }
}

/// Encode a tuple with every element ascending.
#[must_use]
pub fn encode(values: &[Value]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 9);
    for v in values {
        encode_value_into(&mut out, v, Direction::Asc);
    }
    out
}

/// Encode a tuple, taking each element's direction from `directions`.
///
/// Elements past the end of `directions` are encoded ascending, so passing an
/// empty slice is the same as [`encode`].
#[must_use]
pub fn encode_with(values: &[Value], directions: &[Direction]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 9);
    for (i, v) in values.iter().enumerate() {
        let dir = directions.get(i).copied().unwrap_or_default();
        encode_value_into(&mut out, v, dir);
    }
    out
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// A cursor over an encoded tuple.
///
/// The reader decodes one element at a time and leaves the rest of the buffer
/// untouched, so a caller that knows an index key's column count can decode
/// those columns and hand [`TupleReader::remainder`] to a second decode for the
/// trailing primary key.
#[derive(Debug, Clone)]
pub struct TupleReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> TupleReader<'a> {
    /// Start reading at the beginning of `buf`.
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Byte offset of the next unread element.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.pos
    }

    /// The not-yet-decoded remainder of the buffer.
    #[must_use]
    pub fn remainder(&self) -> &'a [u8] {
        self.buf.get(self.pos..).unwrap_or_default()
    }

    /// Whether every byte has been consumed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn byte(&mut self, mask: u8) -> Result<u8> {
        let b = self
            .buf
            .get(self.pos)
            .copied()
            .ok_or(TupleError::Truncated {
                offset: self.pos,
                needed: 1,
            })?;
        self.pos += 1;
        Ok(b ^ mask)
    }

    fn take(&mut self, n: usize, mask: u8) -> Result<Vec<u8>> {
        let end = self.pos.checked_add(n).ok_or(TupleError::Truncated {
            offset: self.pos,
            needed: n,
        })?;
        let slice = self.buf.get(self.pos..end).ok_or(TupleError::Truncated {
            offset: self.pos,
            needed: end - self.buf.len().min(end),
        })?;
        let out = slice.iter().map(|b| b ^ mask).collect();
        self.pos = end;
        Ok(out)
    }

    fn read_escaped(&mut self, mask: u8) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let b = self.byte(mask)?;
            if b != codes::NUL {
                out.push(b);
                continue;
            }
            let offset = self.pos - 1;
            match self.byte(mask)? {
                codes::NUL => return Ok(out),
                codes::ESCAPE => out.push(codes::NUL),
                _ => return Err(TupleError::MalformedEscape { offset }),
            }
        }
    }

    /// Read an integer body, returning `(negative, magnitude)`.
    fn read_int_body(&mut self, code: u8, mask: u8, start: usize) -> Result<(bool, u64)> {
        if code == codes::INT_ZERO {
            return Ok((false, 0));
        }
        let (negative, len) = if code > codes::INT_ZERO {
            (false, usize::from(code - codes::INT_ZERO))
        } else {
            (true, usize::from(codes::INT_ZERO - code))
        };
        let mut magnitude: u64 = 0;
        for _ in 0..len {
            let b = self.byte(mask)?;
            let b = if negative { !b } else { b };
            magnitude = (magnitude << 8) | u64::from(b);
        }
        // Reject leading zero bytes: one value must have exactly one encoding.
        if magnitude_len(magnitude) != len {
            return Err(TupleError::NonCanonicalInteger { offset: start });
        }
        Ok((negative, magnitude))
    }

    fn read_int_as(
        &mut self,
        code: u8,
        mask: u8,
        start: usize,
        target: ValueType,
    ) -> Result<Value> {
        let (negative, magnitude) = self.read_int_body(code, mask, start)?;
        let widened: i128 = if negative {
            -(magnitude as i128)
        } else {
            magnitude as i128
        };
        match target {
            ValueType::I64 => {
                i64::try_from(widened)
                    .map(Value::I64)
                    .map_err(|_| TupleError::IntegerOutOfRange {
                        offset: start,
                        value: widened,
                        target: "i64",
                    })
            }
            ValueType::U64 => {
                u64::try_from(widened)
                    .map(Value::U64)
                    .map_err(|_| TupleError::IntegerOutOfRange {
                        offset: start,
                        value: widened,
                        target: "u64",
                    })
            }
            other => Err(TupleError::TypeMismatch {
                offset: start,
                expected: other.name(),
                found: "integer",
            }),
        }
    }

    /// Decode the next element, which must be null or of type `expected`.
    ///
    /// Nullability is a column property, not a value property, so a null is
    /// always accepted here and rejected (or not) by the schema layer.
    pub fn read(&mut self, expected: ValueType, direction: Direction) -> Result<Value> {
        let mask = direction.mask();
        let start = self.pos;
        let code = self.byte(mask)?;

        if code == codes::NULL {
            return Ok(Value::Null);
        }
        if (codes::INT_MIN..=codes::INT_MAX).contains(&code) {
            return self.read_int_as(code, mask, start, expected);
        }

        let found = match code {
            codes::FALSE | codes::TRUE => "bool",
            codes::BYTES => "bytes",
            codes::STR => "string",
            codes::F64 => "f64",
            codes::UUID => "uuid",
            _ => {
                return Err(TupleError::UnknownTypeCode {
                    offset: start,
                    code,
                });
            }
        };

        let matches = matches!(
            (code, expected),
            (codes::FALSE | codes::TRUE, ValueType::Bool)
                | (codes::BYTES, ValueType::Bytes)
                | (codes::STR, ValueType::Str)
                | (codes::F64, ValueType::F64)
                | (codes::UUID, ValueType::Uuid)
        );
        if !matches {
            return Err(TupleError::TypeMismatch {
                offset: start,
                expected: expected.name(),
                found,
            });
        }

        self.read_body(code, mask, start)
    }

    /// Decode the next element using only its type code.
    ///
    /// Integers come back as [`Value::I64`] when they fit and [`Value::U64`]
    /// otherwise, since the two share an encoding. Use [`TupleReader::read`]
    /// whenever the schema is known; this exists for tooling and diagnostics.
    pub fn read_dynamic(&mut self, direction: Direction) -> Result<Value> {
        let mask = direction.mask();
        let start = self.pos;
        let code = self.byte(mask)?;

        if code == codes::NULL {
            return Ok(Value::Null);
        }
        if (codes::INT_MIN..=codes::INT_MAX).contains(&code) {
            let (negative, magnitude) = self.read_int_body(code, mask, start)?;
            let widened: i128 = if negative {
                -(magnitude as i128)
            } else {
                magnitude as i128
            };
            return Ok(i64::try_from(widened).map_or(Value::U64(magnitude), Value::I64));
        }
        self.read_body(code, mask, start)
    }

    /// Decode a non-null, non-integer body whose code has already been read.
    fn read_body(&mut self, code: u8, mask: u8, start: usize) -> Result<Value> {
        match code {
            codes::FALSE => Ok(Value::Bool(false)),
            codes::TRUE => Ok(Value::Bool(true)),
            codes::BYTES => Ok(Value::Bytes(Bytes::from(self.read_escaped(mask)?))),
            codes::STR => {
                let raw = self.read_escaped(mask)?;
                String::from_utf8(raw)
                    .map(Value::Str)
                    .map_err(|_| TupleError::InvalidUtf8 { offset: start })
            }
            codes::F64 => {
                let raw = self.take(8, mask)?;
                let mut be = [0u8; 8];
                for (slot, b) in be.iter_mut().zip(raw) {
                    *slot = b;
                }
                Ok(Value::F64(ordered_to_f64(u64::from_be_bytes(be))))
            }
            codes::UUID => {
                let raw = self.take(16, mask)?;
                let mut be = [0u8; 16];
                for (slot, b) in be.iter_mut().zip(raw) {
                    *slot = b;
                }
                Ok(Value::Uuid(Uuid::from_bytes(be)))
            }
            _ => Err(TupleError::UnknownTypeCode {
                offset: start,
                code,
            }),
        }
    }

    /// Advance past the next element without materialising it.
    pub fn skip(&mut self, direction: Direction) -> Result<()> {
        self.read_dynamic(direction).map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// Whole-tuple decoding
// ---------------------------------------------------------------------------

/// Decode exactly `types.len()` ascending elements, requiring the whole buffer
/// to be consumed.
pub fn decode(buf: &[u8], types: &[ValueType]) -> Result<Vec<Value>> {
    let (values, rest) = decode_prefix(buf, types)?;
    if !rest.is_empty() {
        return Err(TupleError::TrailingBytes {
            decoded: values.len(),
            remaining: rest.len(),
        });
    }
    Ok(values)
}

/// Decode `types.len()` ascending elements and return the unread remainder.
///
/// # Concatenation invariant
///
/// Anything appended after an encoded tuple must itself be an encoded tuple (or
/// nothing). A raw byte suffix beginning with `0xFF` would be misread as the
/// escaped NUL of a trailing string element, swallowing the suffix. Encoded
/// tuples always start with a type code in `0x01..=0xFE`, so composing them —
/// as an index key composes indexed columns with a primary key — is safe.
pub fn decode_prefix<'a>(buf: &'a [u8], types: &[ValueType]) -> Result<(Vec<Value>, &'a [u8])> {
    decode_prefix_with(buf, types, &[])
}

/// Decode `types.len()` elements with per-element directions, returning the
/// unread remainder. Directions past the end of the slice default to ascending.
pub fn decode_prefix_with<'a>(
    buf: &'a [u8],
    types: &[ValueType],
    directions: &[Direction],
) -> Result<(Vec<Value>, &'a [u8])> {
    let mut reader = TupleReader::new(buf);
    let mut out = Vec::with_capacity(types.len());
    for (i, ty) in types.iter().enumerate() {
        let dir = directions.get(i).copied().unwrap_or_default();
        out.push(reader.read(*ty, dir)?);
    }
    Ok((out, reader.remainder()))
}

/// Decode a whole buffer without schema guidance. See
/// [`TupleReader::read_dynamic`] for the integer caveat.
pub fn decode_dynamic(buf: &[u8]) -> Result<Vec<Value>> {
    let mut reader = TupleReader::new(buf);
    let mut out = Vec::new();
    while !reader.is_empty() {
        out.push(reader.read_dynamic(Direction::Asc)?);
    }
    Ok(out)
}
