//! Native iwd (Intel Wireless Daemon) backend over the system D-Bus.
//!
//! This is the fallback when NetworkManager is not running, and the only backend
//! that existed before ADR-0242. It implements the same neutral
//! [`NetworkBackend`] contract as NetworkManager; consumers cannot tell which
//! service answered.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tessera_dbus::{
    Connection, FromArgs, Message, MessageStream, ObjectHandler, OwnedObjectPath, ToArgs, Value,
};
use tessera_desktop::system::{NetworkState, WifiLinkState, WifiNetwork, WifiSecurity};
use tessera_network::{BackendKind, NetworkBackend, NetworkError, NetworkSnapshot};
use zeroize::Zeroize;

const IWD_SERVICE: &str = "net.connman.iwd";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const DEVICE_IFACE: &str = "net.connman.iwd.Device";
const STATION_IFACE: &str = "net.connman.iwd.Station";
const NETWORK_IFACE: &str = "net.connman.iwd.Network";
const KNOWN_NETWORK_IFACE: &str = "net.connman.iwd.KnownNetwork";
const AGENT_MANAGER_IFACE: &str = "net.connman.iwd.AgentManager";
const AGENT_PATH: &str = "/io/github/ming2k/tessera/iwd_agent";

fn get_str<'a>(props: &'a HashMap<String, Value>, key: &str) -> Option<&'a str> {
    props.get(key).and_then(Value::as_str)
}

fn get_bool(props: &HashMap<String, Value>, key: &str) -> Option<bool> {
    props.get(key).and_then(Value::as_bool)
}

fn get_object_path(props: &HashMap<String, Value>, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|value| match value.unwrap_variant() {
            Value::ObjectPath(path) => Some(path.as_str().to_string()),
            _ => None,
        })
}

fn service_error(context: impl std::fmt::Display) -> NetworkError {
    NetworkError::Service(context.to_string())
}

fn blocking_call<B: ToArgs>(
    connection: &Connection,
    destination: Option<&str>,
    path: &str,
    interface: Option<&str>,
    method: &str,
    body: &B,
) -> tessera_dbus::Result<Message> {
    connection.call_method(destination, path, interface, method, body)
}

/// D-Bus authentication agent for iwd passphrase callbacks.
struct IwdAgent {
    pending_passphrase: Arc<Mutex<Option<String>>>,
}

impl ObjectHandler for IwdAgent {
    fn handle_method_call(
        &self,
        _conn: &Connection,
        msg: &Message,
    ) -> Result<Vec<Value>, (String, String)> {
        match (msg.interface.as_deref(), msg.member.as_deref()) {
            (Some("net.connman.iwd.Agent"), Some("RequestPassphrase")) => {
                let mut guard = self
                    .pending_passphrase
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if let Some(passphrase) = guard.take() {
                    Ok(vec![Value::String(passphrase)])
                } else {
                    Err((
                        "org.freedesktop.DBus.Error.Failed".to_string(),
                        "no passphrase provided for wireless authentication".to_string(),
                    ))
                }
            }
            (Some("net.connman.iwd.Agent"), Some("Cancel")) => {
                let reason = msg.body.first().and_then(Value::as_str).unwrap_or("");
                log::debug!("iwd agent authentication cancelled: {reason}");
                let mut guard = self
                    .pending_passphrase
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if let Some(mut passphrase) = guard.take() {
                    passphrase.zeroize();
                }
                Ok(Vec::new())
            }
            _ => Err((
                "org.freedesktop.DBus.Error.UnknownMethod".to_string(),
                format!(
                    "unknown method {:?} on {:?}",
                    msg.member.as_deref(),
                    msg.interface.as_deref()
                ),
            )),
        }
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
    interface: String,
    state: WifiLinkState,
    active_ssid: Option<String>,
    networks: Vec<WifiNetwork>,
    known_networks: HashMap<String, (String, bool)>,
    /// Whether the last refresh reached iwd. `false` makes the snapshot an
    /// honest unavailable reading instead of last-known state.
    available: bool,
    last_error: Option<String>,
    /// Set by the signal thread on any iwd D-Bus signal and cleared by the
    /// worker after it consumes one refresh. The worker treats this as its
    /// coalescing gate: no flag, no `GetManagedObjects`, no snapshot churn.
    dirty: bool,
}

impl IwdBackend {
    /// Discard a passphrase that was staged for a previous `Connect`.
    ///
    /// iwd may leave the agent uncalled when a connect fails, and a later
    /// connect to a different network must never receive that stale secret.
    fn clear_pending_passphrase(&self) {
        let mut guard = self
            .pending_passphrase
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(mut passphrase) = guard.take() {
            passphrase.zeroize();
        }
    }

