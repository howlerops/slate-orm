//! Re-running a transaction that lost a conflict.
//!
//! A conflict is not a failure; it is the store saying "someone else got there
//! first, look again". The record layer leans on that — a unique index is
//! enforced by two writers colliding on one key — so conflicts are an expected
//! part of normal operation rather than an exceptional one, and every caller
//! would otherwise hand-roll the same loop.
//!
//! The loop is easy to get subtly wrong in three ways, so it lives here once:
//!
//! - **Retrying the wrong things.** Only a conflict can succeed on a second
//!   attempt. A unique violation, an access denial or a fenced writer will fail
//!   identically forever, and retrying them converts a clear error into a hang.
//! - **Reusing stale reads.** The closure runs again against a *fresh*
//!   transaction, so it re-reads at the new snapshot. A retry that replayed
//!   buffered writes computed from the old snapshot would commit a decision
//!   made from data that has since changed.
//! - **Retrying in lockstep.** Without jitter, writers that collided once wait
//!   the same interval and collide again. The backoff is randomised.

use crate::error::{KernelError, Result};
use core::time::Duration;

/// How hard to try before giving up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts, including the first. One means no retries.
    pub max_attempts: u32,
    /// Backoff before the second attempt; doubles from there.
    pub initial_backoff: Duration,
    /// Ceiling on the backoff, before jitter.
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl RetryPolicy {
    /// Tuned for contention on a single-writer head: retries are cheap and
    /// local, so start small, and cap low enough that a caller's latency stays
    /// bounded rather than degrading into a hidden stall.
    pub const DEFAULT: Self = Self {
        max_attempts: 5,
        initial_backoff: Duration::from_millis(2),
        max_backoff: Duration::from_millis(100),
    };

    /// Do not retry at all.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            max_attempts: 1,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
        }
    }

    /// The backoff before attempt number `attempt`, counting the first as 1.
    ///
    /// Exponential, capped, then randomised across `[50%, 100%]` of the result
    /// so that writers which collided once do not line up and collide again.
    #[must_use]
    pub fn backoff_for(&self, attempt: u32) -> Duration {
        if attempt <= 1 {
            return Duration::ZERO;
        }
        let exponent = attempt.saturating_sub(2).min(16);
        let scaled = self
            .initial_backoff
            .saturating_mul(1u32 << exponent)
            .min(self.max_backoff);
        jitter(scaled)
    }
}

/// Randomise a duration into `[half, full]`.
///
/// It need not be a good random number: the only job is to keep colliding
/// writers from waking together. It does have to *vary*, and the first version
/// did not everywhere. It read the clock's sub-second nanoseconds modulo a
/// thousand, and macOS's `SystemTime` counts in microseconds, so that was zero
/// on every call — every backoff exactly half its ceiling, every writer that
/// collided once waking in lockstep to collide again. Linux's clock has the
/// digits, which is why CI never saw it; `backoff_grows_then_stops_growing`
/// failed on the first Mac it ran on.
///
/// So three inputs, none of which has to carry the whole job:
///
/// - the clock's whole nanosecond reading, for variation across time;
/// - a process-wide sequence number, so two calls inside one clock tick still
///   differ — which is the case that failed;
/// - the address of that sequence number, which address-space randomisation
///   places differently in each process, so two processes whose clocks and
///   sequences agree still differ. Not the process id: that panics on
///   `wasm32-unknown-unknown`, which this crate builds for.
fn jitter(full: Duration) -> Duration {
    if full.is_zero() {
        return full;
    }
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let sequence = SEQUENCE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let place = core::ptr::addr_of!(SEQUENCE) as usize as u64;
    spread(full, mix(clock, sequence, place))
}

static SEQUENCE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Three inputs to one well-spread word: SplitMix64's finaliser over their
/// combination, so a sequence that moves by one moves every bit of the output.
const fn mix(clock: u64, sequence: u64, place: u64) -> u64 {
    let mut x = clock ^ place.rotate_left(32) ^ sequence.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// `full` scaled into `[half, full]` by the top 53 bits of `entropy`.
fn spread(full: Duration, entropy: u64) -> Duration {
    let half = full / 2;
    let width = full.saturating_sub(half);
    half + width.mul_f64((entropy >> 11) as f64 / (1u64 << 53) as f64)
}

/// Run `attempt` until it stops losing conflicts.
///
/// `attempt` is called once per try and must do all of its own reading, because
/// each try gets a fresh transaction and therefore a fresh snapshot.
pub async fn with_retries<A, F, T>(policy: RetryPolicy, mut attempt: A) -> Result<T>
where
    A: FnMut(u32) -> F,
    F: Future<Output = Result<T>>,
{
    let mut last: Option<KernelError> = None;
    for number in 1..=policy.max_attempts.max(1) {
        let wait = policy.backoff_for(number);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        match attempt(number).await {
            Ok(value) => return Ok(value),
            Err(error) if error.is_retryable() => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    // Every attempt was a retryable failure; report the last one as-is so the
    // caller sees a conflict rather than an invented "out of retries" error.
    Err(last.unwrap_or(KernelError::TransactionConflict))
}

#[cfg(test)]
mod tests {
    use super::{mix, spread};
    use core::time::Duration;

    /// The case that failed, made independent of the machine: a clock that
    /// does not move between calls. Only the sequence number changes, and the
    /// backoffs must still spread out. On Linux the old code would have passed
    /// the integration test and this one alike only by its clock's luck.
    #[test]
    fn a_clock_that_does_not_move_still_spreads_the_backoff() {
        let full = Duration::from_millis(64);
        let waits: std::collections::BTreeSet<Duration> = (0..64)
            .map(|sequence| spread(full, mix(1_000_000_000, sequence, 0x7f00_0000)))
            .collect();
        assert!(
            waits.len() > 48,
            "{} distinct waits from 64 calls",
            waits.len()
        );
        assert!(waits.iter().all(|w| *w >= full / 2 && *w <= full));
    }

    /// Two processes whose clocks and sequence numbers agree differ by where
    /// their counter lives, and nothing else.
    #[test]
    fn two_processes_in_step_wake_apart() {
        let full = Duration::from_millis(64);
        let one = spread(full, mix(1_000_000_000, 7, 0x7f00_0000));
        let other = spread(full, mix(1_000_000_000, 7, 0x7f00_1000));
        assert_ne!(one, other);
    }
}
