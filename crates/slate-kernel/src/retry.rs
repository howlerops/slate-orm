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
/// The entropy is the wall clock's sub-second field. That is not a good random
/// number, but it does not need to be: the only job is to keep colliding
/// writers from waking together, and any source that differs between processes
/// does that.
fn jitter(full: Duration) -> Duration {
    if full.is_zero() {
        return full;
    }
    let entropy = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::from(d.subsec_nanos()));
    let half = full / 2;
    let spread = full.saturating_sub(half);
    half + spread.mul_f64((entropy % 1000) as f64 / 1000.0)
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
