---
id: ADR-0193
title: "Metadata-derived vtable admission for the hand-rolled Wayland FFI seam"
status: accepted
date: 2026-10-26
scope: wayland/ffi, wayland/protocol, wayland/server
superseded_by: null
negative_knowledge: true
---

# 0193. Metadata-derived vtable admission for the hand-rolled Wayland FFI seam

- Status: Accepted
- Date: 2026-10-26
- Deciders: Tessera Maintainers
- Amends: [ADR-0006](0006-ffi-soundness-discipline.md)

---

## Context and Problem Statement

libwayland dispatches a client request by indexing the object's implementation
array by opcode: a request with opcode N calls `impl[N]`. Because the array is a
`#[repr(C)]` struct of function pointers, an under-sized struct lets the highest
opcodes the *bound version* advertises read past the end. This is the failure
mode [ADR-0006](0006-ffi-soundness-discipline.md) documents.

ADR-0006 defended against it with a compile-time assert per struct,
`assert_impl_opcode_count!(T, N)`, whose `N` — "the number of opcodes the
protocol advertises for the version we bind" — was transcribed by hand. That
mechanism has three structural defects:

1. **The count's source is the struct itself.** The assert compares
   `size_of::<T>()` against a literal a human wrote next to `T`. It therefore
   detects *editing the struct without editing the literal*, but not the case
   that actually matters: a struct and its literal that agree with each other
   while disagreeing with the protocol version the server binds. That is exactly
   how the ADR-0006 incident shipped.
2. **The bound version lives elsewhere.** The version is passed to
   `wl_global_create` in `server/lifecycle.rs`, in a different module from the
   struct and its assert. Nothing ties the two; a bump of the advertised version
   with no matching slot is invisible.
3. **Coverage was incomplete.** 7 of 88 impl structs (`wl_output`,
   `zwp_tablet_*` ×4, `zwp_linux_*_synchronization_v1` ×2) carried no assert at
   all, so the "every struct" invariant was aspirational for the newest, least
   exercised protocols.

An audit that reconstructs the required slot count from libwayland's own
interface tables (`wl_interface.methods[i]`, whose `since` version is the
leading digit of the message signature) found the *current tree to be correct*:
every struct already covers what the server advertises. The problem is not a
shipped bug; it is that nothing would have caught one, and nothing prevents the
next one.

## Decision Drivers

- The required slot count must be **derived from the protocol**, never
  transcribed, so a hand-typed tally cannot diverge from the interface.
- The check must be **fail-closed in release**, where the out-of-bounds read
  actually happens — not only under `debug_assertions`.
- Every impl struct must be covered; there must be no way to register a vtable
  that escapes the audit.
- The advertised version must have **one source of truth**, shared by the bind
  site and the audit.

## Decision Outcome

Vtable sizing becomes a property the server *computes* from libwayland
metadata, not a number it *declares*.

1. `declare_vtables!` in `crates/tessera-wayland/src/ffi.rs` binds each
   `*_interface_impl` struct to the `wl_interface` it implements, in one list.
   Each entry emits a compile-time shape assert (the struct is a whole number of
   function-pointer slots) and contributes to `impl_slots_for`, which answers
   "how many slots does this interface's vtable carry?".
2. `ffi::required_request_slots(interface, version)` derives the count
   libwayland can reach: it walks `methods[0..method_count]`, reads each
   message's `since` from its signature, and returns the highest opcode with
   `since <= version`, plus one.
3. `ffi::checked_resource_create` replaces raw `wl_resource_create` at every
   call site. It admits a resource only when
   `impl_slots_for(interface) >= required_request_slots(interface, version)`,
   panicking on violation. The version it checks is the resource's own bound
   version, so there is nothing to keep in sync with the bind site.
4. Interfaces the server deliberately advertises below their protocol maximum
   (`wl_compositor` at v4, `xdg_wm_base` at v1, …) are pinned as
   `ffi::V_*` constants, consumed by both `wl_global_create` and the audit.
   Sub-object vtables (`wl_surface`, `xdg_positioner`, …) inherit their parent's
   pinned version.
