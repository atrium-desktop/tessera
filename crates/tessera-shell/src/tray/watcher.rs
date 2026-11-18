//! The StatusNotifierWatcher/Host D-Bus machinery.
//!
//! Three roles, all on the session bus:
//!
//! - the served `org.kde.StatusNotifierWatcher` interface (method calls are
//!   dispatched by zbus's own internal executor; the handlers are async only
//!   because that is what the served-interface API drives),
//! - a signal thread that consumes a single blocking message iterator
//!   (`NameOwnerChanged` for liveness, the item's `New*`/`PropertiesChanged`
//!   signals for icon/status refreshes, and `LayoutUpdated` on any open
//!   dbusmenu) and rewrites the shared snapshot,
//! - a command thread that takes [`TrayCommand`]s off an mpsc channel and
//!   invokes `Activate`/`SecondaryActivate`/`ContextMenu` on the item, plus
//!   the dbusmenu `GetLayout`/`Event` methods when the shell requests a
//!   host-rendered context menu.
//!
//! Everything the compositor touches crosses the thread boundary as plain
//! values through `Arc<Mutex<Shared>>`; the compositor's render thread never
//! blocks on D-Bus.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use tessera_dbus::{
    Connection, FromValue, Message, MessageStream, ObjectHandler, OwnedObjectPath, Proxy, Value,
};

use super::{
    MenuState, TrayCommand, TrayHandle, TrayIcon, TrayItem, TrayPixmap, TraySnapshot, TrayStatus,
    argb32_to_premultiplied_bgra, parse_layout_reply, select_pixmap,
};

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_IFACE: &str = "org.kde.StatusNotifierWatcher";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";
const DBUS_IFACE: &str = "org.freedesktop.DBus";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const DBUSMENU_IFACE: &str = "com.canonical.dbusmenu";
/// Preferred icon size: the shell renders 18px cells, so 48 covers 2x output
/// scales with headroom; the closest available size wins.
const PIXMAP_TARGET: i32 = 48;

/// Properties mirrored from one StatusNotifierItem.
#[derive(Default)]
struct ItemProps {
    title: String,
    icon: TrayIcon,
    status: TrayStatus,
    has_menu: bool,
    /// dbusmenu object path (the SNI `Menu` property). `None` when the item
    /// exposes no menu or registers it as `/`.
    menu_path: Option<String>,
}

/// A tracked item: identity for the bus plus the mirrored render state.
struct ItemEntry {
    /// Bus name as registered (well-known or unique).
    bus_name: String,
    /// Unique name of the registrant's connection; used as the signal-sender
    /// match and as the destination for method calls (always routable).
    unique_name: String,
    path: String,
    /// dbusmenu object path; the proxy target for `GetLayout`/`Event`/signals.
    menu_path: Option<String>,
    item: TrayItem,
}

#[derive(Default)]
struct Shared {
    items: HashMap<String, ItemEntry>,
    snapshot: Option<Arc<Mutex<TraySnapshot>>>,
    /// The currently open dbusmenu popover (the worker refreshes it on
    /// `LayoutUpdated`, the render thread reads it under the snapshot lock).
    menu: Option<MenuState>,
    /// Bumped on every `menu` assignment so the render thread can cache its
    /// clone of the menu tree; mirrored into [`TraySnapshot::menu_revision`].
    menu_revision: u64,
    /// Theme-icon resolution memo keyed by icon name: `None` entries are
    /// failed lookups (missing or undecodable), so a bad name never rescans
    /// the theme. Runs on the worker so the render thread only uploads
    /// ready-made pixmaps. Bounded by [`MAX_ICON_CACHE_ENTRIES`]: names come
    /// from arbitrary SNI items on the session bus, and an item that cycles
    /// icon names per state would otherwise grow the memo for the process
    /// lifetime. Evicting only costs a theme re-scan on the next lookup.
    icon_cache: HashMap<String, Option<TrayPixmap>>,
}

/// Ceiling on the theme-icon memo. Realistic trays hold a few dozen distinct
/// names; the ceiling exists so a churning item (or a hostile bus peer)
/// cannot grow the memo without bound. Each entry is a name plus at most one
/// 48×48 RGBA pixmap.
const MAX_ICON_CACHE_ENTRIES: usize = 256;

