# tessera-ipc

`tessera-ipc` implements the Unix domain socket transport, length-delimited
framing, file-descriptor passing, client connectivity policies, and server
dispatch loop for Tessera.

## Responsibilities

- **Transport & Framing**: Length-prefixed binary framing over Unix domain sockets,
  sealed `memfd` capture buffer transport, and file descriptor passing.
- **Socket Paths**: Canonical runtime socket path derivation (`tessera.sock`) and
  per-session directory discovery.
- **Server Dispatch**: Unix listener binding, connection authentication, scope
  negotiation, request routing, and mutation event broadcasting.
- **Client Connectivity**: One-shot and persistent connections (`PersistentConnection`),
  automatic reconnect, pairing handshakes, credential store (`IdentityStore`),
  and observation lease tracking.

## Boundaries

- Message DTOs and vocabulary belong to `tessera-protocol`.
- Durable audit hashing and rotation belong to `tessera-audit`.
- Actual window-management mutations, capture rendering, and security decisions
  belong to the compositor composition root (`tessera`) and `tessera-authority`.
  This crate provides the transport and connection mechanics, not the business policy.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [IPC Reference](../../docs/reference/ipc.md)
- [ADR-0027: IPC and Introspection](../../docs/adr/0027-ipc-and-introspection.md)
- [ADR-0041: Sealed file descriptor pixel transport](../../docs/adr/0041-sealed-file-descriptor-pixel-transport.md)
- [ADR-0125: IPC primitive families and shared IPC client](../../docs/adr/0125-ipc-primitive-families-and-shared-ipc-client.md)
- [ADR-0158: Clean-break workspace boundaries](../../docs/adr/0158-clean-break-workspace-boundaries.md)
