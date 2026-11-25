//! Adapter between `tessera-audit` storage entries and `tessera-protocol` wire entries (ADR-0260).
use crate::{AuditEntry, AuditSnapshot};
use tessera_protocol::{Effect, JournalEntry, JournalMutation, JournalSnapshot, Origin};

pub type Entry = AuditEntry<Origin, JournalMutation, Effect>;

impl From<Entry> for JournalEntry {
    fn from(entry: Entry) -> Self {
        let topic = entry.mutation.topic();
        Self {
            seq: entry.seq,
            ts_mono_ms: entry.ts_mono_ms,
            ts_wall_ms: entry.ts_wall_ms,
            topic,
            correlation_id: entry.correlation_id,
            origin: entry.origin,
            mutation: entry.mutation,
            effect: entry.effect,
        }
    }
}

impl From<JournalEntry> for Entry {
    fn from(entry: JournalEntry) -> Self {
        Self {
            seq: entry.seq,
            ts_mono_ms: entry.ts_mono_ms,
            ts_wall_ms: entry.ts_wall_ms,
            correlation_id: entry.correlation_id,
            origin: entry.origin,
            mutation: entry.mutation,
            effect: entry.effect,
        }
    }
}

impl From<AuditSnapshot<Entry>> for JournalSnapshot {
    fn from(snapshot: AuditSnapshot<Entry>) -> Self {
        Self {
            entries: snapshot.entries.into_iter().map(Into::into).collect(),
            oldest_seq: snapshot.oldest_seq,
            latest_seq: snapshot.latest_seq,
        }
    }
}
