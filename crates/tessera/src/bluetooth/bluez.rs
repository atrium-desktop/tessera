//! Native BlueZ (`org.bluez`) backend over the system D-Bus.
//!
//! Mirrors the iwd bridge's shape (`crate::wireless::iwd`): a blocking zbus
//! connection owned by the subsystem's worker thread, a cached projection of
//! the daemon's managed objects, and one `refresh` that recomputes the whole
//! cache. Nothing here runs on the render thread, and no BlueZ object path or
//! interface name escapes the cache ([`INV-NET-DAEMON-NEUTRAL`]).

use std::borrow::Borrow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tessera_desktop::system::{BluetoothDevice, BluetoothKind, BluetoothLinkState};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

use super::BluetoothBackend;

const BLUEZ_SERVICE: &str = "org.bluez";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";

/// Discovery auto-stop window. BlueZ keeps `Discovering` latched until asked
/// to stop, so the subsystem bounds a scan rather than leaving the adapter
/// broadcasting indefinitely.
const DISCOVERY_WINDOW: Duration = Duration::from_secs(20);

/// How often the worker's `poll` re-reads managed objects. `poll` runs on
/// every worker wakeup (every 500 ms), so this gate keeps the D-Bus round
/// trip off that cadence while discovery is idle.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

fn get_str<'a>(props: &'a HashMap<String, OwnedValue>, key: &str) -> Option<&'a str> {
    props.get(key).and_then(|value| {
        let v: &Value = Borrow::<Value>::borrow(value);
        v.downcast_ref::<&str>().ok()
    })
}

fn get_bool(props: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    props.get(key).and_then(|value| {
        let v: &Value = Borrow::<Value>::borrow(value);
        v.downcast_ref::<bool>().ok()
    })
}

fn get_u32(props: &HashMap<String, OwnedValue>, key: &str) -> Option<u32> {
    props.get(key).and_then(|value| {
        let v: &Value = Borrow::<Value>::borrow(value);
        v.downcast_ref::<u32>().ok()
    })
}

/// Map a BlueZ `Icon` name to the daemon-neutral device class.
fn kind_from_icon(icon: &str) -> Option<BluetoothKind> {
    let icon = icon.to_ascii_lowercase();
    if icon.starts_with("audio-") {
        Some(BluetoothKind::Audio)
    } else if icon.starts_with("input-") {
        Some(BluetoothKind::Input)
    } else if icon.starts_with("phone") {
        Some(BluetoothKind::Phone)
    } else if icon.starts_with("computer") {
        Some(BluetoothKind::Computer)
    } else if icon.starts_with("printer")
        || icon.starts_with("scanner")
        || icon.starts_with("camera")
    {
        Some(BluetoothKind::Imaging)
    } else if icon.starts_with("watch") {
        Some(BluetoothKind::Wearable)
    } else {
        None
    }
}

/// Map the Bluetooth "major device class" bits of a Class of Device value.
fn kind_from_class(class: u32) -> BluetoothKind {
    match (class >> 8) & 0x1f {
        0x01 => BluetoothKind::Computer,
        0x02 => BluetoothKind::Phone,
        0x04 => BluetoothKind::Audio,
        0x05 => BluetoothKind::Input,
        0x06 => BluetoothKind::Imaging,
        0x07 => BluetoothKind::Wearable,
        _ => BluetoothKind::Other,
    }
}

fn device_kind(props: &HashMap<String, OwnedValue>) -> BluetoothKind {
    get_str(props, "Icon")
        .and_then(kind_from_icon)
        .or_else(|| get_u32(props, "Class").map(kind_from_class))
        .unwrap_or_default()
}

pub struct BluezBackend {
    conn: Connection,
    cache: Arc<Mutex<BluezCache>>,
}

#[derive(Default)]
struct BluezCache {
    adapter_path: Option<String>,
    powered: bool,
    discovering: bool,
    /// Instant the bounded discovery window closes; `None` when not scanning.
    discovery_deadline: Option<Instant>,
    /// Instant of the last managed-object refresh, for the `poll` gate.
    last_refresh: Option<Instant>,
    devices: Vec<BluetoothDevice>,
}

impl BluezBackend {
    /// Connect to the system bus, probe for a BlueZ adapter, and cache it.
    ///
    /// Fails when the bus is unreachable, BlueZ is not running, or the host
    /// has no adapter — the caller then reports Bluetooth as unavailable
    /// rather than fabricating a radio.
    pub fn open() -> Result<Self, String> {
        let conn = Connection::system().map_err(|e| format!("system bus unavailable: {e}"))?;
        let backend = Self {
            conn,
            cache: Arc::new(Mutex::new(BluezCache::default())),
        };
        backend.refresh()?;
        if backend.cache.lock().unwrap().adapter_path.is_none() {
            return Err("no bluetooth adapter found".to_string());
        }
        Ok(backend)
    }