    /// Try connecting to the system bus and probing for the iwd daemon.
    ///
    /// iwd must already own `net.connman.iwd`; the service never
    /// D-Bus-activates it. Registering the agent with a stopped iwd would
    /// start it, and on a NetworkManager host that is a second radio manager.
    pub fn open() -> Result<Self, NetworkError> {
        let conn = Connection::system().map_err(|_| NetworkError::Unavailable)?;
        let running: bool = FromArgs::from_args(
            blocking_call(
                &conn,
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &(IWD_SERVICE,),
            )
            .map_err(|error| NetworkError::Service(format!("iwd NameHasOwner failed: {error}")))?
            .body,
        )
        .map_err(|error| NetworkError::Service(format!("iwd NameHasOwner body: {error}")))?;
        if !running {
            return Err(NetworkError::Unavailable);
        }
        let pending_passphrase = Arc::new(Mutex::new(None));

        // Serve the native iwd Agent on the D-Bus connection.
        let agent = Arc::new(IwdAgent {
            pending_passphrase: pending_passphrase.clone(),
        });
        conn.register_object(AGENT_PATH, agent);

        // Register the agent with iwd's AgentManager.
        if let Ok(path) = OwnedObjectPath::try_from(AGENT_PATH) {
            let _ = blocking_call(
                &conn,
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

        // Subscribe to asynchronous iwd signals.
        let signal_conn = conn;
        let signal_cache = backend.cache.clone();
        std::thread::Builder::new()
            .name("tessera-iwd-signals".into())
            .spawn(move || {
                let stream = match MessageStream::for_match_rule(
                    &signal_conn,
                    "type='signal',sender='net.connman.iwd'",
                ) {
                    Ok(stream) => stream,
                    Err(error) => {
                        log::warn!("network service: could not subscribe to iwd signals: {error}");
                        return;
                    }
                };
                while stream.next().is_some() {
                    if let Ok(mut cache) = signal_cache.lock() {
                        cache.dirty = true;
                    }
                }
            })
            .ok();

        Ok(backend)
    }

    fn refresh_cache(conn: &Connection, cache: &Arc<Mutex<IwdCache>>) -> Result<(), NetworkError> {
        let reply = blocking_call(
            conn,
            Some(IWD_SERVICE),
            "/",
            Some(OBJECT_MANAGER_IFACE),
            "GetManagedObjects",
            &(),
        )
        .map_err(|error| match &error {
            tessera_dbus::Error::MethodError { name, .. }
                if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
            {
                // iwd is not on the bus: the backend is unreachable, not
                // merely stale.
                NetworkError::Unavailable
            }
            _ => service_error(format!("iwd ObjectManager call failed: {error}")),
        })?;

        type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, Value>>>;
        let objects: ManagedObjects = FromArgs::from_args(reply.body).map_err(|error| {
            service_error(format!("failed to parse iwd managed objects: {error}"))
        })?;

        let mut station_path = None;
        let mut device_path = None;
        let mut interface = String::new();
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

        for (path, interfaces) in &objects {
            let path = path.as_str().to_string();
            if let Some(properties) = interfaces.get(DEVICE_IFACE) {
                device_path = Some(path.clone());
                if let Some(name) = get_str(properties, "Name") {
                    interface = name.to_string();
                }
                if let Some(powered) = get_bool(properties, "Powered") {
                    device_powered = powered;
                }
            }

            if let Some(properties) = interfaces.get(STATION_IFACE) {
                station_path = Some(path.clone());
                if let Some(state) = get_str(properties, "State") {
                    station_state = match state {
                        "connected" => WifiLinkState::Connected,
                        "connecting" | "roaming" => WifiLinkState::Connecting,
                        _ => WifiLinkState::Disconnected,
                    };
                }
                if let Some(connected) = get_object_path(properties, "ConnectedNetwork") {
                    connected_network_path = Some(connected);
                }
            }

            if let Some(properties) = interfaces.get(NETWORK_IFACE) {
                let name = get_str(properties, "Name").unwrap_or("").to_string();
                if !name.is_empty() {
                    let type_name = get_str(properties, "Type").unwrap_or("psk");
                    let security = match type_name {
                        "open" => WifiSecurity::Open,
                        "8021x" => WifiSecurity::Enterprise,
                        _ => WifiSecurity::WpaPsk,
                    };
                    raw_networks.push(RawNetwork {
                        ssid: name,
                        security,
                        connected: get_bool(properties, "Connected").unwrap_or(false),
                        path: path.clone(),
                    });
                }
            }

            if let Some(properties) = interfaces.get(KNOWN_NETWORK_IFACE) {
                let name = get_str(properties, "Name").unwrap_or("").to_string();
                if !name.is_empty() {
                    let auto_connect = get_bool(properties, "AutoConnect").unwrap_or(true);
                    known_networks.insert(name, (path.clone(), auto_connect));
                }
            }
        }

        if station_path.is_none() && device_path.is_none() {
            return Err(NetworkError::Unsupported(
                "iwd reported no wireless device".to_string(),
            ));
        }

        if !device_powered {
            station_state = WifiLinkState::Disabled;
        }

        let mut active_ssid = None;
        let mut networks = Vec::new();
        for raw in raw_networks {
            let connected = raw.connected
                || connected_network_path
                    .as_deref()
                    .is_some_and(|path| path == raw.path);
            if connected {
                active_ssid = Some(raw.ssid.clone());
            }
            let (is_saved, auto_connect) = known_networks
                .get(&raw.ssid)
                .map(|(_, auto_connect)| (true, *auto_connect))
                .unwrap_or((false, false));
            networks.push(WifiNetwork {
                ssid: raw.ssid,
                // iwd's D-Bus surface does not report RSSI; four bars is the
                // only honest value available, not a measured strength.
                signal_bars: 4,
                security: raw.security,
                is_connected: connected,
                is_saved,
                auto_connect,
                frequency_mhz: None,
            });
        }

        let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
        cache.station_path = station_path;
        cache.device_path = device_path;
        cache.interface = interface;
        cache.state = station_state;
        cache.active_ssid = active_ssid;
        cache.networks = networks;
        cache.known_networks = known_networks;
        // `dirty` is deliberately left untouched: a signal that lands while
        // the reply is deserialized must stay set for the next cadence.
        Ok(())
    }
}

impl NetworkBackend for IwdBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Iwd
    }

