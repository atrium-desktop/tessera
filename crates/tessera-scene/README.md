# tessera-scene

`tessera-scene` loads glTF 2.0 / VRM assets and draws them with a depth-tested
mesh renderer on the Optics gfx public RHI. It backs 3D model wallpapers
(`tessera-wallpaper`) and VRM persona portraits (`tessera-avatar`). Optics
carried this as `optics-scene` plus the gfx scene core until Optics ADR-0174;
Tessera owns it since ADR-0262.

## Responsibilities

- Parse binary glTF (`.glb`): indexed primitives, TRS node tree, skins,
  base-colour textures (decoded by the caller), and glTF animation.
- Retarget VRM Animation 1.0 clips onto VRM 0.x and 1.0 humanoid rigs.
- Draw Unlit and Blinn-Phong materials, static or GPU-skinned, with glTF
  opaque, mask and blend alpha modes, into an `optics_gfx::RenderPass`.

## Layout

- `csrc/`: the C runtime (`json.c`, `glb.c`, `animation.c`, `scene_graph.c`)
  and renderer (`render.c`), built with `cc`.
- `shaders/`: Slang sources. `spv/` holds the committed SPIR-V that the C
  embeds. Regenerate it with `scripts/compile-shaders.sh`.
- `src/`: the safe Rust API (`Scene`, `Animation`, `Camera`, `Material`,
  `SceneLight`) and bindgen FFI (`sys`).

## Boundaries

- Meshes are `optics_gfx_buffer` objects and materials own
  `optics_gfx_graphics_pipeline` objects, so the gfx retire queue destroys them
  only after in-flight frames finish (`[INV-GFX-EXT-02]`).
- No image codecs: callers pass a decoder to
  `Scene::from_glb_with_materials_and_decoder`.
