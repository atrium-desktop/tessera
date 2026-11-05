# Tessera Design Language

The design language is the shared product contract for every Tessera-owned UI
surface: compositor chrome, first-party system applications, the Dock, the
pivot, the command panel, menus, readings, and toasts. Components built by
different crates must read as one product, so the rules in this directory are
normative for contributor-written Tessera UI.

Chrome is a quiet functional layer above client content. Every surface is a flat
painted body: an opaque fill, a hairline boundary, and a painted elevation halo
that separates it from whatever it floats over. The compositor draws no material
behind chrome — no backdrop capture, no blur operator, no per-pixel lens shader
(ADR-0213). That is a visual decision as much as a cost one: a surface that bends
the pixels beneath it is a surface asking to be looked at, and chrome is
infrastructure.

## System map

The design system is layered from low-level decisions to engineering
enforcement. A higher layer may combine lower layers, but it must not invent
private replacements for an adopted lower-layer contract.

| ID | Layer | Question | Entry point |
|----|-------|----------|-------------|
| 1 | Foundations | Which visual and sensory values are available? | [Foundations](foundations/index.md) |
| 2 | Components | Which reusable controls and containers use those values? | [Components](components/index.md) |
| 3 | Patterns | How do components behave across complete interactions? | [Patterns](patterns/index.md) |
| 4 | Guidelines | Which cross-cutting quality rules constrain every layer? | [Guidelines](guidelines/index.md) |
| 5 | Tooling | How are contracts generated, demonstrated, and measured? | [Tooling](tooling/index.md) |

Product specifications such as the surfaces inventory and the command panel apply
this shared system to a particular surface. They remain normative, but they do
not replace the system-wide contracts above.

## Maturity

Every system page carries one maturity label:

| Status | Meaning |
|--------|---------|
| Adopted | The contract has a shared implementation and is normative for new work. |
| Partial | A shared contract exists, but coverage or enforcement is incomplete. |
| Draft | The page reserves scope and review criteria; it is not yet an implementation contract. |

An adopted rule changes through code, tests, and documentation in the same
change. A draft becomes adopted only when its ownership and enforcement are
implemented; deleting a local literal is not sufficient by itself.

## Principles

1. **Content first.** Chrome defers to the client's pixels. A surface's body must
   let the window be the brightest, most detailed thing in the frame.
2. **Painted separation, not sampled.** Elevation is a fill, not a blur. A
   surface's cost is proportional to its area, so chrome can be on screen all day
   without a laptop paying for it in heat and battery.
3. **Concentric geometry.** Shapes nest with continuous rounded corners.
   A body's corner radius follows its component token, never a magic number.
4. **One palette, two appearances.** A surface resolves its tone from the active
   appearance instead of carrying a fixed light or dark look.
5. **Motion is part of the surface.** Springs, merges, and reveals are properties
   of the body itself, choreographed with its geometry.
6. **Restraint.** A role names where a surface sits in the hierarchy, never how
   expensive a material it wears. Content surfaces use quiet fills.
7. **One body, one hierarchy.** Interaction emphasis is a painted wash inside an
   existing body. Hover and selection never introduce a nested floating body or a
   structural accent outline.
8. **Readings belong where the question is asked.** A status reading earns its
   place by being next to the control it describes, or by being transient. It
   does not get a permanent band above every window.

## Product specifications

| Page | Purpose |
|------|---------|
| [Surfaces](surfaces.md) | Surface-role inventory for chrome with tokens and selection rules |
| [Command Panel](command-panel.md) | The command panel: canvas, tabbed main panel, side column, and motion |
| [Persona Portraits](persona.md) | Profile and portrait-content boundaries plus role-based frame styles |

## Engineering ownership

| Layer | Owner |
|------|-------|
| Generic drawing, widgets, layout, and input mechanics | Optics `lens` and `flux` |
| Semantic surface roles, palettes, and material option factories | `tessera-design` |
| Composition of design values into a rendered frame | `tessera-shell` and its components |
| Presentation, damage, and output | `tessera` (composition root) |

## Related decisions

- [Flat chrome and removal of the status HUD](../../adr/0213-flat-chrome-and-removal-of-the-status-hud.md)
- [Tiered system readings](../../adr/0214-tiered-system-readings-and-a-barrier-only-at-the-end.md)
- [Design system crate](../../adr/0046-design-system-crate.md)
- [Animation and effect placement](../../adr/0139-animation-effect-placement.md)
