---
id: ADR-0264
title: "Fail-safe sync_file merging, compile-time UAPI labels, and subset KMS out-fences"
status: accepted
date: 2026-11-27
scope: platform/sync, platform/drm, runtime/presentation, runtime/output_target, xtask
superseded_by: null
negative_knowledge: true
---

# 0264. Fail-safe sync_file merging, compile-time UAPI labels, and subset KMS out-fences

- Status: Accepted
- Date: 2026-11-27
- Deciders: Tessera Maintainers
- Amends: [ADR-0250](0250-fence-accurate-client-buffer-release-and-cached-mirror-composition.md)
  (replaces `Option<OwnedFd>` with typed fail-safe `CompletionFence`, eliminates
  runtime slicing in `sync_file` merging, and fixes multi-output subset
  `OUT_FENCE_PTR` completeness and multi-target direct-scanout fence merging),
  [ADR-0195](0195-panic-discipline-and-poison-recovery.md)
  (extends the production panic ratchet in `xtask check-panics` to track
  `.copy_from_slice(`)
- Amended by: [ADR-0265](0265-per-output-scoped-buffer-retirement-and-completion-fences.md) — adds fail-safe wait fallback on `DMA_BUF_IOCTL_IMPORT_SYNC_FILE` errors and synchronizes both KMS and Vulkan on unmerged-fence timeouts.

---

## Context and Problem Statement

Launching a full-output GPU client such as Flatpak Chrome on an explicit-sync
DRM session crashed the compositor main thread (`exit status 101`, tearing down
`graphical-session.target`) as soon as the compositor exited direct scanout:

```text
crates/tessera-platform/src/sync.rs:51:25:
copy_from_slice: source slice length (11) does not match destination slice length (10)
```

Root-cause investigation revealed a chain of four compounding defects across
the synchronization boundary and its verification gates:

1. **Runtime-checked magic-length UAPI label copy.** During the Aegis → Tessera
   rename, `data.name[..9].copy_from_slice(b"aegis-kms")` was mechanically
   rewritten as `data.name[..10].copy_from_slice(b"tessera-kms")` (11 bytes into
   a 10-byte slice). Because `copy_from_slice` checks length at runtime, it
   compiled without warnings. Prior to
   [ADR-0250](0250-fence-accurate-client-buffer-release-and-cached-mirror-composition.md),
   `merge_sync_fences` only ran across multi-CRTC out-fences, so single-output
   systems never entered the `for fence in fences` loop. Once ADR-0250 merged a
   composited render `acquire_fence` with the `leaving_direct_client` KMS
   `OUT_FENCE_PTR` on single-output sessions, every direct-scanout exit hit the
   panic deterministically.
2. **Fail-open `Option<OwnedFd>` fence merge contract.** `merge_sync_fences`
   returned `Option<OwnedFd>`, conflating "all work already signaled" with
   "`SYNC_IOC_MERGE` failed while two or more fences remained live". Downstream
   `release_retired_buffers_after_composited_frame` treated `None` as
   `RetiredReleaseAction::ReleaseImmediate`, allowing retired client DMA-BUFs
   to be released while GPU/KMS still read or scanned them — a direct violation
   of `[INV-ARCH-159]`. Multi-target direct scanout in `presentation.rs` also
   kept only one target's fence (`fence.or(last_fence)`) instead of merging all
   committed targets' fences.
3. **Multi-output subset commit out-fence mismatch.** `DrmBackend::commit_scanout`
   attached `OUT_FENCE_PTR` only to `targets`, yet validated completeness with
   `fences.len() == self.displays.outputs.len()`. On any multi-output subset
   commit (single-output direct scanout, single presentation-group target
   commit, or `present_to`), valid KMS out-fences were dropped and treated as
   absent, forcing blocking `wait_for_flip` stalls on direct scanout or losing
   the `leaving_direct_client` fence on per-output targets. Furthermore, older
   drivers without `OUT_FENCE_PTR` waited for flips on `overlay_transition`,
   but not on `leaving_direct_client`.
