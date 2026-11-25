# Verification Standards

This document defines how Tessera is verified by hand: running the built
system in a real environment, checking its behavior against stated criteria,
and recording what was observed. It governs every document under
`docs/dev/verification/`.

Verification complements automated testing; it never replaces it. A check a
machine can decide belongs in a test under [Testing Standards](testing.md).

---

## 1. Scope & Intent

**Verification** answers "does the built system actually work here?" on an
environment no automated suite reaches: a nested session on a real desktop,
a preview window under visual inspection, a bare-metal DRM/KMS session, real
PAM policy, a live radio, a physical sink.

A verification document is a **procedure a contributor can execute**. It
states what environment it needs, what to run, what to observe, and what
counts as a pass. It is not a design record, a bug diary, or a task list.

### Boundary Matrix

| Document class | Question it answers | Location | Standard |
| :--- | :--- | :--- | :--- |
| **Automated test** | Does the code hold this property? (machine-decided) | crate `src/`, `tests/` | [Testing](testing.md) |
| **Verification procedure** | Does the running system behave correctly here? | `docs/dev/verification/` | This document |
| **Acceptance gate** | Is this area ready to release on real hardware? | `docs/dev/verification/` | This document |
| **Support matrix** | What has been verified, where, and when? | `docs/dev/verification/support-matrix.md` | This document |
| **Runbook** | How is this subsystem operated and diagnosed? | `docs/dev/runbooks/` | [Runbooks](runbooks.md) |
| **Triage case** | Which layer owns this user-visible symptom? | `docs/dev/triage/` | [Triage index](../triage/index.md) |
| **How-to guide** | How does a user or packager accomplish a goal? | `docs/how-to/` | [Taxonomy](taxonomy.md) |

A procedure that users or packagers also need, and that crosses no internal
boundary, is a how-to guide; the verification index links to it
(for example [How to Validate KMS Overlay Offload](../../how-to/kms-overlay-validation.md)).

---

## 2. The Verification Ladder

Every check runs on the cheapest layer that can fail for the change, and
escalates only when the change crosses that layer's boundary:

```text
Low cost / high speed                                         High cost / broad scope
[ 1: Unit & logic ] ──▶ [ 2: Surface preview ] ──▶ [ 3: Nested integration ] ──▶ [ 4: Physical ]
  automated, hermetic     one surface, own window     full compositor in a window     bare-metal TTY
```

| Layer | Environment | Covers | Does not cover |
| :--- | :--- | :--- | :--- |
| **1. Unit & logic** | In-crate and root suites; no GPU, display, or live source | State machines, layout math, protocol encoding, IPC schemas, config parsing | Rendering, live sources, interaction |
| **2. Surface preview** | Development-only harness binary or presentation toggle in an ordinary Wayland window | Full rendering and interaction of one surface, isolated from the session | Session integration, exclusive input, PAM, physical outputs |
| **3. Nested integration** | Production binaries inside a nested Tessera compositor | Real chrome and lock surfaces, input routing, protocols, multi-window behavior, fail-closed behavior | Atomic modesetting, libinput, libseat, physical hotplug, VT ownership, suspend |
| **4. Physical** | Direct DRM/KMS session from a bare TTY, or the packaged service | KMS scanout, planes, multi-GPU, physical outputs, installed PAM policy, capture refusal, suspend and resume, live radios | Safe unattended iteration |

1. **A result from one layer never stands in for another.** A passing preview
   is not evidence of session-lock behavior; a passing nested run is not
   evidence of KMS scanout.
2. **Escalate in order.** Run a layer only after the layers below it pass for
   the same change.
3. **Layer 1 is automated.** It is listed here so the ladder is complete; its
   rules live in [Testing Standards](testing.md).

---

## 3. Document Classes & Structure

`docs/dev/verification/` holds exactly these classes. Each file belongs to one.

| Class | File name | Purpose |
| :--- | :--- | :--- |
| **Index** | `index.md` | Map of the directory and entry point from [Contributor Documentation](../index.md). |
| **Playbook** | `manual-verification.md` | The scenario decision matrix and the daily-use recipes for every layer. One per repository. |
| **Procedure** | `<area>-verification.md` | A deep, area-specific procedure too large for the playbook. |
| **Acceptance gate** | `<area>-acceptance.md` | A pass/fail checklist that gates a release of an area on real hardware. |
| **Harness guide** | `preview-harnesses.md` | Use and authoring contract for Layer 2 inspection harnesses. |
| **Support matrix** | `support-matrix.md` | The recorded verification status of every area. One per repository. |

Every page other than the index opens with a breadcrumb back to the index:

```text
[Verification Index](index.md) > [Manual Verification Playbook](manual-verification.md) > <Page Title>
```

### Procedure Structure

A procedure contains, in order:

1. **Scope** — what the procedure proves, its ladder layer, and when to run it
   (normally: after the layer below passes).
2. **Prerequisites** — hardware, host services, tools, and builds. Builds run
   *before* the test environment is entered.
3. **Recovery** — how to get the machine back if the test environment becomes
   unusable (a reserved VT, an SSH session, the exact stop command). Recovery
   comes before the first risky step, not after.
4. **Procedure** — numbered, imperative steps with copy-paste commands.
5. **Expected results** — the observable outcome of each step: a log line, CLI
   output, a captured image, or a visible behavior.
