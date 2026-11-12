/// Decoded application-icon textures shared with chrome. `_images` owns the
/// GPU textures; `map` keys raw pointers (borrowed from `_images`) by every
/// `app_id` the entry might run as. The cache must outlive the shell:
/// components hold borrowed handles from the most recently pushed
/// `tessera_shell::component::AppCatalog` (see `tessera_shell::component::IconSet`), so a refreshed cache is
/// swapped in only after the new catalog has been fanned out.
pub(super) struct IconCache {
    pub(super) _images: Vec<flux::Image>,
    pub(super) map: std::collections::HashMap<String, *mut std::ffi::c_void>,
    pub(super) default_icon: Option<*mut std::ffi::c_void>,
}

impl IconCache {
    pub(super) fn as_icon_set(&self) -> tessera_shell::component::IconSet {
        tessera_shell::component::IconSet::from_raw_with_default(
            self.map.clone(),
            self.default_icon,
        )
    }
}

/// Fallback icon used for applications and windows without a desktop icon (ADR-0207).
pub(super) const DEFAULT_APP_ICON_PNG: &[u8] =
    include_bytes!("../../../../assets/icons/unknown.png");
pub(super) const DEFAULT_ICON_KEY: &str = "__tessera_default_app_icon__";

/// Raster extent for an application icon: the launcher asks for a large logical
/// icon, so it scales with the output while the texture stays bounded.
///
/// `tessera-assets` owns the decode and the premultiplied-BGRA layout; this is
/// only the compositor's size policy.
pub(super) fn icon_raster_extent(icon_scale: u32) -> u32 {
    tessera_launch_services::DEFAULT_ICON_SIZE
        .saturating_mul(icon_scale.max(1))
        .clamp(1, 512)
}
pub(super) const HUD_SYMBOLIC_ICON_NAMES: &[&str] = &[
    "audio-volume-muted-symbolic",
    "audio-volume-low-symbolic",
    "audio-volume-medium-symbolic",
    "audio-volume-high-symbolic",
    "network-wireless-signal-excellent-symbolic",
    "network-wired-symbolic",
    "network-offline-symbolic",
    "bluetooth-symbolic",
    "bluetooth-active-symbolic",
    "bluetooth-disabled-symbolic",
    "preferences-system-notifications-symbolic",
    "preferences-system-symbolic",
    "window-close-symbolic",
    "application-x-executable-symbolic",
];

/// Choose the raster scale from the compositor's effective output geometry.
/// The backend scale is only a fallback: direct-display `[[output]]` policy
/// can override it without changing the physical backend's native value.
pub(super) fn effective_icon_scale(output_scale: Option<f32>, backend_scale: f32) -> u32 {
    let scale = output_scale
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .or_else(|| (backend_scale.is_finite() && backend_scale > 0.0).then_some(backend_scale))
        .unwrap_or(1.0);
    scale.ceil().max(1.0) as u32
}

