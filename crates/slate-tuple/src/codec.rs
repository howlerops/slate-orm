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
//! order `null < bool < bytes < string < integer < decimal < double < uuid <
//! vector < array`, which is the order [`Value`]'s own `Ord` gives:
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
//! | `0x23`       | vector, `u32` count then that many `f32`      |
//! | `0x24`       | array, elements in full then `0x00`           |
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
    /// Decimal: this code, then the *integer* encoding of the units.
    ///
    /// Between the integers and `F64` in the cross-type order, and layered on
    /// the integer encoding rather than replacing it. Reusing it is the point:
    /// the ordering of a decimal column is then the ordering of an integer
    /// column, which is already proven, already fuzzed, and cannot be got
    /// subtly wrong in a way that silently corrupts index order. The one new
    /// byte is what keeps a decimal from comparing equal to an integer with
    /// the same units.
    pub(super) const DECIMAL: u8 = 0x1E;
    pub(super) const F64: u8 = 0x21;
    pub(super) const UUID: u8 = 0x22;
    pub(super) const VECTOR: u8 = 0x23;
    /// Array: this code, every element in full, then `NUL`.
    ///
    /// Terminated rather than length-prefixed, which is the one decision in
    /// the encoding that could have gone the other way and been wrong. A
    /// count sorts `[2]` below `[1, 2]`, because one is less than two before
    /// any element is looked at; a terminator sorts them the way lists sort,
    /// because `NUL` is below every element tag (`NULL` is `0x01`) so the
    /// shorter list meets its end where the longer one still has a value.
    ///
    /// No escaping at this level, which is the part that looks unsafe and is
    /// not. An element body may well contain a `0x00`, but the array decoder
    /// never scans for the terminator: it reads elements one at a time, each
    /// consuming exactly its own bytes, so `0x00` is only ever examined on an
    /// element boundary — where no tag can be `0x00`.
    pub(super) const ARRAY: u8 = 0x24;

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

/// [`f64_to_ordered`] for a 32-bit float.
///
/// The same trick at half the width: flip the sign bit of a positive, invert
/// a negative, so the unsigned bit patterns sort as the floats do.
fn f32_to_ordered(v: f32) -> u32 {
    const SIGN: u32 = 1 << 31;
    let bits = if v.is_nan() {
        f32::NAN.to_bits()
    } else {
        v.to_bits()
    };
    if bits & SIGN == 0 { bits ^ SIGN } else { !bits }
}

