# tessera-pivot

`tessera-pivot` is the unified system intent and action surface for the Tessera compositor (ADR-0151).

It supersedes both `tessera-prism` (compact spotlight search) and the legacy full-screen `Launcher` (Launchpad) in `tessera-shell`.

## Architecture & Features

- **System Intent & Action Pivot**: Consolidates application launching, live window switching, and system dispatch into a single ephemeral modal surface.
- **Progressive Disclosure**:
  - **Empty/Browse Mode**: Presents pinned & recent applications alongside category pills (*All*, *Development*, *Office*, *Graphics*, *Media*, *System*, *Utilities*) for complete application discoverability without full-screen disorientation.
  - **Search Mode**: Instant keystrokes collapse the canvas into a ranked, high-throughput action list with auto-highlighted top hit.
- **Unified Keybindings**: Default `Super+Space` opens in search mode; `Super+A` or Dock system tile opens directly in category browsing mode.