impl Shared {
    /// Rebuild the sorted render snapshot after any mutation.
    fn republish(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let mut items: Vec<TrayItem> = self
            .items
            .values()
            .map(|entry| entry.item.clone())
            .collect();
        items.sort_by(|a, b| a.key.cmp(&b.key));
        *snapshot.lock().unwrap_or_else(|e| e.into_inner()) = TraySnapshot {
            items,
            menu: self.menu.clone(),
            menu_revision: self.menu_revision,
        };
    }
}

/// Resolve a theme-named icon to a pixmap in place, memoized per name. A
/// failed resolution keeps the `Name` (the shell renders the fallback glyph)
/// and stays memoized so the theme is not rescanned on the next update.
fn resolve_named_icon(icon: &mut TrayIcon, cache: &mut HashMap<String, Option<TrayPixmap>>) {
    let TrayIcon::Name(name) = icon else {
        return;
    };
    if let Some(pixmap) = cache
        .entry(name.clone())
        .or_insert_with(|| super::icon::resolve_theme_icon(name))
    {
        *icon = TrayIcon::Pixmap(pixmap.clone());
    }
    // Bound the memo: entries are retained even for items that later
    // unregister (names are not tracked back to owners), so cap the count
    // and drop arbitrary entries past the ceiling — a re-scan on the next
    // lookup is the only cost.
    if cache.len() > MAX_ICON_CACHE_ENTRIES {
        let excess = cache.len() - MAX_ICON_CACHE_ENTRIES;
        let victims: Vec<String> = cache.keys().take(excess).cloned().collect();
        for victim in victims {
            cache.remove(&victim);
        }
    }
}

/// Merge freshly read properties into an entry, bumping the icon generation
/// only when the icon actually changed (so the shell's texture cache survives
/// status-only updates).
fn apply_props(item: &mut TrayItem, props: ItemProps) {
    if item.icon != props.icon {
        item.icon_generation += 1;
        item.icon = props.icon;
    }
    item.title = props.title;
    item.status = props.status;
    item.has_menu = props.has_menu;
}

/// The served `org.kde.StatusNotifierWatcher` interface.
struct WatcherIface {
    shared: Arc<Mutex<Shared>>,
}

impl WatcherIface {
    fn register_item(&self, conn: &Connection, service: &str, sender: &str) {
        let (bus_name, path) = if service.starts_with('/') {
            (sender.to_string(), service.to_string())
        } else {
            (service.to_string(), DEFAULT_ITEM_PATH.to_string())
        };
        let key = format!("{bus_name}{path}");
        let destination = if sender.is_empty() {
            bus_name.clone()
        } else {
            sender.to_string()
        };

        {
            let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            let mut item = TrayItem {
                key: key.clone(),
                title: String::new(),
                icon: TrayIcon::None,
                status: TrayStatus::Active,
                has_menu: false,
                icon_generation: 0,
            };
            if let Some(previous) = shared.items.get(&key) {
                item.icon_generation = previous.item.icon_generation;
            }
            shared.items.insert(
                key.clone(),
                ItemEntry {
                    bus_name,
                    unique_name: sender.to_string(),
                    path: path.clone(),
                    menu_path: None,
                    item,
                },
            );
            shared.republish();
        }

        let refresh_conn = conn.clone();
        let shared = Arc::clone(&self.shared);
        let refresh_key = key.clone();
        let sender_owned = sender.to_string();
        let _ = thread::Builder::new()
            .name("sni-initial-properties".into())
            .spawn(move || {
                let mut props = fetch_props_blocking(&refresh_conn, &destination, &path);
                let menu_path = props.menu_path.clone();
                let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
                resolve_named_icon(&mut props.icon, &mut shared.icon_cache);
                let Some(entry) = shared.items.get_mut(&refresh_key) else {
                    return;
                };
                if entry.unique_name != sender_owned || entry.path != path {
                    return;
                }
                entry.menu_path = menu_path;
                apply_props(&mut entry.item, props);
                shared.republish();
            });

        log::info!("tray: registered {key}");
        if let Err(error) = conn.emit_signal(
            None,
            WATCHER_PATH,
            WATCHER_IFACE,
            "StatusNotifierItemRegistered",
            &(service,),
        ) {
            log::warn!("tray: could not emit StatusNotifierItemRegistered: {error}");
        }
    }