6. **Fault split** (when useful) — symptom to owning layer, linking a
   [triage case](../triage/index.md) or subsystem [runbook](runbooks.md) for
   depth.

### Acceptance Gate Structure

An acceptance gate contains, in order:

1. **Scope** — the area, the governing ADR, and what automated suites already
   prove (the gate covers only the remainder).
2. **Prerequisites and recovery** — as for a procedure.
3. **Checks** — `- [ ]` items, each with one observable pass criterion.
4. **Evidence** — what must be kept for a pass (logs, `--json` snapshots,
   captures) and which environment facts to record.
5. **Release rule** — which changes require this gate and which sections must
   pass before release.

---

## 4. Writing Rules

1. **Observable pass criteria.** Every check names what is observed and what
   passes. "Works correctly" is not a criterion; "`tessera display` reports
   one `Extend` source and one `Mirror`" is.
2. **Copy-paste commands.** Commands run from the repository root through
   cargo (`cargo run --locked -p <package> [--bin <bin>] -- [ARGS]`) or from
   `target/<profile>/` when a script needs a stable path. Every command block
   stands alone.
3. **One isolation block for nested runs.** Every nested start of `tessera`
   isolates its data directory so it never contends with the live session's
   audit journal and caches:

   ```bash
   mkdir -p /tmp/tessera-dev/tessera
   TESSERA_BACKEND=nested \
   XDG_DATA_HOME=/tmp/tessera-dev \
   XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
   cargo run --locked -p tessera
   ```

   `./scripts/run-nested-dev.sh` applies the full isolation set
   (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `TESSERA_IPC_SOCKET`).
4. **Presentation toggles change presentation only.** A debug-gated
   environment variable such as `TESSERA_PREVIEW=control-center` may choose
   what is open or seeded at startup. It never alters authentication,
   authorization, or capture behavior. See
   [Development Environment Variables](../environment-variables.md).
5. **Inspection harnesses are separate binaries.** Parameterized inspection of
   a standalone surface is a `*-preview` binary behind the non-default
   `dev-preview` feature; the production binary carries no harness code. The
   contract is in [Preview Harnesses](../verification/preview-harnesses.md#3-architecture--authoring-contract-how-to-write-one).
   Security-sensitive surfaces take this rule absolutely: simulated
   authentication exists only in a harness.
6. **Never real credentials.** Procedures use fake values for passwords and
   secrets; command-line arguments are visible in the process list and shell
   history.
7. **Never destructive host actions without a gate.** A step that changes host
   networking, system services, firewall rules, or the only display states the
   risk and the required isolation (a disposable access point, a second
   output, an out-of-band console) before the step.
8. **No ephemeral notes.** Sprint state, local workarounds, and "not done yet"
   remarks are resolved before merge, never recorded in a procedure.
9. **No rationale.** Why a design was chosen belongs in its ADR; a procedure
   links the ADR in one sentence.

---

## 5. Evidence & the Support Matrix

[`support-matrix.md`](../verification/support-matrix.md) is the single place
that answers "what is verified right now". It records results; procedures
define how to get them.

### Status Vocabulary

| Status | Meaning |
| :--- | :--- |
| **Verified** | The linked procedure or gate passed in full on the recorded environment and date. |
| **Partial** | Some checks passed; the failing or unrun checks are named in the row. |
| **Failing** | A check failed and the failure is not yet fixed; the row links the issue or triage case. |
| **Unrecorded** | No dated result exists. The area may work; nothing in the repository proves it. |

### Rules

1. **Every row links its procedure.** An area without a procedure cannot be
   Verified; write the procedure first.
2. **Results are dated and located.** A Verified, Partial, or Failing row
   records the date (`YYYY-MM-DD`), host, and the environment facts its
   procedure requires (GPU, kernel driver, distribution, service versions,
   sink model).
3. **Status is earned by running the procedure.** It is never inferred from a
   roadmap milestone, an ADR status, or a passing lower layer.
4. **Behavior changes reset status.** A change that alters an area's behavior
   on a layer either re-runs that layer's procedure and updates the row, or
   sets the row to Unrecorded in the same change.
5. **Milestones live in the roadmap.** Planned scope and milestone state belong
   to the public [Roadmap](../../explanation/roadmap.md); the matrix records
   only observed results.

---

## 6. Lifecycle & Maintenance

1. **HOT documents.** A procedure is updated in the same change that alters the
   behavior it checks, the commands it runs, or the environment it needs.
2. **Mechanisms first.** When a manual check becomes automatable, add the
   test, delete the manual step, and note the test in the procedure's Scope
   ([Testing Standards](testing.md)).
3. **Crystallize findings.** A check that reveals a hardware quirk or a
   diagnostic recipe moves that knowledge to the subsystem's
   [runbook](runbooks.md); a symptom-attribution lesson moves to a
   [triage case](../triage/index.md). The procedure keeps only the check.
4. **Names describe purpose.** New files follow the class names in
   [Section 3](#3-document-classes--structure). Do not use a `-testing`
   suffix: these documents are not automated tests.
5. **Links are gated.** `cargo run -p xtask -- check-docs` fails on any dead
   link or anchor in these documents.
