use serde::{Deserialize, Serialize};

/// Ordered event entry used by the live projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry<O, M, E> {
    pub seq: u64,
    pub ts_mono_ms: u64,
    pub origin: O,
    pub mutation: M,
    pub effect: E,
}

/// Bounded view returned to live audit consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditSnapshot<T> {
    pub entries: Vec<T>,
    pub oldest_seq: u64,
    pub latest_seq: u64,
}
