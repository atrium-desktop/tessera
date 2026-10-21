---
id: ADR-0183
title: "Geometric Output Ownership Instead of the Fullscreen State Bit"
status: accepted
date: 2026-10-20
scope: desktop/presentation, desktop/window, wayland/server, shell/component, runtime/presentation, runtime/damage, composite/scanout
superseded_by: null
negative_knowledge: true
---

# 0183. Geometric Output Ownership Instead of the Fullscreen State Bit

- Status: Accepted
- Date: 2026-10-20
- Deciders: Tessera Maintainers
- Amends: [ADR-0064](0064-output-space-use-and-chrome-policy.md), [ADR-0134](0134-compositor-driven-fullscreen-for-any-toplevel.md), [ADR-0182](0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md)

---

## Context and Problem Statement

Every chrome and presentation decision that means "a client owns this output"
was keyed on one bit: `WindowState::fullscreen`, which only the client's
`xdg_toplevel.set_fullscreen` request (or the compositor's `Super+F11`
equivalent) ever sets.

That bit is a **client request**, not an observation:

- `SpaceUse::from_windows` reported `Fullscreen` only for that bit, and the HUD
  derives its dormancy from `SpaceUse` (`Hud::dormant`), so the HUD's
  liquid-glass chips and their 12 px Gaussian stayed composited over a window
  that filled the output.
- `Server::visible_fullscreen_window` gated the wallpaper elision and the
  wallpaper's damage term on the same bit, so the 2D background and the 3D
  model kept rendering under a full-output client.
- `plan_primary_plane` reads `Shell::composition_requirements`, so any visible
  shell pixel or live backdrop effect rejects direct scanout
  (`ScanoutRejectReason::VisibleShellPixels` / `LiveBackdropEffect`) *before*
  the client's own eligibility is even considered.

Now combine that with the way most toolkits implement fullscreen on Wayland:
GLFW's Wayland backend — what Minecraft, and the great majority of Java and
indie games ship — emulates fullscreen by resizing the toplevel to the monitor
extent and never sends `set_fullscreen`. ADR-0134 already recorded this class
of application ("a game that ships a windowed mode only") and its answer was a
*user* remedy (`Super+F11`); the automatic path stayed closed.

The consequence is not cosmetic. For exactly the workload ADR-0182 built the
zero-cost path for, the compositor pays, every frame:

1. a full-output composite instead of a primary-plane page flip (the prepared
   client buffer is sampled through Vulkan rather than scanned out),
2. a backdrop capture pass and a multi-resolution blur for chrome that only
   exists because the window "is not fullscreen",
3. a full-output wallpaper clear and blit that cannot reach a visible pixel.

niri, which has no persistent chrome, keeps direct scanout for the same
application and therefore does none of it. The frame rate difference users
observe is this tax.

## Decision Drivers

- **Chrome policy must follow observed pixel ownership, not a protocol
  request.** A window that covers an output and is opaque *is* the output's
  presentation, regardless of which request the client sent.
- **One resolved verdict, consumed everywhere.** HUD dormancy, Dock lockout,
  wallpaper elision, the damage baseline, and direct-scanout eligibility must
  read one answer; five consumers must not re-derive five variants.
- **Chrome must stop inferring domain policy from raw snapshots.** Chrome is
  presentation-only (ADR-0021): it renders what it is told about. Deriving
  space use inside `Hud` and `Dock` from a `&[Window]` put a server-side rule
  inside the chrome tier and duplicated it per component.
- **No visual regression for declared fullscreen.** The declaration still
  counts while the configure round-trip is in flight.
- **Keep the domain layer effect-free and testable without a compositor**
  ([ADR-0155](0155-workspace-tiering-shell-lexicon-and-state-canonization.md), [ADR-0159](0159-package-boundaries-follow-capabilities.md)).

## Decision Outcome

### 1. One presentation-domain value type derives extent and opacity

`tessera-desktop::presentation::SurfacePresentation` is a plain value of the
committed declaration, and it owns the only definition of "the logical extent
this surface occupies" and "nothing behind it can show through":

- `logical_extent`: `wp_viewport` destination → viewport source → buffer extent
  divided by `buffer_scale`, with a 90°/270° buffer transform swapping the axes
  (viewport rectangles are already post-transform, so they are not swapped
  again).
- `is_opaque`: an opaque `DRM_FORMAT` (`XRGB8888`, `XBGR8888`, `XBGR2101010`),
  an shm commit whose format was XRGB (the ingest path already forces every
  padding byte to `0xff`, so the record must keep that fact — BGRA8 storage has
  no format tag), or a declared `wl_surface.set_opaque_region` covering the
  whole extent.
