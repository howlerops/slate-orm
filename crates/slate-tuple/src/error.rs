//! Errors produced by the tuple codec.

/// Failures that can occur while decoding an encoded tuple.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TupleError {
    /// The input ended in the middle of an element.
    #[error("truncated tuple: needed {needed} more byte(s) at offset {offset}")]
    Truncated {
        /// Byte offset at which the read ran out of input.
        offset: usize,
        /// Number of additional bytes the read required.
        needed: usize,
    },

    /// A type code byte was read that this codec version does not define.
    #[error("unknown type code 0x{code:02x} at offset {offset}")]
    UnknownTypeCode {
        /// Byte offset of the offending type code.
        offset: usize,
        /// The undefined code.
        code: u8,
    },

    /// The element decoded correctly but is not of the type the caller asked for.
    #[error("type mismatch at offset {offset}: expected {expected}, found {found}")]
    TypeMismatch {
        /// Byte offset of the element's type code.
        offset: usize,
        /// The type the caller requested.
        expected: &'static str,
        /// The type actually present.
        found: &'static str,
    },

    /// An integer decoded successfully but does not fit the requested width.
    #[error("integer {value} at offset {offset} does not fit in {target}")]
    IntegerOutOfRange {
        /// Byte offset of the element.
        offset: usize,
        /// The decoded value, widened.
        value: i128,
        /// Name of the requested target type.
        target: &'static str,
    },

    /// A string element did not contain valid UTF-8.
    #[error("invalid UTF-8 in string element at offset {offset}")]
    InvalidUtf8 {
        /// Byte offset of the element.
        offset: usize,
    },

    /// A byte string's escape sequence was malformed.
    #[error("malformed escape sequence at offset {offset}")]
    MalformedEscape {
        /// Byte offset of the bad escape.
        offset: usize,
    },

    /// An integer used a non-minimal byte length, which would give one value
    /// two distinct encodings and break index-key uniqueness.
    #[error("non-canonical integer encoding at offset {offset}")]
    NonCanonicalInteger {
        /// Byte offset of the element.
        offset: usize,
    },

    /// Bytes remained after decoding every requested element.
    #[error("{remaining} trailing byte(s) after decoding {decoded} element(s)")]
    TrailingBytes {
        /// Number of elements successfully decoded.
        decoded: usize,
        /// Number of bytes left over.
        remaining: usize,
    },
}

/// Convenience alias for tuple codec results.
pub type Result<T> = core::result::Result<T, TupleError>;
