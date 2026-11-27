---
id: ADR-0262
title: "Tessera owns its backdrop materials, 3D scene runtime and chrome glyphs on the Optics gfx public RHI"
status: accepted
date: 2026-11-27
scope: rendering, runtime/backdrop, wallpaper, avatar, shell/icons, dependencies, build
superseded_by: null
negative_knowledge: true
---

# 0262. Tessera owns its backdrop materials, 3D scene runtime and chrome glyphs on the Optics gfx public RHI

- Status: Accepted
- Date: 2026-11-27
- Deciders: Tessera Maintainers
- Amends: [ADR-0253](0253-a-backdrop-capture-beneath-chrome-draws-the-glass-materials.md)
  (the frosted and liquid-glass filters come from `tessera-material` instead of
  `optics-material`; the capture/blur/filter/slab pipeline is unchanged)
- Amends: [ADR-0261](0261-in-process-librsvg-for-theme-vector-assets.md)
  (Decision 5 is void: optics-ui no longer links `liboptics_svg` or offers
  `register_svg_icon`; chrome glyphs are `tessera-design::icons`)

---

## Context and Problem Statement

Optics v0.0.88 ([Optics ADR-0174](https://github.com/ming2k/optics/blob/v0.0.88/docs/adr/0174-pure-graphics-ui-core-retire-format-parsers-3d-and-materials.md))
made Optics a pure six-library 2D graphics and UI stack and removed, with no
compatibility shims:

- `liboptics_material` / `optics-material` — the frosted and liquid-glass
  filters Tessera's chrome backdrop draws (ADR-0253);
- `liboptics_scene` / `optics-scene` and the gfx 3D scene core
  (`<optics/gfx/scene.h>`) — the glTF/VRM loader, animation and depth-tested
  mesh renderer behind the 3D wallpaper and the VRM persona portrait;
- `liboptics_svg` and optics-ui's bundled 288-icon pack with its
  process-global registry — every chrome glyph except four chevrons, plus
  `register_svg_icon`, through which the shell drew its own glyphs.

Tessera ships all three features. Adopting v0.0.88 needs a new owner for each.

## Decision Drivers

- **Feature parity.** Frosted and liquid-glass chrome, 3D model wallpapers and
  VRM portraits are shipped behaviour.
- **Optics' boundary is upstream policy.** Optics deliberately rejects format
  parsers, 3D, branded materials and icon packs; pushing them back upstream
  contradicts Optics ADR-0174.
- **Public API only.** Tessera code may use only Optics' public headers and
  bindings, never gfx internals, so the next Optics release cannot break it
  silently.
- **In-flight safety.** GPU objects a frame still references must never be
  destroyed early.
- **No new build prerequisites for packagers.** A Tessera build must not need
  a shader compiler.

## Considered Options

- **Option 1: Vendor the retired C sources and shaders into Tessera-owned
  crates, rebuilt on the Optics gfx public RHI.** Chosen.
- **Option 2: Rewrite materials and the scene runtime in Rust** (e.g. the
  `gltf` crate plus hand-written Vulkan). Rejected for now: it re-derives tested
  code (glTF/VRM parsing, VRM retargeting, the lens shader's analytic SDF
  composite) for no behavioural gain, and adds a crate graph ADR-0206 resists.
- **Option 3: Pin Optics at v0.0.87.** Rejected: it freezes Tessera on an
  unmaintained branch and forfeits every later Optics fix.
- **Option 4: Drop the features.** Rejected: user-visible regressions.

## Decision Outcome

1. **`crates/tessera-material`** carries the frosted and liquid-glass filters
   (C under `csrc/`, compute shaders under `shaders/`) with the Rust API
   previously in `optics-material`. Acrylic, mica and the layered backdrop
   compositor, which Tessera never used, are not carried. The C uses only
   public gfx headers (`compute.h`, `vulkan.h`, `profile.h`, `observe.h`) and
   attributes its allocations to `owner = "tessera/material"`.
2. **`crates/tessera-scene`** carries the glTF 2.0 / VRM runtime (`json.c`,
   `glb.c`, `animation.c`, `scene_graph.c`) and the mesh renderer
   (`render.c`), with the Rust API previously split across `optics-scene` and
   `optics_gfx::scene`. Draws record into the public
   `optics_gfx::RenderPass`. The renderer is **rebuilt, not copied**:
   - meshes are `optics_gfx_buffer` objects and each material owns two
     `optics_gfx_graphics_pipeline` objects (static, skinned), so the gfx
     retire queue defers their destruction past every in-flight frame;
   - the public pipeline descriptor takes one vertex binding, so skinned
     meshes read joints and weights through a buffer device address indexed by
     `gl_VertexIndex` instead of a second binding;
   - the public blend presets are opaque and premultiplied source-over, so the
     fragment shaders premultiply blended (glTF `BLEND`) surfaces;
   - the analytic vector-coverage material and tagged-colour (ICC/LUT) texture
     decode depended on gfx internals and are dropped; 8-bit textures into a
     16F target are still decoded from sRGB.
3. **Symbols are Tessera's.** The C API is `tessera_material_*` and
   `tessera_scene_*` with Tessera-local descriptor tags, so nothing claims to
   be Optics and nothing can collide with a future Optics symbol.
4. **Shaders are precompiled.** Each crate embeds committed `spv/*.spv` with
   C23 `#embed`. `scripts/compile-shaders.sh` regenerates them from the Slang
   sources (it needs `slangc`, `spirv-val` and the Optics gfx shader headers),
   and `--check` fails on stale SPIR-V. Cargo builds never run `slangc`.
5. **Linking is unchanged.** Both crates compile their C statically with `cc`
   and find the gfx and Vulkan headers through `OPTICS_SOURCE_DIR`
   (local worktree) or the `optics-gfx` pkg-config package. `optics-gfx-sys`
   still owns the gfx link and its static/dynamic mode.
6. **`tessera-design::icons` owns the chrome glyphs.** `Glyph` lists every glyph
   chrome draws: the Feather command streams Optics shipped up to v0.0.87
   (MIT, `crates/tessera-design/FEATHER-LICENSE`) plus Tessera's keyboard
   backlight and Bluetooth-off glyphs. `icons::install(&mut Ui)` registers them
   on every optics-ui context the shell, settings and lock screen create, right
   after creation. `Glyph::id()` resolves without a context handle because a
   fresh per-context registry assigns slots sequentially. `install` verifies
   every context got the same ids and reports one that did not.

### Invariants & Behavioral Boundaries

- `[INV-GFX-EXT-01] Public Optics surface only.` `tessera-material` and
  `tessera-scene` include only installed Optics public headers; a gfx internal
  header or symbol in either crate is a defect.
- `[INV-GFX-EXT-02] Deferred GPU lifetime.` Every GPU object these crates
  create is a refcounted gfx object (`optics_gfx_buffer`,
  `optics_gfx_graphics_pipeline`, `optics_gfx_compute_pipeline`,
  `optics_gfx_image`) released through gfx, never a raw Vulkan object the
  crate destroys itself.
- `[INV-GFX-EXT-03] Committed SPIR-V matches its source.`
  `scripts/compile-shaders.sh --check` passes on every commit that touches a
  `.slang` file.
- `[INV-ICON-01] Glyphs install first.` `tessera_design::icons::install` runs on
  a context before anything else registers an icon there.

## Rejected Alternatives & Negative Knowledge

### Keeping the copied renderer on raw Vulkan pipelines
- **Why considered**: The v0.0.87 renderer created raw `VkPipeline`s and
  allocated mesh memory directly, and a verbatim copy is the smallest diff.
- **Why rejected**: It used gfx internals (`optics_gfx_vk_alloc_buffer`,
  `optics_gfx_vk_retire_pipeline`, private device and frame fields). Without
  them a raw pipeline cannot be destroyed safely while a frame that bound it is
  in flight, short of a device-wide idle wait.

### Compiling shaders in `build.rs`
- **Why considered**: No binary artifacts in Git.
- **Why rejected**: It makes `slangc` a build prerequisite for every packager
  and CI lane that builds Tessera, for shaders that change rarely.

### Headless optics-ui contexts for glyph tests
- **Why rejected**: In Optics v0.0.88 a device-less context allocates the icon
  slot table with unzeroed `malloc`, so registering more than 16 icons corrupts
  the registry and double-frees on release. Production contexts are always
  device-backed (zeroed allocation) and unaffected; glyph tests use a real
  device.

## Consequences

### Positive
- Optics v0.0.88 adopted with feature parity: frosted and liquid-glass chrome,
  3D model wallpapers and VRM portraits render as before.
- The material and scene code now evolves with Tessera's needs, under its
  own tests (`tessera-material/tests/gpu.rs` composites both filters on a
  device; the wallpaper model tests render the procedural knot).
- Optics' retired libraries no longer appear in the packaging prerequisites.

### Negative
- Tessera now maintains about 6.7k lines of vendored C (headers included) and
  roughly 900 lines of Slang.
- Glyph ids depend on install order; a context that registers an icon before
  `install` gets no usable chrome glyphs (reported, never mis-drawn).
- Tagged-colour (ICC/LUT) glTF textures decode as untagged sRGB.
