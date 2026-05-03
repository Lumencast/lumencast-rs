//! Per-scene replay buffer (LSDP/1.1 §18.1).
//!
//! Bounded ring of recent `(seq, patches, cause)` emissions so a 1.1
//! client reconnecting with `since_sequence` can resume without a fresh
//! snapshot.

use std::collections::VecDeque;
use std::sync::Arc;

use lumencast_protocol::types::{Cause, Patch};

/// Default capacity (LSDP/1.1 §18.1 SHOULD ≥ 256).
pub(crate) const DEFAULT_REPLAY_BUFFER_SIZE: usize = 256;

/// One entry in the replay buffer.
#[derive(Clone, Debug)]
pub struct ReplayRecord {
    /// Per-scene seq this emission was assigned.
    pub seq: u64,
    /// The leaf patches in the original delta.
    pub patches: Arc<[Patch]>,
    /// Optional provenance metadata (LSDP/1.1 §3.2.3).
    pub cause: Option<Cause>,
}

/// Outcome of a `since` query.
#[derive(Clone, Debug)]
pub struct ReplaySlice {
    /// Records strictly after the requested `since_seq`, monotonic.
    pub records: Vec<ReplayRecord>,
    /// `false` when the requested resume point is older than the
    /// buffer's earliest entry — caller MUST fall back to a fresh
    /// snapshot per §18.1.
    pub covered: bool,
}

/// Bounded ring of replay records.
#[derive(Debug)]
pub(crate) struct ReplayBuffer {
    cap: usize,
    records: VecDeque<ReplayRecord>,
}

impl ReplayBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        let cap = if capacity == 0 {
            DEFAULT_REPLAY_BUFFER_SIZE
        } else {
            capacity
        };
        Self {
            cap,
            records: VecDeque::with_capacity(cap),
        }
    }

    /// Record one emission. Caller is responsible for monotonic `seq`.
    pub(crate) fn push(&mut self, record: ReplayRecord) {
        if self.records.len() == self.cap {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    /// Return every record with `seq > since_seq`, in monotonic order.
    pub(crate) fn since(&self, since_seq: u64) -> ReplaySlice {
        if self.records.is_empty() {
            return ReplaySlice {
                records: Vec::new(),
                covered: true,
            };
        }
        let earliest = self.records.front().unwrap().seq;
        if since_seq + 1 < earliest {
            return ReplaySlice {
                records: Vec::new(),
                covered: false,
            };
        }
        let records: Vec<ReplayRecord> = self
            .records
            .iter()
            .filter(|r| r.seq > since_seq)
            .cloned()
            .collect();
        ReplaySlice {
            records,
            covered: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn reset(&mut self) {
        self.records.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(seq: u64) -> ReplayRecord {
        ReplayRecord {
            seq,
            patches: Arc::from(Vec::<Patch>::new().into_boxed_slice()),
            cause: None,
        }
    }

    #[test]
    fn push_then_since_returns_everything() {
        let mut b = ReplayBuffer::new(4);
        for i in 1..=3 {
            b.push(rec(i));
        }
        let s = b.since(0);
        assert!(s.covered);
        assert_eq!(
            s.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn ring_wraparound() {
        let mut b = ReplayBuffer::new(4);
        for i in 1..=10 {
            b.push(rec(i));
        }
        assert_eq!(b.len(), 4);
        let s = b.since(6);
        assert!(s.covered);
        assert_eq!(
            s.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![7, 8, 9, 10]
        );
    }

    #[test]
    fn gap_not_covered() {
        let mut b = ReplayBuffer::new(4);
        for i in 1..=10 {
            b.push(rec(i));
        }
        // earliest retained is 7 ; requesting since=2 means "give me 3..10"
        // but we only have 7..10.
        let s = b.since(2);
        assert!(!s.covered);
    }

    #[test]
    fn caught_up() {
        let mut b = ReplayBuffer::new(4);
        for i in 1..=3 {
            b.push(rec(i));
        }
        let s = b.since(3);
        assert!(s.covered);
        assert!(s.records.is_empty());
    }

    #[test]
    fn reset_clears() {
        let mut b = ReplayBuffer::new(4);
        b.push(rec(1));
        b.reset();
        assert!(b.is_empty());
    }

    #[test]
    fn empty_buffer_always_covered() {
        let b = ReplayBuffer::new(4);
        let s = b.since(99);
        assert!(s.covered);
        assert!(s.records.is_empty());
    }
}
