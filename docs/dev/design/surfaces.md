# Surfaces

Chrome surfaces pick one surface role each. Every role resolves to a flat painted
body: an opaque fill, a hairline boundary, a corner radius, and a painted
elevation halo. The compositor draws no material behind chrome — there is no
backdrop capture, blur operator, or lens shader anywhere in a frame (ADR-0213).
Tokens live in `tessera-design` (see
[ADR-0046](../../adr/0046-design-system-crate.md)); components read them through
material factories instead of hard-coding values.

## Role selection

| Role | When to use |
|------|-------------|
| `ProminentPanel` | The panel that owns attention: prompts, the app picker, the pivot |
| `FloatingPanel` | Panels that carry content: the window switcher, Dock live previews |
| `Menu` | Text-bearing transient surfaces: the app context menu |
| `Tooltip` | A one-line name bubble anchored to an owner |
| `Dock` | The Dock bar and its collapsed capsule |
| `Chip` | Compact floating status pills |
| `Tile` | A cell on a panel canvas, or a reading inside the Dock body |

Do not improvise new bodies per component. If none of the rows fits, extend
`tessera-design` with a semantic role and document it here.

Selection markers are not surfaces. A region selection, window highlight, or
other marquee over pixels the user is about to capture never takes a body at
all: no fill, no halo, no rounded corners. It is a square-cornered border over
untouched content, because the preview must show exactly what the capture will
include. Rounding corners the capture actually keeps reads as a floating surface
rather than a tool. Optical instruments are the exception — the pixel-picking
loupe frames a reading, so it keeps a rounded body.

## Surface tokens (dark appearance)

| Token | Value | Used by |
|-------|-------|---------|
| `panel_surface` | rgb(20, 23, 32), alpha 242 | Floating and prominent panel bodies |
| `panel_border` | white, alpha 34, 1 px | Hairline boundary of a floating body |
| `popover_surface` | rgb(26, 30, 42), alpha 248 | Menu and popover bodies over arbitrary content |
| `popover_border` | white, alpha 72, 1 px | Popover edge against content |
| `card_surface` | white, alpha 14 | Card and tile fill on panels and the Dock |
| `shadow_ink` | black, alpha 255 | Painted elevation halo beneath a floating body |
| `radii.glass_panel` | 18 px | Shared outer radius for Dock, preview, and switcher panels |
| `selection.hover_tint` | white, alpha 6 | Immediate pointer feedback inside a body |
| `selection.selected_tint` | white, alpha 3 | Selected-content wash inside the parent body |
| `preview.inactive_content_brightness` | 0.74 | Opaque brightness for nonfocused preview siblings |
| `preview.focused` | scale 1.0, lift 0 px | Stationary focus inside an anchored preview panel |
| `preview.staged` | scale 1.06, lift 7 px | Restrained foreground staging in the window switcher |
| `CommandPanelColors::background` | rgb(18, 18, 20), alpha 255 | Command panel dark grouped background |
| `CommandPanelColors::surface` | rgb(28, 28, 30), alpha 255 | Command panel dark elevated surfaces |
| `CommandPanelColors::surface_recessed` | rgb(22, 22, 24), alpha 255 | Command panel dark recessed surfaces |
| `CommandPanelColors::border` | white, alpha 28 | Command panel quiet separators |
| `CommandPanelColors::accent` | rgb(10, 132, 255) | Command panel active tabs, sliders, and gauges |
| `CommandPanelColors::selection_surface` | rgb(24, 55, 86), alpha 255 | Command panel dark selected surface |

The light command-panel appearance maps the same roles to `rgb(242, 242, 247)`
for the grouped background, opaque white elevated surfaces, dark text, and
`rgb(0, 122, 255)` for the accent.

## Surface roles

A role names where a surface sits in the hierarchy, never how expensive a body it
wears. Roles vary in two ways: the elevation shadow, and the fill.

| Role | Shadow alpha | Blur | Y offset | Use |
|------|--------------|------|----------|-----|
| `Chip` | 0.16 | 4 px | 2 px | Transient system readings |
| `Tooltip` | 0.14 | 10 px | 5 px | Dock hover labels and similar attached hints |
| `Menu` | 0.18 | 16 px | 8 px | Text-bearing transient surfaces: the app context menu |
| `FloatingPanel` | 0.18 | 16 px | 8 px | Dock live previews, the window switcher |
| `ProminentPanel` | 0.20 | 18 px | 9 px | Modal prompts, the app picker, and the pivot |
| `Dock` | 0.20 | 12 px | 6 px | The resting Dock; morphing scales blur and offset with its body |
| `Tile` | 0.00 | — | — | A reading or control cell sitting on a panel or the Dock body |

`Menu` is the legibility role: it takes the palettes' highest-contrast body
(alpha 248 dark, 250 light) rather than boosting an effect, because it floats
over arbitrary application content and carries text.

The current role-to-component mapping:

| Component | Role |
|-----------|------|
| App context menu | `Menu` |
| Dock bar and collapsed capsule | `Dock` |
| Dock workspace pager | `Tile` (inside the Dock body) |
| Dock hover surface | `Tooltip`, or `FloatingPanel` when it hosts live previews |
| Window switcher | `FloatingPanel` |
| Transient system reading | `Chip` |
| Modal prompts and app picker | `ProminentPanel` |
| Pivot | `ProminentPanel` |

## Layering rules

- A body must be opaque enough to read on its own fill: alpha at least 236.
  There is no effect behind it to lean on, and a translucent body over
  application content is a smear.
- Popovers keep a 1 px hairline: without a boundary, a flat body has no edge at
  all against similarly toned content.
- Separation comes from the painted halo, not from tint. If a surface does not
  read as floating, fix its role's elevation rather than raising its alpha.
- Toast and reading text uses the product text colors at full strength. Contrast
  is a property of the palette, not of an adaptive shader.
- A floating body is placed with `materials::place_floating`, which emits the
  halo and the body together. A body without its halo is a rectangle pasted on
  the desktop.

## Interaction hierarchy

Choices use one shared progression across components:

| State | Treatment |
|-------|-----------|
| Rest | No indicator; content and the parent body carry the hierarchy |
| Hover | Neutral alpha-6 foreground wash, no outline and no new body |
| Selected | Neutral alpha-3 wash on the parent body; nonfocused preview content stays opaque at 74% brightness |
| Semantic accent | Reserved for meaning such as a primary action, status, warning, or application identity; never used only to mark the current structural choice |

The Dock live-preview panel and window switcher use this exact contract,
including the shared control and panel radii. Client pixels, subsurfaces, and
popups belonging to one preview are all composited through the same
`control`-radius clip, so the content and interaction geometry have one
silhouette. Selection moves between cards *inside* their one parent body.
Components must not reinterpret selection as a blue-white border, a nested
floating body, or an opaque fill.

Preview visibility and preview hierarchy are independent. Opacity is reserved
for opening and closing the complete panel; it must not de-emphasize sibling
preview pixels, because translucent client content blends with the panel below
and appears washed out. Use the shared brightness token instead.

## Preview anatomy

A preview group is one `FloatingPanel` body. Each card is ordinary content inside
that parent, never a nested body. The card's outer rectangle includes its live
preview and label and is the authoritative pointer and selection target. The
preview rectangle alone clips the live client surface with `radii.control`.

The `Focused` selection treatment leaves card geometry stationary. The `Staged`
treatment adds the shared scale and upward lift for presentations such as the
held-modifier window switcher. Both treatments retain the same neutral selected
wash and sibling-brightness policy. Staging changes geometry, not the hierarchy.
