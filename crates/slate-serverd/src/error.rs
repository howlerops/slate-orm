//! Why the server would not start.
//!
//! One error type for the whole startup path, because there is exactly one
//! thing the process does with any of them: print it and exit non-zero. A
//! hierarchy of typed errors would let a caller distinguish cases, and there
//! is no caller — `main` is the only consumer, and the tests read the message.
//!
//! What the type does carry is *where*, separately from *what*. A message that
//! says only "`size > 'x'`: `x` is not an i64" leaves an operator searching a
//! long file for which of eleven indexes it means.

use core::fmt;

/// A refusal to start, with the place it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fault {
    /// Where in the configuration, as a human reads it: "table `docs`, index
    /// `by_kind`, `where`". Empty for a fault with no location.
    place: String,
    /// What is wrong, and where possible what to write instead.
    detail: String,
}

impl Fault {
    /// A fault with no particular location — a missing section, a bad address.
    pub(crate) fn new(detail: impl Into<String>) -> Self {
        Self {
            place: String::new(),
            detail: detail.into(),
        }
    }

    /// A fault at a named place in the file.
    pub(crate) fn at(place: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            place: place.into(),
            detail: detail.into(),
        }
    }

    /// Add an outer location to a fault raised deeper down.
    ///
    /// Prepended rather than replaced: the inner location is the specific one
    /// and dropping it would leave "in table `docs`" against an error about
    /// one of its indexes.
    pub(crate) fn within(mut self, place: impl Into<String>) -> Self {
        let outer = place.into();
        self.place = if self.place.is_empty() {
            outer
        } else {
            format!("{outer}, {}", self.place)
        };
        self
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.place.is_empty() {
            write!(f, "{}", self.detail)
        } else {
            write!(f, "in {}:\n{}", self.place, self.detail)
        }
    }
}

impl std::error::Error for Fault {}

/// Anything on the startup path.
pub(crate) type Started<T> = Result<T, Fault>;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn nesting_keeps_the_inner_location() {
        let fault = Fault::at("index `by_kind`, `where`", "nope").within("table `docs`");
        assert_eq!(
            fault.to_string(),
            "in table `docs`, index `by_kind`, `where`:\nnope"
        );
    }

    #[test]
    fn a_fault_with_no_place_prints_only_the_detail() {
        assert_eq!(Fault::new("nope").to_string(), "nope");
    }
}
