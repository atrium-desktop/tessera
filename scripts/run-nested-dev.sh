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
# Environment overrides:
#   TESSERA_DEV_DATA_HOME   isolated XDG_DATA_HOME (default /tmp/tessera-dev)
#   TESSERA_DEV_PROFILE     cargo profile (default release-fast; debug only for a debugger)
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
data_home=${TESSERA_DEV_DATA_HOME:-/tmp/tessera-dev}
profile=${TESSERA_DEV_PROFILE:-release-fast}

cd "$repo_root"
mkdir -p "$data_home/tessera"

# `cargo run -p tessera` does not build the binaries of other workspace
# packages, and the compositor supervises those as trusted siblings (in a debug
# build it also authorizes a sibling `tessera-settings`).
build_args=""
run_args=""
case "$profile" in
    debug) ;;
    release) build_args="--release"; run_args="--release" ;;
    *)
        build_args="--profile $profile"
        run_args="--profile $profile"
        ;;
esac

# The two argument lists are built from a fixed, quoted set of words above; the
# unquoted expansions below are intentional and never carry user input.
# shellcheck disable=SC2086
cargo build --locked $build_args \
    -p tessera-network-system -p tessera-cast -p tessera-settings

printf '%s\n' \
    'run-nested-dev: the outer session composites this window; validate damage/pacing fixes with ./scripts/run-drm-dev.sh on a spare VT.' >&2

export TESSERA_PREVIEW=control-center
export TESSERA_BACKEND=nested
export XDG_DATA_HOME="$data_home"
export XDG_DATA_DIRS="$HOME/.local/share:/usr/local/share:/usr/share"

# `--` keeps compositor arguments out of Cargo's own flag parsing.
# shellcheck disable=SC2086
exec cargo run --locked $run_args -p tessera -- "$@"