    /// Refresh the cached adapter and device projection from BlueZ.
    pub fn refresh(&self) -> Result<(), String> {
        let reply = self
            .conn
            .call_method(
                Some(BLUEZ_SERVICE),
                "/",
                Some(OBJECT_MANAGER_IFACE),
                "GetManagedObjects",
                &(),
            )
            .map_err(|e| format!("bluez ObjectManager call failed: {e}"))?;

        type ManagedObjects =
            HashMap<zbus::zvariant::OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
        let objects: ManagedObjects = reply
            .body()
            .deserialize()
            .map_err(|e| format!("failed to parse bluez managed objects: {e}"))?;

        let mut adapter_path = None;
        let mut powered = false;
        let mut discovering = false;
        let mut devices = Vec::new();

        for (path, ifaces) in &objects {
            if let Some(props) = ifaces.get(ADAPTER_IFACE) {
                // One adapter is the session's radio; a second is ignored,
                // matching the single-adapter presentation chrome assumes.
                if adapter_path.is_none() {
                    adapter_path = Some(path.as_str().to_string());
                    powered = get_bool(props, "Powered").unwrap_or(false);
                    discovering = get_bool(props, "Discovering").unwrap_or(false);
                }
            }

            if let Some(props) = ifaces.get(DEVICE_IFACE) {
                let address = get_str(props, "Address").unwrap_or("").to_string();
                if address.is_empty() {
                    continue;
                }
                let name = get_str(props, "Alias")
                    .or_else(|| get_str(props, "Name"))
                    .unwrap_or("")
                    .to_string();
                devices.push(BluetoothDevice {
                    address,
                    name,
                    kind: device_kind(props),
                    connected: get_bool(props, "Connected").unwrap_or(false),
                    paired: get_bool(props, "Paired").unwrap_or(false),
                    trusted: get_bool(props, "Trusted").unwrap_or(false),
                });
            }
        }

        sort_devices(&mut devices);

        let mut cache = self.cache.lock().unwrap();
        cache.adapter_path = adapter_path;
        cache.powered = powered;
        cache.discovering = discovering;
        cache.devices = devices;
        cache.last_refresh = Some(Instant::now());
        Ok(())
    }

    fn adapter_path(&self) -> Result<String, String> {
        self.cache
            .lock()
            .unwrap()
            .adapter_path
            .clone()
            .ok_or_else(|| "no bluetooth adapter found".to_string())
    }

    fn device_path(&self, address: &str) -> Result<String, String> {
        let adapter = self.adapter_path()?;
        // BlueZ device object paths are the adapter path plus `dev_` and the
        // address with colons replaced by underscores.
        let suffix = address.replace(':', "_");
        Ok(format!("{adapter}/dev_{suffix}"))
    }

    fn set_property(
        &self,
        path: &str,
        iface: &str,
        name: &str,
        value: Value<'_>,
    ) -> Result<(), String> {
        self.conn
            .call_method(
                Some(BLUEZ_SERVICE),
                path,
                Some(PROPERTIES_IFACE),
                "Set",
                &(iface, name, value),
            )
            .map_err(|e| format!("bluez set {iface}.{name} failed: {e}"))?;
        Ok(())
    }
}

/// Presentation order: connected first, then paired, then in-range only;
/// stable within a group so the panel does not reshuffle between frames.
fn sort_devices(devices: &mut [BluetoothDevice]) {
    devices.sort_by_key(|device| {
        let rank = if device.connected {
            0
        } else if device.paired {
            1
        } else {
            2
        };
        (
            rank,
            device.name.to_ascii_lowercase(),
            device.address.clone(),
        )
    });
}