    fn snapshot(&self) -> NetworkSnapshot {
        let cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        if !cache.available {
            return NetworkSnapshot::unavailable(
                BackendKind::Iwd,
                cache
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "iwd did not answer".to_string()),
            );
        }
        let connected = cache.state == WifiLinkState::Connected;
        NetworkSnapshot {
            available: true,
            backend: BackendKind::Iwd,
            network: if connected {
                NetworkState::Wifi
            } else {
                NetworkState::Offline
            },
            // iwd owns the station role only; the compositor's sysfs probe is
            // still the coarse wired/offline authority on an iwd-only host.
            network_authoritative: false,
            network_interface: cache.interface.clone(),
            wifi_enabled: Some(cache.state != WifiLinkState::Disabled),
            wifi_state: cache.state,
            wifi_ssid: cache.active_ssid.clone(),
            wifi_networks: cache.networks.clone(),
            last_error: None,
        }
    }

    fn refresh(&self) -> Result<(), NetworkError> {
        match Self::refresh_cache(&self.conn, &self.cache) {
            Ok(()) => {
                let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
                cache.available = true;
                cache.last_error = None;
                Ok(())
            }
            Err(error) => {
                let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
                cache.available = false;
                cache.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn poll(&self) -> bool {
        // Consume the flag atomically: read whether a signal arrived since
        // the last poll, then clear it. A signal during the refresh below
        // leaves the flag set for the next cadence.
        let dirty = {
            let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
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

    fn scan(&self) -> Result<(), NetworkError> {
        let station = self
            .cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .station_path
            .clone()
            .ok_or_else(|| NetworkError::Unsupported("no wireless station device found".into()))?;

        blocking_call(
            &self.conn,
            Some(IWD_SERVICE),
            station.as_str(),
            Some(STATION_IFACE),
            "Scan",
            &(),
        )
        .map_err(|error| service_error(format!("iwd Scan failed: {error}")))?;

        let _ = self.refresh();
        Ok(())
    }

    fn connect(&self, ssid: &str, passphrase: Option<&str>) -> Result<(), NetworkError> {
        self.refresh()?;
        // Clear first: `None` must clear a secret left by a failed attempt to
        // another network, not reuse it.
        self.clear_pending_passphrase();
        let target_path = {
            let cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
            cache
                .networks
                .iter()
                .find(|network| network.ssid == ssid)
                .ok_or_else(|| NetworkError::NetworkNotFound(ssid.to_string()))?;
            cache.station_path.clone().ok_or_else(|| {
                NetworkError::Unsupported("no wireless station device found".into())
            })?
        };

        let reply = blocking_call(
            &self.conn,
            Some(IWD_SERVICE),
            target_path.as_str(),
            Some(STATION_IFACE),
            "GetOrderedNetworks",
            &(),
        )
        .map_err(|error| service_error(format!("GetOrderedNetworks failed: {error}")))?;

        type OrderedNetwork = (OwnedObjectPath, HashMap<String, Value>);
        let ordered: Vec<OrderedNetwork> = FromArgs::from_args(reply.body).map_err(|error| {
            service_error(format!("failed to deserialize ordered networks: {error}"))
        })?;

        let mut network_path = None;
        for (path, _) in &ordered {
            if let Ok(reply) = blocking_call(
                &self.conn,
                Some(IWD_SERVICE),
                path.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(NETWORK_IFACE, "Name"),
            ) && let Ok(value) = Value::from_args(reply.body)
                && let Some(name) = value.as_str()
                && name == ssid
            {
                network_path = Some(path.as_str().to_string());
                break;
            }
        }

        let network_path =
            network_path.ok_or_else(|| NetworkError::NetworkNotFound(ssid.to_string()))?;

        if let Some(passphrase) = passphrase {
            let mut guard = self
                .pending_passphrase
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *guard = Some(passphrase.to_string());
        }

        let result = blocking_call(
            &self.conn,
            Some(IWD_SERVICE),
            network_path.as_str(),
            Some(NETWORK_IFACE),
            "Connect",
            &(),
        );
        if result.is_err() {
            // The agent may never have been asked; do not keep the secret.
            self.clear_pending_passphrase();
        }
        result.map_err(|error| service_error(format!("iwd Connect failed: {error}")))?;

        let _ = self.refresh();
        Ok(())
    }

    fn disconnect(&self) -> Result<(), NetworkError> {
        let station = self
            .cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .station_path
            .clone()
            .ok_or_else(|| NetworkError::Unsupported("no wireless station device found".into()))?;

        blocking_call(
            &self.conn,
            Some(IWD_SERVICE),
            station.as_str(),
            Some(STATION_IFACE),
            "Disconnect",
            &(),
        )
        .map_err(|error| service_error(format!("iwd Disconnect failed: {error}")))?;

        let _ = self.refresh();
        Ok(())
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
        let device = self
            .cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .device_path
            .clone()
            .ok_or_else(|| NetworkError::Unsupported("no wireless device found".into()))?;

        blocking_call(
            &self.conn,
            Some(IWD_SERVICE),
            device.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &(
                DEVICE_IFACE,
                "Powered",
                Value::Variant(Box::new(Value::Bool(enabled))),
            ),
        )
        .map_err(|error| service_error(format!("failed to set device powered state: {error}")))?;

        let _ = self.refresh();
        Ok(())
    }

    fn forget(&self, ssid: &str) -> Result<(), NetworkError> {
        let target = self
            .cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .known_networks
            .get(ssid)
            .map(|(path, _)| path.clone())
            .ok_or_else(|| NetworkError::NetworkNotFound(ssid.to_string()))?;

        blocking_call(
            &self.conn,
            Some(IWD_SERVICE),
            target.as_str(),
            Some(KNOWN_NETWORK_IFACE),
            "Forget",
            &(),
        )
        .map_err(|error| service_error(format!("iwd Forget failed: {error}")))?;
        let _ = self.refresh();
        Ok(())
    }

    fn set_auto_connect(&self, ssid: &str, auto_connect: bool) -> Result<(), NetworkError> {
        let target = self
            .cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .known_networks
            .get(ssid)
            .map(|(path, _)| path.clone())
            .ok_or_else(|| NetworkError::NetworkNotFound(ssid.to_string()))?;

        blocking_call(
            &self.conn,
            Some(IWD_SERVICE),
            target.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &(
                KNOWN_NETWORK_IFACE,
                "AutoConnect",
                Value::Variant(Box::new(Value::Bool(auto_connect))),
            ),
        )
        .map_err(|error| service_error(format!("iwd Set AutoConnect failed: {error}")))?;
        let _ = self.refresh();
        Ok(())
    }
}