/// Enumerate XDG applications.
///
/// First-party external applications use the same installed metadata path as
/// every other client. Development tooling stages that layout under `target/`
/// instead of synthesizing compositor-only external entries.
pub(super) fn application_catalog(
    icon_theme: &str,
    icon_scale: u32,
) -> Vec<tessera_desktop::app::Entry> {
    tessera_launch_services::enumerate_with_theme_and_scale(icon_theme, icon_scale.max(1))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IconFileStamp {
    len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    device: u64,
    inode: u64,
}

/// Snapshot only icons the catalog actually uses. Metadata follows symlinks,
/// so a Flatpak `current/active` update is noticed even when the exported icon
/// path itself remains unchanged.
pub(super) fn snapshot_icons(
    apps: &[tessera_desktop::app::Entry],
) -> std::collections::BTreeMap<std::path::PathBuf, Option<IconFileStamp>> {
    use std::os::unix::fs::MetadataExt;

    let mut snapshot = std::collections::BTreeMap::new();
    for path in apps.iter().filter_map(|entry| entry.icon_path.as_ref()) {
        snapshot.entry(path.clone()).or_insert_with(|| {
            std::fs::metadata(path).ok().map(|metadata| IconFileStamp {
                len: metadata.len(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        });
    }
    snapshot
}

/// Maximum number of apps selected when automatic population is explicitly
/// enabled for an empty pin list.
pub(super) const AUTOPOPULATE_MAX: usize = 12;

/// Resolve the dock's pinned entries. When `pinned` names apps, each name is
/// resolved against the enumerated entries by id / desktop-stem / WM class /
/// icon name (case-insensitive), in the order given; unresolved names are
/// logged and skipped. When `pinned` is empty and `autopopulate` is set, the
/// first [`AUTOPOPULATE_MAX`] apps that have a decoded icon are pinned
/// automatically; with `autopopulate` off, an empty list stays empty (the
/// user's manual "no pins" choice).
pub(super) fn resolve_pinned(
    apps: &[tessera_desktop::app::Entry],
    icons: &std::collections::HashMap<String, *mut std::ffi::c_void>,
    pinned: &[String],
    autopopulate: bool,
) -> Vec<tessera_desktop::app::Entry> {
    if pinned.is_empty() {
        if !autopopulate {
            return Vec::new();
        }
        return apps
            .iter()
            .filter(|e| e.match_keys().iter().any(|k| icons.contains_key(k)))
            .take(AUTOPOPULATE_MAX)
            .cloned()
            .collect();
    }
    let mut out = Vec::with_capacity(pinned.len());
    for name in pinned {
        match apps
            .iter()
            .find(|entry| entry_matches_pin_name(entry, name))
        {
            Some(e) => out.push(e.clone()),
            None => log::warn!("dock: pinned app '{name}' not found among enumerated entries"),
        }
    }
    out
}

/// Resolve pins only when the Dock implementation is part of this binary.
/// Other chrome still receives the full application/icon catalog, while an
/// empty pinned projection avoids retaining Dock-only product state in a
/// build that cannot present it.
pub(super) fn resolve_chrome_pins(
    apps: &[tessera_desktop::app::Entry],
    icons: &std::collections::HashMap<String, *mut std::ffi::c_void>,
    pinned: &[String],
    autopopulate: bool,
) -> Vec<tessera_desktop::app::Entry> {
    #[cfg(feature = "chrome-dock")]
    {
        resolve_pinned(apps, icons, pinned, autopopulate)
    }
    #[cfg(not(feature = "chrome-dock"))]
    {
        let _ = (apps, icons, pinned, autopopulate);
        Vec::new()
    }
}

/// Match every configuration spelling accepted for a persistent pin. The full
/// desktop-file id is a configuration identity, while `Entry::match_keys`
/// covers the extensionless id and runtime aliases used by windows and icons.
fn entry_matches_pin_name(entry: &tessera_desktop::app::Entry, name: &str) -> bool {
    entry.matches_persistent_id(name) || entry.match_keys().contains(&name.to_ascii_lowercase())
}

fn canonical_pins(pinned: &[String]) -> Vec<String> {
    let mut canonical = Vec::with_capacity(pinned.len());
    for id in pinned {
        if !canonical.iter().any(|existing| existing == id) {
            canonical.push(id.clone());
        }
    }
    canonical
}

/// Apply explicit pin mutations to a configured list. Actions are idempotent:
/// pinning an existing application or unpinning an absent one is a no-op.
/// Application identity uses the same aliases as [`resolve_pinned`].
pub(super) fn apply_pin_actions(
    apps: &[tessera_desktop::app::Entry],
    pinned: &[String],
    actions: &[tessera_shell::component::PinAction],
) -> Vec<String> {
    let mut out = canonical_pins(pinned);
    for action in actions {
        let id = match action {
            tessera_shell::component::PinAction::Pin(id)
            | tessera_shell::component::PinAction::Unpin(id) => id,
        };
        let Some(entry) = apps.iter().find(|entry| entry.id == *id) else {
            continue;
        };
        let matches = |name: &String| entry_matches_pin_name(entry, name);
        match action {
            tessera_shell::component::PinAction::Pin(_) if !out.iter().any(matches) => {
                out.push(entry.id.clone());
            }
            tessera_shell::component::PinAction::Unpin(_) => out.retain(|name| !matches(name)),
            tessera_shell::component::PinAction::Pin(_) => {}
        }
    }
    out
}

/// Convert an opt-in automatic selection into a concrete list before its first
/// manual edit. This preserves every other visible tile when one automatic
/// tile is removed and permanently hands control to the user.
pub(super) fn materialize_pins_for_manual_edit(
    apps: &[tessera_desktop::app::Entry],
    icons: &std::collections::HashMap<String, *mut std::ffi::c_void>,
    pinned: &[String],
    autopopulate: bool,
) -> Vec<String> {
    if pinned.is_empty() && autopopulate {
        return resolve_pinned(apps, icons, pinned, true)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
    }
    canonical_pins(pinned)
}

/// One icon decoded to GPU-ready BGRA8 pixels. Produced off the frame loop
/// (icon decoding reads files and compiles SVGs — far too slow for the main
/// thread); the main loop only uploads the pixels as flux textures.
pub(super) struct DecodedIcon {
    /// Every map key the texture is inserted under. For applications these
    /// are the ids a window might report as `app_id` (StartupWMClass, the
    /// desktop-id stem, and the icon name, all lowercased); for HUD symbols
    /// the `tessera-hud:*` keys.
    pub(super) keys: Vec<String>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) bgra: Vec<u8>,
    /// HUD symbols overwrite existing keys; application icons let the first
    /// key to claim a texture win so a texture is never double-counted.
    pub(super) overwrite: bool,
}

/// Decode one asset for the icon cache.
///
/// `tessera-assets` owns the format dispatch, premultiplication and channel
/// order. A broken icon is a logged fallback, never a failed scan.
fn decode_asset(path: &std::path::Path, extent: u32) -> Option<tessera_assets::RasterImage> {
    tessera_assets::decode_file(path, extent)
        .inspect_err(|error| log::debug!("icon: {} did not decode: {error}", path.display()))
        .ok()
}

/// Decode every application and HUD icon into premultiplied BGRA8 pixels. Runs on
/// the app-scan worker thread: no GPU work, only file I/O and in-process vector
/// compilation.
pub(super) fn decode_icons(
    apps: &[tessera_desktop::app::Entry],
    icon_theme: &str,
    icon_scale: u32,
) -> Vec<DecodedIcon> {
    let mut decoded = Vec::new();
    let extent = icon_raster_extent(icon_scale);

    // Default application fallback icon decoded from the embedded PNG (ADR-0207).
    if let Ok(default) = tessera_assets::decode_bytes(DEFAULT_APP_ICON_PNG, extent) {
        decoded.push(DecodedIcon {
            keys: vec![
                DEFAULT_ICON_KEY.to_string(),
                "application-x-executable".to_string(),
                "unknown".to_string(),
            ],
            width: default.width(),
            height: default.height(),
            bgra: default.into_bgra(),
            overwrite: false,
        });
    } else {
        log::warn!("icon: the embedded default icon did not decode");
    }

    for entry in apps {
        let Some(path) = &entry.icon_path else {
            continue;
        };
        let Some(raster) = decode_asset(path, extent) else {
            continue;
        };
        decoded.push(DecodedIcon {
            keys: entry.match_keys(),
            width: raster.width(),
            height: raster.height(),
            bgra: raster.into_bgra(),
            overwrite: false,
        });
    }

    // HUD status assets come from the same icon theme as applications. Vectors
    // are rasterized at output scale (and subsequently sampled down by lens),
    // avoiding the coarse single-pixel strokes of compositor glyphs while
    // retaining the host theme's silhouettes and proportions.
    let mut symbolic_names: Vec<String> = HUD_SYMBOLIC_ICON_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for level in (0..=100).step_by(10) {
        symbolic_names.push(format!("battery-level-{level}-symbolic"));
        symbolic_names.push(format!("battery-level-{level}-charging-symbolic"));
    }
    for name in symbolic_names {
        let Some(path) = tessera_launch_services::resolve_icon_scaled(
            &name,
            Some(icon_theme),
            &[],
            24,
            icon_scale.max(1),
        ) else {
            log::debug!("hud icon: '{name}' was not found in theme '{icon_theme}'");
            continue;
        };
        let Some(mut raster) = decode_asset(&path, extent) else {
            continue;
        };
        // Symbolic themes commonly encode a dark CSS foreground intended for
        // toolkit recolouring. The compositor has no GTK style context, so
        // apply the HUD's light foreground while preserving coverage.
        raster.recolour_symbolic([246, 246, 248]);
        decoded.push(DecodedIcon {
            keys: vec![format!("tessera-hud:{name}")],
            width: raster.width(),
            height: raster.height(),
            bgra: raster.into_bgra(),
            overwrite: true,
        });
    }
    decoded
}

/// Upload decoded icons as flux textures, keyed by every id the window might
/// report as `app_id`. Pure GPU work: all file I/O and decoding happened in
/// [`decode_icons`] on the worker thread.
pub(super) fn build_icon_cache(device: &flux::Device, decoded: &[DecodedIcon]) -> IconCache {
    use std::ffi::c_void;
    let mut images: Vec<flux::Image> = Vec::new();
    let mut map: std::collections::HashMap<String, *mut c_void> = std::collections::HashMap::new();
    let mut hud_count = 0usize;

    for icon in decoded {
        match flux::Image::from_bytes(
            device,
            icon.width,
            icon.height,
            flux::Format::Bgra8Unorm,
            &icon.bgra,
        ) {
            Ok(img) => {
                let ptr = img.as_raw() as *mut c_void;
                // The dock resolves both icons and running-window matches
                // through these same keys.
                if icon.overwrite {
                    for key in &icon.keys {
                        map.insert(key.clone(), ptr);
                    }
                    hud_count += 1;
                } else {
                    for key in &icon.keys {
                        map.entry(key.clone()).or_insert(ptr);
                    }
                }
                images.push(img);
            }
            Err(e) => log::warn!(
                "icon: upload failed for {:?}: {e:?}",
                icon.keys.first().map(String::as_str).unwrap_or("?")
            ),
        }
    }

    let default_icon = map.get(DEFAULT_ICON_KEY).copied();

    log::info!(
        "icons: {} application texture(s), {hud_count} themed HUD symbol(s)",
        images.len().saturating_sub(hud_count)
    );
    IconCache {
        _images: images,
        map,
        default_icon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> tessera_desktop::app::Entry {
        tessera_desktop::app::Entry {
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn unpinning_an_absent_app_does_not_pin_it() {
        let apps = vec![app("org.example.Editor.desktop")];
        let pinned = apply_pin_actions(
            &apps,
            &[],
            &[tessera_shell::component::PinAction::Unpin(
                "org.example.Editor.desktop".into(),
            )],
        );
        assert!(pinned.is_empty());
    }

    #[test]
    fn configured_full_desktop_id_resolves() {
        let apps = vec![app("org.example.Editor.desktop")];
        let resolved = resolve_pinned(
            &apps,
            &std::collections::HashMap::new(),
            &["org.example.Editor.desktop".into()],
            false,
        );
        assert_eq!(resolved, apps);
    }

    #[test]
    fn pin_actions_are_idempotent_and_match_application_aliases() {
        let mut editor = app("org.example.Editor.desktop");
        editor.startup_wm_class = Some("ExampleEditor".into());
        let apps = vec![editor];

        let pinned = apply_pin_actions(
            &apps,
            &["exampleeditor".into()],
            &[tessera_shell::component::PinAction::Pin(
                "org.example.Editor.desktop".into(),
            )],
        );
        assert_eq!(pinned, vec!["exampleeditor"]);

        let pinned = apply_pin_actions(
            &apps,
            &pinned,
            &[tessera_shell::component::PinAction::Unpin(
                "org.example.Editor.desktop".into(),
            )],
        );
        assert!(pinned.is_empty());
    }

    #[test]
    fn first_manual_edit_preserves_other_opt_in_automatic_pins() {
        let apps = vec![
            app("org.example.Editor.desktop"),
            app("org.example.Terminal.desktop"),
        ];
        let mut icons = std::collections::HashMap::new();
        icons.insert("org.example.editor".into(), std::ptr::dangling_mut());
        icons.insert("org.example.terminal".into(), std::ptr::dangling_mut());

        let materialized = materialize_pins_for_manual_edit(&apps, &icons, &[], true);
        let pinned = apply_pin_actions(
            &apps,
            &materialized,
            &[tessera_shell::component::PinAction::Unpin(
                "org.example.Editor.desktop".into(),
            )],
        );
        assert_eq!(pinned, vec!["org.example.Terminal.desktop"]);
    }

    #[test]
    fn default_app_icon_png_decodes_successfully() {
        let raster =
            tessera_assets::decode_bytes(DEFAULT_APP_ICON_PNG, 48).expect("default icon decodes");
        assert_eq!((raster.width(), raster.height()), (48, 48));
        assert_eq!(raster.as_bgra().len(), 48 * 48 * 4);
    }

    #[test]
    fn every_resolved_icon_extension_has_a_decoder() {
        // The resolver may only return files the decoder can paint. XPM was
        // advertised once with no decoder behind it, so the asset silently
        // disappeared (ADR-0222 `[INV-ASSET-06]`, ADR-0233).
        for extension in tessera_launch_services::ICON_EXTENSIONS {
            assert!(
                tessera_assets::supports_extension(extension),
                "resolver advertises {extension} with no decoder"
            );
        }
    }

    #[test]
    fn icon_extent_scales_with_the_output_and_stays_bounded() {
        assert_eq!(icon_raster_extent(1), 128);
        assert_eq!(icon_raster_extent(2), 256);
        assert_eq!(icon_raster_extent(9), 512);
    }
}