impl BluetoothBackend for BluezBackend {
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let adapter = self.adapter_path()?;
        self.set_property(&adapter, ADAPTER_IFACE, "Powered", Value::Bool(enabled))?;
        if !enabled {
            let mut cache = self.cache.lock().unwrap();
            cache.discovery_deadline = None;
        }
        let _ = self.refresh();
        Ok(())
    }

    fn scan(&self) -> Result<(), String> {
        let adapter = self.adapter_path()?;
        let (powered, discovering) = {
            let cache = self.cache.lock().unwrap();
            (cache.powered, cache.discovering)
        };
        if !powered {
            return Err("bluetooth adapter is powered off".to_string());
        }
        if !discovering {
            self.conn
                .call_method(
                    Some(BLUEZ_SERVICE),
                    adapter.as_str(),
                    Some(ADAPTER_IFACE),
                    "StartDiscovery",
                    &(),
                )
                .map_err(|e| format!("bluez StartDiscovery failed: {e}"))?;
        }
        // Extend (or arm) the bounded window; a repeated request never leaves
        // discovery running forever.
        self.cache.lock().unwrap().discovery_deadline = Some(Instant::now() + DISCOVERY_WINDOW);
        let _ = self.refresh();
        Ok(())
    }

    fn pair(&self, address: &str) -> Result<(), String> {
        let path = self.device_path(address)?;
        self.conn
            .call_method(
                Some(BLUEZ_SERVICE),
                path.as_str(),
                Some(DEVICE_IFACE),
                "Pair",
                &(),
            )
            .map_err(|e| format!("bluez Pair failed: {e}"))?;
        // Pair alone leaves the peer untrusted; trusting it lets BlueZ
        // reconnect on proximity, which is what "connected" means to a user
        // who just paired a headset or keyboard.
        let _ = self.set_property(&path, DEVICE_IFACE, "Trusted", Value::Bool(true));
        let _ = self.refresh();
        Ok(())
    }

    fn connect(&self, address: &str) -> Result<(), String> {
        let path = self.device_path(address)?;
        self.conn
            .call_method(
                Some(BLUEZ_SERVICE),
                path.as_str(),
                Some(DEVICE_IFACE),
                "Connect",
                &(),
            )
            .map_err(|e| format!("bluez Connect failed: {e}"))?;
        let _ = self.refresh();
        Ok(())
    }

    fn disconnect(&self, address: &str) -> Result<(), String> {
        let path = self.device_path(address)?;
        self.conn
            .call_method(
                Some(BLUEZ_SERVICE),
                path.as_str(),
                Some(DEVICE_IFACE),
                "Disconnect",
                &(),
            )
            .map_err(|e| format!("bluez Disconnect failed: {e}"))?;
        let _ = self.refresh();
        Ok(())
    }

    fn forget(&self, address: &str) -> Result<(), String> {
        let adapter = self.adapter_path()?;
        self.conn
            .call_method(
                Some(BLUEZ_SERVICE),
                adapter.as_str(),
                Some(ADAPTER_IFACE),
                "RemoveDevice",
                &(
                    zbus::zvariant::ObjectPath::try_from(self.device_path(address)?)
                        .map_err(|e| format!("invalid device path: {e}"))?,
                ),
            )
            .map_err(|e| format!("bluez RemoveDevice failed: {e}"))?;
        let _ = self.refresh();
        Ok(())
    }

    fn poll(&self) {
        let mut cache = self.cache.lock().unwrap();
        // Close an expired discovery window before refreshing, so the read
        // below observes `Discovering = false` again.
        if let Some(deadline) = cache.discovery_deadline
            && Instant::now() >= deadline
        {
            if let Some(adapter) = cache.adapter_path.clone() {
                // The call is bounded and off the render thread; a failure
                // only means the adapter is gone, and the refresh below
                // reconciles that anyway.
                let _ = self.conn.call_method(
                    Some(BLUEZ_SERVICE),
                    adapter.as_str(),
                    Some(ADAPTER_IFACE),
                    "StopDiscovery",
                    &(),
                );
            }
            cache.discovery_deadline = None;
        }
        let scanning = cache.discovery_deadline.is_some() || cache.discovering;
        let due = cache
            .last_refresh
            .map(|last| last.elapsed() >= REFRESH_INTERVAL)
            .unwrap_or(true);
        drop(cache);
        if scanning || due {
            let _ = self.refresh();
        }
    }

    fn enabled(&self) -> Option<bool> {
        let cache = self.cache.lock().unwrap();
        // Stable once an adapter has been seen: `refresh` only rewrites the
        // cache on success, so a transient bus failure yields the last known
        // power state rather than a `None` that would make the runtime's
        // rfkill fallback and this value fight and oscillate.
        cache.adapter_path.as_ref().map(|_| cache.powered)
    }

    fn state(&self) -> BluetoothLinkState {
        let cache = self.cache.lock().unwrap();
        if cache.adapter_path.is_none() {
            return BluetoothLinkState::Unavailable;
        }
        if cache.devices.iter().any(|device| device.connected) {
            return BluetoothLinkState::Connected;
        }
        if cache.discovery_deadline.is_some() || cache.discovering {
            return BluetoothLinkState::Scanning;
        }
        if !cache.powered {
            return BluetoothLinkState::Disabled;
        }
        BluetoothLinkState::Idle
    }

    fn devices(&self) -> Vec<BluetoothDevice> {
        self.cache.lock().unwrap().devices.clone()
    }
}
