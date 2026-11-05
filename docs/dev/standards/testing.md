# Testing Documentation & Execution Standards

This document specifies the engineering standards for testing strategy, test
execution, and testing documentation. It defines the universal governance rules
for organizing verification workflows across development environments.

---

## 1. Testing Philosophy & Governance

An engineering repository enforces a strict boundary between **system
mechanics** (how an architecture is constructed and why it functions) and
**testing runbooks** (how an engineer or automated agent verifies behavior and
catches regressions).

1. **Scenario-Driven Organization**:
   Testing documentation must be structured around developer intent and execution
   scenarios (e.g. rapid logic sanity, component preview harnesses, nested or
   emulated integration, bare-metal physical validation), rather than subsystem
   component taxonomies.
2. **Clarity of Purpose & Means**:
   Every testing procedure must declare two explicit components:
   - **Purpose**: What behavioral invariant, visual state, or protocol boundary
     is being validated.
   - **Means**: A concrete, copy-pasteable execution recipe containing all
     required environment overrides, isolation flags, and arguments.
3. **Mechanisms vs. Testing Separation**:
   Subsystem internals (such as buffer swapchain mechanics, scheduling
   algorithms, or kernel delegation pipelines) belong in architecture
   specifications and design records. Testing documentation must focus
   exclusively on the operational feedback loop: execution entry point, client
   or input injection, expected symptoms, and pass/fail criteria.
4. **Cost-Ordered Verification Ladder**:
   Testing execution must progress from the lowest-cost, highest-velocity layer
   to higher-cost, broader-scope environments:
   - **Layer 1: Unit & Logic Verification**: In-memory, hermetic, sub-second
     tests with zero GPU, display server, or external bus dependencies.
   - **Layer 2: Component Preview Harnesses**: Standalone inspection binaries
     and previews with simulated dependencies, validating rendering and local
     interaction without launching the full runtime.
   - **Layer 3: Nested & Emulated Integration**: Full production runtime hosted
     inside an existing desktop or containerized environment with isolated
     runtime directories, sockets, and storage.
   - **Layer 4: Physical & End-to-End Validation**: Direct deployment to target
     hardware or bare-metal sessions, validating hardware driver ownership,
     modesetting, and system power transitions.

---

## 2. Testing Directory Architecture & Progressive Disclosure

Testing documentation is organized hierarchically to prevent information sprawl
and minimize cognitive overhead:

```text
testing/
├── index.md           # Testing index: test philosophy, matrix, and document map
├── manual-testing.md  # Unified, scenario-based manual testing playbook
└── <scenario>.md      # Specialized scenario runbooks (when depth expands)
```

### Document Split Criteria

1. **The Primary Playbook (`manual-testing.md`)**:
   Serves as the central, high-signal manual testing guide. It contains the
   scenario decision matrix, canonical isolated execution commands, client
   injection procedures, and a rapid smoke checklist.
2. **Progressive Disclosure via Scenario Documents**:
   When an individual scenario accumulates extensive operational depth,
   specialized hardware permutations, or multi-step troubleshooting, it is
   extracted into a focused `<scenario>.md` file within the testing directory.
3. **Upward Linking & Breadcrumbs**:
   Every specialized scenario document must maintain a breadcrumb link back to
   the primary playbook and the testing index. The primary playbook retains
   lightweight pointers to specialized scenario runbooks.

---

## 3. Authoring Guidelines & Quality Invariants

When authoring or updating testing documents:

- **Explicit Isolation by Default**: Never assume an ambient clean state.
  Execution recipes must explicitly define necessary directory, socket, or
  environment overrides to guarantee safe parallel execution alongside active
  host sessions.
- **Single-Sentence Rationale**: When an isolation parameter or flag is required
  (e.g. preventing file lock contention or socket collisions), provide a
  one-sentence explanation directly adjacent to the command.
- **Actionable Smoke Checklists**: Conclude testing procedures with a concrete
  checklist of observable visual, interactive, or log assertions that confirm a
  healthy state.
