---
id: ADR-0260
title: "Unified In-Memory Observability Event Bus and Removal of Durable Local Audit Chain"
status: accepted
date: 2026-11-25
scope: audit, protocol, ipc, runtime, config, cli
superseded_by: null
negative_knowledge: true
---

# 0260. Unified In-Memory Observability Event Bus and Removal of Durable Local Audit Chain

- Status: Accepted
- Date: 2026-11-25
- Supersedes: [ADR-0136](0136-authenticated-bounded-audit-replay-and-storage-guards.md), [ADR-0137](0137-audit-segment-manifest-and-retention.md)
- Amends: [ADR-0033](0033-mutation-journal.md), [ADR-0104](0104-actor-sessions-resource-grants-and-accessibility-adapter.md), [ADR-0026](0026-configuration-system.md), [ADR-0027](0027-ipc-and-introspection.md)

---

## Context and Problem Statement

ADR-0033 introduced an in-memory bounded mutation journal (`4096`-entry FIFO ring buffer) so IPC clients and agents could observe compositor decisions without polling. Later, ADR-0104, ADR-0136, and ADR-0137 layered a local durable SHA-256 hash chain, HMAC-SHA-256 checkpoint, background full-chain verification gate, 64 MiB gzipped segment sealing, exclusive file locking, and fail-stop (`abort()`) storage guards onto that journal at `$XDG_STATE_HOME/tessera/audit/events-v2.jsonl`.

In practice, that durable local hash-chain model fails both security and systems-engineering criteria for a desktop compositor:

1. **Invalid local threat model**: The JSONL log, checkpoint, and HMAC key live in the same owner-writable directory under the user's UID. An attacker who compromises the user's UID can read the key and forge or delete the entire chain; a sandboxed IPC client cannot reach the filesystem at all. The cryptographic chain therefore adds high complexity without strengthening the real security boundary.
2. **Compositor availability hazard**: Every audited mutation performed synchronous `fdatasync` under a global mutex, and reaching the storage quota (`2 GiB`) or free-space reserve (`512 MiB`) called `std::process::abort()`, terminating the user's entire desktop session.
3. **Multi-instance conflict and startup latency**: Holding an exclusive `flock` on a single user-global state file blocked concurrent bare-metal DRM instances across VTs/gettys and caused linear startup verification overhead as segments accumulated.
4. **Impedance mismatch with multi-dimensional observability**: Future observability consumers (e.g., MCP tool-call streams, debugging HUDs, automation rules) need a low-latency, topic-filterable, causally correlated event bus where trusted components can also publish semantic events — not a synchronous WAL that penalizes every new event dimension with disk syncs and quota pressure.

## Decision Drivers

- Keep compositor hot paths strictly non-blocking and infallible: observability must never `fdatasync` on the mutation path or `abort()` the desktop session.
- Provide a unified, extensible, multi-dimensional event pipeline (`EventTopic`, `correlation_id`, topic-filtered `GetJournal` / `SubscribeJournal`, and controlled `PublishEvent` for trusted services such as `tessera-mcp`).
- Zero-copy fan-out (`Arc<JournalEntry>`) across IPC subscription lanes so serialization stays on per-connection writer threads.
- Break clean without carrying legacy disk-chain code, offline CLI subcommands, or obsolete `[audit]` disk quota configuration.

## Considered Options

- **Option 1 (Chosen)**: Unified in-memory bounded observability event bus (`tessera-audit` as pure ring buffer, protocol v35 with `EventTopic`, `ts_wall_ms`, `correlation_id`, topic-filtered queries/subscriptions, `PublishEvent`, and `Arc<JournalEntry>` fan-out), completely removing the local durable hash-chain store.
- **Option 2**: Keep the durable hash-chain store behind an opt-in config flag or `TESSERA_EPHEMERAL_AUDIT=1`.
- **Option 3**: Remove `tessera-audit` and all journal IPC endpoints entirely.

## Decision Outcome

Chosen option: **Option 1**, because it preserves and elevates the high-value observability pipeline from ADR-0033 while eliminating the security theater, disk I/O jitter, lock conflicts, and availability hazards of local durable hash-chaining.

### Architectural Changes

1. **`tessera-audit` becomes a pure, infallible in-memory ring buffer**:
   - Removes `ChainedEventStore`, SHA-256 hash chaining, HMAC checkpoints, background verification gates, gzip segment sealing (`segments.rs`), `AuditError`, `AuditStoreOptions`, and external crates `flate2`, `hmac`, `sha2`, `libc`, `zeroize`.
   - `AuditEntry<O, M, E>` carries `seq`, `ts_mono_ms`, `ts_wall_ms`, `correlation_id`, `origin`, `mutation`, and `effect`.
   - `AuditLog<T>` maintains a bounded FIFO `VecDeque<T>` (default `DEFAULT_CAPACITY = 4096`) with monotonic `seq` across evictions and infallible `append`.
