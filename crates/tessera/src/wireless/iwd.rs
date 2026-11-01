//! Native iwd (Intel Wireless Daemon) backend over the system D-Bus.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tessera_desktop::system::{WifiLinkState, WifiNetwork, WifiSecurity};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

use super::WirelessBackend;

const IWD_SERVICE: &str = "net.connman.iwd";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const DEVICE_IFACE: &str = "net.connman.iwd.Device";
const STATION_IFACE: &str = "net.connman.iwd.Station";
const NETWORK_IFACE: &str = "net.connman.iwd.Network";
const KNOWN_NETWORK_IFACE: &str = "net.connman.iwd.KnownNetwork";
const AGENT_MANAGER_IFACE: &str = "net.connman.iwd.AgentManager";
const AGENT_PATH: &str = "/io/github/ming2k/tessera/iwd_agent";

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

fn get_object_path(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props.get(key).and_then(|value| {
        let v: &Value = Borrow::<Value>::borrow(value);
        v.downcast_ref::<zbus::zvariant::ObjectPath>()
            .ok()
            .map(|p| p.as_str().to_string())
    })
}

/// D-Bus authentication agent for iwd passphrase callbacks.
struct IwdAgent {
    pending_passphrase: Arc<Mutex<Option<String>>>,
}

#[zbus::interface(name = "net.connman.iwd.Agent")]
impl IwdAgent {
    async fn request_passphrase(
        &self,
        _network: zbus::zvariant::ObjectPath<'_>,
    ) -> zbus::fdo::Result<String> {
        let mut guard = self
            .pending_passphrase
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(pass) = guard.take() {
            Ok(pass)
        } else {
            Err(zbus::fdo::Error::Failed(
                "no passphrase provided for wireless authentication".to_string(),
            ))
        }
    }

    async fn cancel(&self, reason: String) {
        log::debug!("iwd agent authentication cancelled: {reason}");
        let mut guard = self
            .pending_passphrase
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
}

pub struct IwdBackend {
    conn: Connection,
    cache: Arc<Mutex<IwdCache>>,
    pending_passphrase: Arc<Mutex<Option<String>>>,
}

#[derive(Default)]
struct IwdCache {
    station_path: Option<String>,
    device_path: Option<String>,
    state: WifiLinkState,
    active_ssid: Option<String>,
    networks: Vec<WifiNetwork>,
    known_networks: HashMap<String, (String, bool)>,
    /// Set by the signal thread on any iwd D-Bus signal and cleared by the
    /// worker after it consumes one refresh. The worker treats this as its
    /// coalescing gate: no flag, no `GetManagedObjects`, no snapshot churn.
    dirty: bool,
}

impl IwdBackend {
    /// Try connecting to the system bus and probing for the iwd daemon.
    pub fn open() -> Result<Self, String> {
        let conn = Connection::system().map_err(|e| format!("system bus unavailable: {e}"))?;
        let pending_passphrase = Arc::new(Mutex::new(None));

        // Serve native iwd Agent on the D-Bus connection
        let agent = IwdAgent {
            pending_passphrase: pending_passphrase.clone(),
        };
        let _ = conn.object_server().at(AGENT_PATH, agent);

        // Register agent with iwd AgentManager
        if let Ok(path) = zbus::zvariant::ObjectPath::try_from(AGENT_PATH) {
            let _ = conn.call_method(
                Some(IWD_SERVICE),
                "/net/connman/iwd",
                Some(AGENT_MANAGER_IFACE),
                "RegisterAgent",
                &(path,),
            );
        }

        let backend = Self {
            conn: conn.clone(),
            cache: Arc::new(Mutex::new(IwdCache::default())),
            pending_passphrase,
        };
        backend.refresh()?;

        // Subscribe to asynchronous iwd signals over D-Bus (PropertiesChanged,
        // InterfacesAdded, InterfacesRemoved) to maintain zero-polling live state.
        //
        // The signal thread is deliberately a *dirty-flag setter*, not a
        // refetcher: `MessageIterator` blocks on the next message, so doing a
        // `GetManagedObjects` here would serialize behind the daemon's signal
        // flood during scans/association and could stack up. The worker's
        // existing cadence is the natural coalescing window: it sees the flag,
        // runs one refresh, and clears it — a signal burst collapses to one
        // round-trip regardless of how many signals the daemon emits.
        let signal_conn = conn;
        let signal_cache = backend.cache.clone();
        std::thread::Builder::new()
            .name("tessera-iwd-signals".into())
            .spawn(move || {
                tessera_sched::mark_background();
                let iter = match zbus::blocking::MessageIterator::for_match_rule(
                    "type='signal',sender='net.connman.iwd'",
                    &signal_conn,
                    Some(64),
                ) {
                    Ok(iter) => iter,
                    Err(error) => {
                        log::warn!(
                            "wireless subsystem: could not subscribe to iwd signals: {error}"
                        );
                        return;
                    }
                };
                for _msg in iter.flatten() {
                    if let Ok(mut cache) = signal_cache.lock() {
                        cache.dirty = true;
                    }
                }
            })
            .ok();

        Ok(backend)
    }

