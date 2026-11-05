# Elevation and Materials

Status: **Adopted**.

Materials describe how a surface participates in the product hierarchy.
Elevation is a semantic role, not a freely chosen shadow. A surface selects one
surface role based on its function.

The product paints every chrome body. The compositor draws no material behind
chrome: there is no backdrop capture, no blur operator, and no per-pixel lens
shader anywhere in a frame (ADR-0213). A surface's per-frame cost is therefore
proportional to the pixels it covers, which is what lets chrome sit on screen all
day on a laptop panel.

## Surface roles

A role resolves to an opaque fill, a hairline boundary, a corner radius, and a
shadow in logical pixels. Roles name where a surface sits in the hierarchy, never
how expensive a material it wears.

| Role | Use |
|------|-----|
| `Chip` | Compact floating status pills |
| `Tooltip` | A one-line name bubble anchored to an owner |
| `Menu` | Text-bearing transient surfaces (context menus) |
| `FloatingPanel` | Panels that carry content: the switcher, live previews |
| `ProminentPanel` | The panel that owns attention: prompts, pickers, the pivot |
| `Dock` | The dock bar and its collapsed capsule |
| `Tile` | A chrome cell on a panel canvas; carries no shadow |

`SurfaceRole::floats` answers whether a role sits above arbitrary desktop content
and therefore needs a separating shadow. Ask it instead of testing for a role by
name.

## Separation is painted

A floating body still has to read as floating, and it cannot buy that from a blur.
`materials::elevation_halo` returns two inflated rounded fills at low alpha,
outermost first, drawn beneath the body: a soft edge from two fills, with no
sampling radius. `materials::place_floating` places the halo and the body
together, so no surface can emit one without the other.

## Elevation rules

- Choose a surface role by function. Do not assemble local shadow or fill tuples
  for a static surface.
- Keep content cards within their parent body. A nested floating body is not a
  generic elevation step.
- A body must be opaque enough to read on its own fill: alpha at least 236. There
  is no effect behind it to lean on.
- Text-bearing surfaces take the palettes' highest-contrast body rather than
  boosting an effect: a menu floats over arbitrary content and carries text.
- Use the modal veil for modal scope, not to compensate for insufficient text
  contrast inside a panel.

## Ownership boundary

`tessera-design` owns semantic surface parameters and pure `lens` option
factories. The compositor connects those parameters to rendering. This boundary
follows [ADR-0139](../../../adr/0139-animation-effect-placement.md).

## Verification

Surface changes require tests for semantic role mapping and visual review on
bright, dark, detailed, and saturated backdrops. Text-bearing surfaces also
require contrast evidence in both appearances. A screenshot on one wallpaper is
not sufficient validation, because a flat body has to separate from *every*
content it may float over.