2. **Protocol v35 multi-dimensional event vocabulary (`tessera-protocol`)**:
   - Adds `EventTopic` (`Window`, `Workspace`, `InteractionDomain`, `Settings`, `Authority`, `System`, `Mcp`, `Custom`) with deterministic `JournalMutation::topic()`.
   - Adds `JournalMutation::External { topic, source, kind, detail }` so trusted platform services (such as `tessera-mcp`) can emit structured events into the same bus.
   - Adds `ts_wall_ms`, `topic`, and `correlation_id` to `JournalEntry`.
   - Extends `Request::GetJournal { since, topics }` and `Request::SubscribeJournal { topics }` with optional server-side topic filtering (`None` = all topics).
   - Adds `Request::PublishEvent { topic, source, kind, correlation_id, detail }` -> `Response::EventPublished { seq }` for controlled event injection.
3. **Zero-copy fan-out and topic filtering (`tessera-ipc`)**:
   - `JournalBroadcaster::broadcast(&self, entry: Arc<JournalEntry>)` fans out `Arc<JournalEntry>` into bounded per-connection writer lanes (`Outbound::JournalEvent(Arc<JournalEntry>)`), eliminating per-subscriber deep clones on the broadcaster lock.
   - Each `SubscriptionLane` stores `topics: Option<Vec<EventTopic>>` and applies topic matching before scope authorization.
4. **Runtime, Config, and CLI clean break (`tessera`, `tessera-config`, `tessera-control`)**:
   - Compositor runtime initializes `AuditLog::new(capacity)` unconditionally (no disk open, no exclusive file lock, no `std::process::abort()` on append).
   - `[audit]` configuration replaces the five disk-quota fields with a single `capacity` bound (`64..=65_536`, default `4_096`).
   - Removes the offline `tessera audit [status|verify|export|prune]` CLI subcommands and drops `tessera-audit` from `tessera-control` dependencies.

### Invariants & Behavioral Boundaries

- `[INV-AUDIT-01] Non-Blocking Infallible Publish`: Recording an event onto the observability bus must never perform synchronous disk I/O, acquire a filesystem lock, or terminate the compositor.
- `[INV-AUDIT-02] Bounded Memory with Gap Visibility`: The in-memory ring buffer strictly caps retained entries at its configured capacity via FIFO eviction while keeping `seq` strictly monotonic so lagging subscribers always detect gaps (`since < oldest_seq`).
- `[INV-AUDIT-03] Scope-Gated Multi-Dimensional Observation`: Topic filtering narrows delivery and never widens it; every delivered `JournalEntry` remains subject to the connection's live `Scope` and principal isolation rules.
- `[INV-AUDIT-04] Controlled External Injection`: `Request::PublishEvent` requires `control` capability (and an authorized non-locked-down scope) and is validated for bounded payload size so untrusted peers cannot flood or spoof the bus.

## Rejected Alternatives & Negative Knowledge

### Option 2: Keep durable hash-chain store behind a feature or config flag
- **Why considered**: Avoids breaking offline `tessera audit` CLI subcommands.
- **Why rejected**: Preserves ~1,700 lines of high-maintenance cryptographic/storage code and five external dependencies (`flate2`, `hmac`, `sha2`, `libc`, `zeroize`) for a local same-UID threat model that does not provide genuine tamper resistance. If durable archival is ever needed in the future, it must be implemented as an asynchronous, non-blocking downstream subscriber (or forwarded to `systemd-journald`/remote SIEM) rather than a synchronous write barrier on the compositor.

### Option 3: Remove the journal and IPC observability endpoints entirely
- **Why considered**: Maximum code deletion.
- **Why rejected**: Eliminates real-time causal observability for agents, MCP bridges, and future multi-dimensional desktop instrumentation. The bounded in-memory ring buffer is cheap, self-contained, and already integrated into the IPC protocol.

## Consequences

### Positive
- Zero disk I/O and zero `fdatasync` stalls on compositor mutation paths.
- Eliminates `std::process::abort()` crash risk from full disks or audit quotas.
- Eliminates the bare-metal multi-VT/getty `AuditError::Locked` startup collision.
- Establishes a clean, zero-copy, topic-filterable, causally correlated event bus ready for MCP and future platform services.
- Shrinks `tessera-audit` dependency surface from 8 crates down to `serde` and `tessera-protocol`.

### Negative / Trade-offs
- Journal history is session-scoped and does not survive compositor restarts by default.
- Offline CLI subcommands (`tessera audit status|verify|export|prune`) and legacy `[audit]` disk-quota config keys are removed as a clean break.
