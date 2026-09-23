//! Turning a string into the terms an inverted index holds.
//!
//! One function, used in exactly two places that must agree or the index
//! returns nothing: the write path, which stores an entry per term, and the
//! evaluation of a `contains` predicate, which asks whether a row's text holds
//! them. A tokenizer that differs between those two is not a bug that shows up
//! as a crash — it is an index that quietly matches fewer rows than the table
//! contains, and the only way to notice is to compare against a scan.
//!
//! # What it does, and what it deliberately does not
//!
//! Lowercase, split on anything that is not a letter or a digit, drop what is
//! left empty, sort and deduplicate. `Ursula K. Le Guin` is
//! `["guin", "k", "le", "ursula"]`.
//!
//! **No stemming.** A stemmer is a language model: `universities → universe`
//! is Porter's own documented over-stem, and a wrong one does not fail, it
//! silently changes which rows match. It is also per-language, and nothing in
//! this layer knows what language a column holds. Offering an English stemmer
//! to a column of German product names would be worse than offering none,
//! because the caller would have no way to tell it was happening.
//!
//! **No stop words.** Dropping `to`, `be`, `or` and `not` makes
//! `to be or not to be` unsearchable, and the rows it saves are a rounding
//! error on a keyspace that is already one entry per distinct term per row.
//! The cost of a common term is paid by whoever searches for it, which is the
//! right person to pay it.
//!
//! **No Unicode normalisation beyond lowercasing.** `café` and `cafe` are
//! different terms here. Folding them together means NFD plus a combining-mark
//! filter, which is a dependency and a decision about which marks are
//! decoration — and in Swedish `å` is not an `a` with a hat on. Said plainly
//! rather than half-done.
//!
//! `char::is_alphanumeric` rather than `is_ascii_alphanumeric`, so a term in a
//! non-Latin script is a term rather than a gap between separators. The split
//! is therefore wrong for scripts that do not separate words — Japanese and
//! Chinese tokenize into one enormous term — and that is a real limit rather
//! than an oversight; segmenting them needs a dictionary.

/// The distinct terms of `text`, lowercased, sorted and deduplicated.
///
/// Sorted so that the entries a row writes are in key order, which is what
/// lets the write path compare two rows' term sets without allocating a map;
/// deduplicated because a second entry for the same term would be the same key
/// written twice, and the second write would silently replace the first.
#[must_use]
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|piece| !piece.is_empty())
        .map(str::to_lowercase)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::tokenize;

    #[test]
    fn it_lowercases_splits_sorts_and_deduplicates() {
        assert_eq!(tokenize("Ursula K. Le Guin"), ["guin", "k", "le", "ursula"]);
        // The repeat collapses, and the order is the keyspace's rather than
        // the sentence's.
        assert_eq!(tokenize("the cat the hat"), ["cat", "hat", "the"]);
    }

    #[test]
    fn punctuation_and_runs_of_it_are_separators_and_not_terms() {
        assert_eq!(tokenize("a--b, c!!!"), ["a", "b", "c"]);
        assert!(tokenize("!!! ??? ---").is_empty());
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn a_digit_is_part_of_a_term() {
        // `is_alphanumeric`, not `is_alphabetic`: a caller searching for a
        // model number is the obvious case, and splitting `k9` into `k` would
        // make it unfindable.
        assert_eq!(tokenize("Model K9 rev2"), ["k9", "model", "rev2"]);
    }

    #[test]
    fn a_non_latin_script_tokenizes_rather_than_vanishing() {
        // `is_alphanumeric` is the whole reason. Under
        // `is_ascii_alphanumeric` this would be three separators and no terms,
        // which is an empty index nobody would notice until a search over a
        // Cyrillic column returned nothing.
        // Sorted by code point rather than by any locale's collation, which
        // is why `и` lands between the other two: the keyspace is bytes and a
        // term's place in it is its encoding, not its place in an alphabet.
        assert_eq!(tokenize("Война и мир"), ["война", "и", "мир"]);
    }

    #[test]
    fn an_accent_is_part_of_the_term_it_is_on() {
        // Documented rather than folded: see the module docs. `café` and
        // `cafe` are different terms, and a caller who wants them together
        // normalises before storing.
        assert_eq!(tokenize("Café cafe"), ["cafe", "café"]);
    }
}