- `owns(output)`: opaque **and** the output rect is entirely covered by the
  surface rect.

Three copies of the extent rule existed; all are gone:

- `tessera-composite`'s `evaluate_surface` dropped its own
  `surface_logical_extent`/`surface_is_fully_opaque` copies and delegates.
- `tessera-wayland`'s `surface_logical_size` — used by hit testing, popup
  placement, session-lock sizing, and the occlusion walk — now returns
  `SurfacePresentation::logical_extent`.
- The ownership scan builds the same type from the live surface record through a
  single constructor (`tessera_wayland::surface_presentation`).

The crates already all depend on `tessera-desktop`, which stays free of
platform, transport, and GPU types.

### 2. `SpaceUse` is resolved from occupancy facts, not window snapshots

`SpaceUse::from_windows` is removed. `SpaceUse::resolve` takes
`SpaceOccupant` values (`minimized`, `maximized`, `self_placed`, `fullscreen`,
`covers_output`) and reports `Fullscreen` when a visible, non-minimized window
declared fullscreen **or** placed its own geometry to cover an output and is
opaque. `covers_output` is a plain bool so the desktop rule stays ignorant of
surface protocol.

Geometric ownership is **restricted to self-placed windows**
(`LayoutRole::Floating`, not maximized). That is not a hedge, it is the
distinction the bug turned on:

- A maximized or tiled window covers the output because the *compositor* sized
  it that way. With auto-hide enabled the work area *is* the whole output, so
  every maximized window covers it exactly. Chrome policy deliberately keeps
  the HUD for maximized windows ([ADR-0064](0064-output-space-use-and-chrome-policy.md)),
  and that policy stands.
- A floating window that fills the output chose that geometry itself. That is
  precisely the fullscreen emulation this ADR exists to recognise.

`Server::space_use()` is the single producer: one walk over live surfaces,
restricted to exactly the set chrome is shown (presentation-visible *and*
observed by the human Interaction Domain, matching `windows_filtered`), that
short-circuits on the first owner and tests the **presented** output geometry
(`output_logical_rect`), the same rect the occlusion walk, hit testing, and
tiling use. `visible_fullscreen_window` and its `client_owns_output` wrapper are
gone.

The verdict is scanned **once per frame**, not once per consumer: the composition
root stores it in `CompositorRuntime::space_use` at the top of
`sync_shell_and_ipc_snapshots`, and the wallpaper elision, both
`assess_frame_damage` wallpaper terms, the redraw-idle policy, and the
`wallpaper_occluded` term read that field. Calling the server again from each
consumer would re-derive — and re-scan — the same answer four times per frame,
which is the per-frame cost this ADR exists to remove.

### 3. An shm commit records that its content is opaque

`wl_shm` pixels are copied into a BGRA8 side buffer with no format tag, so the
one fact that decides whole-surface opacity — "this commit was XRGB, every
padding byte was forced to `0xff`" — was thrown away at ingest and cannot be
recovered later. `SurfaceRec::shm_opaque` records it at commit.
Without it a covering software-rendered client (llvmpipe's Wayland WSI is
`wl_shm`) can never own an output, which is precisely the population this ADR
is about; direct scanout remains impossible for it for an unrelated reason
(`ScanoutRejectReason::ShmClientSurface`), so the fact is only load-bearing for
chrome stand-down and wallpaper suspension.

### 4. Chrome consumes the verdict instead of deriving it

`ChromeUpdate::SpaceUse(SpaceUse)` is pushed by `Shell::set_space_use`, seeded
into components added later, and fanned out by the composition root from
`Server::space_use()` in `sync_shell_and_ipc_snapshots` — before
`Shell::prepare_frame`, `assess_frame_damage`, and `plan_primary_plane`, so the
stand-down still lands on the same frame as the transition (ADR-0182 §5).

- `Hud::update(ChromeUpdate::SpaceUse)` sets `output_owned` from
  `SpaceUse::client_owns_output()`. The field is named for the fact, not for the
  protocol bit it used to copy.
- `Dock::apply_space_use` owns the collapse/dismissal response; `update_windows`
  keeps only the geometric obscuration policy.
- `CommandPanel` closes when a client takes the output.

The verdict is resolved every frame rather than only when
`windows_signature` moves: a client can begin declaring an opaque region, or
swap the buffer backing it, without changing geometry, and the chrome must
respond on the frame it happens.

