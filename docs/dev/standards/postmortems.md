# Incident Postmortem Standards

This document establishes the blameless postmortem standard, timeline
reconstruction methodology, and systemic defense analysis for incidents in
Tessera.

Tessera is a compositor, so most incidents are not process crashes but **visual
correctness or session-integrity failures**: tearing, ghosting, input deadlock,
a failed scanout that leaves a frame unconsumed, or a blocked lock screen. The
structure below applies to all of them.

---

## 1. Blameless Culture & Intent

Incidents are opportunities to identify systemic fragility, missing automation,
and testing gaps.

- **Strictly Blameless**: Postmortems must NEVER assign personal blame or
  attribute root causes to "human error" or "developer negligence".
- **Focus on Systemic Safeguards**: The central question is always: *Why did the
  renderer contract, the test suite, or the architecture allow an unsafe state
  to manifest?*

---

## 2. Postmortem Structure

Every postmortem recorded under `docs/dev/triage/` or a dedicated postmortem
record must follow this structure:

1. **Executive Summary**: High-level impact (what the user saw, how long it
   lasted, whether session state or data integrity was affected).
2. **Timeline of Events**:
   - $T_0$: Introduction of the defect (commit, dependency bump, Optics tag).
   - $T_{\text{impact}}$: First manifestation in a session or test.
   - $T_{\text{detect}}$: Detection by a test, a preview harness, or a user
     report.
   - $T_{\text{mitigate}}$: Mitigation or revert applied.
   - $T_{\text{resolve}}$: Permanent fix verified.
3. **Detection Gaps**: Why did unit tests, surface previews, nested integration,
   or CI fail to detect this before release? Name the layer that should have
   caught it (see [Surface Testing](../surface-testing.md)).
4. **Trigger & Underlying Root Cause**: Detailed technical analysis of the
   state-machine failure, pre-pass frame skew, damage-region omission, or
   boundary leak.
5. **Action Items & Defense-in-Depth**:
   - Preventative actions (add a type-level invariant, tighten a boundary rule).
   - Detection actions (add a specific regression test, extend a preview
     harness).
   - Each action item must name a verifiable completion criterion.

---

## 3. Symptom-Class Pages

When a root cause belongs to a recurring symptom class — cursor attribution,
popup placement, chrome animation and backdrop coherence — the durable output is
a page under [`docs/dev/triage/`](../triage/index.md). A triage page records the
invariant, the diagnostic recipe, and the attribution conclusion so the next
occurrence resolves in minutes instead of reopening the investigation.
