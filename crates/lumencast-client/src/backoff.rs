//! Reconnection schedule for LSDP/1 §7.
//!
//! ```text
//! attempt 1 → 0 ms
//! attempt 2 → 500 ms
//! attempt 3 → 1 s
//! attempt 4 → 2 s
//! attempt 5 → 4 s
//! attempt 6 → 8 s
//! attempt 7 → 15 s
//! attempt 8 → 30 s
//! attempt ≥ 9 → 60 s (cap)
//! ```
//!
//! ±25% jitter is applied. Pseudo-random source: nanoseconds of the
//! current wall clock — good enough to avoid thundering herd; stable
//! tests can disable jitter via [`backoff_for_attempt_with_jitter`].

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Compute the delay before reconnect attempt `attempt` (1-indexed).
#[must_use]
pub(crate) fn backoff_for_attempt(attempt: u32) -> Duration {
    backoff_for_attempt_with_jitter(attempt, true)
}

/// Variant exposing jitter control for reproducible tests.
#[must_use]
pub(crate) fn backoff_for_attempt_with_jitter(attempt: u32, jitter: bool) -> Duration {
    let base_ms: u64 = match attempt {
        0 | 1 => 0,
        2 => 500,
        3 => 1_000,
        4 => 2_000,
        5 => 4_000,
        6 => 8_000,
        7 => 15_000,
        8 => 30_000,
        _ => 60_000,
    };
    if base_ms == 0 {
        return Duration::ZERO;
    }
    let factor = if jitter { jitter_factor() } else { 1.0 };
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation
    )]
    let scaled = (base_ms as f64 * factor).round() as u64;
    Duration::from_millis(scaled)
}

/// Pseudo-random factor in `[0.75, 1.25]`.
fn jitter_factor() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let normalized = f64::from(nanos) / 1_000_000_000.0;
    0.75 + normalized * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_no_jitter() {
        let pairs = [
            (1, 0),
            (2, 500),
            (3, 1_000),
            (4, 2_000),
            (5, 4_000),
            (6, 8_000),
            (7, 15_000),
            (8, 30_000),
            (9, 60_000),
            (50, 60_000),
        ];
        for (attempt, expected) in pairs {
            let d = backoff_for_attempt_with_jitter(attempt, false);
            assert_eq!(d, Duration::from_millis(expected), "attempt {attempt}");
        }
    }

    #[test]
    fn jitter_within_bounds() {
        for _ in 0..10 {
            let d = u64::try_from(backoff_for_attempt_with_jitter(3, true).as_millis()).unwrap();
            assert!(
                (750..=1_250).contains(&d),
                "jittered backoff out of bounds: {d}"
            );
        }
    }
}