    fn registered_items(&self) -> Value {
        let items: Vec<Value> = self
            .shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .items
            .keys()
            .cloned()
            .map(Value::String)
            .collect();
        Value::Array("s".into(), items)
    }
}

impl ObjectHandler for WatcherIface {
    fn handle_method_call(
        &self,
        conn: &Connection,
        msg: &Message,
    ) -> Result<Vec<Value>, (String, String)> {
        match (msg.interface.as_deref(), msg.member.as_deref()) {
            (Some(WATCHER_IFACE), Some("RegisterStatusNotifierItem")) => {
                let service = msg
                    .body
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let sender = msg.sender.as_deref().unwrap_or("");
                self.register_item(conn, &service, sender);
                Ok(Vec::new())
            }
            (Some(WATCHER_IFACE), Some("RegisterStatusNotifierHost")) => Ok(Vec::new()),
            (Some(PROPERTIES_IFACE), Some("Get")) => {
                let prop = msg.body.get(1).and_then(Value::as_str).unwrap_or("");
                let val = match prop {
                    "RegisteredStatusNotifierItems" => self.registered_items(),
                    "IsStatusNotifierHostRegistered" => Value::Bool(true),
                    "ProtocolVersion" => Value::Int32(0),
                    _ => {
                        return Err((
                            "org.freedesktop.DBus.Error.UnknownProperty".into(),
                            format!("unknown property {prop}"),
                        ));
                    }
                };
                Ok(vec![Value::Variant(Box::new(val))])
            }
            (Some(PROPERTIES_IFACE), Some("GetAll")) => {
                let entries = vec![
                    Value::DictEntry(
                        Box::new(Value::String("RegisteredStatusNotifierItems".into())),
                        Box::new(Value::Variant(Box::new(self.registered_items()))),
                    ),
                    Value::DictEntry(
                        Box::new(Value::String("IsStatusNotifierHostRegistered".into())),
                        Box::new(Value::Variant(Box::new(Value::Bool(true)))),
                    ),
                    Value::DictEntry(
                        Box::new(Value::String("ProtocolVersion".into())),
                        Box::new(Value::Variant(Box::new(Value::Int32(0)))),
                    ),
                ];
                Ok(vec![Value::Array("{sv}".into(), entries)])
            }
            _ => Err((
                "org.freedesktop.DBus.Error.UnknownMethod".into(),
                format!("unknown method {:?}.{:?}", msg.interface, msg.member),
            )),
        }
    }
}

/// Same read through the blocking proxy (signal thread).
fn fetch_props_blocking(conn: &Connection, destination: &str, path: &str) -> ItemProps {
    let proxy = Proxy::new(conn, destination, path, ITEM_IFACE);
    let id: Option<String> = proxy.get_property("Id").ok();
    let title: Option<String> = proxy.get_property("Title").ok();
    let status: Option<String> = proxy.get_property("Status").ok();
    let icon_name: Option<String> = proxy.get_property("IconName").ok();
    let icon_pixmap: Option<Vec<(i32, i32, Vec<u8>)>> = proxy.get_property("IconPixmap").ok();
    let menu: Option<OwnedObjectPath> = proxy.get_property("Menu").ok();
    build_props(id, title, status, icon_name, icon_pixmap, menu)
}

fn build_props(
    id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    icon_name: Option<String>,
    icon_pixmap: Option<Vec<(i32, i32, Vec<u8>)>>,
    menu: Option<OwnedObjectPath>,
) -> ItemProps {
    let title = title
        .filter(|title| !title.is_empty())
        .or(id)
        .unwrap_or_default();
    let status = TrayStatus::parse(status.as_deref().unwrap_or("Active"));
    // Prefer the raw pixmap because it preserves the item's exact artwork.
    // Theme names remain available as a deterministic generic-icon fallback.
    let icon = match (icon_pixmap, icon_name) {
        (Some(pixmaps), name) => match select_pixmap(&pixmaps, PIXMAP_TARGET) {
            Some(index) => {
                let (w, h, data) = &pixmaps[index];
                TrayIcon::Pixmap(TrayPixmap {
                    width: *w as u32,
                    height: *h as u32,
                    bgra: argb32_to_premultiplied_bgra(data),
                })
            }
            None => name
                .filter(|name| !name.is_empty())
                .map_or(TrayIcon::None, TrayIcon::Name),
        },
        (None, Some(name)) if !name.is_empty() => TrayIcon::Name(name),
        (None, _) => TrayIcon::None,
    };
    let menu_path = menu
        .filter(|path| path.as_str() != "/")
        .map(|path| path.as_str().to_string());
    let has_menu = menu_path.is_some();
    ItemProps {
        title,
        icon,
        status,
        has_menu,
        menu_path,
    }
}

