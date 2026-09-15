//! What a stored entry costs to hold, which is not what it costs to write down.
//!
//! `MemoryStore` keys and values are `bytes::Bytes`, and `Bytes::from(Vec)`
//! adopts the vector's **capacity**, not its length, keeping it for the life of
//! the entry. Every encoder upstream sizes its buffer by a guess —
//! `slate_tuple::encode` reserves nine bytes a value, `keys` a header plus the
//! same — so without a shrink at the boundary a stored entry carries that
//! guess's error forever.
//!
//! On the 100,000-trip sample that was 88 bytes a row, about a fifth of what
//! the store held.
//!
//! **The footprint itself is not asserted here**, and the first version of this
//! file wrongly thought it was. It turned a stored `Bytes` back into a `Vec`
//! and checked `capacity() == len()` — but `Vec::from(Bytes)` reuses the
//! allocation only when the `Bytes` uniquely owns it, and one read out of the
//! store never does, so it copies and the capacity always equals the length.
//! The assertion passed with the shrink removed. Capacity is not observable
//! through `Bytes` at all, so the guard that the bytes really shrink lives in
//! `slate-slatedb`'s `tests/footprint.rs`, which installs a counting allocator
//! this crate must not.
//!
//! What is left here is the other half, and it is the half that could corrupt
//! something: shrinking reallocates, and a reallocated key must still hold the
//! same bytes, sort the same way, and match the tombstone that deletes it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use slate_kernel::KvStore;
use slate_kernel::memory::MemoryStore;

/// A buffer with the shape the encoders produce: a capacity guess, and rather
/// less in it than the guess reserved.
fn slack(len: usize, capacity: usize) -> Vec<u8> {
    assert!(capacity > len, "the point is the slack");
    let mut out = Vec::with_capacity(capacity);
    out.extend(std::iter::repeat_n(b'x', len));
    out
}

#[tokio::test]
async fn the_shrink_does_not_change_what_is_stored() {
    // The whole risk of shrinking in the write path: a `Vec` reallocated to a
    // smaller block must still hold the same bytes, and the key must still
    // compare and sort the same way. Cheap to rule out, and the kind of thing
    // that would otherwise be found by an oracle three weeks later.
    let store = MemoryStore::new();
    let txn = store.begin().await.unwrap();
    let mut expected = Vec::new();
    for i in 0u16..64 {
        let mut key = Vec::with_capacity(200);
        key.extend_from_slice(&i.to_be_bytes());
        let mut value = Vec::with_capacity(500);
        value.extend_from_slice(format!("value for {i}").as_bytes());
        expected.push((key.clone(), value.clone()));
        txn.put(key, value).unwrap();
    }
    txn.commit().await.unwrap();

    expected.sort();
    let stored: Vec<(Vec<u8>, Vec<u8>)> = store
        .entries()
        .into_iter()
        .map(|(key, value)| (key, value.to_vec()))
        .collect();
    assert_eq!(
        stored, expected,
        "shrinking changed the bytes or their order"
    );
}

#[tokio::test]
async fn a_tombstone_s_key_is_shrunk_too() {
    let store = MemoryStore::new();
    let txn = store.begin().await.unwrap();
    txn.put(slack(9, 64), slack(4, 64)).unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.delete(slack(9, 64)).unwrap();
    txn.commit().await.unwrap();
    assert_eq!(store.len(), 0, "the delete did not match the put");
}