    /// Refresh cached managed objects and state from iwd.
    pub fn refresh(&self) -> Result<(), String> {
        Self::refresh_cache(&self.conn, &self.cache)
    }

    fn refresh_cache(conn: &Connection, cache: &Arc<Mutex<IwdCache>>) -> Result<(), String> {
        let reply = conn
            .call_method(
                Some(IWD_SERVICE),
                "/",
                Some(OBJECT_MANAGER_IFACE),
                "GetManagedObjects",
                &(),
            )
            .map_err(|e| format!("iwd ObjectManager call failed: {e}"))?;

        type ManagedObjects =
            HashMap<zbus::zvariant::OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
        let objects: ManagedObjects = reply
            .body()
            .deserialize()
            .map_err(|e| format!("failed to parse iwd managed objects: {e}"))?;

        let mut station_path = None;
        let mut device_path = None;
        let mut station_state = WifiLinkState::Disconnected;
        let mut connected_network_path: Option<String> = None;
        let mut device_powered = true;
        let mut known_networks: HashMap<String, (String, bool)> = HashMap::new();

        struct RawNetwork {
            ssid: String,
            security: WifiSecurity,
            connected: bool,
            path: String,
        }
        let mut raw_networks = Vec::new();

        for (path, ifaces) in &objects {
            let path_str = path.as_str().to_string();
            if let Some(props) = ifaces.get(DEVICE_IFACE) {
                device_path = Some(path_str.clone());
                if let Some(powered) = get_bool(props, "Powered") {
                    device_powered = powered;
                }
            }

            if let Some(props) = ifaces.get(STATION_IFACE) {
                station_path = Some(path_str.clone());
                if let Some(s) = get_str(props, "State") {
                    station_state = match s {
                        "connected" => WifiLinkState::Connected,
                        "connecting" => WifiLinkState::Connecting,
                        "roaming" => WifiLinkState::Connecting,
                        _ => WifiLinkState::Disconnected,
                    };
                }
                if let Some(p) = get_object_path(props, "ConnectedNetwork") {
                    connected_network_path = Some(p);
                }
            }

            if let Some(props) = ifaces.get(NETWORK_IFACE) {
                let name = get_str(props, "Name").unwrap_or("").to_string();
                if !name.is_empty() {
                    let type_str = get_str(props, "Type").unwrap_or("psk");
                    let security = match type_str {
                        "open" => WifiSecurity::Open,
                        "8021x" => WifiSecurity::Enterprise,
                        _ => WifiSecurity::WpaPsk,
                    };
                    let connected = get_bool(props, "Connected").unwrap_or(false);

                    raw_networks.push(RawNetwork {
                        ssid: name,
                        security,
                        connected,
                        path: path_str.clone(),
                    });
                }
            }

            if let Some(props) = ifaces.get(KNOWN_NETWORK_IFACE) {
                let name = get_str(props, "Name").unwrap_or("").to_string();
                if !name.is_empty() {
                    let auto_connect = get_bool(props, "AutoConnect").unwrap_or(true);
                    known_networks.insert(name, (path_str.clone(), auto_connect));
                }
            }
        }

        if !device_powered {
            station_state = WifiLinkState::Disabled;
        }

        let mut active_ssid = None;
        let mut final_networks = Vec::new();

        for raw in raw_networks {
            let is_conn = raw.connected
                || connected_network_path
                    .as_deref()
                    .map(|p| p == raw.path)
                    .unwrap_or(false);
            if is_conn {
                active_ssid = Some(raw.ssid.clone());
            }
            let is_saved = known_networks.contains_key(&raw.ssid);
            let auto_connect = known_networks
                .get(&raw.ssid)
                .map(|(_, ac)| *ac)
                .unwrap_or(false);
            final_networks.push(WifiNetwork {
                ssid: raw.ssid,
                signal_bars: 4,
                security: raw.security,
                is_connected: is_conn,
                is_saved,
                auto_connect,
                frequency_mhz: None,
            });
        }

        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.station_path = station_path;
        cache.device_path = device_path;
        cache.state = station_state;
        cache.active_ssid = active_ssid;
        cache.networks = final_networks;
        cache.known_networks = known_networks;
        // `dirty` is deliberately left untouched here: the worker consumes it
        // in `poll` before calling this, and a signal that lands while the
        // reply is being deserialized must stay set for the next cadence,
        // otherwise its state change would be dropped and never published.

        Ok(())
    }
}

impl WirelessBackend for IwdBackend {
    fn refresh(&self) -> Result<(), String> {
        Self::refresh_cache(&self.conn, &self.cache)
    }

    fn poll(&self) -> bool {
        // Consume the flag atomically: read whether a signal arrived since
        // the last poll, then clear it. A signal that lands *during* the
        // refresh below leaves the flag set for the next cadence, so a state
        // change can never be dropped by the clear.
        let dirty = {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            let dirty = cache.dirty;
            cache.dirty = false;
            dirty
        };
        if dirty {
            let _ = self.refresh();
            true
        } else {
            false
        }
    }

