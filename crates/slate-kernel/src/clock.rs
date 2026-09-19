//! Where a managed column's timestamp comes from.
//!
//! # Why this is a trait and not `SystemTime::now()`
//!
//! Calling the wall clock directly in the write path would be three lines and
//! would make every test that touches a managed column a test of the machine's
//! clock. This repository's standard is an oracle where one exists and a
//! deterministic case where one does not, and "the row came back with a time
//! somewhere near now" is neither — it is an assertion that passes on a broken
//! implementation as easily as a correct one, because *any* plausible number
//! satisfies it.
//!
//! With a clock that can be set, a test writes a row at 1000, updates it at
//! 2000, and asserts exactly `[1000, 2000]`. That distinguishes "stamped on
//! insert and preserved" from "stamped on every write" and from "never
//! stamped", which no bounds check can.
//!
//! # Seconds
//!
//! Every time in this system is `i64` seconds since the Unix epoch: the
//! `date_trunc` family, `CalendarPart`, `Round`, the timezone tables, and every
//! timestamp column in every example. A managed column in milliseconds would
//! read back a thousand times too large from every one of those functions, with
//! nothing reporting it — the same class of failure as a decimal's scale, and
//! the reason that one is in the schema fingerprint.
//!
//! The cost is stated rather than hidden: two writes in the same second give
//! the same `updated_at`, so it cannot be used to order writes within a second
//! or as a concurrency token. `update_if_unchanged` is the thing for that, and
//! it compares the whole row.

use core::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Supplies the current time for a managed column.
pub trait Clock: fmt::Debug + Send + Sync {
    /// Seconds since the Unix epoch.
    fn now(&self) -> i64;
}

/// The machine's clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        // A clock before 1970 gives `Err`, and the elapsed duration in that
        // error is how far before. Answering the negative rather than zero
        // because a row stamped `0` on a misconfigured machine reads as "the
        // epoch", which is a plausible-looking lie; a negative number is
        // visibly wrong, which is what somebody wants to see.
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
            Err(before) => {
                i64::try_from(before.duration().as_secs()).map_or(i64::MIN, |seconds| -seconds)
            }
        }
    }
}

/// A clock a test sets by hand.
///
/// In the library rather than in a test module because four crates test
/// against managed columns and a fifth would otherwise copy it.
#[derive(Debug, Clone)]
pub struct FixedClock(Arc<std::sync::atomic::AtomicI64>);

impl FixedClock {
    /// A clock reading `seconds` until it is told otherwise.
    #[must_use]
    pub fn at(seconds: i64) -> Self {
        Self(Arc::new(std::sync::atomic::AtomicI64::new(seconds)))
    }

    /// Move the clock to `seconds`.
    ///
    /// Takes `&self` rather than `&mut self`: a store owns its clock behind an
    /// `Arc`, so a test that could only advance it through `&mut` could not
    /// advance it at all once the store was built.
    pub fn set(&self, seconds: i64) {
        self.0.store(seconds, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_is_after_the_repository_existed_and_before_it_rots() {
        // Not "near now", which asserts nothing: a stub returning a constant
        // would pass any tolerance wide enough to survive a slow machine. A
        // window of decades still catches the two failures that matter — a
        // clock returning zero, and one returning milliseconds, which would
        // read as the year 56,000.
        let now = SystemClock.now();
        assert!(now > 1_700_000_000, "{now} is before this was written");
        assert!(now < 4_000_000_000, "{now} is not seconds; milliseconds?");
    }

    #[test]
    fn a_fixed_clock_reads_what_it_was_set_to_through_a_clone() {
        // The clone is the point: a store holds one and a test holds another,
        // and a `set` on either has to be visible to both or no test can move
        // time after the store is built.
        let clock = FixedClock::at(1_000);
        let held = clock.clone();
        assert_eq!(held.now(), 1_000);
        clock.set(2_000);
        assert_eq!(held.now(), 2_000);
    }
}