/// Inverse of [`f32_to_ordered`].
fn ordered_to_f32(ordered: u32) -> f32 {
    const SIGN: u32 = 1 << 31;
    let bits = if ordered & SIGN == 0 {
        !ordered
    } else {
        ordered ^ SIGN
    };
    f32::from_bits(bits)
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

    /// Encode a nested element into the same buffer.
    ///
    /// A `Sink` borrows the output vector for its whole life, so an array —
    /// whose elements are encoded by recursing on that vector — cannot reach
    /// it any other way. Reborrowing through the sink rather than dropping it
    /// keeps every arm of the encoder looking the same.
    fn push_element(&mut self, value: &Value, direction: Direction) {
        encode_value_into(self.buf, value, direction);
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

/// Encode the *start* of a string or byte value: everything a longer value
/// sharing this prefix would also begin with.
///
/// The full encoding ends in a terminator, which is what makes it prefix-free
/// and therefore sortable. Leaving the terminator off gives the opposite and
/// equally useful thing: a byte prefix shared by every encoding of a value
/// that starts this way. `LIKE 'abc%'` becomes a key range through this.
///
/// Returns `false` and writes nothing for a value that is not a string or
/// bytes, since no other type has meaningful prefixes.
pub fn encode_prefix_into(out: &mut Vec<u8>, value: &Value, direction: Direction) -> bool {
    let mut sink = Sink {
        buf: out,
        mask: direction.mask(),
    };
    let bytes = match value {
        Value::Bytes(b) => {
            sink.push(codes::BYTES);
            &b[..]
        }
        Value::Str(s) => {
            sink.push(codes::STR);
            s.as_bytes()
        }
        _ => return false,
    };
    for &b in bytes {
        sink.push(b);
        if b == codes::NUL {
            sink.push(codes::ESCAPE);
        }
    }
    true
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
        Value::Decimal(v) => {
            sink.push(codes::DECIMAL);
            // The integer encoding, unchanged, including its own type code.
            // Prefix-freeness comes along with it: an integer element is
            // prefix-free, so one behind a fixed byte still is.
            sink.push_int(*v < 0, v.unsigned_abs());
        }
        Value::F64(v) => {
            sink.push(codes::F64);
            sink.extend(&f64_to_ordered(*v).to_be_bytes());
        }
        Value::Vector(elements) => {
            sink.push(codes::VECTOR);
            // Length-prefixed rather than terminated. A vector's elements are
            // fixed width, so the count is enough to know where it ends —
            // which makes the encoding prefix-free without needing an escape,
            // and makes the byte order match the value order: shorter first,
            // then element-wise.
            let count = u32::try_from(elements.len()).unwrap_or(u32::MAX);
            sink.extend(&count.to_be_bytes());
            for element in elements.iter().take(count as usize) {
                sink.extend(&f32_to_ordered(*element).to_be_bytes());
            }
        }
        Value::Uuid(u) => {
            sink.push(codes::UUID);
            sink.extend(u.as_bytes());
        }
        Value::Array(elements) => {
            sink.push(codes::ARRAY);
            for element in elements {
                sink.push_element(element, direction);
            }
            // See `codes::ARRAY` for why this is a terminator and not a count.
            sink.push(codes::NUL);
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

    /// A big-endian `u32`, for a vector's element count.
    fn take_u32(&mut self, mask: u8) -> Result<u32> {
        let raw = self.take(4, mask)?;
        let mut be = [0u8; 4];
        for (slot, b) in be.iter_mut().zip(raw) {
            *slot = b;
        }
        Ok(u32::from_be_bytes(be))
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
            codes::DECIMAL => "decimal",
            codes::UUID => "uuid",
            codes::VECTOR => "vector",
            codes::ARRAY => "array",
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
                | (codes::VECTOR, ValueType::Vector)
                | (codes::DECIMAL, ValueType::Decimal)
                | (codes::ARRAY, ValueType::Array)
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
        self.read_dynamic_masked(direction.mask())
    }

    /// [`TupleReader::read_dynamic`] with the mask already resolved.
    ///
    /// Split out for the array decoder, which has a mask and no `Direction`:
    /// reconstructing one from the mask would be a second place that has to
    /// agree with [`Direction::mask`], and the compiler would not check it.
    fn read_dynamic_masked(&mut self, mask: u8) -> Result<Value> {
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
            codes::DECIMAL => {
                let code = self.byte(mask)?;
                if !(codes::INT_MIN..=codes::INT_MAX).contains(&code) {
                    return Err(TupleError::UnknownTypeCode {
                        offset: start,
                        code,
                    });
                }
                let (negative, magnitude) = self.read_int_body(code, mask, start)?;
                // Through `i128` so that `i64::MIN` survives the round trip:
                // its magnitude does not fit in an `i64`, so negating after
                // narrowing would overflow. The `try_from` then refuses
                // anything genuinely out of range rather than wrapping.
                let widened: i128 = if negative {
                    -(magnitude as i128)
                } else {
                    magnitude as i128
                };
                i64::try_from(widened).map(Value::Decimal).map_err(|_| {
                    TupleError::UnknownTypeCode {
                        offset: start,
                        code,
                    }
                })
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
            codes::VECTOR => {
                let count = self.take_u32(mask)?;
                let mut elements = Vec::with_capacity(count.min(1 << 16) as usize);
                for _ in 0..count {
                    let raw = self.take(4, mask)?;
                    let mut be = [0u8; 4];
                    for (slot, b) in be.iter_mut().zip(raw) {
                        *slot = b;
                    }
                    elements.push(ordered_to_f32(u32::from_be_bytes(be)));
                }
                Ok(Value::Vector(elements))
            }
            codes::ARRAY => self.read_array(mask),
            _ => Err(TupleError::UnknownTypeCode {
                offset: start,
                code,
            }),
        }
    }

    /// Decode an array body: elements until the terminator.
    ///
    /// Elements come back dynamically typed rather than schema-directed,
    /// which for integers means `I64` where it fits and `U64` otherwise —
    /// the same answer [`TupleReader::read_dynamic`] gives a bare integer, and
    /// the same *bytes* either way, so nothing about the ordering or the
    /// round trip depends on which one you get.
    ///
    /// **An array inside an array is refused, not counted.** Decision 3 of
    /// `docs/arrays.md` declines nesting at the schema level because
    /// [`ValueType::Array`] is fieldless and cannot name an inner element
    /// type. A decoder that accepted what the schema cannot express would be
    /// recursing to a depth its caller chooses, on bytes that need not have
    /// come from this encoder — the stack-overflow shape the wire's
    /// expression converter already carries an explicit ceiling against.
    /// Refusing outright is both cheaper than a ceiling and the truth about
    /// what this version stores.
    fn read_array(&mut self, mask: u8) -> Result<Value> {
        let mut elements = Vec::new();
        loop {
            let at = self.pos;
            let code = self.byte(mask)?;
            if code == codes::NUL {
                return Ok(Value::Array(elements));
            }
            if code == codes::ARRAY {
                return Err(TupleError::UnknownTypeCode { offset: at, code });
            }
            // Rewound because every element decoder reads its own tag. Peeking
            // rather than dispatching here is what keeps this loop from being
            // a second copy of `read_dynamic`'s table.
            self.pos = at;
            elements.push(self.read_dynamic_masked(mask)?);
        }
    }

    /// Advance past the next element without materialising it.
    ///
    /// Genuinely without: decoding a value only to drop it allocated a `String`
    /// or a `Bytes` per skipped column, which is the whole cost of skipping a
    /// column a query does not read.
    pub fn skip(&mut self, direction: Direction) -> Result<()> {
        let mask = direction.mask();
        let start = self.pos;
        let code = self.byte(mask)?;

        if code == codes::NULL || code == codes::FALSE || code == codes::TRUE {
            return Ok(());
        }
        if (codes::INT_MIN..=codes::INT_MAX).contains(&code) {
            let len = if code > codes::INT_ZERO {
                usize::from(code - codes::INT_ZERO)
            } else {
                usize::from(codes::INT_ZERO - code)
            };
            return self.advance(len);
        }
        match code {
            codes::BYTES | codes::STR => self.skip_escaped(mask),
            codes::F64 => self.advance(8),
            // One byte, then a whole integer element — so skipping a decimal
            // is reading its inner code and skipping that. Recursing rather
            // than duplicating the length arithmetic above: the two would
            // otherwise have to be kept in step by hand, and a skip that
            // disagreed with the decoder by one byte would read the *next*
            // column's bytes as this one's.
            codes::DECIMAL => {
                let inner = self.byte(mask)?;
                if !(codes::INT_MIN..=codes::INT_MAX).contains(&inner) {
                    return Err(TupleError::UnknownTypeCode {
                        offset: start,
                        code: inner,
                    });
                }
                let len = if inner > codes::INT_ZERO {
                    usize::from(inner - codes::INT_ZERO)
                } else {
                    usize::from(codes::INT_ZERO - inner)
                };
                self.advance(len)
            }
            codes::UUID => self.advance(16),
            codes::VECTOR => {
                let count = self.take_u32(mask)?;
                self.advance(count as usize * 4)
            }
            // An array has no length prefix, so skipping one is reading one
            // without keeping it. The saving over `read_array` is the `Value`
            // per element, which is the whole reason `skip` exists.
            //
            // The nesting refusal is written out again rather than shared,
            // and that is a real cost: it is a second place that has to agree
            // with the decoder, exactly the hazard the `DECIMAL` arm above
            // avoids by recursing. Sharing is what would break here — the two
            // differ in precisely the thing `skip` is for — so
            // `a_deeply_nested_array_is_refused_rather_than_recursed` asserts
            // the refusal at both entry points, and `skipping_agrees_with_decoding`
            // holds the lengths together over generated arrays.
            codes::ARRAY => loop {
                let at = self.pos;
                let inner = self.byte(mask)?;
                if inner == codes::NUL {
                    return Ok(());
                }
                if inner == codes::ARRAY {
                    return Err(TupleError::UnknownTypeCode {
                        offset: at,
                        code: inner,
                    });
                }
                self.pos = at;
                self.skip(direction)?;
            },
            _ => Err(TupleError::UnknownTypeCode {
                offset: start,
                code,
            }),
        }
    }

    /// Move past `n` bytes, or report how many were missing.
    fn advance(&mut self, n: usize) -> Result<()> {
        let end = self.pos.checked_add(n).filter(|end| *end <= self.buf.len());
        match end {
            Some(end) => {
                self.pos = end;
                Ok(())
            }
            None => Err(TupleError::Truncated {
                offset: self.pos,
                needed: n - (self.buf.len() - self.pos.min(self.buf.len())),
            }),
        }
    }

    /// Move past a zero-escaped byte string without copying it.
    fn skip_escaped(&mut self, mask: u8) -> Result<()> {
        loop {
            if self.byte(mask)? != codes::NUL {
                continue;
            }
            let offset = self.pos - 1;
            match self.byte(mask)? {
                codes::NUL => return Ok(()),
                codes::ESCAPE => {}
                _ => return Err(TupleError::MalformedEscape { offset }),
            }
        }
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
