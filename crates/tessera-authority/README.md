# tessera-authority

`tessera-authority` contains the transport-neutral authority mechanisms used by
the Tessera compositor and IPC broker.

The `authority` module owns Actor capability vocabulary, authenticated live
bindings, identity profiles, bounded sessions, exact resource grants,
semantic observation leases, resource scopes, and optimistic action
validation.

Audit persistence lives in `tessera-audit`; this crate owns authorization,
identity, leases, and resource grants. It does not parse IPC, access Wayland
objects, render chrome, run agents, or dispatch input.
