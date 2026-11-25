#!/bin/sh
# Start a nested development session inside the current Wayland desktop.
#
# Nested output is composited by the OUTER session: the inner compositor
# renders into a buffer and the host applies its own damage bookkeeping. A
# stale or half-refreshed region can therefore come from the host even when the
# inner compositor is correct, so nested is a development convenience, not
# evidence for a damage or pacing fix. Validate those on a spare VT with
# ./scripts/run-drm-dev.sh, or after restarting the host session on the same
# build.
#
# Options:
#   --build-only          compile tessera and supervised siblings, then exit
#   --no-build            skip cargo build and exec the existing binary
#   --profile <NAME>      cargo profile (default: release-fast)
#   -h, --help            show this help and exit
#   --                    pass remaining arguments through to tessera
#
# Environment overrides:
#   TESSERA_DEV_CONFIG_HOME isolated XDG_CONFIG_HOME (default /tmp/tessera-dev/config)
#   TESSERA_DEV_DATA_HOME   isolated XDG_DATA_HOME (default /tmp/tessera-dev/data)
#   TESSERA_DEV_STATE_HOME  isolated XDG_STATE_HOME (default /tmp/tessera-dev/state)
#   TESSERA_DEV_IPC_SOCKET  isolated TESSERA_IPC_SOCKET (default /tmp/tessera-dev/tessera.sock)
#   TESSERA_DEV_PROFILE     cargo profile (default release-fast; debug only for a debugger)
#   TESSERA_DEV_BUILD_ONLY=1
#                           same as --build-only
#   TESSERA_DEV_NO_BUILD=1  same as --no-build
#   TESSERA_PREVIEW         preview surface (default control-center; set empty to disable)
set -eu
IFS=$(printf ' \t\nx')
IFS=${IFS%x}

die() {
    printf 'run-nested-dev: %s\n' "$*" >&2
    exit 2
}

usage() {
    cat <<'EOF'
Usage: run-nested-dev.sh [--build-only | --no-build] [--profile <NAME>] [-- <tessera-args>...]

Start a nested Tessera development session inside the current Wayland desktop.

Options:
  --build-only        Build tessera and supervised sibling binaries, then exit
  --no-build          Skip cargo build and exec the existing binary
  --profile <NAME>    Cargo profile (default: release-fast)
  -h, --help          Show this help and exit
  --                  Forward all remaining arguments to tessera

Environment overrides:
  TESSERA_DEV_CONFIG_HOME Isolated XDG_CONFIG_HOME (default /tmp/tessera-dev/config)
  TESSERA_DEV_DATA_HOME   Isolated XDG_DATA_HOME (default /tmp/tessera-dev/data)
  TESSERA_DEV_STATE_HOME  Isolated XDG_STATE_HOME (default /tmp/tessera-dev/state)
  TESSERA_DEV_IPC_SOCKET  Isolated TESSERA_IPC_SOCKET (default /tmp/tessera-dev/tessera.sock)
  TESSERA_DEV_PROFILE     Cargo profile (default release-fast)
  TESSERA_DEV_BUILD_ONLY=1
                          Same as --build-only
  TESSERA_DEV_NO_BUILD=1  Same as --no-build
  TESSERA_PREVIEW         Preview surface (default control-center; set TESSERA_PREVIEW= to disable)
EOF
}

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd -P)

config_home=${TESSERA_DEV_CONFIG_HOME:-/tmp/tessera-dev/config}
data_home=${TESSERA_DEV_DATA_HOME:-/tmp/tessera-dev/data}
state_home=${TESSERA_DEV_STATE_HOME:-/tmp/tessera-dev/state}
ipc_socket=${TESSERA_DEV_IPC_SOCKET:-/tmp/tessera-dev/tessera.sock}
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
    if [ -z "${WAYLAND_DISPLAY:-}" ]; then
        die "WAYLAND_DISPLAY is unset; start this inside a Wayland session or use ./scripts/run-drm-dev.sh on a spare VT"
    fi
    if [ -z "${XDG_RUNTIME_DIR:-}" ] || [ ! -d "$XDG_RUNTIME_DIR" ]; then
        die "XDG_RUNTIME_DIR is unset or missing"
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
    # supervises as a trusted sibling (in a debug build it also authorizes a
    # sibling `tessera-settings`).
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
settings_bin="$target_dir/$profile_dir/tessera-settings"

if [ "$no_build" != 1 ]; then
    build_workspace
fi

if [ "$build_only" = 1 ]; then
    printf 'run-nested-dev: built %s\n' "$compositor_bin" >&2
    exit 0
fi

if [ ! -x "$compositor_bin" ]; then
    die "missing executable $compositor_bin; run './scripts/run-nested-dev.sh --profile $profile --build-only' first"
fi

mkdir -p "$config_home/tessera" "$data_home/tessera" "$state_home/tessera" "$(dirname -- "$ipc_socket")"

# Authorize the workspace-built tessera-settings binary inside the isolated dev
# config so Settings is immediately writable under any profile (including
# release-fast where debug_assertions is disabled) without touching the host
# user's ~/.config/tessera/config.toml (ADR-0257).
dev_config="$config_home/tessera/config.toml"
if [ ! -f "$dev_config" ]; then
    cat >"$dev_config" <<EOF
schema_version = 2

[ipc.scope_executables]
tessera-settings = ["$settings_bin"]
EOF
fi

printf '%s\n' \
    'run-nested-dev: the outer session composites this window; validate damage/pacing fixes with ./scripts/run-drm-dev.sh on a spare VT.' >&2
printf 'run-nested-dev: isolated config=%s ipc=%s\n' "$dev_config" "$ipc_socket" >&2
printf 'run-nested-dev: split-screen Settings -> TESSERA_IPC_SOCKET=%s %s\n' "$ipc_socket" "$settings_bin" >&2

# Default to opening the Control Center preview for quick UI iteration, while
# honoring an explicit override (including TESSERA_PREVIEW="" to disable it).
if [ "${TESSERA_PREVIEW+set}" != set ]; then
    export TESSERA_PREVIEW=control-center
elif [ -z "$TESSERA_PREVIEW" ]; then
    unset TESSERA_PREVIEW
fi

export TESSERA_BACKEND=nested
export TESSERA_IPC_SOCKET="$ipc_socket"
export XDG_CONFIG_HOME="$config_home"
export XDG_DATA_HOME="$data_home"
export XDG_STATE_HOME="$state_home"
export XDG_DATA_DIRS="$HOME/.local/share:/usr/local/share:/usr/share"

exec "$compositor_bin" "$@"
