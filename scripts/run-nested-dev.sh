#!/bin/sh
set -eu

# Nested output is composited by the OUTER session, not by this process. The
# inner compositor renders into a buffer; the outer Tessera (or whatever
# compositor hosts this window) still applies its own damage/bookkeeping and
# can leave stale or half-refreshed pixels on screen. Therefore nested is a
# development convenience, not a valid acceptance environment for a damage or
# pacing fix: validate those on DRM (tty2) or after restarting the outer
# session on the same build. An installed /usr/bin/tessera older than the
# source tree is enough to reintroduce the symptom under an otherwise fixed
# inner compositor.
#
# A compositor is a real-time program: the debug profile's opt-level 1 is for
# stepping through code, not for judging frame pacing on a 5 MP output.
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
data_home=${TESSERA_DEV_DATA_HOME:-/tmp/tessera-dev}

cd "$repo_root"
mkdir -p "$data_home/tessera"

# Default to the near-release profile (full symbols, no LTO compile pause).
# Set TESSERA_DEV_PROFILE=debug when a debugger is the point.
profile=${TESSERA_DEV_PROFILE:-release-fast}
case "$profile" in
    debug) run_flags="" ;;
    release) run_flags="--release" ;;
    *) run_flags="--profile $profile" ;;
esac

printf '%s\n' \
    'run-nested-dev: the outer session composites this window; use ./scripts/run-drm-dev.sh on a spare VT to validate damage/pacing fixes.' >&2

TESSERA_COMMAND_PANEL_OPEN=1 \
TESSERA_BACKEND=nested \
XDG_DATA_HOME="$data_home" \
XDG_DATA_DIRS="$HOME/.local/share:/usr/local/share:/usr/share" \
exec cargo run --locked $run_flags -p tessera "$@"
