//! In-memory, bounded, infallible FIFO event ring buffer (ADR-0260).
//!
//! Domain crates own event schemas and topic semantics, while this crate owns
//! monotonic sequencing, bounded memory retention, and gap-detectable snapshots.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{AuditEntry, AuditSnapshot};

/// Default live ring capacity (ADR-0033, ADR-0260).
pub const DEFAULT_CAPACITY: usize = 4_096;

/// Stable sequence access required by projections and ring queries.
pub trait Sequence {
    fn sequence(&self) -> u64;
}

impl<O, M, E> Sequence for AuditEntry<O, M, E> {
    fn sequence(&self) -> u64 {
        self.seq
    }
}

/// Current wall-clock time in Unix milliseconds, saturated to `0` if the
/// system clock precedes the Unix epoch.
#[must_use]
pub fn now_wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// In-memory append-only FIFO ring buffer with monotonic sequence numbers and
/// detectable eviction gaps (`[INV-AUDIT-01]`, `[INV-AUDIT-02]`).
#[derive(Debug)]
pub struct AuditLog<T> {
    entries: VecDeque<T>,
    next_seq: u64,
    capacity: usize,
}

impl<T> AuditLog<T> {
    /// Create a new bounded ring buffer holding at most `capacity` entries
    /// (clamped to at least 1).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            entries: VecDeque::with_capacity(capacity.min(DEFAULT_CAPACITY)),
            next_seq: 1,
            capacity,
        }
    }

    #[must_use]
    pub fn default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    #[must_use]
    pub fn oldest_seq(&self) -> u64
    where
        T: Sequence,
    {
        self.entries
            .front()
            .map(Sequence::sequence)
            .unwrap_or(self.next_seq)
    }

    #[must_use]
    pub fn latest_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return all retained entries with `seq > after`, together with the ring's
    /// current `oldest_seq` and `latest_seq` bounds so callers can detect gaps.
    #[must_use]
    pub fn since(&self, after: u64) -> AuditSnapshot<T>
    where
        T: Clone + Sequence,
    {
        self.since_filtered(after, |_| true)
    }

    /// Return retained entries with `seq > after` that satisfy `predicate`,
    /// preserving the ring's global `oldest_seq` and `latest_seq` bounds.
    #[must_use]
    pub fn since_filtered<F>(&self, after: u64, mut predicate: F) -> AuditSnapshot<T>
    where
        T: Clone + Sequence,
        F: FnMut(&T) -> bool,
    {
        AuditSnapshot {
            entries: self
                .entries
                .iter()
                .filter(|entry| entry.sequence() > after && predicate(entry))
                .cloned()
                .collect(),
            oldest_seq: self.oldest_seq(),
            latest_seq: self.latest_seq(),
        }
    }

    /// Push a pre-sequenced entry into the FIFO ring, evicting the oldest entry
    /// when at capacity.
    pub fn push(&mut self, entry: T) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
        self.next_seq = self.next_seq.saturating_add(1);
    }
}

impl<O, M, E> AuditLog<AuditEntry<O, M, E>> {
    /// Append one event with the current wall clock and no correlation id.
    pub fn append(
        &mut self,
        ts_mono_ms: u64,
        origin: O,
        mutation: M,
        effect: E,
    ) -> AuditEntry<O, M, E>
    where
        O: Clone,
        M: Clone,
        E: Clone,
    {
        self.append_correlated(ts_mono_ms, now_wall_ms(), None, origin, mutation, effect)
    }

    /// Append one event with explicit wall-clock timestamp and optional
    /// causal `correlation_id`.
    pub fn append_correlated(
        &mut self,
        ts_mono_ms: u64,
        ts_wall_ms: u64,
        correlation_id: Option<u64>,
        origin: O,
        mutation: M,
        effect: E,
    ) -> AuditEntry<O, M, E>
    where
        O: Clone,
        M: Clone,
        E: Clone,
    {
        let entry = AuditEntry {
            seq: self.next_seq,
            ts_mono_ms,
            ts_wall_ms,
            correlation_id,
            origin,
            mutation,
            effect,
        };
        self.push(entry.clone());
        entry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_eviction_preserves_monotonic_sequences() {
        let mut log = AuditLog::<AuditEntry<&str, u32, &str>>::new(3);
        let e1 = log.append(10, "chrome", 1, "ok");
        let e2 = log.append(20, "chrome", 2, "ok");
        let e3 = log.append(30, "chrome", 3, "ok");
        assert_eq!((e1.seq, e2.seq, e3.seq), (1, 2, 3));
        assert_eq!(log.len(), 3);
        assert_eq!(log.oldest_seq(), 1);
        assert_eq!(log.latest_seq(), 3);

        let e4 = log.append_correlated(40, 1_700_000_000_000, Some(99), "ipc", 4, "ok");
        assert_eq!(e4.seq, 4);
        assert_eq!(e4.correlation_id, Some(99));
        assert_eq!(log.len(), 3);
        assert_eq!(log.oldest_seq(), 2);
        assert_eq!(log.latest_seq(), 4);

        let snap = log.since(0);
        assert_eq!(snap.entries.len(), 3);
        assert_eq!(snap.entries[0].seq, 2);
        assert_eq!(snap.oldest_seq, 2);
        assert_eq!(snap.latest_seq, 4);
    }

    #[test]
    fn since_filtered_narrows_entries_without_moving_global_bounds() {
        let mut log = AuditLog::<AuditEntry<&str, u32, &str>>::new(8);
        log.append(1, "chrome", 10, "ok");
        log.append(2, "mcp", 20, "ok");
        log.append(3, "chrome", 30, "ok");

        let snap = log.since_filtered(0, |e| e.origin == "mcp");
        assert_eq!(snap.entries.len(), 1);
        assert_eq!(snap.entries[0].seq, 2);
        assert_eq!(snap.oldest_seq, 1);
        assert_eq!(snap.latest_seq, 3);
    }
}