A verdict change is itself **unsigned damage**. Nothing in `windows_signature`
moves when ownership flips through declared opacity, and no client commit owns
the pixels the HUD leaves behind (or returns to), so the publish path sets
`damage.chrome_dirty` — the existing term for "shell mutation applied outside
the signed paths" — and the frame repaints. Without it, chrome standing down
leaves its last pixels in the repaint history and chrome coming back stays
invisible until unrelated damage arrives.

### 5. A covering window becomes direct-scanout eligible end to end

With chrome dormant and the wallpaper elided, the existing scene facts in
`evaluate_scene` (single dmabuf toplevel, no overlay, hardware cursor) are
satisfied by construction for a full-output game, so the frame reaches the
primary plane. `evaluate_surface` still independently requires the buffer to be
opaque and mode-sized; ownership never *substitutes* for scanout eligibility, it
only removes the chrome that used to veto it.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-72] Single Ownership Resolution`: `Server::space_use()` is the only
  producer of presentation ownership, and the composition root resolves it
  exactly **once per frame** into `CompositorRuntime::space_use`. Chrome
  components, the wallpaper terms, the damage baseline, and the scanout planner
  consume that one value; none may re-derive ownership from
  `WindowState::fullscreen`, from window geometry, or by calling the server
  again.
  Enforced by `ChromeUpdate::SpaceUse` being the only chrome channel,
  `SpaceUse::from_windows` no longer existing (a re-derivation is a compile
  error in the shell), and the single `self.space_use = self.server.space_use()`
  assignment feeding every consumer.
- A verdict change is unsigned damage: the publish path sets
  `damage.chrome_dirty`, because neither the window signature nor any client
  commit owns the pixels chrome leaves behind or returns to.
- A surface "owns" an output only when it is opaque **and** covers the output
  rect completely, and only a self-placed (floating, non-maximized) window may
  claim that ownership geometrically. Enforced by
  `test_inv_arch_72_ownership_requires_opaque_coverage`
  (`SurfacePresentation::owns`) and
  `test_inv_arch_72_geometric_claim_requires_self_placement`
  (`SpaceUse::resolve`), plus
  `test_inv_arch_72_covering_space_use_stands_chrome_down` for the chrome
  consumer.
- Ownership never weakens the scanout checks in
  `tessera-composite::presentation::scanout`: `evaluate_surface` keeps its own
  opacity, geometry, transform, viewport, and plane-format conditions.
- `xdg_toplevel.set_fullscreen` remains a declaration input, so a client that
  asked for fullscreen owns the presentation during the configure round-trip.

## Verification

- `crates/tessera-desktop/src/presentation.rs`: extent derivation (buffer
  scale, viewport destination, viewport source, quarter-turn transform), format
  opacity, declared opaque region, multi-output coverage, and the negative
  cases.
- `crates/tessera-desktop/src/window.rs`: `SpaceUse::resolve` — the
  borderless-covering case, the minimized case, fullscreen precedence, and the
  self-placement restriction.
- `crates/tessera-shell`: `Hud` and `Dock` respond to the pushed verdict;
  `test_inv_arch_72_covering_space_use_stands_chrome_down` asserts the covering
  case stands chrome down while `Maximized` does not, and
  `releasing_output_ownership_restores_a_dock_that_was_obscured_before_it`
  pins the Dock's release path.
- Hardware: per [VT and DRM Testing](../dev/vt-drm-testing.md), a full-output
  GLFW client (a game resized to the monitor, no `set_fullscreen`) must log
  `direct scanout active` and show the wallpaper/shadow/blur elided while it
  keeps running, and releasing it must restore the HUD and Dock on the same
  frame with no leftover chrome pixels.
- Not covered mechanically: the field plumbing in
  `tessera_wayland::surface_presentation`, because a `SurfaceRec` cannot be
  fabricated without a live `wl_display`. The rules it feeds are covered by the
  domain tests above; the mapping is verified on hardware by the step above.

## Consequences

### Positive
- A window that fills the output reaches the zero-GPU-cost path whether or not
  it speaks the fullscreen state bit, which is the common case for games.
- Chrome policy has exactly one producer and one consumer channel; a new
  component cannot invent a different notion of "client owns the output".
- The opacity and extent rules stop existing twice (compositor and server).
- `Super+F11` stops being the *only* way to reach a full-output presentation.

### Negative / Trade-offs
- Standing chrome down for geometric coverage means a floating window resized
  to exactly cover the output hides the HUD. That is the same class of policy
  the Dock already applies through `dock_obscured`, it only applies to windows
  that sized themselves, and it reverses the moment the window shrinks. The
  alternative (keeping glass over a game) is the defect this ADR removes.
- The self-placed restriction means a toolkit that emulates fullscreen by
  maximizing instead of resizing (rare; SDL's borderless modes resize) keeps
  the HUD and stays composited. It still gets a correct, chrome-aware work
  area, and `Super+F11` remains the explicit escape hatch.
- The verdict is desktop-global, because chrome is one lens canvas with no
  per-output layout, and it is resolved against the *presented* output
  geometry. On a multi-output desktop a geometric owner on a secondary output
  is therefore not recognised as an owner (a declared fullscreen window still
  is, through its declaration). Resolving per output requires per-output
  ownership *and* per-output chrome, which is
  [ADR-0185](0185-per-output-presentation-domains.md); claiming ownership for a
  rect the compositor does not present would be worse than not detecting it.
- The `SpaceUse::Fullscreen` variant name stays. It is the *space-use class* of
  ADR-0064 ("a visible window consumes the complete output"), not the protocol
  bit; the class now has two ways in, and renaming it would only move the
  ambiguity into the wire enum (`Event::SpaceUseChanged`).
- Ownership requires *opaque* coverage, so a full-output window with an alpha
  buffer and no declared opaque region keeps chrome. That is deliberate: the
  composite still has to run for it, so eliding chrome would only lose the HUD
  without reaching scanout.
- `Server::space_use()` walks live surfaces and allocates the output rect list
  once per call; it is called a handful of times per frame. A surface-count
  regression would be visible in a static-screen profile, not in steady play.

## Rejected Alternatives & Negative Knowledge

### 1. Teach chrome to compare window geometry against the output itself
- **Why considered**: Keeps the server untouched; `Hud` and `Dock` already
  receive the display size.
- **Why rejected**: The rule would then exist in every component, and it cannot
  be answered in the chrome tier at all: it needs the *committed surface*
  extent, buffer scale, `wp_viewport`, and the declared opaque region, none of
  which a `Window` snapshot carries. Chrome is presentation-only (ADR-0021);
  the server is the only tier that can observe pixels.

### 2. Add a `Window::owns_output` field to the window snapshot
- **Why considered**: Zero new channels; `SpaceUse::from_windows` would keep
  working.
- **Why rejected**: `Window` is a wire DTO. Adding a resolved server verdict to
  it pollutes the protocol model with derived state, forces it through every
  IPC consumer, and still leaves each consumer free to reinterpret it. A
  dedicated `ChromeUpdate::SpaceUse` names the concept and keeps the snapshot
  truthful.

### 3. Treat every covering window as owning the output, ignoring opacity and
layout role
- **Why considered**: Simplest predicate: "its rect covers the output".
- **Why rejected**: Two separate mistakes. Ignoring opacity hides the HUD for a
  translucent full-output overlay (a screen dimmer) and still fails to reach
  scanout — both costs, no benefit. Ignoring the layout role hides the HUD for
  every maximized window on a desktop whose Dock auto-hides, because the work
  area then *is* the output; chrome policy explicitly keeps the HUD for
  maximized windows (ADR-0064).

### 4. Keep `visible_fullscreen_window` alongside `space_use`
- **Why considered**: Fewer call-site edits.
- **Why rejected**: Two producers of the same verdict drift. The whole failure
  mode is one consumer disagreeing with another about who owns the output.

### 5. Cache the verdict inside `Server`, keyed by the window signature
- **Why considered**: One resolution per signature change, no runtime field.
- **Why rejected**: Coverage also depends on the committed *surface* — a client
  can start declaring an opaque region, or swap the dma-buf behind a same-sized
  buffer, without moving `windows_signature`. A signature-keyed cache would
  serve a stale verdict exactly on the transition this ADR is about. The frame
  loop already owns "once per frame"; that is the correct cache boundary.

### 6. Auto-set the fullscreen state bit for covering windows
- **Why considered**: Reuses ADR-0134's machinery; one code path afterwards.
- **Why rejected**: It would send `XDG_TOPLEVEL_STATE_FULLSCREEN` configures the
  user never asked for, permanently mutating the client's window state and its
  saved floating rect, and it would make un-fullscreening a window a mutating
  operation on every resize. Ownership is an observation for *presentation*;
  the state bit is a *request*. Conflating them is what caused the defect.
