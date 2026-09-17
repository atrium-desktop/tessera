# tessera-audit

`tessera-audit` provides durable, tamper-evident, integrity-checked event storage
and bounded audit projections for Tessera.

## Responsibilities

- Store mutation journal records in an append-only, disk-backed format.
- Compute HMAC and SHA-256 hash chains across log records to detect tampering or corruption.
- Manage segmented log files, automatic rotation, compression (`flate2`), and retention policies.
- Provide bounded in-memory projections and sequence-based snapshot replay (`since(after)`).
- Replay and verify past audit logs under authenticated scopes.

## Boundaries

`tessera-audit` is strictly focused on verifiable audit logging:
- No IPC socket listening or framing (those belong in `tessera-ipc`).
- No actor grant authorization decisions (those belong in `tessera-authority`).
- No compositor runtime or window-management mutations.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [How-to: Manage Audit History](../../docs/how-to/manage-audit-history.md)
- [ADR-0033: Mutation journal](../../docs/adr/0033-mutation-journal.md)
- [ADR-0136: Authenticated bounded audit replay and storage guards](../../docs/adr/0136-authenticated-bounded-audit-replay-and-storage-guards.md)
- [ADR-0137: Audit segment manifest and retention](../../docs/adr/0137-audit-segment-manifest-and-retention.md)