4. **Gate and test blind spots.** `xtask check-panics` ([ADR-0195](0195-panic-discipline-and-poison-recovery.md))
   counted `.unwrap()`, `.expect(`, and panic macros, but ignored
   `.copy_from_slice(`. Unit tests in `sync.rs` only exercised 0 and 1 fence
   and never entered the multi-fence merge loop.

## Decision Drivers

- **Zero runtime panic paths in the presentation synchronization hot path.**
  Fixed-size kernel UAPI labels and ABI sizes must be verified at compile time.
- **Fail-safe synchronization by construction.** A failed `SYNC_IOC_MERGE`
  ioctl or an incomplete KMS out-fence set must never collapse into "already
  signaled".
- **Uniform subset-commit semantics across single- and multi-output topologies.**
  Out-fence accounting must match the committed `targets` slice, not the global
  output table.
- **Mechanical enforcement.** Every failure mode observed here must be closed
  by compiler checks, typed return values, `xtask` gates, and regression tests.

## Considered Options

1. **Break clean on the sync completion contract and harden all four layers:**
   compile-time `fixed_cstr` UAPI label + ABI size assertion, typed
   `CompletionFence` (`Signaled | Live(OwnedFd) | Unmerged(Vec<OwnedFd>)`),
   subset-accurate `out_fences_complete` + uniform `commit_scanout` flip-wait
   fallback, multi-target direct-scanout fence merging, `check-panics` coverage
   of `.copy_from_slice(`, and multi-fence unit tests.
2. **Minimal slice-length fix (`[..10]` → `[..name.len()]`).**

## Decision Outcome

Chosen option: **Option 1**, because a one-line slice fix leaves the fail-open
`Option<OwnedFd>` contract, the multi-output subset `OUT_FENCE_PTR` bug, the
multi-target direct-scanout fence overwrite, and the `check-panics` blind spot
intact.

### Concrete Design

1. **Compile-time kernel UAPI encoding (`crates/tessera-platform/src/sync.rs`).**
   - Encode `MERGE_FENCE_NAME: [u8; 32]` via a `const fn fixed_cstr` that
     asserts `bytes.len() < N` and rejects interior NUL bytes at compile time.
   - Assert `std::mem::size_of::<SyncMergeData>() == 48` at compile time to
     match `SYNC_IOC_MERGE = _IOWR('>', 3, struct sync_merge_data)`.
   - Deny `clippy::indexing_slicing`, `clippy::unwrap_used`,
     `clippy::expect_used`, and `clippy::panic` on production code in `sync.rs`.
2. **Typed `CompletionFence` across `tessera-platform` and `tessera`.**
   - `CompletionFence::{Signaled, Live(OwnedFd), Unmerged(Vec<OwnedFd>)}`
     replaces `Option<OwnedFd>` across `sync.rs`, `DrmBackend`, `Host`, and
     `CompositorRuntime` (`last_composited_fence`).
   - If `SYNC_IOC_MERGE` fails at any step, `merge_sync_fences` retains the
     accumulated fence plus all remaining unmerged fences in
     `CompletionFence::Unmerged`, pruned of any already-signaled fds.
   - `plan_retired_release` maps `CompletionState::UnmergedFences` to
     `RetiredReleaseAction::WaitUnmergedAndRelease`: the runtime polls the
     remaining live fences (handling `EINTR` with a bounded deadline) and
     escalates to `device.wait_idle()` on timeout or poll error before posting
     `wl_buffer.release`.
   - `release_retired_buffers_after_composited_frame` borrows the live fd for
     release and moves `CompletionFence` directly into `last_composited_fence`,
     eliminating one `dup()`/`close()` cycle per composited frame.
   - Multi-target direct scanout collects and merges every target's completion
     fence rather than retaining only one.
