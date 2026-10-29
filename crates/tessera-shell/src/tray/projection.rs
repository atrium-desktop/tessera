//! Render-thread StatusNotifierItem projection shared by every chrome surface.
//!
//! The display-only HUD and the interactive command panel present the same SNI
//! set: read the worker's snapshot, upload each item's pixmap to the GPU once
//! per icon generation, and drop textures for items that disappear. That policy
//! previously lived twice, once per surface, and could silently drift. It now
//! lives here, and each surface only decides how to lay the cells out.
//!
//! The projection never blocks: while the worker holds the snapshot lock (it
//! may be publishing a large menu tree) the previous frame's cells are reused
//! rather than stalling the render thread.

use std::collections::HashMap;

use super::{TrayHandle, TrayIcon};

/// One visible SNI cell distilled from the snapshot, before a surface lays it
/// out.
#[derive(Clone)]
pub struct TrayCell {
    /// Stable item identity, `{bus_name}{object_path}` as registered.
    pub key: String,
    /// Item title, for tooltips and menu headers.
    pub title: String,
    /// Whether the item exposes a `com.canonical.dbusmenu` context menu.
    pub has_menu: bool,
    /// Whether an icon texture was uploaded for this cell.
    pub textured: bool,
}

/// Render-thread half of the SNI tray: the shared snapshot handle plus the
/// uploaded icon textures, projecting the visible cells once per frame.
pub struct TrayProjection {
    device: flux::Device,
    handle: TrayHandle,
    /// Uploaded SNI textures keyed by item key, tagged with the snapshot's icon
    /// generation so status-only updates do not re-upload.
    textures: HashMap<String, (u64, flux::Image)>,
    /// Last frame's cells, reused when the snapshot lock is contended (the
    /// worker publishing a large menu tree must never stall rendering).
    cached: Vec<TrayCell>,
}

impl TrayProjection {
    /// Wrap a borrowed flux device and the shared tray handle. The caller must
    /// keep the underlying device alive past the projection; see the
    /// composition root's drop order.
    pub fn new(device: flux::Device, handle: TrayHandle) -> Self {
        TrayProjection {
            device,
            handle,
            textures: HashMap::new(),
            cached: Vec::new(),
        }
    }

    /// Upload new or changed icons, prune removed items, and return the visible
    /// cells for this frame. Runs on the render thread; never touches D-Bus.
    pub fn cells(&mut self) -> Vec<TrayCell> {
        let Ok(snapshot) = self.handle.snapshot().try_lock() else {
            return self.cached.clone();
        };
        self.textures
            .retain(|key, _| snapshot.items.iter().any(|item| &item.key == key));
        let mut cells = Vec::new();
        for item in &snapshot.items {
            if !item.is_visible() {
                continue;
            }
            let stale = self
                .textures
                .get(&item.key)
                .map(|(generation, _)| *generation != item.icon_generation)
                .unwrap_or(true);
            if let TrayIcon::Pixmap(pixmap) = &item.icon {
                if stale {
                    match flux::Image::from_bytes(
                        &self.device,
                        pixmap.width,
                        pixmap.height,
                        flux::Format::Bgra8Unorm,
                        &pixmap.bgra,
                    ) {
                        Ok(image) => {
                            self.textures
                                .insert(item.key.clone(), (item.icon_generation, image));
                        }
                        Err(error) => {
                            log::warn!("tray: icon upload for {} failed: {error}", item.key);
                            self.textures.remove(&item.key);
                        }
                    }
                }
            } else {
                // `None` ships no icon; a `Name` left in the snapshot means the
                // worker's theme resolution failed (memoized there, so the theme
                // is never rescanned on the render thread). Either way the item
                // must not keep rendering the previous texture.
                self.textures.remove(&item.key);
            }
            cells.push(TrayCell {
                key: item.key.clone(),
                title: item.title.clone(),
                has_menu: item.has_menu,
                textured: self.textures.contains_key(&item.key),
            });
        }
        self.cached = cells.clone();
        cells
    }

    /// The uploaded texture for `key`, if any, for the surface's glyph draw.
    pub fn texture(&self, key: &str) -> Option<&flux::Image> {
        self.textures.get(key).map(|(_, image)| image)
    }

    /// The shared tray handle, for a surface that dispatches commands or reads
    /// non-icon snapshot data (e.g. the dbusmenu tree).
    pub fn handle(&self) -> &TrayHandle {
        &self.handle
    }
}
