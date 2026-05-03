//! Server-side allocator and client-side gap-detecting tracker for
//! frame sequence numbers (LSDP/1 §5).

use crate::errors::LumencastError;
use crate::frames::ServerFrame;

/// Server-side sequence allocator.
///
/// Starts at `0`; each call to [`SequenceAllocator::next`] returns the
/// next sequence number (so the first emitted frame is `seq = 1`).
/// [`SequenceAllocator::reset`] is called after a `scene_changed` frame
/// so that the following `snapshot` resets to `seq = 1`.
#[derive(Debug, Default)]
pub struct SequenceAllocator {
    last: u64,
}

impl SequenceAllocator {
    /// Build a fresh allocator at `seq = 0` (next emitted is `1`).
    #[must_use]
    pub fn new() -> Self {
        Self { last: 0 }
    }

    /// Allocate the next sequence number.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.last += 1;
        self.last
    }

    /// Last allocated sequence number (`0` if nothing emitted yet).
    #[must_use]
    pub fn last(&self) -> u64 {
        self.last
    }

    /// Reset the allocator. Used immediately before emitting the
    /// `snapshot` that follows a `scene_changed` frame.
    pub fn reset(&mut self) {
        self.last = 0;
    }
}

/// Client-side sequence tracker with gap detection.
///
/// Tracks the last observed `seq` and validates that subsequent frames
/// arrive contiguously. A duplicate (`seq <= last`) is dropped silently
/// (the runtime returns [`Observation::Duplicate`]); a gap (`seq >
/// last + 1`) returns [`LumencastError::SequenceGap`] so the runtime can
/// close and reconnect.
///
/// [`SequenceTracker::reset`] is called by the runtime after receiving
/// a `scene_changed` frame, since the following `snapshot` must reset
/// to `seq = 1`.
#[derive(Debug, Default)]
pub struct SequenceTracker {
    last: u64,
}

/// Outcome of feeding a frame to a [`SequenceTracker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// Frame is the next expected one; `seq` was advanced.
    Accepted,
    /// Frame is a replay of an already-observed `seq`; runtime SHOULD
    /// drop it silently.
    Duplicate,
    /// Frame has no sequence number (`pong`); tracker untouched.
    Heartbeat,
}

impl SequenceTracker {
    /// Build a fresh tracker.
    #[must_use]
    pub fn new() -> Self {
        Self { last: 0 }
    }

    /// Feed a server frame to the tracker. Returns:
    ///
    /// - [`Observation::Accepted`] on the next contiguous frame.
    /// - [`Observation::Duplicate`] on a replay (`seq <= last`).
    /// - [`Observation::Heartbeat`] for `pong`.
    /// - [`LumencastError::SequenceGap`] on a missing-seq detection. The
    ///   runtime MUST close the WebSocket and reconnect.
    pub fn observe(&mut self, frame: &ServerFrame) -> Result<Observation, LumencastError> {
        let Some(seq) = frame.seq() else {
            return Ok(Observation::Heartbeat);
        };
        self.observe_seq(seq)
    }

    /// Variant of [`SequenceTracker::observe`] that takes a raw `seq`.
    ///
    /// LSDP/1.1 §18.1.1 — the first frame on a fresh tracker accepts
    /// any `seq >= 1` as the baseline (per-scene seq, late-joining
    /// subscribers may see snapshot at `seq > 1`). `seq == 0` is
    /// still invalid.
    pub fn observe_seq(&mut self, seq: u64) -> Result<Observation, LumencastError> {
        if self.last == 0 && seq < 1 {
            return Err(LumencastError::SequenceGap {
                expected: 1,
                got: seq,
            });
        }
        if self.last == 0 {
            self.last = seq;
            return Ok(Observation::Accepted);
        }
        if seq <= self.last {
            return Ok(Observation::Duplicate);
        }
        let expected = self.last + 1;
        if seq != expected {
            return Err(LumencastError::SequenceGap { expected, got: seq });
        }
        self.last = seq;
        Ok(Observation::Accepted)
    }

    /// Rebase the tracker to a snapshot's `seq`. Called after
    /// `scene_changed` or back-pressure recovery — the tracker takes
    /// the snapshot value as the new baseline regardless of previous
    /// state.
    pub fn observe_snapshot(&mut self, seq: u64) {
        if seq >= 1 {
            self.last = seq;
        }
    }

    /// Reset to `0`. Use [`Self::observe_snapshot`] instead when
    /// rebasing to a known snapshot seq.
    pub fn reset(&mut self) {
        self.last = 0;
    }

    /// Last observed `seq` (or `0` if none yet).
    #[must_use]
    pub fn last(&self) -> u64 {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocator_increments() {
        let mut a = SequenceAllocator::new();
        assert_eq!(a.next(), 1);
        assert_eq!(a.next(), 2);
        assert_eq!(a.next(), 3);
        a.reset();
        assert_eq!(a.next(), 1);
    }

    #[test]
    fn tracker_accepts_contiguous() {
        let mut t = SequenceTracker::new();
        assert_eq!(t.observe_seq(1).unwrap(), Observation::Accepted);
        assert_eq!(t.observe_seq(2).unwrap(), Observation::Accepted);
        assert_eq!(t.observe_seq(3).unwrap(), Observation::Accepted);
    }

    #[test]
    fn tracker_drops_replay() {
        let mut t = SequenceTracker::new();
        t.observe_seq(1).unwrap();
        t.observe_seq(2).unwrap();
        assert_eq!(t.observe_seq(2).unwrap(), Observation::Duplicate);
        assert_eq!(t.observe_seq(1).unwrap(), Observation::Duplicate);
    }

    #[test]
    fn tracker_detects_gap() {
        let mut t = SequenceTracker::new();
        t.observe_seq(1).unwrap();
        let err = t.observe_seq(3).unwrap_err();
        match err {
            LumencastError::SequenceGap { expected, got } => {
                assert_eq!(expected, 2);
                assert_eq!(got, 3);
            }
            _ => panic!("wrong error: {err:?}"),
        }
    }

    #[test]
    fn tracker_reset_after_scene_changed() {
        let mut t = SequenceTracker::new();
        t.observe_seq(1).unwrap();
        t.observe_seq(2).unwrap();
        t.reset();
        assert_eq!(t.observe_seq(1).unwrap(), Observation::Accepted);
    }

    #[test]
    fn tracker_accepts_late_join_baseline() {
        // LSDP/1.1 §18.1.1 — fresh tracker accepts any seq >= 1 as the
        // baseline (per-scene seq, late-joining subscribers may see
        // snapshot at seq > 1).
        let mut t = SequenceTracker::new();
        assert_eq!(t.observe_seq(42).unwrap(), Observation::Accepted);
        assert_eq!(t.observe_seq(43).unwrap(), Observation::Accepted);
        assert_eq!(t.last(), 43);
    }

    #[test]
    fn tracker_rejects_seq_zero() {
        let mut t = SequenceTracker::new();
        let err = t.observe_seq(0).unwrap_err();
        assert!(matches!(err, LumencastError::SequenceGap { .. }));
    }

    #[test]
    fn observe_snapshot_rebases() {
        // After scene_changed or back-pressure recovery, the tracker
        // takes the snapshot seq as the new baseline.
        let mut t = SequenceTracker::new();
        t.observe_seq(1).unwrap();
        t.observe_seq(2).unwrap();
        t.observe_snapshot(5);
        assert_eq!(t.last(), 5);
        assert_eq!(t.observe_seq(6).unwrap(), Observation::Accepted);
    }
}