/// Start the watcher and both worker threads. See [`super::spawn`].
pub(super) fn spawn() -> Option<TrayHandle> {
    let conn = match Connection::session() {
        Ok(conn) => conn,
        Err(error) => {
            log::warn!("tray: could not connect to session bus ({error}); SNI tray disabled");
            return None;
        }
    };
    let shared = Arc::new(Mutex::new(Shared::default()));
    let iface = Arc::new(WatcherIface {
        shared: Arc::clone(&shared),
    });
    // Serve the interface before requesting the name so no registration can
    // arrive at a name we own but do not serve yet.
    conn.register_object(WATCHER_PATH, iface);
    if let Err(error) = conn.request_name(WATCHER_NAME) {
        log::info!("tray: {WATCHER_NAME} unavailable ({error}); SNI tray disabled");
        return None;
    }
    // Advertise that a host (this compositor) is present; items that
    // registered before we started would re-register now.
    if let Err(error) = conn.emit_signal(
        None,
        WATCHER_PATH,
        WATCHER_IFACE,
        "StatusNotifierHostRegistered",
        &(),
    ) {
        log::warn!("tray: could not emit StatusNotifierHostRegistered: {error}");
    }

    let snapshot = Arc::new(Mutex::new(TraySnapshot::default()));
    shared.lock().unwrap_or_else(|e| e.into_inner()).snapshot = Some(Arc::clone(&snapshot));
    let (tx, rx) = mpsc::channel::<TrayCommand>();

    let signal_conn = conn.clone();
    let signal_shared = Arc::clone(&shared);
    let spawned_signals = thread::Builder::new()
        .name("sni-tray-signals".to_string())
        .spawn(move || signal_loop(signal_conn, signal_shared));
    let spawned_commands = thread::Builder::new()
        .name("sni-tray-commands".to_string())
        .spawn(move || command_loop(conn, shared, rx));
    if spawned_signals.is_err() || spawned_commands.is_err() {
        log::warn!("tray: could not spawn worker threads; SNI tray disabled");
        return None;
    }
    Some(TrayHandle {
        snapshot,
        commands: tx,
    })
}

/// Consume bus signals: drop items whose name vanished, refresh items whose
/// icon/title/status changed.
fn signal_loop(conn: Connection, shared: Arc<Mutex<Shared>>) {
    let stream = match MessageStream::for_match_rule(&conn, "type='signal'") {
        Ok(stream) => stream,
        Err(error) => {
            log::warn!("tray: could not subscribe to bus signals: {error}");
            return;
        }
    };
    while let Some(message) = stream.next() {
        handle_signal(&conn, &shared, &message);
    }
    log::info!("tray: signal stream ended; SNI tray stopped");
}

