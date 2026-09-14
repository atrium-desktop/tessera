# ADR-0151: Pivot: Unified system intent and action surface

- Status: Proposed
- Date: 2026-09-12
- Scope: `tessera-pivot`, `tessera-prism`, `tessera-shell`, `tessera-model`, `tessera-dock`;
  supersedes [ADR-0073](0073-prism-search-and-explicit-application-shortcuts.md),
  amends [ADR-0022](0022-application-launcher.md) and [ADR-0021](0021-chrome-component-trait.md)

## Context

Desktop environments have historically split application discovery into two disparate surfaces:
1. A **full-screen application grid** (the "Launchpad" pattern, implemented in `tessera-shell::chrome::launcher` per [ADR-0022](0022-application-launcher.md)), intended for visual browsing and casual exploration.
2. A **compact search bar** (the "Spotlight" pattern, implemented as `tessera-prism` per [ADR-0073](0073-prism-search-and-explicit-application-shortcuts.md)), intended for targeted retrieval by name.

In practice, this duality introduces friction and architectural bloat:

- **Immersive disruption:** A full-screen grid obscures all user context and desktop state with heavy blur and oversized icons. On modern high-resolution displays, it presents low information density and high cognitive cost. Power users bypass it entirely in favor of instant search.
- **Discoverability hazard of pure search:** Conversely, stripping away visual browsing entirely in favor of a minimalist single-line search box leaves users stranded when they do not remember an application's exact name, seek visual cues (e.g. an icon color), or wish to inventory newly installed tools.
- **Conflicting modal chrome lifecycles:** Maintaining two concurrent application chrome surfaces requires complex mutual-exclusion locks (e.g. closing Prism when opening Launcher and vice-versa), duplicate presentation logic, competing keybindings (`Super+A` vs `Super+Space`), and redundant backdrop passes.
- **Narrow scope of application launchers:** Modern computing demands unified intent dispatch. A modern desktop interface does not solely launch `.desktop` binaries; users frequently switch across open windows and workspaces, trigger system settings, evaluate mathematical expressions, and interact with language model agents (`tessera-agent-broker` and `tessera-mcp`).

The compositor requires a single, coherent, and extensible interaction nexus: **Pivot**.

---

## Decision

### 1. Consolidate Launcher and Prism into `tessera-pivot`

We supersede [ADR-0073](0073-prism-search-and-explicit-application-shortcuts.md) and retire both the full-screen `Launcher` chrome in `tessera-shell` and the `tessera-prism` crate.

A new standalone chrome component crate, **`crates/chrome/tessera-pivot`**, becomes the sole trusted system overlay for intent entry, discovery, and dispatch.

`tessera-pivot` implements `tessera_chrome::Chrome` and consumes `tessera_model::launcher::Launcher` alongside system command and workspace models. It owns its geometry, layout morphing, input capture, and liquid-glass presentation.

### 2. Establish Pivot as the System Intent & Action Cockpit

Pivot is not merely an application launcher. It is defined as the compositor's **central intent dispatcher** organized around four functional tiers:

1. **Entities & Spatial Navigation (Level 1):**
   - Application launching and instance focusing.
   - Live window switching across workspaces by window title or application class.
   - Recent projects and document navigation.
2. **System Command Palette (Level 2):**
   - Direct execution of core desktop actions without opening full settings panels (e.g. toggle dark/light theme, lock session, adjust display brightness or audio sink, reload configuration).
3. **Inline Micro-Utilities (Level 3):**
   - Immediate evaluation for math expressions, unit conversions, and clipboard history lookups.
4. **Agent & MCP Gateway (Level 4):**
   - Seamless handoff to `tessera-agent-broker` and `tessera-mcp` when an input query represents a natural language instruction or complex task rather than an exact keyword match.

### 3. Progressive Disclosure: Single Unified, Modal-Free Surface

Pivot rejects artificial "modes" (no separate browse vs search modal states) in favor of a **single unified front-end**:

```text
┌────────────────────────────────────────────────────────┐
│  🔍  Search apps, windows, actions, or ask...           │  <- Query bar
├────────────────────────────────────────────────────────┤
│  [All]  [Development]  [Office]  [Media]  [System] ... │  <- Category filters
├────────────────────────────────────────────────────────┤
│  ┌───┐  ┌───┐  ┌───┐  ┌───┐  ┌───┐  ┌───┐             │
│  │ 💻│  │ 🌐│  │ 📝│  │ 🎨│  │ 📁│  │ ⚙️│             │  <- Pinned / Categories
│  └───┘  └───┘  └───┘  └───┘  └───┘  └───┘             │
│  Terminal  Web    Notes   Figma  Files  Settings       │
└────────────────────────────────────────────────────────┘
```

- **Natural Progressive Response (No Mode Toggles):**
  - When query is empty: Pivot naturally presents categorical application pills (*All*, *Dev*, *Office*, etc.) and navigable application tiles for instant visual inventory.
  - When typing: Keystrokes instantly collapse the body into a ranked, high-throughput matching list with auto-highlighted top hits.
  - Backspacing back to an empty query seamlessly restores the full categorical inventory.

### 4. Separation of Concerns: Pivot vs File Manager

Application inventory management is divided strictly by user intent:

- **Pivot (Ephemeral Invocation & Discovery):**
  Owns rapid launching, category browsing, keyboard search, and contextual dispatch. It remains an ephemeral, lightweight chrome surface.
- **File Manager / System Settings (Durable Asset Management):**
  The file manager (or system settings) exposes a virtual `Applications` target. This view owns durable asset operations: inspecting disk size, reviewing application sandbox permissions, sorting by installation date, and triggering package removal or uninstallation. Pivot does not replicate file-system inspection or package management workflows.

### 5. Unified Activation and Dock Integration

- **Keyboard:** A single global shortcut (`Super+Space`) opens and closes Pivot. There is no `Super+A` or modal distinction.
- **Pointer:** Clicking the primary system tile on `tessera-dock` summons the same unified Pivot surface.
- **Lifecycle:** As a single component, Pivot eliminates all mutual-exclusion race conditions, transition conflicts, and duplicated backdrop blur allocations.

---

## Alternatives

- **Retain distinct Launchpad and Prism surfaces:**
  Rejected. Maintaining over 2,000 lines of bespoke full-screen grid code in `tessera-shell` alongside `tessera-prism` incurs high maintenance overhead and leaves the full-screen grid rarely used.
- **Strip application browsing down to a pure single-line search bar:**
  Rejected. Pure search fails user discoverability when software names are unknown or forgotten, and degrades the mouse-driven experience for casual or tablet-mode interactions.
- **Relegate all application browsing exclusively to the File Manager:**
  Rejected. Launching an entire persistent file-management window just to explore what applications exist carries excessive visual and cognitive weight for a simple launch task.

---

## Consequences

- `crates/chrome/tessera-prism` is deprecated and superseded by `crates/chrome/tessera-pivot`.
- The legacy `Launcher` struct and rendering logic (~1,890 lines) are removed from `tessera-shell`.
- The compositor manages a single intent overlay, cutting peak GPU blur passes and memory usage during chrome transitions.
- The Dock and keybind configuration transition cleanly: `prism` and `launchpad` actions become aliases for activating Pivot in search or browse modes respectively.
- Follow-up implementation phases will incrementally bind System Commands (Level 2), Micro-Utilities (Level 3), and `tessera-agent-broker` intent delegation (Level 4) into `tessera-pivot`'s provider pipeline.