    fn scan(&self) -> Result<(), String> {
        let station = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .station_path
            .clone()
            .ok_or_else(|| "no wireless station device found".to_string())?;

        self.conn
            .call_method(
                Some(IWD_SERVICE),
                station.as_str(),
                Some(STATION_IFACE),
                "Scan",
                &(),
            )
            .map_err(|e| format!("iwd Scan failed: {e}"))?;

        let _ = self.refresh();
        Ok(())
    }

    fn connect(&self, ssid: &str, passphrase: Option<&str>) -> Result<(), String> {
        self.refresh()?;
        let target_path = {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            let network = cache.networks.iter().find(|n| n.ssid == ssid);
            if network.is_none() {
                return Err(format!("network '{ssid}' not found"));
            }
            cache.station_path.clone()
        };

        let station = target_path.ok_or_else(|| "no wireless station device found".to_string())?;

        let reply = self
            .conn
            .call_method(
                Some(IWD_SERVICE),
                station.as_str(),
                Some(STATION_IFACE),
                "GetOrderedNetworks",
                &(),
            )
            .map_err(|e| format!("GetOrderedNetworks failed: {e}"))?;

        type OrderedNet = (zbus::zvariant::OwnedObjectPath, HashMap<String, OwnedValue>);
        let ordered: Vec<OrderedNet> = reply
            .body()
            .deserialize()
            .map_err(|e| format!("failed to deserialize ordered networks: {e}"))?;

        let mut net_path = None;
        for (path, _) in &ordered {
            if let Ok(reply) = self.conn.call_method(
                Some(IWD_SERVICE),
                path.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(NETWORK_IFACE, "Name"),
            ) && let Ok(val) = reply.body().deserialize::<zbus::zvariant::OwnedValue>()
                && let Ok(name) = <&str>::try_from(&val)
                && name == ssid
            {
                net_path = Some(path.as_str().to_string());
                break;
            }
        }

        let target_net =
            net_path.ok_or_else(|| format!("failed to find network path for '{ssid}'"))?;

        // Stage passphrase for iwd Agent callback if required
        if let Some(pass) = passphrase {
            let mut guard = self
                .pending_passphrase
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *guard = Some(pass.to_string());
        }

        self.conn
            .call_method(
                Some(IWD_SERVICE),
                target_net.as_str(),
                Some(NETWORK_IFACE),
                "Connect",
                &(),
            )
            .map_err(|e| format!("iwd Connect failed: {e}"))?;

        let _ = self.refresh();
        Ok(())
    }

    fn disconnect(&self) -> Result<(), String> {
        let station = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .station_path
            .clone()
            .ok_or_else(|| "no wireless station device found".to_string())?;

        self.conn
            .call_method(
                Some(IWD_SERVICE),
                station.as_str(),
                Some(STATION_IFACE),
                "Disconnect",
                &(),
            )
            .map_err(|e| format!("iwd Disconnect failed: {e}"))?;

        let _ = self.refresh();
        Ok(())
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let device = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .device_path
            .clone()
            .ok_or_else(|| "no wireless device found".to_string())?;

        self.conn
            .call_method(
                Some(IWD_SERVICE),
                device.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Set",
                &(
                    DEVICE_IFACE,
                    "Powered",
                    zbus::zvariant::Value::Bool(enabled),
                ),
            )
            .map_err(|e| format!("failed to set device powered state: {e}"))?;

        let _ = self.refresh();
        Ok(())
    }

    fn forget(&self, ssid: &str) -> Result<(), String> {
        let target_path = {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.known_networks.get(ssid).map(|(p, _)| p.clone())
        };

        if let Some(path) = target_path {
            self.conn
                .call_method(
                    Some(IWD_SERVICE),
                    path.as_str(),
                    Some(KNOWN_NETWORK_IFACE),
                    "Forget",
                    &(),
                )
                .map_err(|e| format!("iwd Forget failed: {e}"))?;
            let _ = self.refresh();
            Ok(())
        } else {
            Err(format!("known network '{ssid}' not found"))
        }
    }

    fn set_auto_connect(&self, ssid: &str, auto_connect: bool) -> Result<(), String> {
        let target_path = {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.known_networks.get(ssid).map(|(p, _)| p.clone())
        };

        if let Some(path) = target_path {
            self.conn
                .call_method(
                    Some(IWD_SERVICE),
                    path.as_str(),
                    Some("org.freedesktop.DBus.Properties"),
                    "Set",
                    &(KNOWN_NETWORK_IFACE, "AutoConnect", Value::new(auto_connect)),
                )
                .map_err(|e| format!("iwd Set AutoConnect failed: {e}"))?;
            let _ = self.refresh();
            Ok(())
        } else {
            Err(format!("known network '{ssid}' not found"))
        }
    }

    fn state(&self) -> WifiLinkState {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).state
    }

    fn networks(&self) -> Vec<WifiNetwork> {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .networks
            .clone()
    }

    fn active_ssid(&self) -> Option<String> {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_ssid
            .clone()
    }

    fn available(&self) -> bool {
        true
    }
}
