# tessera-control

`tessera-control` implements the domain-oriented session control and command
dispatch library for the unified `tessera` binary.

## Responsibilities

- Parse display, window, workspace, notification, Interaction Domain, permission, system,
  event, and journal commands.
- Connect to the compositor IPC socket and negotiate capabilities.
- Format query results for humans or as JSON.
- Stream compositor and mutation-journal events.
- Generate shell completions for the unified `tessera` command.
- Keep command dispatch testable without gfx, optics-ui, Vulkan, or Wayland server
  dependencies.

## Boundaries

Protocol types and framing belong to `tessera-ipc`. Window-management behavior
belongs to the running compositor. This crate translates native resource
commands into typed IPC requests; it is not a second control plane.

## Development

Run the tests from the repository root:

```bash
cargo nextest run -p tessera-control
```

## Related Documentation

- [Command-Line Reference](../../docs/reference/cli.md)
- [IPC Reference](../../docs/reference/ipc.md)
