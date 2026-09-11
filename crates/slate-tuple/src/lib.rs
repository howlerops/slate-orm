//! Order-preserving tuple encoding for the slate-orm record layer.
//!
//! The record layer stores everything in one ordered key-value keyspace, so the
//! encoding of a key *is* the index. This crate is the piece that guarantees
//! byte order equals value order:
//!
//! ```
//! use slate_tuple::{Value, ValueType, decode, encode};
//!
//! let a = encode(&[Value::Str("acme".into()), Value::I64(-1)]);
//! let b = encode(&[Value::Str("acme".into()), Value::I64(7)]);
//! assert!(a < b);
//!
//! let round_tripped = decode(&b, &[ValueType::Str, ValueType::I64]).unwrap();
//! assert_eq!(round_tripped, vec![Value::Str("acme".into()), Value::I64(7)]);
//! ```
//!
//! Decoding is schema-directed: the caller supplies the column types, which is
//! what lets signed and unsigned integers share one order-preserving encoding.
//! See [`codec`] for the wire format.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod codec;
pub mod error;
pub mod range;
pub mod value;

pub use codec::{
    Direction, TupleReader, decode, decode_dynamic, decode_prefix, decode_prefix_with, encode,
    encode_value_into, encode_with,
};
pub use error::{Result, TupleError};
pub use range::{key_successor, prefix_range, prefix_successor};
pub use value::{Value, ValueType};