fn handle_signal(conn: &Connection, shared: &Arc<Mutex<Shared>>, message: &Message) {
    let interface = message.interface.as_deref();
    let member = message.member.as_deref();
    let sender = message.sender.clone();

    match (interface, member) {
        (Some(DBUS_IFACE), Some("NameOwnerChanged")) => {
            if message.body.len() < 3 {
                return;
            }
            let Some(name) = message.body[0].as_str().map(str::to_string) else {
                return;
            };
            let Some(new_owner) = message.body[2].as_str() else {
                return;
            };
            if !new_owner.is_empty() {
                return;
            }
            let removed: Vec<String> = {
                let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
                let keys: Vec<String> = shared
                    .items
                    .iter()
                    .filter(|(_, entry)| entry.unique_name == name || entry.bus_name == name)
                    .map(|(key, _)| key.clone())
                    .collect();
                for key in &keys {
                    shared.items.remove(key);
                }
                if !keys.is_empty() {
                    shared.republish();
                }
                keys
            };
            for key in removed {
                log::info!("tray: unregistered {key}");
                if let Err(error) = conn.emit_signal(
                    None,
                    WATCHER_PATH,
                    WATCHER_IFACE,
                    "StatusNotifierItemUnregistered",
                    &(key.as_str(),),
                ) {
                    log::warn!("tray: could not emit StatusNotifierItemUnregistered: {error}");
                }
            }
        }
        (Some(ITEM_IFACE), Some("NewIcon" | "NewTitle" | "NewStatus" | "NewAttentionIcon"))
        | (Some(PROPERTIES_IFACE), Some("PropertiesChanged")) => {
            let Some(sender) = sender else { return };
            let path = message.path.as_ref().map(ToString::to_string);
            let key = {
                let shared = shared.lock().unwrap_or_else(|e| e.into_inner());
                shared
                    .items
                    .iter()
                    .find(|(_, entry)| {
                        (entry.unique_name == sender || entry.bus_name == sender)
                            && Some(&entry.path) == path.as_ref()
                    })
                    .map(|(key, _)| key.clone())
            };
            let Some(key) = key else { return };
            let mut props =
                fetch_props_blocking(conn, &sender, path.as_deref().unwrap_or(DEFAULT_ITEM_PATH));
            let menu_path = props.menu_path.clone();
            let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
            resolve_named_icon(&mut props.icon, &mut shared.icon_cache);
            if let Some(entry) = shared.items.get_mut(&key) {
                entry.menu_path = menu_path;
                apply_props(&mut entry.item, props);
                shared.republish();
            }
        }
        (Some(DBUSMENU_IFACE), Some("LayoutUpdated")) => {
            // The signal carries `(revision, parent)`; we only need to know
            // whether it belongs to the currently open menu so the render
            // thread sees fresh rows on the next frame.
            let path = message.path.as_ref().map(ToString::to_string);
            let needs_refresh = match path {
                None => false,
                Some(path) => {
                    let shared = shared.lock().unwrap_or_else(|e| e.into_inner());
                    match &shared.menu {
                        Some(menu) => shared
                            .items
                            .get(&menu.key)
                            .map(|entry| entry.menu_path.as_deref() == Some(path.as_str()))
                            .unwrap_or(false),
                        None => false,
                    }
                }
            };
            if needs_refresh {
                refresh_open_menu(conn, shared);
            }
        }
        _ => {}
    }
}

/// Re-fetch the layout for the currently open menu (called from the signal
/// thread when `LayoutUpdated` arrives). Holds the lock only briefly to look
/// up the target; the `GetLayout` round-trip happens unlocked.
fn refresh_open_menu(conn: &Connection, shared: &Arc<Mutex<Shared>>) {
    let target = {
        let shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(menu) = &shared.menu else {
            return;
        };
        let Some(entry) = shared.items.get(&menu.key) else {
            return;
        };
        let destination = if entry.unique_name.is_empty() {
            entry.bus_name.clone()
        } else {
            entry.unique_name.clone()
        };
        let Some(menu_path) = entry.menu_path.clone() else {
            return;
        };
        (menu.key.clone(), destination, menu_path)
    };
    let Some((root, revision)) = fetch_menu_layout(conn, &target.1, &target.2) else {
        return;
    };
    let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
    shared.menu = Some(MenuState {
        key: target.0,
        root,
        revision,
    });
    shared.menu_revision += 1;
    shared.republish();
}

/// Invoke activation methods on items as the shell's clicks come in.
fn command_loop(conn: Connection, shared: Arc<Mutex<Shared>>, rx: mpsc::Receiver<TrayCommand>) {
    while let Ok(command) = rx.recv() {
        match command {
            TrayCommand::Activate { key, x, y } => {
                invoke_item_method(&conn, &shared, &key, "Activate", x, y);
            }
            TrayCommand::SecondaryActivate { key, x, y } => {
                invoke_item_method(&conn, &shared, &key, "SecondaryActivate", x, y);
            }
            TrayCommand::ContextMenu { key, x, y } => {
                invoke_item_method(&conn, &shared, &key, "ContextMenu", x, y);
            }
            TrayCommand::FetchMenu { key } => fetch_menu_command(&conn, &shared, &key),
            TrayCommand::MenuEvent { key, id } => menu_event_command(&conn, &shared, &key, id),
            TrayCommand::CloseMenu { key } => close_menu_command(&shared, &key),
        }
    }
}

