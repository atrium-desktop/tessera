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
        let mut guard = self.pending_passphrase.lock().unwrap();
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
        let mut guard = self.pending_passphrase.lock().unwrap();
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
            conn,
            cache: Arc::new(Mutex::new(IwdCache::default())),
            pending_passphrase,
        };
        backend.refresh()?;
        Ok(backend)
    }

    /// Refresh cached managed objects and state from iwd.
    pub fn refresh(&self) -> Result<(), String> {
        let reply = self
            .conn
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
                        path: path_str,
                    });
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
            final_networks.push(WifiNetwork {
                ssid: raw.ssid,
                signal_bars: 4,
                security: raw.security,
                is_connected: is_conn,
                is_saved: true,
            });
        }

        let mut cache = self.cache.lock().unwrap();
        cache.station_path = station_path;
        cache.device_path = device_path;
        cache.state = station_state;
        cache.active_ssid = active_ssid;
        cache.networks = final_networks;

        Ok(())
    }
}

impl WirelessBackend for IwdBackend {
    fn scan(&self) -> Result<(), String> {
        let station = self
            .cache
            .lock()
            .unwrap()
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
            let cache = self.cache.lock().unwrap();
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

        type OrderedNet = (
            zbus::zvariant::OwnedObjectPath,
            HashMap<String, OwnedValue>,
        );
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
            let mut guard = self.pending_passphrase.lock().unwrap();
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
            .unwrap()
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
            .unwrap()
            .device_path
            .clone()
            .ok_or_else(|| "no wireless device found".to_string())?;

        self.conn
            .call_method(
                Some(IWD_SERVICE),
                device.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Set",
                &(DEVICE_IFACE, "Powered", zbus::zvariant::Value::Bool(enabled)),
            )
            .map_err(|e| format!("failed to set device powered state: {e}"))?;

        let _ = self.refresh();
        Ok(())
    }

    fn state(&self) -> WifiLinkState {
        self.cache.lock().unwrap().state
    }

    fn networks(&self) -> Vec<WifiNetwork> {
        self.cache.lock().unwrap().networks.clone()
    }

    fn active_ssid(&self) -> Option<String> {
        self.cache.lock().unwrap().active_ssid.clone()
    }
}
