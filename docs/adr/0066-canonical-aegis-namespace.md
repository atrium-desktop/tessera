---
id: ADR-0066
title: "Canonical Aegis namespace"
status: accepted
date: 2026-07-28
superseded_by: null
---

# ADR-0066: Canonical Aegis namespace

- Status: Accepted
- Date: 2026-07-28
- Note (2026-11-25): the canonical product name is now **Tessera**. Read this record's table with the Tessera forms: product `Tessera`, binary and namespace `tessera`, environment prefix `TESSERA_`, configuration directory `$XDG_CONFIG_HOME/tessera`, runtime socket `$XDG_RUNTIME_DIR/tessera.sock`, and MCP tool prefix `mcp__tessera__*`. Realm rows apply to Interaction Domains ([ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md)); the portal backend is the separate `xdg-desktop-portal-atrium` project, and the fuji skill was retired ([ADR-0113](0113-platform-ai-backend-and-agent-product-removal.md)). The no-compatibility-alias rule binds unchanged.
- Extended by: [ADR-0263](0263-first-party-crates-carry-the-tessera-namespace-prefix.md) — Cargo package names and their directories carry the `tessera` namespace prefix.

## Context

The workspace, installed binaries, desktop files, and portal backend already
used the Aegis product name. Earlier internal and external surfaces still used
a second product namespace. That split reached configuration paths,
environment variables, desktop identity, Realm isolation, diagnostics, MCP
tool names, and the fuji desktop skill.

The mixed namespace made startup diagnostics ambiguous and required users and
integrations to know which generation of a surface they were configuring.
Keeping compatibility aliases would preserve that ambiguity and make the
noncanonical namespace part of the long-term interface.

The project is pre-1.0 and can complete the rename before those interfaces
become stable.

## Decision

Aegis is the only product name. Use these canonical forms:

| Surface | Canonical form |
|---------|----------------|
| Product name in prose and diagnostics | `Aegis` |
| Binary, desktop identity, and namespace | `aegis` |
| Environment-variable prefix | `AEGIS_` |
| Configuration directory | `$XDG_CONFIG_HOME/aegis` |
| Runtime socket | `$XDG_RUNTIME_DIR/aegis.sock` |
| Realm runtime and cgroup names | `aegis-*` |
| Realm security context | `aegis.realm.<id>` |
| Portal backend | `org.freedesktop.impl.portal.desktop.aegis` |
| fuji MCP server and public tool prefix | `aegis` and `mcp__aegis__*` |
| fuji desktop skill | `aegis-desktop-realm` |

Do not provide compatibility aliases for the superseded namespace. Active
code, tests, installation resources, and current documentation use only the
canonical forms. Accepted ADRs and released changelog entries retain their
original text as immutable historical records.

This decision amends the naming details in ADR-0026, ADR-0027, ADR-0042, and
ADR-0050 without changing their configuration, IPC, isolation, or agent
architecture decisions.

## Alternatives

- **Keep both namespaces indefinitely.** Rejected because every new public
  surface would need alias behavior and documentation, preserving the source
  of the ambiguity.
- **Rename only user-visible diagnostics.** Rejected because configuration,
  desktop discovery, Realm isolation, and MCP integrations would continue to
  disagree with the product name.
- **Add one release of compatibility aliases.** Rejected because the project
  is pre-1.0 and the explicit goal is one consistent namespace rather than a
  staged dual-name interface.

## Consequences

- Users must rename environment variables and move existing configuration to
  `$XDG_CONFIG_HOME/aegis/config.toml`.
- fuji configurations and permission rules must use the `aegis` MCP server,
  `mcp__aegis__*` tools, and `aegis-desktop-realm` skill.
- Existing compositor and Realm processes must restart before every runtime
  identifier uses the canonical namespace.
- Logs, process names, cgroups, sandbox paths, Wayland metadata, and portal
  selection now identify the same Aegis product.
