#!/bin/sh
# Regenerate the precompiled SPIR-V embedded by tessera-material and
# tessera-scene (ADR-0262).
#
# Cargo builds never run a shader compiler: each crate embeds the committed
# `spv/*.spv` with C23 #embed. Run this after editing any `shaders/*.slang`
# and commit the result. `--check` recompiles into a temporary directory and
# fails if the committed SPIR-V is stale.
#
# Requires `slangc` and `spirv-val`. The material shaders include the analytic
# SDF from the Optics gfx public shader headers: set OPTICS_SOURCE_DIR to an
# Optics checkout (default: ../optics).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
optics=${OPTICS_SOURCE_DIR:-$root/../optics}
gfx_include="$optics/libs/gfx/include"
if [ ! -f "$gfx_include/optics/gfx/shaders/optics_gfx_sdf.slang" ]; then
    printf '%s\n' "compile-shaders: no Optics gfx shader headers under $gfx_include" \
        "set OPTICS_SOURCE_DIR to an Optics checkout" >&2
    exit 2
fi

check=0
if [ "${1:-}" = "--check" ]; then
    check=1
fi

flags="-target spirv -profile spirv_1_6 -Wno-41012 -entry main"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
stale=0

# compile <crate> <source stem> <stage> <output stem> [extra slangc args...]
compile() {
    crate=$1
    src=$2
    stage=$3
    out=$4
    shift 4
    dir="$root/crates/$crate"
    # shellcheck disable=SC2086 # $flags is a deliberate word list.
    slangc $flags -stage "$stage" -I "$dir/shaders" -I "$gfx_include" \
        -I "$gfx_include/optics/gfx/shaders" "$@" \
        "$dir/shaders/$src.slang" -o "$work/$out.spv"
    spirv-val --target-env vulkan1.3 "$work/$out.spv"
    if [ "$check" -eq 1 ]; then
        if ! cmp -s "$work/$out.spv" "$dir/spv/$out.spv"; then
            printf 'stale: crates/%s/spv/%s.spv\n' "$crate" "$out" >&2
            stale=1
        fi
    else
        cp "$work/$out.spv" "$dir/spv/$out.spv"
    fi
}

for shader in liquid_glass frosted storage_clear; do
    compile tessera-material "$shader.comp" compute "$shader.comp"
    compile tessera-material "$shader.comp" compute "${shader}_16f.comp" \
        -DTESSERA_MATERIAL_STORAGE_RGBA16F
done
compile tessera-material backdrop_stats.comp compute backdrop_stats.comp

for shader in scene_unlit scene_phong; do
    compile tessera-scene "$shader.vert" vertex "$shader.vert"
    compile tessera-scene "$shader.frag" fragment "$shader.frag"
done
compile tessera-scene scene_unlit_skin.vert vertex scene_unlit_skin.vert
compile tessera-scene scene_phong_skin.vert vertex scene_phong_skin.vert

if [ "$stale" -ne 0 ]; then
    echo "compile-shaders: run scripts/compile-shaders.sh and commit the SPIR-V" >&2
    exit 1
fi
