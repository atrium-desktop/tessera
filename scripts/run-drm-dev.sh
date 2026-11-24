#!/bin/sh
# Start a direct DRM/KMS development session from a spare Linux VT (getty).
#
# A compositor is a real-time program: a debug build's opt-level 1 measures
# debugger ergonomics, not frame pacing. Prebuild from the graphical session
# first so compiler latency stays out of the hardware test:
#
#   ./scripts/run-drm-dev.sh --build-only
#
# Then switch VTs (`Ctrl+Alt+F3`), log in, and run:
#
#   ./scripts/run-drm-dev.sh --no-build
#
# (Or run `./scripts/run-drm-dev.sh` directly to build if needed and launch.)
# For the gated overlay KMS validation, follow
# docs/how-to/kms-overlay-validation.md.
#
# Options:
#   --build-only          compile tessera and supervised siblings, then exit
#   --no-build            skip cargo build and exec the existing binary
#   --profile <NAME>      cargo profile (default: release-fast)
#   -h, --help            show this help and exit
#   --                    pass remaining arguments through to tessera
#
# Environment overrides:
#   TESSERA_DEV_DATA_HOME         isolated XDG_DATA_HOME (default /tmp/tessera-dev-drm/data)
#   TESSERA_DEV_STATE_HOME        isolated XDG_STATE_HOME (default /tmp/tessera-dev-drm/state)
#   TESSERA_DEV_PROFILE           cargo profile (default release-fast; debug only for symbols)
#   TESSERA_DEV_BUILD_ONLY=1      same as --build-only
#   TESSERA_DEV_NO_BUILD=1        same as --no-build
#   TESSERA_DEV_DBUS_RUN_SESSION=0
#                                 keep the host D-Bus session instead of isolating it
#   TESSERA_PREVIEW               optional preview surface (unset by default for real DRM)
set -eu
IFS=$(printf ' \t\nx')
IFS=${IFS%x}

die() {
    printf 'run-drm-dev: %s\n' "$*" >&2
    exit 2
}

usage() {
    cat <<'EOF'
Usage: run-drm-dev.sh [--build-only | --no-build] [--profile <NAME>] [-- <tessera-args>...]

Start a direct DRM/KMS development session from a spare Linux VT (getty).

Recommended workflow:
  1. In your graphical session: ./scripts/run-drm-dev.sh --build-only
  2. Switch to a spare VT (Ctrl+Alt+F3), log in, and run:
       ./scripts/run-drm-dev.sh --no-build

Options:
  --build-only        Build tessera and supervised sibling binaries, then exit
  --no-build          Skip cargo build and exec the existing binary
  --profile <NAME>    Cargo profile (default: release-fast)
  -h, --help          Show this help and exit
  --                  Forward all remaining arguments to tessera

Environment overrides:
  TESSERA_DEV_DATA_HOME          Isolated XDG_DATA_HOME (default /tmp/tessera-dev-drm/data)
  TESSERA_DEV_STATE_HOME         Isolated XDG_STATE_HOME (default /tmp/tessera-dev-drm/state)
  TESSERA_DEV_PROFILE            Cargo profile (default release-fast)
  TESSERA_DEV_BUILD_ONLY=1       Same as --build-only
  TESSERA_DEV_NO_BUILD=1         Same as --no-build
  TESSERA_DEV_DBUS_RUN_SESSION=0 Keep the host D-Bus session instead of dbus-run-session
  TESSERA_PREVIEW                Optional preview surface (unset by default on DRM)
EOF
}

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd -P)

data_home=${TESSERA_DEV_DATA_HOME:-/tmp/tessera-dev-drm/data}
state_home=${TESSERA_DEV_STATE_HOME:-/tmp/tessera-dev-drm/state}
profile=${TESSERA_DEV_PROFILE:-release-fast}
build_only=${TESSERA_DEV_BUILD_ONLY:-0}
no_build=${TESSERA_DEV_NO_BUILD:-0}

while [ "$#" -gt 0 ]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        --build-only)
            build_only=1
            shift
            ;;
        --no-build)
            no_build=1
            shift
            ;;
        --profile)
            [ "$#" -ge 2 ] || die "--profile requires a value"
            profile=$2
            shift 2
            ;;
        --profile=*)
            profile=${1#--profile=}
            shift
            ;;
        --)
            shift
            break
            ;;
        *)
            break
            ;;
    esac
done

