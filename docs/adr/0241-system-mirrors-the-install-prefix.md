---
id: ADR-0241
title: "system/ mirrors the install prefix and is checked against its manifest"
status: accepted
date: 2026-11-16
scope: system, docs/dev/packaging, xtask, ci
superseded_by: null
negative_knowledge: true
---

# ADR-0241: system/ mirrors the install prefix and is checked against its manifest

- Status: Accepted
- Date: 2026-11-16
- Deciders: Tessera Maintainers
- Consulted: `docs/dev/packaging.md` (install manifest), `xtask` (gates), CI
- Amends: [ADR-0069](0069-documentation-owned-installation-and-throwaway-development-staging.md)
  (location of installed metadata)

---

## Context and Problem Statement

Files that ship next to the binaries — the session unit and shutdown target,
the session launcher, the lock-screen PAM service, and the Settings desktop
entry and icon — lived in `contrib/`, grouped by integration kind.

- **The name claimed the wrong support level.** By wide convention `contrib/`
  holds optional, community-maintained extras. Every file in it is a core
  install artifact; the packaging manifest marks each one `core`.
- **Grouping by kind misfiled files.** `tessera-session` is a launcher installed
  to `/usr/bin`, but it sat in `contrib/systemd/` because it starts a unit.
- **The manifest drifted unchecked.** The hand-written install table in
  `docs/dev/packaging.md` kept two `contrib/xdg-desktop-portal/` rows after the
  portal moved to its own repository, with nothing saying those paths belong
  to another tree.

## Decision Outcome

1. **`system/` replaces `contrib/` and mirrors the install prefix.** The name
   says where the files go — into the installed system — and is not a synonym
   of `assets/`, which is compiled into binaries. A file at
   `system/<path>` installs to `/usr/<path>`; `system/etc/<path>` installs to
   `/etc/<path>`. A file's location is its destination, so the manifest is
   derivable rather than authoritative, and packaging scripts can install a
   directory at a time.
2. **Placement and modes are part of the contract.** Every file sits below a
   standard prefix directory (`bin`, `etc`, `lib`, `libexec`, `sbin`, `share`).
   Files under `system/bin/` are `0755`; everything else is `0644`. Because
   `system` is an ordinary word, only `system/<prefix dir>…` counts as a path
   reference in prose ("system/microphone audio" does not).
3. **`cargo run -p xtask -- check-system-files` fails closed in CI** when a
   `system/` file has no manifest row or a row whose destination its path does
   not imply, when a row names a missing file, when a mode is wrong, or when
   any living Markdown quotes a `system/` path (or glob) that does not resolve.
   ADRs and the changelog are historical and are not scanned.
4. **Desktop entries are validated** with `desktop-file-validate` in CI.
5. **Foreign paths are labelled.** Portal install rows live in their own table
   that names the portal repository as the source tree.

### Invariants & Behavioral Boundaries

- `assets/` holds data compiled into binaries; `system/` holds files installed
  beside them. Nothing is in both.
- `system/` contains only installable files; there is no README or staging
  helper inside it.

## Rejected Alternatives & Negative Knowledge

- **Keep `contrib/`, regrouped.** Fixes the misfiled launcher but keeps a name
  that tells packagers these files are optional.
- **`dist/`.** Briefly adopted, then rejected before release: across the
  JavaScript and Python ecosystems `dist/` is generated build output, cargo-dist
  uses the word for release artifacts, and GitHub's Node and Python
  `.gitignore` templates (and many global excludes) list `dist/`, so a
  contributor's new file there can be silently left untracked.
- **`resources/`, `data/`, or `extra/`.** `resources/` has direct precedent in
  niri, and `data/` is the GNOME/Meson convention, but each is a near-synonym of
  `assets/`; the two roots must be told apart by name, not by documentation.
- **`install/`.** CMake and ROS use it for generated install trees, and some
  ignore templates list it.
- **Mirror absolute paths (`system/usr/…`).** Hard-codes `/usr` and fights
  `--prefix=/usr/local`; the prefix-relative layout installs anywhere, and
  systemd searches `/usr/local/lib/systemd/user` as well.
- **Validate units with `systemd-analyze verify` in CI.** It requires the
  `ExecStart` binary at its installed path, which a CI checkout does not have.

## Consequences

### Positive

- The tree, the manifest, and every documented install command agree, and a
  drift is a CI failure instead of a packager's bug report.
- A file's destination is readable from its path.

### Negative / Trade-offs

- Breaking for packaging scripts that reference `contrib/` paths.
- Browsing by integration kind (`systemd`, `pam`) now follows FHS paths
  (`lib/systemd/user`, `etc/pam.d`).
