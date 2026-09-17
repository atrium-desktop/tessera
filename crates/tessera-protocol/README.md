# tessera-protocol

`tessera-protocol` defines the versioned wire schema, commands, events, and
audit entry data transfer objects (DTOs) for Tessera IPC.

## Responsibilities

- Define versioned IPC protocol structures: `Request`, `Response`, `Event`,
  `Command`, `TransactOp`, `TransactReceipt`.
- Define wire-level mutation journal entries and snapshot types.
- Declare `PROTOCOL_VERSION` and capability flags exchanged during IPC connection hello.
- Serialize and deserialize wire messages through `serde` / `serde_json`.
- Zeroize sensitive secret prompt credentials using `zeroize`.
- Forbid unsafe code (`#![forbid(unsafe_code)]`).

## Boundaries

`tessera-protocol` is strictly a pure data exchange schema:
- No socket listening, connecting, framing, or descriptor passing (those belong in `tessera-ipc`).
- No durable cryptographic signing, HMAC hashing, or append-only segment storage (those belong in `tessera-audit`).
- No window management execution or state storage (those belong in `tessera-desktop` and `tessera`).
- No GPU or display rendering types.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [IPC Reference](../../docs/reference/ipc.md)
- [ADR-0027: IPC and Introspection](../../docs/adr/0027-ipc-and-introspection.md)
- [ADR-0125: IPC primitive families](../../docs/adr/0125-ipc-primitive-families-and-shared-ipc-client.md)
