# ADR-0159: Package boundaries follow capabilities

- Status: Accepted
- Date: 2026-09-17
- Scope: Workspace ownership, application composition, IPC, and dependency enforcement
- Amends: [ADR-0158](0158-clean-break-workspace-boundaries.md)

## Context

Flat directories improve navigation but do not justify a package per concern.
The workspace has application-private resource owners, entry-point wrappers,
and UI helpers packaged as independent libraries. Conversely, shared model
modules contain presentation policy and platform code reaches through IPC
packages for unrelated values. The existing role checker defaults unknown
packages to an unrestricted role and permits shell-to-application dependencies.

## Decision

Use modules by default. Retain a package only for a concrete dependency,
build, shared capability, or independently delivered adapter boundary.
Keep the flat directory layout. Package count is an outcome, not an invariant.

Consolidate session composition, engine resource ownership, and principal
registry persistence in the Tessera application. Preserve explicit resource
lifetimes and teardown order inside the application. Consolidate the CLI
library and control executable in one package with one command model.
Consolidate Unix client, server, and transport implementations in one IPC
package; retain a separate transport-free protocol package.

Move shell-only widgets into shell modules. Move GPU capture and encoding
into render, while file delivery stays with the application. Keep independent
Wayland protocol generation, platform backends, rendering, authority, semantic
validation, audit persistence, configuration, and independently consumed UI
resource capabilities. Keep launcher isolation mechanisms separately reviewable.
Remove the unintegrated remote prototype from the production workspace and
preserve it as an explicitly experimental project.

### Invariants & Behavioral Boundaries

- No production library depends on the Tessera application package.
- Domain packages cannot depend on platform, storage, transport, or graphics
  implementations. Shared value packages cannot become miscellaneous helpers.
- Protocol messages have no socket or persistence dependency. IPC mechanisms
  cannot own application policy.
- Wayland and backend implementations cannot depend on shell or application
  code. Application-selected runtime paths are supplied at their boundary.
- Shell widgets remain internal. System probing and desktop use-case execution
  belong to the application; shell consumes snapshots and emits actions.
- Native-library generation has one build owner shared by its consumers.
- Every production package has an explicit dependency policy; unknown packages
  fail validation. Normal, conditional, build, and test edges are inspected,
  with test-only allowances stated separately.
- Moving code must preserve behavior and tests. No compatibility facade
  packages remain for removed package names.

## Alternatives

### Package per responsibility (rejected)

Modules already express private responsibilities. Extra manifests and public
APIs require justification beyond naming or hypothetical future consumers.

### One monolithic package (rejected)

This would mix GUI and headless dependencies and weaken domain and adapter
boundaries. Shared FFI generation and independently delivered services warrant
real package boundaries.

### Fixed package budget (rejected)

Counting packages cannot establish whether a dependency is appropriate.

## Consequences

Application-private APIs become private modules. IPC clients and servers share
a build unit, trading finer build granularity for a smaller public package
surface. Explicit dependency policies require deliberate updates when adding
capabilities. Presentation ownership must be reviewed separately from directory
moves: pure UI layout is not automatically desktop domain policy.

The accepted decisions in ADR-0158 remain historical; this record replaces
its package-granularity choices where they conflict with the rules above.
