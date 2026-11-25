[Verification Index](index.md) > Support Matrix

---

# Support Matrix

The recorded verification status of every area that automated tests cannot
fully cover. Each row states what was observed, where, and when, and links the
procedure that produces the result. Status vocabulary and update rules are in
[Verification Standards](../standards/verification.md#5-evidence--the-support-matrix).

Milestone scope and planned work are in the [Roadmap](../../explanation/roadmap.md).
A roadmap milestone marked complete is not a verification result: a row here is
Verified only when its procedure has passed on a recorded environment.

---

## 1. Surface Previews (Layer 2)

| Area | Status | Last result | Environment | Procedure | Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| Lock screen preview | Unrecorded | — | — | [Surface: lock preview](surface-verification.md#run-the-lock-preview) | |
| Settings preview | Unrecorded | — | — | [Surface: Settings preview](surface-verification.md#run-the-settings-preview) | |
| Control Center sheet preview | Unrecorded | — | — | [Surface: Control Center](surface-verification.md#control-center-preview-in-a-nested-session) | |
| Persona avatar dump | Unrecorded | — | — | [Playbook 1B](manual-verification.md#1b-3d-persona--vrm-avatar-preview) | |

## 2. Nested Integration (Layer 3)

| Area | Status | Last result | Environment | Procedure | Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| Nested session smoke | Unrecorded | — | — | [Playbook smoke checklist](manual-verification.md#8-30-second-smoke-checklist) | |
| Inner client injection | Unrecorded | — | — | [Playbook Scenario 3](manual-verification.md#5-scenario-3-inner-client-injection--protocol-checks) | |
| Production lock in nested session | Unrecorded | — | — | [Surface: nested lock](surface-verification.md#nested-integration-the-production-lock) | |
| Settings against a nested compositor | Unrecorded | — | — | [Surface: Settings nested](surface-verification.md#nested-integration-live-compositor-feedback) | |

## 3. Physical (Layer 4)

| Area | Status | Last result | Environment | Procedure | Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| DRM/KMS single-output desktop | Unrecorded | — | — | [VT/DRM Verification](vt-drm-verification.md#try-the-desktop) | Roadmap M4 is in progress pending hardware verification. |
| Presentation loop and direct scanout | Unrecorded | — | — | [VT/DRM Verification](vt-drm-verification.md#try-the-desktop) | |
| Client GPU acceleration | Unrecorded | — | — | [VT/DRM Verification](vt-drm-verification.md#verify-client-gpu-acceleration) | |
| Multi-output, mirror, absolute input | Unrecorded | — | — | [Multi-Output Acceptance](multi-output-acceptance.md) | Requires two outputs on one DRM card. |
| KMS overlay offload | Unrecorded | — | — | [How to Validate KMS Overlay Offload](../../how-to/kms-overlay-validation.md) | |
| Physical lock, PAM, suspend and resume | Unrecorded | — | — | [Surface: physical session](surface-verification.md#verify-a-physical-session) | |
| Network live checks | Partial | 2026-11-17 | Reference host, NetworkManager with wpa_supplicant | [Network gate §1](network-hardware-acceptance.md#1-automated-live-checks) | See [network record](#network-live-checks-2026-11-17). Profile CRUD live test not run. |
| Network station operations | Unrecorded | — | — | [Network gate §2](network-hardware-acceptance.md#2-station-operations-isolated-test-ap-required) | Requires an isolated test AP. |
| Miracast group formation and streaming | Unrecorded | — | — | [Network gate §3](network-hardware-acceptance.md#3-casting-group-formation-real-sink) | The [runbook](../runbooks/miracast.md#open-questions) records undated sessions with the reference sink and open questions; no dated gate result exists. |

---

## 4. Result Records

Detailed observations behind rows whose status is not Unrecorded. Newest
first; keep one record per run.

### Network live checks (2026-11-17)

Host: reference host, NetworkManager with wpa_supplicant.

- `wlp0s20f3` station connected, `p2p-dev-wlp0s20f3` present and ready;
- live snapshot: `Wifi`, `wifi_enabled = true`, SSID reported, real signal bars;
- live P2P discovery: authority `NetworkManager`, no sink in range;
- `tessera-cast probe`: `wifi-p2p device ready`;
- a stopped iwd stays `inactive` when the service is explicitly started with
  `--backend iwd`; it reports unavailable instead of activating it;
- not run: the profile CRUD live test skipped because the headless session has
  no polkit agent to create a transient profile; run it from a real desktop
  session.
