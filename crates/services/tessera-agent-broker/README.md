# tessera-agent-broker

`tessera-agent-broker` is the native side of the agent capability broker
(ADR-0088, ADR-0090): the compositor-held source of truth for "who may
borrow what".

## Responsibilities

- `PrincipalRegistry`: agent pairing, credentials (stored as SHA-256
  digests with owner-only file permissions), principal labels, and
  approved capability ceilings. Persisted at
  `$XDG_DATA_HOME/tessera/principals.json`.
- `GrantStore`: durable runtime-grant decisions (`grants.json`), same
  size/permission discipline.
- The agent-requestable capability vocabulary and the runtime-gated
  families that always require an interactive first-use grant.

## Non-goals

- No IPC transport: the broker is a state domain; the compositor's IPC
  server (tessera-ipc) and the MCP bridge (tessera-mcp) drive it.
- No interactive consent UI: the pick/consent chain stays in the
  compositor runtime's chrome driving code.
