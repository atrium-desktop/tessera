# tessera-audit

`tessera-audit` provides the bounded, infallible, in-memory FIFO event ring buffer
backing Tessera's unified observability event bus (ADR-0260).

## Responsibilities

- Maintain a bounded in-memory FIFO ring buffer of ordered `AuditEntry` records with monotonic `seq`.
- Record monotonic timestamps, wall-clock timestamps, and optional causal `correlation_id`.
- Produce gap-detectable `AuditSnapshot` views (`since(after)`, `since_filtered(after, pred)`).

## Boundaries

`tessera-audit` is strictly an in-memory event ring:
- No disk I/O, file locking, or synchronous `fdatasync` on compositor hot paths.
- No IPC socket listening or framing (those belong in `tessera-ipc`).
- No actor grant authorization decisions (those belong in `tessera-authority`).

## Related Documentation

- [ADR-0033: Mutation journal](../../docs/adr/0033-mutation-journal.md)
- [ADR-0260: Unified In-Memory Observability Event Bus](../../docs/adr/0260-unified-in-memory-observability-event-bus.md)