fn invoke_item_method(
    conn: &Connection,
    shared: &Arc<Mutex<Shared>>,
    key: &str,
    method: &str,
    x: i32,
    y: i32,
) {
    let target = shared
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .items
        .get(key)
        .map(|entry| {
            let destination = if entry.unique_name.is_empty() {
                entry.bus_name.clone()
            } else {
                entry.unique_name.clone()
            };
            (destination, entry.path.clone())
        });
    let Some((destination, path)) = target else {
        return;
    };
    let proxy = Proxy::new(conn, destination.as_str(), path.as_str(), ITEM_IFACE);
    if let Err(error) = proxy.call::<_, ()>(method, &(x, y)) {
        log::warn!("tray: {method} on {key} failed: {error}");
    }
}

/// `FetchMenu` handler: look up the item's dbusmenu path, call `GetLayout`,
/// parse the reply, and stash the [`MenuState`] in `Shared`. Replaces any
/// menu already open.
fn fetch_menu_command(conn: &Connection, shared: &Arc<Mutex<Shared>>, key: &str) {
    let target = shared
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .items
        .get(key)
        .map(|entry| {
            let destination = if entry.unique_name.is_empty() {
                entry.bus_name.clone()
            } else {
                entry.unique_name.clone()
            };
            (destination, entry.menu_path.clone())
        });
    let Some((destination, Some(menu_path))) = target else {
        return;
    };
    let Some((root, revision)) = fetch_menu_layout(conn, &destination, &menu_path) else {
        return;
    };
    let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
    shared.menu = Some(MenuState {
        key: key.to_string(),
        root,
        revision,
    });
    shared.menu_revision += 1;
    shared.republish();
}

/// `MenuEvent` handler: send `Event(id, "clicked", u32 variant, timestamp)`
/// on the item's dbusmenu proxy. The spec leaves `data` unused for `clicked`;
/// a zero `u32` variant matches what most toolkits send.
fn menu_event_command(conn: &Connection, shared: &Arc<Mutex<Shared>>, key: &str, id: i32) {
    let target = shared
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .items
        .get(key)
        .map(|entry| {
            let destination = if entry.unique_name.is_empty() {
                entry.bus_name.clone()
            } else {
                entry.unique_name.clone()
            };
            (destination, entry.menu_path.clone())
        });
    let Some((destination, Some(menu_path))) = target else {
        return;
    };
    let data = Value::Variant(Box::new(Value::Uint32(0)));
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    let proxy = Proxy::new(
        conn,
        destination.as_str(),
        menu_path.as_str(),
        DBUSMENU_IFACE,
    );
    if let Err(error) = proxy.call::<_, ()>("Event", &(id, "clicked", data, timestamp)) {
        log::warn!("tray: Event(clicked) on {key}:{id} failed: {error}");
    }
}

/// `CloseMenu` handler: clear `Shared.menu` if it matches `key`. The bar
/// sends this on every close so the worker can stop refreshing on
/// `LayoutUpdated`.
fn close_menu_command(shared: &Arc<Mutex<Shared>>, key: &str) {
    let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
    if shared
        .menu
        .as_ref()
        .map(|menu| menu.key == key)
        .unwrap_or(false)
    {
        shared.menu = None;
        shared.menu_revision += 1;
        shared.republish();
    }
}

/// `GetLayout(0, -1, [property-names])` round-trip on the worker thread.
/// Returns the parsed root and revision, or `None` on any D-Bus or parse
/// failure (the caller falls back to the legacy `ContextMenu` behavior).
fn fetch_menu_layout(
    conn: &Connection,
    destination: &str,
    menu_path: &str,
) -> Option<(super::MenuNode, u32)> {
    let proxy = Proxy::new(conn, destination, menu_path, DBUSMENU_IFACE);
    let _ = proxy.call::<_, bool>("AboutToShow", &(0_i32,));
    let property_names: Vec<&str> = super::MENU_PROPERTY_NAMES.to_vec();
    let msg = conn
        .call_method(
            Some(destination),
            menu_path,
            Some(DBUSMENU_IFACE),
            "GetLayout",
            &(0_i32, -1_i32, property_names),
        )
        .ok()?;
    if msg.body.len() < 2 {
        return None;
    }
    let revision = u32::from_value(msg.body[0].clone()).ok()?;
    let root_node =
        <(i32, HashMap<String, Value>, Vec<Value>)>::from_value(msg.body[1].clone()).ok()?;
    let (root, revision) = parse_layout_reply((revision, root_node))?;
    Some((root, revision))
}
