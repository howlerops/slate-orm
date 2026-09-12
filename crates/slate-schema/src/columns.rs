//! A set of column ordinals.
//!
//! Built once per query and consulted once per column per row, so it is a
//! bitset rather than a list: the inner loop asks "is this column wanted?" for
//! every column of every row, and a linear scan there is the kind of thing that
//! quietly costs more than the work it guards.

use crate::table::Ordinal;

const BITS: usize = u64::BITS as usize;

/// Which columns of a table something needs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColumnSet {
    words: Vec<u64>,
    len: usize,
}

impl ColumnSet {
    /// An empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            words: Vec::new(),
            len: 0,
        }
    }

    /// Every column of a table with `columns` columns.
    #[must_use]
    pub fn all(columns: usize) -> Self {
        let mut set = Self::new();
        for ordinal in 0..columns {
            set.insert(Ordinal(ordinal));
        }
        set
    }

    /// Add a column.
    pub fn insert(&mut self, ordinal: Ordinal) {
        let word = ordinal.0 / BITS;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        if let Some(slot) = self.words.get_mut(word) {
            let bit = 1u64 << (ordinal.0 % BITS);
            if *slot & bit == 0 {
                *slot |= bit;
                self.len += 1;
            }
        }
    }

    /// Whether `ordinal` is in the set.
    #[must_use]
    pub fn contains(&self, ordinal: Ordinal) -> bool {
        self.words
            .get(ordinal.0 / BITS)
            .is_some_and(|word| word & (1u64 << (ordinal.0 % BITS)) != 0)
    }

    /// How many columns are in the set.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Add every column of `other`.
    pub fn union_with(&mut self, other: &Self) {
        if other.words.len() > self.words.len() {
            self.words.resize(other.words.len(), 0);
        }
        self.len = 0;
        for (slot, word) in self.words.iter_mut().zip(other.words.iter().chain([&0u64])) {
            *slot |= *word;
        }
        self.len = self.words.iter().map(|w| w.count_ones() as usize).sum();
    }

    /// Whether the set holds every column of a table with `columns` columns.
    ///
    /// Worth asking: a decode that wants everything should not pay a membership
    /// check per column to discover that.
    #[must_use]
    pub const fn covers_all(&self, columns: usize) -> bool {
        self.len >= columns
    }

    /// Whether every column of `other` is also in this set.
    #[must_use]
    pub fn contains_all(&self, other: &Self) -> bool {
        other
            .words
            .iter()
            .enumerate()
            .all(|(i, word)| self.words.get(i).is_some_and(|mine| mine & word == *word))
    }
}

impl FromIterator<Ordinal> for ColumnSet {
    fn from_iter<I: IntoIterator<Item = Ordinal>>(iter: I) -> Self {
        let mut set = Self::new();
        for ordinal in iter {
            set.insert(ordinal);
        }
        set
    }
}
