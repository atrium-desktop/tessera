#!/bin/sh
# Measure the compositor's GPU Render-engine load for a windowed and a
# fullscreen test video, and report whether direct scanout engaged.
#
# Run it from a terminal inside the tessera session you want to measure. On
# a DRM session, a fullscreen opaque video should drop to about 0% because KMS
# scans the client buffer out directly. If it stays near the windowed cost or
# above it, the log lines this script prints name the blocker.
#
# Usage: measure-scanout.sh [--pid PID] [--log FILE] [--seconds N] [--size WxH] [--rate FPS]
#
#   --pid PID     compositor process (default: newest `tessera` process)
#   --log FILE    compositor log to search (default: the user journal). Use
#                 this for scripts/run-drm-dev.sh sessions started with
#                 `2> FILE`.
#   --seconds N   sample window per scenario (default 6)
#   --size WxH    test-pattern size (default 1280x720)
#   --rate FPS    test-pattern frame rate (default 60)
#
# The verdict comes from the compositor's own log (`direct scanout active` or
# `direct scanout blocked: <reason>`). The load figures support it but are
# skewed by any other client animating at the same time, such as a game left
# running. Exit status: 0 when direct scanout engaged, 1 otherwise.
#
# Requires mpv. The load figure is the share of wall time that the compositor's
# DRM clients kept the Render engine busy (i915/xe fdinfo
# `drm-engine-render`), the same counter gputop shows per client.
set -eu

pid=
log_file=
seconds=6
size=1280x720
rate=60

die() {
    printf 'measure-scanout: %s\n' "$*" >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --pid) pid=${2:?}; shift 2 ;;
        --log) log_file=${2:?}; shift 2 ;;
        --seconds) seconds=${2:?}; shift 2 ;;
        --size) size=${2:?}; shift 2 ;;
        --rate) rate=${2:?}; shift 2 ;;
        -h|--help) sed -n '2,/^set -eu/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done

command -v mpv >/dev/null 2>&1 || die "mpv not found"
[ -n "${WAYLAND_DISPLAY:-}" ] || die "WAYLAND_DISPLAY is unset; run inside the tessera session"
if [ -z "$pid" ]; then
    pid=$(pgrep -nx tessera) || die "no tessera process found; pass --pid"
fi
[ -d "/proc/$pid/fdinfo" ] || die "process $pid not found"

# Sum drm-engine-render over the compositor's distinct DRM clients. Several
# file descriptors can share one client id, so each id is counted once.
render_ns() {
    for info in /proc/"$pid"/fdinfo/*; do
        awk '/^drm-client-id:/ {id=$2} /^drm-engine-render:/ {ns=$2}
             END {if (id != "") print id, ns + 0}' "$info" 2>/dev/null || true
    done | sort -u -k1,1 | awk '{sum += $2} END {printf "%d\n", sum}'
}

now_ns() {
    date +%s%N
}

# Busy share in tenths of a percent over the sample window.
sample() {
    before=$(render_ns)
    start=$(now_ns)
    sleep "$seconds"
    after=$(render_ns)
    end=$(now_ns)
    echo $(( (after - before) * 1000 / (end - start) ))
}

format_permille() {
    printf '%d.%d%%' $(( $1 / 10 )) $(( $1 % 10 ))
}

run_case() {
    label=$1
    shift
    mpv --no-config --really-quiet --no-audio --loop=inf "$@" \
        "av://lavfi:testsrc2=size=${size}:rate=${rate}" >/dev/null 2>&1 &
    player=$!
    sleep 3
    load=$(sample)
    kill "$player" 2>/dev/null || true
    wait "$player" 2>/dev/null || true
    printf '%-12s %s\n' "$label" "$(format_permille "$load")"
    eval "load_$label=$load"
}

marker=$(date '+%Y-%m-%d %H:%M:%S')
log_start=0
if [ -n "$log_file" ]; then
    [ -r "$log_file" ] || die "cannot read $log_file"
    log_start=$(wc -l <"$log_file")
fi

# Compositor log lines written since the run began.
run_log() {
    if [ -n "$log_file" ]; then
        tail -n +"$((log_start + 1))" "$log_file"
    else
        journalctl --user --since "$marker" --no-pager -o cat 2>/dev/null
    fi
}
printf 'compositor pid %s, %s@%s test pattern, %ss per sample\n' "$pid" "$size" "$rate" "$seconds"
idle=$(sample)
printf '%-12s %s\n' idle "$(format_permille "$idle")"
run_case windowed --geometry="$size"
run_case fullscreen --fs
sleep 1

printf '\nscanout decisions during the run:\n'
decisions=$(run_log | grep -E 'direct scanout|reclaimed the primary plane|claims visible pixels' || true)
if [ -n "$decisions" ]; then
    printf '%s\n' "$decisions" | tail -n 20 | sed 's/^/  /'
else
    printf '  (none)\n'
fi

# The compositor's own decision is the verdict. Load is supporting evidence:
# another GPU client (a game left running) inflates every sample.
# shellcheck disable=SC2154 # set through eval in run_case
printf '\nwindowed -> fullscreen load: %s -> %s\n' \
    "$(format_permille "$load_windowed")" "$(format_permille "$load_fullscreen")"
if printf '%s\n' "$decisions" | grep -q 'direct scanout active'; then
    printf 'verdict: direct scanout ENGAGED for the fullscreen client\n'
elif printf '%s\n' "$decisions" | grep -q 'direct scanout blocked'; then
    printf 'verdict: direct scanout BLOCKED; the blocked line names the reason\n'
    exit 1
else
    printf 'verdict: no scanout decision logged (nested session, or info logging disabled)\n'
    exit 1
fi
