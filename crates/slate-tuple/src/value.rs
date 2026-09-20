//! The closed value model shared by the codec, the schema layer and the kernel.
//!
//! The set of variants is deliberately fixed. A closed type system is what lets
//! the codec guarantee that byte order equals value order, and lets the kernel
//! reason about index bounds without a dynamic type registry.

use bytes::Bytes;
use core::cmp::Ordering;
use uuid::Uuid;

/// A single tuple element.
///
/// [`Value`] has a *total* order (see the [`Ord`] impl) that is byte-for-byte
/// identical to the order of its encoding. Two values that compare equal always
/// encode to the same bytes, and vice versa.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Value {
    /// SQL-style null. Sorts before every other value.
    Null,
    /// Boolean. `false` sorts before `true`.
    Bool(bool),
    /// Opaque byte string.
    Bytes(Bytes),
    /// UTF-8 text. Ordered by its UTF-8 bytes, which matches code point order.
    Str(String),
    /// Signed 64-bit integer.
    I64(i64),
    /// Unsigned 64-bit integer. Shares an encoding with [`Value::I64`], so the
    /// two order numerically against each other.
    U64(u64),
    /// IEEE-754 double. NaN is canonicalised on encode and sorts above all
    /// other doubles, including positive infinity.
    F64(f64),
    /// An exact decimal, as a count of the column's smallest unit.
    ///
    /// **The scale lives in the schema, not in the value.** A `Decimal(1250)`
    /// in a column declared `scale = 2` is `12.50`; the same value in a column
    /// declared `scale = 0` is `1250`. That is the whole design, and it is what
    /// makes the type cheap and exact at once:
    ///
    /// - it encodes as an integer, so the ordering is the integer ordering —
    ///   already proven, already fuzzed, and impossible to get subtly wrong in
    ///   a way that silently corrupts index order;
    /// - every value in a column shares a scale, so comparison and `SUM` are
    ///   exact integer operations with no rescaling and no rounding;
    /// - `1.50` and `1.5` cannot both exist, so equal values have one encoding
    ///   — which index keys require and a self-describing decimal would have to
    ///   normalise for.
    ///
    /// The cost is that a value does not know how to print itself. Rendering
    /// needs the column, which every layer that renders one already has. That
    /// is stated plainly rather than hidden, because it is the one thing a
    /// caller has to remember.
    ///
    /// This is also what a careful application does with money in a database
    /// that has no decimal type — store minor units in an integer — with the
    /// difference that the scale is written down in the schema instead of in a
    /// comment, and nothing can read the column as an ordinary integer by
    /// accident.
    Decimal(i64),
    /// UUID, ordered by its 16 big-endian bytes.
    Uuid(Uuid),
    /// A dense vector of 32-bit floats, for embeddings.
    ///
    /// Ordered by length and then element-wise, which is deterministic and
    /// total but not *meaningful*: nothing about a vector's position in that
    /// order says anything about its similarity to another. The order exists
    /// so a vector can be grouped, deduplicated and stored, not so it can be
    /// ranged over — and the schema layer refuses a vector in a key or an
    /// index for exactly that reason.
    ///
    /// Nearness is a [`Scalar`](../slate_kernel/scalar/enum.Scalar.html)
    /// computed per row, and nearest-neighbour search is `ORDER BY` that with
    /// a `LIMIT`.
    Vector(Vec<f32>),
    /// A homogeneous list, ordered element-wise with a shorter list first.
    ///
    /// The element type lives on the column, not in this variant, for the same
    /// reason a decimal's scale does: [`ValueType`] is fieldless, `Copy` and
    /// `const`-matchable, and an `Array(Box<ValueType>)` would cost all three
    /// to describe one column. `docs/arrays.md` works that trade through.
    ///
    /// Ordering is `[1] < [1, 2] < [2]` — lexicographic, shorter-is-less —
    /// which is what an *ordered key* has to give and what a length-prefixed
    /// encoding gets backwards. The codec earns it with a terminator rather
    /// than a count; see `codec`'s module docs.
    ///
    /// **Arrays do not nest, in this version.** Not because the encoding
    /// cannot — it recurses naturally — but because [`ValueType::Array`]
    /// cannot name an inner element type, so a nested array is a value the
    /// schema cannot describe, and because depth would then be an input the
    /// caller chooses. The decoder refuses one rather than bounding it.
    Array(Vec<Value>),
}

/// The type tag of a [`Value`], used to drive schema-directed decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ValueType {
    /// See [`Value::Bool`].
    Bool,
    /// See [`Value::Bytes`].
    Bytes,
    /// See [`Value::Str`].
    Str,
    /// See [`Value::I64`].
    I64,
    /// See [`Value::U64`].
    U64,
    /// See [`Value::F64`].
    F64,
    /// See [`Value::Decimal`]. The scale is declared on the column.
    Decimal,
    /// See [`Value::Uuid`].
    Uuid,
    /// See [`Value::Vector`].
    Vector,
    /// See [`Value::Array`]. The element type is declared on the column.
    Array,
}