5. The hand-typed `assert_impl_opcode_count!(T, N)` macro and all 88 of its
   call sites are deleted. Its replacement detects strictly more: any struct
   wrong for its advertised version, for all 88 structs, not 81.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-50] Metadata-Derived Vtable Admission`: every `*_interface_impl`
  struct is declared in `declare_vtables!`, and every client resource is created
  through `checked_resource_create`. No vtable may be hand-sized or registered
  outside the audit. Enforced by `cargo run -p xtask -- check-vtables` (a
  structural gate over `ffi.rs` and the resource-creation call sites) and by
  `ffi::admission_tests::test_inv_declared_slot_counts_match_libwayland`, which
  re-derives every struct's requirement from libwayland metadata.

## Consequences

### Positive

- The whole defect class ADR-0006 targeted is now mechanically impossible to
  reintroduce silently: a request added to a protocol without a matching struct
  slot fails the build (shape assert) or the first bind (admission assert).
- Six previously unasserted interfaces are covered by construction; the audit
  iterates all 88.
- Advertised versions have a single declaration site, so a version bump and its
  vtable obligation move together.
- The audit is a plain data check with no GPU, display, or client required, so
  it runs in the fast CI tier.

### Negative / Trade-offs

- `checked_resource_create` is a thin wrapper on a hot path; the cost is one
  pointer-compare chain scan per created object (88 entries, bounded), which is
  negligible against object creation itself. Correctness is established once per
  object, not per request.
- The pinned `V_*` constants are a small, explicit surface that must be kept
  truthful. The `test_inv_*` audit reads them, so a constant that stops matching
  the protocol is caught rather than assumed.
- `declare_vtables!` must list every struct; the `check-vtables` gate fails
  closed if a struct is added without a declaration.

## Rejected Alternatives & Negative Knowledge

### Keep the hand-typed `assert_impl_opcode_count!(T, N)`
- **Why considered**: It is compile-time, costs nothing at runtime, and already
  exists.
- **Why rejected**: Its `N` is sourced from the struct rather than the protocol,
  so it cannot detect a struct that is internally consistent but wrong for the
  bound version — the exact shape of the ADR-0006 incident. It also leaves the
  advertised version (in `lifecycle.rs`) and the count (in `ffi.rs`)
  unconnected, and 7 structs were never covered.

### Generate the impl structs from the protocol XML with `wayland-scanner`
- **Why considered**: ADR-0006 names this "the long-term right answer": codegen
  would eliminate the manual struct entirely.
- **Why rejected (for now)**: It changes the signature of every request handler
  and the binding type of every vtable, a blast radius across all protocol and
  extension modules that dwarfs the defect it removes. The metadata-derived
  check removes the same defect class without touching a single handler, and it
  remains compatible with a later codegen migration (the audit would simply move
  to the generated tables).

### Wrap `wl_resource_set_implementation` instead of `wl_resource_create`
- **Why considered**: The impl pointer is in hand there, so the struct type
  could be inferred.
- **Why rejected**: Two call sites select the impl dynamically and erase it to
  `*const c_void`, defeating inference, and would need special-casing. Decisively,
  a resource's version is fixed when the resource is created, so the admission
  check belongs at creation; checking at implementation-install time would add
  the interface argument to 89 call sites for no additional correctness.

### Assert only under `debug_assertions`
- **Why considered**: The condition is a programming error that tests should
  catch before release.
- **Why rejected**: The out-of-bounds read it guards against is a release-only
  memory-safety hazard. A check that vanishes in release protects the wrong
  build.

## Verification

- `cargo run -p xtask -- check-vtables` — asserts every impl struct is declared
  in `declare_vtables!` and no call site uses raw `wl_resource_create` (CI, Gate
  A).
- `cargo nextest run -p tessera-wayland -E 'test(ffi::admission_tests)'` —
  `test_inv_declared_slot_counts_match_libwayland` re-derives every declared
  struct's requirement from libwayland metadata at the advertised version.
- `cargo nextest run -p tessera-wayland` — the full suite, including the
  real-client `clipboard_e2e` / `drag_e2e` probes that exercise resource
  creation end to end.
