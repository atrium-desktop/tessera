# ADR-0148: Activation operates on transient trees

- Status: Proposed
- Date: 2026-09-08

## Context

A modal descendant — an in-app dialog, or a cross-client portal prompter
(FileChooser, Account, ScreenCast source chooser, …) wired to the app window
through `xdg_toplevel.set_parent` over `zxdg_importer_v2` (see ADR-0099) —
suspends its parent: the app cannot process input until the modal is
answered, the compositor draws the `parent_modal_scrim` over the parent
(`suspended_by_modal`, published since the input-routing bridge), and clicks
on the parent are redirected to the topmost modal descendant with an
attention pulse.

That suspension is only half of the tree story, and the halves disagree.
The pointer and touch press paths (ADR-0009 lineage) already treat the
transient tree as the unit of activation: clicking a suspended parent
focuses the modal leaf. But every *other* activation path treats the
window as an isolated unit:

- the held-Super window switcher commits its selection through
  `focus_surface_by_id`, which lands keyboard focus on the parent even
  though the parent is suspended — the user arrives at a scrimmed window
  where every keypress is swallowed;
- dock, overview, and shell window-list clicks go through the same
  `focus_surface_by_id` entry point with the same hole;
- `switcher_candidates` lists a cross-client portal prompter as its own
  switchable card, even though the prompter is not an independent
  activation target: it is part of the requesting app's tree. It can also
  be *hidden* behind its parent, so a user can switch to a card and watch
  the picker stay buried;
- focusing any member of a tree raises the whole tree
  (`change_keyboard_focus` already raises the root plus every transient
  descendant), so the raise side is already tree-atomic — only the focus
  *landing* is not.

Agent seats are deliberately excluded from this story:
`forward_agent_input_to` targets one authorized window with
`synthetic_target` set and does not pass through the user-activation
entry point (ADR-0103 target-local semantics). The legacy
`InjectInput` drain also focuses its target before delivery — through
an exact-focus entry point that deliberately bypasses the modal
redirect, because silently retargeting an agent's authorized window to
a different client's prompter would deliver its keystrokes to a process
it was never granted. Agent input keeps addressing exactly the window
it was granted.

## Decision

**User activation is defined on transient trees, and every human-seat
activation path converges on one predicate.**

1. **One landing rule.** `focus_surface_by_id_reveal` — the single
   entry point for switcher commit, dock clicks, overview clicks, and
   shell window-list clicks — resolves the activation target through
   `topmost_modal_descendant(root)`: if the requested window's tree has
   a live modal leaf, keyboard focus lands on the leaf and the leaf
   receives the attention pulse; otherwise focus lands on the requested
   window. The redirect is a landing rule, not a workspace rule: it
   never changes which workspace is revealed, only which member of the
   revealed tree holds the keyboard.
2. **One candidate rule.** The window switcher enumerates *roots*, not
   toplevels: a mapped toplevel with a live parent whose root differs
   from its own root (in practice, the cross-client portal prompter) is
   not an independent candidate. The user reaches the prompter by
   switching to the requesting app's card, and the landing rule carries
   focus to the modal leaf. In-app dialogs (same root as the parent)
   remain candidates — they are stackable, closable windows of the
   focused application, not foreign surfaces.
3. **Pointer and touch convergence.** The press paths keep their
   existing behavior but call the same `topmost_modal_descendant`
   resolution the landing rule uses, instead of each carrying an inline
   copy. One predicate, three call sites, zero drift.
4. **Suspension stays honest in chrome.** The switcher and overview
   keep marking suspended parents (`⧗`, scrim previews). With the
   landing rule in place that mark finally means something actionable:
   selecting the card lands you in the modal that is blocking the
   window, not in a dead parent.

What this decision deliberately does **not** do:

- **No global switching lock.** A picker being open never forbids
  switching to other windows or workspaces. Users consult reference
  material while a picker is open; mainstream compositors do not lock
  switching, and portal dialogs are cancelable, not jail sentences.
- **No always-on-top band for prompters.** Prompters follow the
  transient-tree raise that already exists; pinning a portal dialog
  above every window of every workspace would break the always-on-top
  band's exclusivity (ADR-0084) and user workspaces.
- **Unparented prompters stay ordinary toplevels.** A portal request
  without `parent_window` produces a parentless prompter; it remains a
  normal switchable window. The rules above bind on the live-parent
  link, never on app_id or process identity.
- **Agent input is untouched.** `synthetic_target` delivery keeps
  addressing the granted window; no redirect applies outside the human
  seat's activation paths.

## Alternatives

- **Forbid switching while a portal prompt is open** (global modal):
  rejected. It punishes the common case (looking up a filename in
  another window) to fix the rare one, and the compositor cannot know
  whether a prompter is "meaningful" to leave — the app-level modal
  state already handles that via suspension.
- **Pin prompters always-on-top**: rejected for the reasons above;
  also redundant once activation lands on the leaf and every raise is
  already tree-atomic.
- **Filter by prompter app_id instead of the parent link**: rejected —
  it hard-codes portal identity into the compositor, breaks the moment
  another cross-client dialog appears (IME candidates, third-party
  helpers), and misfires for a legitimately parentless portal window.
  The `set_parent` link *is* the semantic.
- **Only fix the switcher, leave dock/overview alone**: rejected as the
  "incremental" option because the divergence *is* the bug; three paths
  obeying one rule is the same size of change as one path obeying it.

## Consequences

- Selecting a suspended app in the switcher, dock, or overview now
  lands in its modal leaf: the user is never delivered to a window they
  cannot operate, and the portal dialog can never be "lost" behind its
  parent after a switch away and back.
- The switcher card list shrinks by the (usually zero or one)
  cross-client prompter entries; MRU reconciliation already prunes
  missing ids, so an active session loses nothing when a prompter stops
  being a candidate.
- The landing rule is a behavior change for one edge: explicitly
  focusing a parent that *has* a live modal child now focuses the
  child. Shell callers that intend "focus this exact window" for
  automation should keep using agent-side targeted input, which is
  unaffected.
- `topmost_modal_descendant` becomes part of the activation contract;
  its cost is O(surfaces) per activation, which is negligible at the
  activation rate and matches the existing press-path cost.
- Follow-up work: none required; the raise side and chrome suspension
  markers already exist and now compose with a single landing rule.
