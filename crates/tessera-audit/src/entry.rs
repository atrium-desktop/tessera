use serde::{Deserialize, Serialize};

/// Ordered event entry retained by the bounded in-memory observability ring (ADR-0260).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry<O, M, E> {
    pub seq: u64,
    pub ts_mono_ms: u64,
    #[serde(default)]
    pub ts_wall_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<u64>,
    pub origin: O,
    pub mutation: M,
    pub effect: E,
}

/// Bounded view returned to live observability consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditSnapshot<T> {
    pub entries: Vec<T>,
    pub oldest_seq: u64,
    pub latest_seq: u64,
}