impl ValueType {
    /// Every variant, in declaration order.
    ///
    /// Exists so that a caller enumerating the type space cannot silently
    /// cover eight of nine. `crates/slate-tuple/tests/untrusted.rs` did
    /// exactly that: its list of types to fuzz was written before `Decimal`
    /// and nothing connected the two, so the adversarial decoder suite never
    /// once told the decoder to expect one.
    ///
    /// The enum is `#[non_exhaustive]`, so a test outside this crate *cannot*
    /// write the exhaustive match that would catch it — the compiler demands a
    /// wildcard and the wildcard is the hole. Inside the crate it can, which
    /// is why this list and the test that pins it live here rather than beside
    /// the fuzzer that needed them.
    pub const ALL: [Self; 10] = [
        Self::Bool,
        Self::Bytes,
        Self::Str,
        Self::I64,
        Self::U64,
        Self::F64,
        Self::Decimal,
        Self::Uuid,
        Self::Vector,
        Self::Array,
    ];

    /// A human-readable name, used in error messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Bytes => "bytes",
            Self::Str => "string",
            Self::I64 => "i64",
            Self::U64 => "u64",
            Self::F64 => "f64",
            Self::Decimal => "decimal",
            Self::Uuid => "uuid",
            Self::Vector => "vector",
            Self::Array => "array",
        }
    }
}

impl core::fmt::Display for ValueType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

impl Value {
    /// The type tag of this value, or `None` for [`Value::Null`].
    ///
    /// Null is not a type of its own: nullability is a property of a column, so
    /// a null carries no type tag at the value level.
    #[must_use]
    pub const fn value_type(&self) -> Option<ValueType> {
        match self {
            Self::Null => None,
            Self::Bool(_) => Some(ValueType::Bool),
            Self::Bytes(_) => Some(ValueType::Bytes),
            Self::Str(_) => Some(ValueType::Str),
            Self::I64(_) => Some(ValueType::I64),
            Self::U64(_) => Some(ValueType::U64),
            Self::F64(_) => Some(ValueType::F64),
            Self::Decimal(_) => Some(ValueType::Decimal),
            Self::Uuid(_) => Some(ValueType::Uuid),
            Self::Vector(_) => Some(ValueType::Vector),
            Self::Array(_) => Some(ValueType::Array),
        }
    }

