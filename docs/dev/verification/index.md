# Verification

How to run Tessera during development, check by hand that a change works, and
see what has been verified so far. Procedures here follow
[Verification Standards](../standards/verification.md); automated tests follow
[Testing Standards](../standards/testing.md).

To build Tessera and start it for the first time, begin with the
[Build & Run Guide](../build-and-run.md).

---

## 1. Start Here

| Need | Go to |
| :--- | :--- |
| Pick the cheapest check for a change and run it | [Manual Verification Playbook](manual-verification.md) |
| See what is verified, where, and when | [Support Matrix](support-matrix.md) |
| Inspect one surface in its own window | [Preview Harnesses](preview-harnesses.md) |
| Release an area that touches real hardware | The acceptance gate for that area, below |

---

## 2. Documents

| Document | Class | Ladder layer | Scope |
| :--- | :--- | :--- | :--- |
| [`manual-verification.md`](manual-verification.md) | Playbook | 1–4 | Scenario decision matrix, native-free logic lane, previews, nested sessions, client injection, VT/DRM entry, smoke checklist. |
| [`support-matrix.md`](support-matrix.md) | Support matrix | 2–4 | Recorded status of every area, with date, environment, and procedure. |
| [`preview-harnesses.md`](preview-harnesses.md) | Harness guide | 2 | Using `tessera-settings-preview` and `tessera-lock-preview`; the authoring contract for new harnesses. |
| [`surface-verification.md`](surface-verification.md) | Procedure | 2–4 | Control Center, Settings, and lock screen: previews, agent pixel capture, nested lock, physical lock, production exclusion. |
| [`vt-drm-verification.md`](vt-drm-verification.md) | Procedure | 4 | Bare-metal DRM/KMS session: desktop pass, presentation loop, planes and direct scanout, client GPU acceleration. |
| [`multi-output-acceptance.md`](multi-output-acceptance.md) | Acceptance gate | 4 | Output roles, mirror letterbox, absolute input, simultaneous direct scanout, tentative display revert, per-output targets. |
| [`network-hardware-acceptance.md`](network-hardware-acceptance.md) | Acceptance gate | 4 | Live NetworkManager/iwd checks, station operations on an isolated AP, Miracast group formation (ADR-0242). |

Related procedures outside this directory:

- [Nested Backend Development](../nested-backend.md) — socket discovery,
  process replacement, and backend limits for Layer 3.
- [How to Validate KMS Overlay Offload](../../how-to/kms-overlay-validation.md)
  — the overlay candidate, complete-set `TEST_ONLY`, and deactivation matrix
  (public, because packagers run it).
- [Miracast Operational Runbook](../runbooks/miracast.md) — sink quirks and
  diagnostic recipes behind the casting gate.

---

## 3. The Ladder in One Line

Unit & logic → surface preview → nested integration → physical. Run the
cheapest layer that can fail for the change; a result from one layer never
stands in for another. The full model is in
[Verification Standards](../standards/verification.md#2-the-verification-ladder),
and what CI does not cover is in
[Testing Standards](../standards/testing.md#6-automation-boundary).