3. **Subset-accurate KMS `OUT_FENCE_PTR` completion (`crates/tessera-platform/src/drm/device.rs`).**
   - `out_fences_complete(use_out_fences, collected, targets.len())` validates
     the returned fence count against `targets.len()`.
   - When `needs_kms_fence` (`DirectClient || overlay_transition ||
     leaving_direct_client`) holds and complete out-fences were not obtained
     (driver lacks `OUT_FENCE_PTR` or returned fewer fds than `targets.len()`),
     `commit_scanout` waits for the flip (`wait_for_flip`) before returning
     `CompletionFence::Signaled`.
4. **Panic ratchet extension (`xtask/src/tasks/check_panics.rs`).**
   - Include `.copy_from_slice(` in production `panic_sites` and update
     `xtask/panic-baseline.toml` so no unreviewed runtime-checked slice copy
     can enter production code undetected.

### Invariants & Behavioral Boundaries

Amends existing invariants rather than minting redundant tags
([Invariants](../dev/invariants.md) §4):

- **`[INV-ARCH-159]` (Amended):** A failed `SYNC_IOC_MERGE` or incomplete KMS
  `OUT_FENCE_PTR` set must never degrade into an immediate unsynchronized
  buffer release. Every presentation boundary either hands off a live merged
  `sync_file` covering all participating targets, waits on all unmerged live
  fences / page flips before releasing, or synchronizes the Vulkan device.
  **Enforcement:** `CompletionFence` type exhaustiveness,
  `test_inv_arch_159_merge_failure_preserves_all_remaining_live_fences`,
  `test_inv_arch_159_real_sync_ioc_merge_two_fences_never_panics_and_fails_safe`,
  `test_inv_arch_159_subset_commit_out_fences_match_target_count`, and
  `test_inv_arch_159_retired_release_never_defers_across_frames`.
- **`[INV-ARCH-51]` (Amended):** Production `.copy_from_slice(` calls are
  counted in the panic-site ratchet alongside `.unwrap()`, `.expect(`, and
  panic macros.
  **Enforcement:** `cargo run -p xtask -- check-panics`.

## Rejected Alternatives & Negative Knowledge

### Option 2: Minimal `[..10]` → `[..name.len()]` patch
- **Why considered**: Fixes the immediate Chrome direct-scanout crash in one
  line.
- **Why rejected**: Leaves four structural defects active: (1) runtime slice
  indexing still sits in the kernel UAPI path; (2) `merge_sync_fences` still
  returns `None` on ioctl failure, failing open into premature DMA-BUF release;
  (3) multi-output subset commits still drop KMS out-fences due to comparing
  against `self.displays.outputs.len()`; (4) `check-panics` still misses
  `.copy_from_slice(`.

### Silent fallback to `device.wait_idle()` inside `merge_sync_fences`
- **Why considered**: Could keep `Option<OwnedFd>` in public signatures.
- **Why rejected**: `tessera-platform::sync` is a pure OS `sync_file` boundary
  with no Vulkan device handle and cannot distinguish Vulkan-only stalls from
  KMS flip fences; keeping `Option<OwnedFd>` also keeps `Signaled` and
  `MergeFailed` indistinguishable in callers and tests.

## Consequences

### Positive
- Eliminates the session-fatal panic when exiting direct scanout (e.g. Flatpak
  Chrome and other full-output GPU clients) on explicit-sync DRM sessions.
- Makes fail-open retired-buffer release unrepresentable across single-output,
  multi-output, direct-scanout, and `NoDamage` paths.
- Fixes multi-output subset `OUT_FENCE_PTR` handling and multi-target direct
  scanout fence retention.
- Closes the `.copy_from_slice(` blind spot in `xtask check-panics`.

### Negative / Trade-offs
- Breaking signature change (`Option<OwnedFd>` → `CompletionFence`) across
  `tessera-platform` and `tessera` runtime presentation helpers; all in-tree
  callers are updated in the same change.
- If `SYNC_IOC_MERGE` fails at runtime (e.g. fd exhaustion), waiting on
  unmerged fences introduces a synchronous wait on that degraded frame; this is
  the deliberate fail-safe trade-off over corrupting live client swapchains.