    /// A human-readable type name, used in error messages.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self.value_type() {
            None => "null",
            Some(t) => t.name(),
        }
    }

    /// Whether this value is [`Value::Null`].
    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Read `text` as a decimal at `scale`, as a count of the smallest unit.
    ///
    /// `"19.99"` at scale 2 is `Decimal(1999)`; `"19.9"` at scale 2 is
    /// `Decimal(1990)`, because a written number shorter than the scale is
    /// padded, not reinterpreted. `"19"` is `Decimal(1900)`.
    ///
    /// # Why this is not `f64::parse` followed by a multiply
    ///
    /// Because that is the bug this type exists to avoid, and it is not
    /// theoretical: `("8.20".parse::<f64>() * 100.0) as i64` is **819**, since
    /// 8.2 has no exact double and the product lands just below 820.
    /// Splitting on the point and reading two integers has no such step, and
    /// is also the only version that can tell `"19.999"` at scale 2 from
    /// `"19.99"` — the first is refused rather than quietly rounded, because
    /// a caller who wrote three places meant three.
    ///
    /// # Errors
    /// A message naming what is wrong with `text`, suitable for showing to
    /// whoever typed it: a bad character, more places than the scale, or a
    /// magnitude past `i64`.
    pub fn decimal_from_str(text: &str, scale: u8) -> Result<Self, String> {
        let text = text.trim();
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (whole, fraction) = match digits.split_once('.') {
            Some((w, f)) => (w, f),
            None => (digits, ""),
        };
        // An empty whole part is how `".50"` arrives and it means zero — but
        // only when there are digits *somewhere*. `"."` and `""` and `"-"`
        // each leave both halves empty, and none of them is a number. The
        // first version substituted the zero first and then checked, which
        // read `"."` as zero; `what_is_not_a_decimal` caught it.
        if whole.is_empty() && fraction.is_empty() {
            return Err(format!("{text:?} is not a decimal number"));
        }
        let whole = if whole.is_empty() { "0" } else { whole };
        if !whole.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(format!("{text:?} is not a decimal number"));
        }
        let places = usize::from(scale);
        // More places than the column has is refused only when the extra
        // digits carry something. `19.830` at scale 2 is exactly 1983 units
        // and refusing it would be pedantry — a trailing zero is how people
        // write a price, and a number that loses nothing has not lost
        // anything. `19.999` is a different matter: two of those digits cannot
        // be held, and dropping them silently is the whole failure mode.
        let (fraction, dropped) = fraction.split_at(fraction.len().min(places));
        if !dropped.bytes().all(|b| b == b'0') {
            return Err(format!(
                "{text:?} has {} places after the point and this column has {places} — \
                 the last {} would be dropped and {dropped:?} is not zero, so write \
                 the number this column can hold or change the column's scale",
                fraction.len() + dropped.len(),
                dropped.len()
            ));
        }
        // Pad rather than scale by a power of ten: `"19.9"` at scale 2 is
        // 1990 units, and the padding is what makes that obvious rather than
        // a multiplication whose exponent has to be derived.
        let mut units = String::with_capacity(whole.len() + places);
        units.push_str(whole);
        units.push_str(fraction);
        for _ in fraction.len()..places {
            units.push('0');
        }
        let magnitude = units
            .parse::<i128>()
            .map_err(|_| format!("{text:?} does not fit in a decimal column"))?;
        let signed = if negative { -magnitude } else { magnitude };
        i64::try_from(signed)
            .map(Self::Decimal)
            .map_err(|_| format!("{text:?} does not fit in a decimal column"))
    }

    /// Write `units` at `scale` the way the number was written down.
    ///
    /// The inverse of [`Value::decimal_from_str`] wherever both can represent
    /// the value, which `decimal_text_round_trips` checks by sampling the
    /// whole `i64` range.
    ///
    /// Takes the units rather than `&self` because the only value it can
    /// render is a [`Value::Decimal`], and a method that silently did
    /// something else for the other eight variants would be a worse API than
    /// one the caller has to unwrap for.
    #[must_use]
    pub fn decimal_to_string(units: i64, scale: u8) -> String {
        let places = usize::from(scale);
        if places == 0 {
            return units.to_string();
        }
        // Through `unsigned_abs` rather than `-units`, because `i64::MIN` has
        // no positive counterpart and negating it panics in debug and wraps in
        // release. The sign is carried separately and put back at the end.
        let magnitude = units.unsigned_abs().to_string();
        let padded = if magnitude.len() <= places {
            format!("{}{magnitude}", "0".repeat(places - magnitude.len() + 1))
        } else {
            magnitude
        };
        let split = padded.len() - places;
        format!(
            "{}{}.{}",
            if units < 0 { "-" } else { "" },
            &padded[..split],
            &padded[split..]
        )
    }

    /// Rank of the value's *class* in the cross-type order.
    ///
    /// Mirrors the ordering of the type codes emitted by the codec:
    /// null < bool < bytes < string < integer < decimal < double < uuid <
    /// vector < array.
    /// Signed and unsigned integers share a rank because they share an encoding.
    ///
    /// A decimal gets a rank of its own rather than sharing the integers'. It
    /// encodes *as* an integer, so sharing would be tempting and would be
    /// wrong: the integer `1250` and a scale-2 decimal `12.50` have the same
    /// units and are not the same number, and a shared rank would make them
    /// compare equal. They are different types and they sort apart.
    const fn class_rank(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Bool(_) => 1,
            Self::Bytes(_) => 2,
            Self::Str(_) => 3,
            Self::I64(_) | Self::U64(_) => 4,
            Self::Decimal(_) => 5,
            Self::F64(_) => 6,
            Self::Uuid(_) => 7,
            Self::Vector(_) => 8,
            Self::Array(_) => 9,
        }
    }

    /// Widen any integer variant to `i128` so signed and unsigned compare.
    const fn as_i128(&self) -> Option<i128> {
        match self {
            Self::I64(v) => Some(*v as i128),
            Self::U64(v) => Some(*v as i128),
            _ => None,
        }
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        let rank = self.class_rank().cmp(&other.class_rank());
        if rank != Ordering::Equal {
            return rank;
        }
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Bytes(a), Self::Bytes(b)) => a.cmp(b),
            (Self::Str(a), Self::Str(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Self::Uuid(a), Self::Uuid(b)) => a.as_bytes().cmp(b.as_bytes()),
            // Length first so the encoding, which is length-prefixed, sorts
            // the same way. Element-wise after that, with NaN handled as it is
            // for a lone double so two vectors that encode alike compare alike.
            (Self::Vector(a), Self::Vector(b)) => a.len().cmp(&b.len()).then_with(|| {
                for (x, y) in a.iter().zip(b) {
                    let ordering = match (x.is_nan(), y.is_nan()) {
                        (true, true) => Ordering::Equal,
                        (true, false) => Ordering::Greater,
                        (false, true) => Ordering::Less,
                        (false, false) => x.total_cmp(y),
                    };
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
                Ordering::Equal
            }),
            // `Vec<Value>`'s own ordering, which is lexicographic with a
            // shorter list first — exactly what the encoding's terminator
            // gives, and the reason the terminator was chosen over a count.
            // Written as a delegation rather than a hand-rolled loop because a
            // hand-rolled one is where the two could drift apart, and the
            // ordering oracle would then be checking a copy of the bug.
            (Self::Array(a), Self::Array(b)) => a.cmp(b),
            // NaN is canonicalised on encode, so all NaNs are one value here.
            (Self::F64(a), Self::F64(b)) => match (a.is_nan(), b.is_nan()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => a.total_cmp(b),
            },
            // Units against units. Every value in a decimal column shares a
            // scale, so this is the whole comparison — no rescaling, no
            // rounding, and exactly the integer ordering the encoding gives.
            (Self::Decimal(a), Self::Decimal(b)) => a.cmp(b),
            _ => match (self.as_i128(), other.as_i128()) {
                (Some(a), Some(b)) => a.cmp(&b),
                // The integers are the only class sharing a rank across
                // variants, so anything else reaching here is a variant added
                // without an arm above.
                //
                // That is not hypothetical: `Value::Decimal` was added with a
                // rank and no arm, fell through to here, and every decimal
                // compared *equal* to every other. `byte_order_matches_value_order`
                // failed on the first run with `i64::MAX` against `i64::MIN`,
                // which is the property suite doing its job — and the reason
                // this arm no longer claims to be unreachable.
                _ => Ordering::Equal,
            },
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

macro_rules! from_impl {
    ($($ty:ty => $variant:ident $(as $cast:ty)?),* $(,)?) => {
        $(impl From<$ty> for Value {
            fn from(v: $ty) -> Self {
                Self::$variant(v $(as $cast)?)
            }
        })*
    };
}

from_impl! {
    bool => Bool,
    i8 => I64 as i64,
    i16 => I64 as i64,
    i32 => I64 as i64,
    i64 => I64,
    u8 => U64 as u64,
    u16 => U64 as u64,
    u32 => U64 as u64,
    u64 => U64,
    f32 => F64 as f64,
    f64 => F64,
    String => Str,
    Bytes => Bytes,
    Uuid => Uuid,
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::Str(v.to_owned())
    }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Self::Bytes(Bytes::from(v))
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Self::Null, Into::into)
    }
}