[ -n "$profile" ] || die "profile must not be empty"
if [ "$build_only" = 1 ] && [ "$no_build" = 1 ]; then
    die "--build-only and --no-build cannot be used together"
fi

if [ "$build_only" != 1 ]; then
    if [ "$(id -u)" -eq 0 ]; then
        die "refusing to run the DRM backend as root; log in as the seat user"
    fi

    if [ -z "${XDG_RUNTIME_DIR:-}" ] || [ ! -d "$XDG_RUNTIME_DIR" ]; then
        die "XDG_RUNTIME_DIR is unset or missing; log in through getty/PAM, not a bare shell"
    fi

    tty_path=$(tty 2>/dev/null || true)
    case "$tty_path" in
        /dev/tty[0-9]*) ;;
        *)
            die "stdin is not a Linux VT (tty is ${tty_path:-none}); switch to a spare VT (Ctrl+Alt+F3), log in, and run this there (or pass --build-only to prebuild)"
            ;;
    esac

    if command -v loginctl >/dev/null 2>&1 && [ -n "${XDG_SESSION_ID:-}" ]; then
        session_active=$(loginctl show-session "$XDG_SESSION_ID" --property=Active --value 2>/dev/null || true)
        if [ "$session_active" = "no" ]; then
            die "session $XDG_SESSION_ID is not active; switch to its VT before starting"
        fi
    fi

    if [ -S "$XDG_RUNTIME_DIR/tessera.sock" ]; then
        printf '%s\n' \
            "run-drm-dev: warning: $XDG_RUNTIME_DIR/tessera.sock is already owned by another session; this dev compositor will run without IPC/idle supervision (use Super+Ctrl+Q to quit it)." >&2
    fi
fi

cd "$repo_root"

# Local Optics development worktrees copy .cargo/optics-local.toml into
# .cargo/config.toml, patching Optics crates to ../optics and mutating
# Cargo.lock locally. Keep --locked on canonical checkouts (including main).
use_locked=1
if [ -f "$repo_root/.cargo/config.toml" ] &&
    grep -Fqx '[patch."https://github.com/ming2k/optics"]' "$repo_root/.cargo/config.toml"; then
    use_locked=0
fi

build_workspace() {
    command -v cargo >/dev/null 2>&1 || die "cargo not found in PATH"
    set -- build
    if [ "$use_locked" = 1 ]; then
        set -- "$@" --locked
    fi
    case "$profile" in
        debug|dev) ;;
        release) set -- "$@" --release ;;
        *) set -- "$@" --profile "$profile" ;;
    esac
    # Build the compositor together with every first-party companion binary it
    # supervises as a trusted sibling.
    set -- "$@" \
        -p tessera \
        -p tessera-idle \
        -p tessera-lock \
        -p tessera-network-system \
        -p tessera-cast \
        -p tessera-settings
    cargo "$@"
}

target_dir=${CARGO_TARGET_DIR:-$repo_root/target}
case "$profile" in
    debug|dev) profile_dir="debug" ;;
    *) profile_dir="$profile" ;;
esac
compositor_bin="$target_dir/$profile_dir/tessera"

if [ "$no_build" != 1 ]; then
    build_workspace
fi

if [ "$build_only" = 1 ]; then
    printf 'run-drm-dev: built %s\n' "$compositor_bin" >&2
    exit 0
fi

if [ ! -x "$compositor_bin" ]; then
    die "missing executable $compositor_bin; run './scripts/run-drm-dev.sh --profile $profile --build-only' first"
fi

mkdir -p "$data_home/tessera" "$state_home/tessera"

export TESSERA_BACKEND=drm
export XDG_DATA_HOME="$data_home"
export XDG_STATE_HOME="$state_home"
export XDG_DATA_DIRS="$HOME/.local/share:/usr/local/share:/usr/share"
unset WAYLAND_DISPLAY DISPLAY

# Treat an explicit empty TESSERA_PREVIEW="" as disabled rather than preview mode.
if [ "${TESSERA_PREVIEW+set}" = set ] && [ -z "$TESSERA_PREVIEW" ]; then
    unset TESSERA_PREVIEW
fi

# Isolate the DRM test session on a private D-Bus session bus so portals and
# single-instance clients do not route back to a desktop running on another VT.
if [ "${TESSERA_DEV_DBUS_RUN_SESSION:-1}" != 0 ] &&
    command -v dbus-run-session >/dev/null 2>&1; then
    exec dbus-run-session -- "$compositor_bin" "$@"
fi

exec "$compositor_bin" "$@"
