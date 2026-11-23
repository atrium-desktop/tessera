#!/bin/sh
# Build and install Tessera onto the host system (/usr and /etc).
#
# Run as a normal user (not with sudo): Cargo builds unprivileged with Optics
# statically linked into each binary, and /usr/bin/sudo is used only to install
# the resulting files as root:root.
#
# Optics discovery:
#   - If a sibling Optics checkout exists at ../optics (or OPTICS_SOURCE_DIR is
#     set), this script builds optimized static Optics archives in
#     <optics>/build-static-release and links them directly — no system Optics
#     installation is required.
#   - Otherwise, it links the system-installed static Optics archives found via
#     pkg-config.
set -eu
umask 022
IFS=$(printf ' \t\nx')
IFS=${IFS%x}

die() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}

if [ $# -ne 0 ]; then
    printf 'usage: %s (takes no arguments)\n' "$0" >&2
    exit 2
fi

if [ "$(id -u)" -eq 0 ]; then
    die "do not run as root; run as a normal user so cargo builds unprivileged"
fi

[ -x /usr/bin/sudo ] || die "/usr/bin/sudo not found"
[ -x /usr/bin/install ] || die "/usr/bin/install not found"
[ -x /usr/bin/pkg-config ] || die "/usr/bin/pkg-config not found"
command -v cargo >/dev/null 2>&1 || die "cargo not found in PATH"

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
bin_dir="$repo_root/target/release"

BINARIES='
tessera
tessera-idle
tessera-lock
tessera-settings
tessera-mcp
tessera-networkd
tessera-cast
tessera-castd
'

optics_src=""
if [ -n "${OPTICS_SOURCE_DIR:-}" ]; then
    optics_src=$(CDPATH='' cd -- "$OPTICS_SOURCE_DIR" && pwd -P)
    [ -f "$optics_src/meson.build" ] || die "OPTICS_SOURCE_DIR has no meson.build: $optics_src"
elif [ -f "$repo_root/../optics/meson.build" ]; then
    optics_src=$(CDPATH='' cd -- "$repo_root/../optics" && pwd -P)
fi

if [ -n "$optics_src" ]; then
    command -v meson >/dev/null 2>&1 || die "meson not found in PATH (required to build static Optics from $optics_src)"
    optics_build="${OPTICS_BUILD_DIR:-$optics_src/build-static-release}"
    printf 'Building static Optics (release) in %s...\n' "$optics_build"
    if [ -f "$optics_build/build.ninja" ]; then
        meson configure "$optics_build" \
            -Ddefault_library=static \
            -Dbuildtype=release \
            -Dtests=false \
            -Dexamples=false
    else
        meson setup "$optics_build" "$optics_src" \
            -Ddefault_library=static \
            -Dbuildtype=release \
            -Dtests=false \
            -Dexamples=false
    fi
    meson compile -C "$optics_build"

    printf 'Building Tessera (release) with statically linked Optics...\n'
    (
        cd -- "$repo_root"
        unset CARGO_TARGET_DIR \
            OPTICS_DYNAMIC \
            OPTICS_GFX_USE_INSTALLED OPTICS_SCENE_USE_INSTALLED OPTICS_UI_USE_INSTALLED \
            OPTICS_SVG_USE_INSTALLED OPTICS_TEXT_USE_INSTALLED \
            OPTICS_GFX_BUILD_DIR OPTICS_GFX_SOURCE_DIR \
            OPTICS_SCENE_BUILD_DIR OPTICS_SCENE_SOURCE_DIR \
            OPTICS_UI_BUILD_DIR OPTICS_UI_SOURCE_DIR \
            OPTICS_SVG_BUILD_DIR OPTICS_SVG_SOURCE_DIR \
            OPTICS_TEXT_BUILD_DIR OPTICS_TEXT_SOURCE_DIR

        OPTICS_STATIC=1 \
        OPTICS_SOURCE_DIR="$optics_src" \
        OPTICS_BUILD_DIR="$optics_build" \
            cargo build --locked --release \
                -p tessera \
                -p tessera-idle \
                -p tessera-lock \
                -p tessera-settings \
                -p tessera-mcp \
                -p tessera-network-system \
                -p tessera-cast
    )
else
    for mod in optics-gfx optics-scene optics-ui optics-svg optics-text; do
        if ! /usr/bin/pkg-config --exists "$mod" 2>/dev/null; then
            die "missing system Optics pkg-config module: $mod (install matching static Optics or place checkout at ../optics)"
        fi
    done

    printf 'Building Tessera (release) against system static Optics...\n'
    (
        cd -- "$repo_root"
        unset CARGO_TARGET_DIR \
            OPTICS_DYNAMIC \
            OPTICS_BUILD_DIR OPTICS_SOURCE_DIR \
            OPTICS_GFX_BUILD_DIR OPTICS_GFX_SOURCE_DIR \
            OPTICS_SCENE_BUILD_DIR OPTICS_SCENE_SOURCE_DIR \
            OPTICS_UI_BUILD_DIR OPTICS_UI_SOURCE_DIR \
            OPTICS_SVG_BUILD_DIR OPTICS_SVG_SOURCE_DIR \
            OPTICS_TEXT_BUILD_DIR OPTICS_TEXT_SOURCE_DIR

        OPTICS_STATIC=1 \
        OPTICS_GFX_USE_INSTALLED=1 \
        OPTICS_SCENE_USE_INSTALLED=1 \
        OPTICS_UI_USE_INSTALLED=1 \
        OPTICS_SVG_USE_INSTALLED=1 \
        OPTICS_TEXT_USE_INSTALLED=1 \
            cargo build --locked --release \
                -p tessera \
                -p tessera-idle \
                -p tessera-lock \
                -p tessera-settings \
                -p tessera-mcp \
                -p tessera-network-system \
                -p tessera-cast
    )
fi

check_regular_file() {
    [ -f "$1" ] && [ ! -L "$1" ] || die "expected regular non-symlink file: $1"
}

check_rpath() {
    bin=$1
    [ -x /usr/bin/readelf ] || return 0

    rpaths=$(/usr/bin/readelf -d -- "$bin" | sed -n \
        's/.*(RPATH).*\[\(.*\)\]/\1/p; s/.*(RUNPATH).*\[\(.*\)\]/\1/p' | tr '\n' ':')
    [ -z "$rpaths" ] && return 0

    old_ifs=$IFS
    IFS=':'
    for entry in $rpaths; do
        [ -z "$entry" ] && continue
        case "$entry" in
            /usr/lib|/usr/lib/*|/usr/lib64|/usr/lib64/*|/usr/local/lib|/usr/local/lib/*|/usr/local/lib64|/usr/local/lib64/*)
                ;;
            *)
                IFS=$old_ifs
                die "refusing to install $bin with non-system RPATH/RUNPATH: $entry"
                ;;
        esac
    done
    IFS=$old_ifs
}

check_no_optics_shared() {
    bin=$1
    [ -x /usr/bin/readelf ] || return 0

    needed=$(/usr/bin/readelf -d -- "$bin" | sed -n \
        's/.*(NEEDED).*Shared library: \[\(liboptics_\(gfx\|ui\|scene\|svg\|text\|window\|material\|app\)\.so[^]]*\)\]/\1/p')
    if [ -n "$needed" ]; then
        die "refusing to install $bin with dynamically linked Optics library: $needed"
    fi
}

install_file() {
    mode=$1
    src=$2
    dst=$3
    check_regular_file "$src"
    /usr/bin/sudo /usr/bin/install -o root -g root "-Dm$mode" -- "$src" "$dst"
}

# Validate every built binary before prompting for privilege escalation.
for bin in $BINARIES; do
    path="$bin_dir/$bin"
    check_regular_file "$path"
    [ -x "$path" ] || die "binary is not executable: $path"
    check_rpath "$path"
    check_no_optics_shared "$path"
done

printf 'Installing Tessera to /usr and /etc...\n'
/usr/bin/sudo -v

for bin in $BINARIES; do
    install_file 0755 "$bin_dir/$bin" "/usr/bin/$bin"
done

install_file 0755 \
    "$repo_root/system/bin/tessera-session" \
    /usr/bin/tessera-session
install_file 0644 \
    "$repo_root/system/lib/systemd/user/tessera.service" \
    /usr/lib/systemd/user/tessera.service
install_file 0644 \
    "$repo_root/system/lib/systemd/user/tessera-shutdown.target" \
    /usr/lib/systemd/user/tessera-shutdown.target
install_file 0644 \
    "$repo_root/system/share/applications/com.hihusky.Tessera.Settings.desktop" \
    /usr/share/applications/com.hihusky.Tessera.Settings.desktop
install_file 0644 \
    "$repo_root/system/share/icons/hicolor/scalable/apps/com.hihusky.Tessera.Settings.svg" \
    /usr/share/icons/hicolor/scalable/apps/com.hihusky.Tessera.Settings.svg
install_file 0644 \
    "$repo_root/system/etc/pam.d/tessera-lock" \
    /etc/pam.d/tessera-lock
install_file 0644 \
    "$repo_root/LICENSE" \
    /usr/share/licenses/tessera/LICENSE

if [ -x /usr/bin/update-desktop-database ] && [ -d /usr/share/applications ]; then
    /usr/bin/sudo /usr/bin/update-desktop-database -q /usr/share/applications || true
fi
if [ -x /usr/bin/gtk-update-icon-cache ] && [ -d /usr/share/icons/hicolor ]; then
    /usr/bin/sudo /usr/bin/gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi

printf '\nTessera installed.\n'
printf 'Run systemctl --user daemon-reload and restart your session to use the new build.\n'