#[cfg(test)]
// Both guards report a collision by naming the two types that collided, which
// reads better as a `panic!` than as an `assert!` over an `Option`.
#[allow(clippy::panic)]
mod value_type_tests {
    use super::ValueType;

    /// Each type prints as itself, and no two print alike.
    ///
    /// `name` feeds `TypeMismatch`, which reads "expected {expected}, found
    /// {found}" — two types sharing a name turn that into "expected vector,
    /// found vector", which is a message that cannot be acted on. Nothing
    /// tested `name` at all, for any type, until a mutation renaming `Array`
    /// to `"vector"` survived the whole suite.
    #[test]
    fn every_type_has_a_name_of_its_own() {
        let mut seen: std::collections::BTreeMap<&str, ValueType> =
            std::collections::BTreeMap::new();
        for kind in ValueType::ALL {
            let name = kind.name();
            assert!(!name.is_empty(), "{kind:?} has no name");
            if let Some(clash) = seen.insert(name, kind) {
                panic!("{kind:?} and {clash:?} both print as {name:?}");
            }
        }
    }

    /// `ALL` lists every variant, held to the enum by the compiler.
    ///
    /// The `match` is exhaustive and wildcard-free, which is only possible in
    /// the crate that defines a `#[non_exhaustive]` enum. An eleventh variant
    /// stops this compiling, naming this test, before any suite goes quietly
    /// one type short.
    #[test]
    fn all_lists_every_variant() {
        fn position(kind: ValueType) -> usize {
            match kind {
                ValueType::Bool => 0,
                ValueType::Bytes => 1,
                ValueType::Str => 2,
                ValueType::I64 => 3,
                ValueType::U64 => 4,
                ValueType::F64 => 5,
                ValueType::Decimal => 6,
                ValueType::Uuid => 7,
                ValueType::Vector => 8,
                ValueType::Array => 9,
            }
        }

        // Positions rather than `contains`: it checks membership *and* that
        // nothing is listed twice, which a `contains` loop over a list with a
        // duplicate and a gap would pass.
        for (at, kind) in ValueType::ALL.iter().enumerate() {
            assert_eq!(position(*kind), at, "{kind} is out of place in ALL");
        }
    }
}
