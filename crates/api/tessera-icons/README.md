# tessera-icons

`tessera-icons` resolves icon names to on-disk files using the freedesktop.org
[Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/).

## Responsibilities

- Scale-aware resolution from `index.theme` directory metadata
  (`Type=Fixed/Scalable/Threshold`, `Scale`, `MinSize`/`MaxSize`).
- Recursive theme inheritance with `hicolor` as the mandatory final fallback.
- Unthemed fallbacks such as `/usr/share/pixmaps/<name>.png`.
- XDG base-directory search bases (`~/.icons`, `<data>/icons`, pixmaps).

## Non-goals

- No desktop-entry parsing: application discovery lives in
  `tessera-desktop-entries`.
- No image decoding: callers own the pixel pipeline.

The crate is dependency-minimal and side-effect free apart from filesystem
reads. It exists so any consumer — application launchers, the SNI tray,
portrait assets — can resolve icons without depending on desktop-entry
parsing.
