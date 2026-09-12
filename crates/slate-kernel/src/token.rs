//! Proof that a write happened, for reading it back.
//!
//! Adding read replicas breaks two things callers reasonably assume, and both
//! break silently and only under load:
//!
//! - **Read-your-writes.** Write to the head, read from a replica, and the row
//!   is not there yet.
//! - **Monotonic reads.** Two reads land on replicas at different lag, and the
//!   second sees less than the first — data appears to move backwards.
//!
//! One mechanism fixes both. A commit hands back the sequence number it landed
//! at; a read carrying that sequence may only be served by a view that has
//! reached it. Threading the highest token seen through a session gives
//! monotonic reads for free, because a later read can never be served by a view
//! behind an earlier one.
//!
//! The token is a *durable* sequence, so a write acknowledged before its flush
//! — see the `Visible` durability setting — is not readable from a replica yet.
//! That is a real constraint rather than an oversight: a replica reads object
//! storage, so there is nothing else it could observe.

/// A sequence number a read must be able to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReadToken(u64);

impl ReadToken {
    /// A token for `sequence`.
    #[must_use]
    pub const fn new(sequence: u64) -> Self {
        Self(sequence)
    }

    /// The sequence this token requires.
    #[must_use]
    pub const fn sequence(self) -> u64 {
        self.0
    }

    /// Whether a view at `visible` can serve a read carrying this token.
    #[must_use]
    pub const fn satisfied_by(self, visible: u64) -> bool {
        visible >= self.0
    }
}

/// The freshness a caller needs from a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Freshness {
    /// Any replica will do, however far behind.
    ///
    /// Correct for aggregate or dashboard reads, and the cheapest option
    /// because it never waits and never falls back to the writer.
    #[default]
    Any,
    /// The read must see at least this sequence.
    ///
    /// This is read-your-writes: pass the token from the commit whose effects
    /// must be visible.
    AtLeast(ReadToken),
    /// The read must go to the writer.
    ///
    /// The only way to see a write that has not been flushed yet, and the
    /// escape hatch when a caller cannot tolerate any lag.
    Latest,
}

/// Tracks the highest token a caller has seen, giving monotonic reads.
///
/// A session threads this through its reads: every commit raises the
/// watermark, and every subsequent read carries it, so no read can be served by
/// a view older than one the caller already observed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadWatermark {
    highest: Option<ReadToken>,
}

impl ReadWatermark {
    /// A watermark that has seen nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self { highest: None }
    }

    /// Record a token, keeping the highest seen.
    pub const fn observe(&mut self, token: ReadToken) {
        match self.highest {
            Some(current) if current.0 >= token.0 => {}
            _ => self.highest = Some(token),
        }
    }

    /// Record a commit's result, which is `None` when it wrote nothing.
    pub const fn observe_commit(&mut self, token: Option<ReadToken>) {
        if let Some(token) = token {
            self.observe(token);
        }
    }

    /// The highest token seen so far.
    #[must_use]
    pub const fn highest(self) -> Option<ReadToken> {
        self.highest
    }

    /// The freshness a read should ask for to stay monotonic.
    #[must_use]
    pub const fn freshness(self) -> Freshness {
        match self.highest {
            Some(token) => Freshness::AtLeast(token),
            None => Freshness::Any,
        }
    }
}
