# tessera-launch-services

Desktop application discovery, icon-theme lookup, sandboxed process launching,
and system intent search for Tessera.

## Responsibilities

- **Discovery (`entries`)**: Scan XDG application directories, parse `.desktop`
  files, extract localized names/categories, and strip `Exec` field codes.
- **Icons (`icons`)**: Scale-aware freedesktop icon theme lookup, inheritance,
  and directory resolution.
- **Launching (`launcher`)**: Spawning detached processes, terminal wrapping,
  and bubblewrap/cgroup interaction domain sandboxing.
- **Intent Search (`search`)**: Progressively disclosed intent query engine,
  math calculation, command palette execution, and agent prompt delegation.

## Related Documentation

- [ADR-0164: Universal domain pruning and primitives canonization](../../docs/adr/0164-universal-domain-pruning-and-primitives-canonization.md)
- [ADR-0160: Consolidate application subsystem into tessera-apps](../../docs/adr/0160-consolidate-application-subsystem.md)
- [ADR-0157: Headless intent subsystem](../../docs/adr/0157-headless-intent-subsystem-and-pivot-surface-decoupling.md)
- [Architecture](../../docs/explanation/architecture.md)
