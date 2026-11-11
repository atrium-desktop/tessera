#!/bin/sh
# Start a direct DRM/KMS development session from a spare VT (getty).
#
# A compositor is a real-time program: a debug build's opt-level 1 measures
# debugger ergonomics, not frame pacing. Build the optimized binary from the
# graphical session first so compiler latency stays out of the hardware test:
#
#   cargo build --profile release-fast -p tessera
#
# Then switch VTs, log in, and run:
#
#   ./scripts/run-drm-dev.sh
#
# Environment overrides:
#   TESSERA_DEV_DATA_HOME       isolated XDG_DATA_HOME (default /tmp/tessera-dev-drm)
#   TESSERA_DEV_PROFILE         cargo profile (default release-fast; debug for symbols only)
#   TESSERA_DEV_DBUS_RUN_SESSION=0
#                               keep the host D-Bus session instead of isolating it
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
data_home=${TESSERA_DEV_DATA_HOME:-/tmp/tessera-dev-drm}
profile=${TESSERA_DEV_PROFILE:-release-fast}

cd "$repo_root"

if [ "$(id -u)" -eq 0 ]; then
    printf '%s\n' 'refusing to run the DRM backend as root; log in as the seat user' >&2
    exit 2
fi

if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    printf '%s\n' 'XDG_RUNTIME_DIR is unset; log in through getty/PAM, not a bare shell' >&2
    exit 2
fi

if command -v loginctl >/dev/null 2>&1 && [ -n "${XDG_SESSION_ID:-}" ]; then
    session_active=$(loginctl show-session "$XDG_SESSION_ID" --property=Active --value 2>/dev/null || true)
    if [ "$session_active" = "no" ]; then
        printf '%s\n' "session $XDG_SESSION_ID is not active; switch to its VT before starting" >&2
        exit 2
    fi
fi

mkdir -p "$data_home/tessera"

case "$profile" in
    debug) run_flags="" ;;
    release) run_flags="--release" ;;
    *) run_flags="--profile $profile" ;;
esac
set -- cargo run $run_flags -p tessera "$@"

if command -v dbus-run-session >/dev/null 2>&1 &&
    [ "${TESSERA_DEV_DBUS_RUN_SESSION:-1}" != 0 ]; then
    set -- dbus-run-session -- "$@"
fi

exec env -u WAYLAND_DISPLAY \
    TESSERA_COMMAND_PANEL_OPEN=1 \
    TESSERA_BACKEND=drm \
    XDG_DATA_HOME="$data_home" \
    XDG_DATA_DIRS="$HOME/.local/share:/usr/local/share:/usr/share" \
    "$@"
