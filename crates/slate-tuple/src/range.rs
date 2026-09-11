//! Key range helpers built on the encoding's prefix property.
//!
//! Encoded tuples are prefix-closed: every key beginning with `encode(&[a])`
//! is a key whose first element is `a`. That is what turns "first index column
//! equals `a`" into a contiguous scan, and what makes a tenant prefix a
//! physical isolation boundary rather than a filter.

use core::ops::Bound;

/// The smallest key that sorts strictly after every key starting with `prefix`.
///
/// Returns `None` when no such key exists — an empty prefix, or one made
/// entirely of `0xFF` bytes — which means the range is unbounded above.
#[must_use]
pub fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let last_incrementable = prefix.iter().rposition(|&b| b != u8::MAX)?;
    let mut out = prefix.get(..=last_incrementable)?.to_vec();
    // `last_incrementable` points at a byte below 0xFF, so this cannot wrap.
    if let Some(byte) = out.last_mut() {
        *byte += 1;
    }
    Some(out)
}

/// The half-open range covering exactly the keys that start with `prefix`.
#[must_use]
pub fn prefix_range(prefix: &[u8]) -> (Bound<Vec<u8>>, Bound<Vec<u8>>) {
    let start = Bound::Included(prefix.to_vec());
    match prefix_successor(prefix) {
        Some(end) => (start, Bound::Excluded(end)),
        None => (start, Bound::Unbounded),
    }
}

/// The smallest key that sorts strictly after `key`.
///
/// Appending `0x00` works for any key because no key is a strict prefix of
/// another with only a zero byte between them: this is the successor in the
/// "shortlex over all byte strings" order that the store uses.
#[must_use]
pub fn key_successor(key: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(key.len() + 1);
    out.extend_from_slice(key);
    out.push(0x00);
    out
}
