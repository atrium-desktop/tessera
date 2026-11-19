//! The Kestrel implementation of [`P2pRadio`].
//!
//! When `kestrel-service` (`com.hihusky.Kestrel`) owns the system radio, it
//! drives Wi-Fi Direct (`com.hihusky.Kestrel.P2P`) and session L3
//! (`com.hihusky.Kestrel.Session`), reporting both `GroupInterface` and
//! `PeerIp` directly once a group reaches `ready`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tessera_dbus::{Connection, FromArgs, FromValue, OwnedObjectPath, Value};

use super::networkmanager::{display_from_wfd_ies, source_wfd_ies};
use super::{DiscoveredPeer, DisplayCapabilities, P2pRadio};
use crate::iwd::{IwdError, blocking_call};

pub const KESTREL_SERVICE: &str = "com.hihusky.Kestrel";
const KESTREL_ROOT: &str = "/com/hihusky/Kestrel";
const MANAGER_IFACE: &str = "com.hihusky.Kestrel.Manager";
const DEVICE_IFACE: &str = "com.hihusky.Kestrel.Device";
const P2P_IFACE: &str = "com.hihusky.Kestrel.P2P";
const SESSION_IFACE: &str = "com.hihusky.Kestrel.Session";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";

const FIND_TIMEOUT_SECONDS: u32 = 30;
pub const DEFAULT_ACTIVATION_TIMEOUT: Duration = Duration::from_secs(45);

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, Value>>>;

/// Read-only probe: whether a running Kestrel instance exposes a P2P device.
///
/// Never D-Bus-activates `com.hihusky.Kestrel`.
/// `Ok(false)` covers "Kestrel is not running" and "no P2P-capable device":
/// both are conclusive absences.
pub fn observe_p2p_device() -> Result<bool, IwdError> {
    match KestrelRadio::open() {
        Ok(radio) => {
            radio.enable_p2p()?;
            Ok(true)
        }
        Err(IwdError::ServiceUnavailable | IwdError::Unsupported(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Kestrel P2P radio authority for casting.
pub struct KestrelRadio {
    connection: Connection,
    device: String,
    finding: Mutex<bool>,
    /// Active `com.hihusky.Kestrel.Session` object paths formed by this radio,
    /// keyed by the peer identifier passed to `form_group`.
    sessions: Mutex<HashMap<String, OwnedObjectPath>>,
    activation_timeout: Duration,
}

impl KestrelRadio {
    /// Open the first P2P-capable Kestrel device on an already running service.
    pub fn open() -> Result<Self, IwdError> {
        let connection = open_running_connection()?;
        let objects = managed_objects(&connection)?;
        let device = find_p2p_device(&objects).ok_or_else(|| {
            IwdError::Unsupported("Kestrel reported no P2P-capable wireless device".to_owned())
        })?;
        Ok(Self {
            connection,
            device,
            finding: Mutex::new(false),
            sessions: Mutex::new(HashMap::new()),
            activation_timeout: DEFAULT_ACTIVATION_TIMEOUT,
        })
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    fn close_session(&self, session: &OwnedObjectPath) -> Result<(), IwdError> {
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            session.as_str(),
            Some(SESSION_IFACE),
            "Close",
            &(),
        )
        .map(|_| ())
        .map_err(|error| classify(error, "Session.Close"))
    }
}

impl P2pRadio for KestrelRadio {
    fn authority(&self) -> &'static str {
        "Kestrel"
    }

    fn enable_p2p(&self) -> Result<(), IwdError> {
        let enabled: bool = get(
            &self.connection,
            KESTREL_ROOT,
            MANAGER_IFACE,
            "WirelessEnabled",
        )?;
        let hw_enabled: bool = get(
            &self.connection,
            KESTREL_ROOT,
            MANAGER_IFACE,
            "WirelessHardwareEnabled",
        )?;
        if !enabled || !hw_enabled {
            return Err(IwdError::Unsupported(
                "Kestrel wireless is disabled; enable Wi-Fi to cast".to_owned(),
            ));
        }
        Ok(())
    }

    fn register_source(&self, advertisement: DisplayCapabilities) -> Result<(), IwdError> {
        let ies = source_wfd_ies(&advertisement);
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            self.device.as_str(),
            Some(P2P_IFACE),
            "SetWfdSubelements",
            &(ies,),
        )
        .map(|_| ())
        .map_err(|error| classify(error, "P2P.SetWfdSubelements"))
    }

    fn unregister_source(&self) -> Result<(), IwdError> {
        let empty: Vec<u8> = Vec::new();
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            self.device.as_str(),
            Some(P2P_IFACE),
            "SetWfdSubelements",
            &(empty,),
        )
        .map(|_| ())
        .map_err(|error| classify(error, "P2P.SetWfdSubelements"))
    }

    fn request_discovery(&self) -> Result<(), IwdError> {
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            self.device.as_str(),
            Some(P2P_IFACE),
            "Find",
            &(FIND_TIMEOUT_SECONDS,),
        )
        .map(|_| ())
        .map_err(|error| classify(error, "P2P.Find"))?;
        *self.finding.lock().unwrap_or_else(|e| e.into_inner()) = true;
        Ok(())
    }

    fn peers(&self) -> Result<Vec<DiscoveredPeer>, IwdError> {
        let reply = blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            self.device.as_str(),
            Some(P2P_IFACE),
            "GetPeers",
            &(),
        )
        .map_err(|error| classify(error, "P2P.GetPeers"))?;
        let raw_peers: Vec<Value> = FromArgs::from_args(reply.body)
            .map_err(|error| IwdError::Protocol(format!("P2P.GetPeers: {error}")))?;

        let objects = managed_objects(&self.connection).unwrap_or_default();
        let active_sessions = ready_sessions_for_device(&objects, &self.device);

        let mut peers = Vec::with_capacity(raw_peers.len());
        for raw in raw_peers {
            let (address, name, _device_type, wfd_ies) = decode_peer_tuple(raw)?;
            let session = active_sessions.get(&address.to_ascii_lowercase());
            let connected = session.is_some();
            let connected_interface = session.and_then(|s| s.group_interface.clone());
            let connected_ip = session.and_then(|s| s.peer_ip.clone());
            peers.push(DiscoveredPeer {
                path: address.clone(),
                name: (!name.trim().is_empty()).then_some(name),
                device_address: Some(address),
                display: display_from_wfd_ies(&wfd_ies),
                connected,
                connected_interface,
                connected_ip,
            });
        }
        Ok(peers)
    }

    fn form_group(&self, peer_path: &str) -> Result<(), IwdError> {
        let reply = blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            self.device.as_str(),
            Some(P2P_IFACE),
            "Connect",
            &(peer_path, "pbc", "", u8::MAX, false),
        )
        .map_err(|error| classify(error, "P2P.Connect"))?;
        let session = OwnedObjectPath::from_args(reply.body)
            .map_err(|error| IwdError::Protocol(format!("P2P.Connect: {error}")))?;

        let deadline = Instant::now() + self.activation_timeout;
        loop {
            let state = get::<String>(&self.connection, session.as_str(), SESSION_IFACE, "State");
            match state.as_deref() {
                Ok("ready") => break,
                Ok("closed") | Err(_) => {
                    let detail = get::<String>(
                        &self.connection,
                        session.as_str(),
                        SESSION_IFACE,
                        "FailureDetail",
                    )
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "session closed".to_owned());
                    let _ = self.close_session(&session);
                    return Err(IwdError::Protocol(format!(
                        "Kestrel could not form the P2P group with {peer_path}: {detail}"
                    )));
                }
                Ok(_) if Instant::now() >= deadline => {
                    let _ = self.close_session(&session);
                    return Err(IwdError::Protocol(format!(
                        "Kestrel P2P group with {peer_path} did not become ready within {}s",
                        self.activation_timeout.as_secs()
                    )));
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(250)),
            }
        }
        let iface = get::<String>(
            &self.connection,
            session.as_str(),
            SESSION_IFACE,
            "GroupInterface",
        )
        .unwrap_or_default();
        log::info!("radio: Kestrel P2P group with {peer_path} is ready on {iface}");
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer_path.to_owned(), session);
        Ok(())
    }

    fn disconnect_peer(&self, peer_path: &str) -> Result<(), IwdError> {
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer_path)
            .cloned();
        let Some(session) = session else {
            return Ok(());
        };
        match self.close_session(&session) {
            Ok(()) => {}
            Err(IwdError::Unsupported(_)) => {}
            Err(error) => return Err(error),
        }
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer_path);
        Ok(())
    }
}

impl Drop for KestrelRadio {
    fn drop(&mut self) {
        let sessions: Vec<OwnedObjectPath> = {
            let mut guard = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            guard.drain().map(|(_, path)| path).collect()
        };
        for session in sessions {
            if let Err(error) = self.close_session(&session) {
                log::debug!("radio: Kestrel session already closed: {error}");
            }
        }
        let finding = *self.finding.lock().unwrap_or_else(|e| e.into_inner());
        if finding
            && let Err(error) = blocking_call(
                &self.connection,
                Some(KESTREL_SERVICE),
                self.device.as_str(),
                Some(P2P_IFACE),
                "StopFind",
                &(),
            )
        {
            log::debug!("radio: P2P.StopFind: {error}");
        }
    }
}

struct SessionLink {
    group_interface: Option<String>,
    peer_ip: Option<String>,
}

fn open_running_connection() -> Result<Connection, IwdError> {
    let connection =
        Connection::system().map_err(|error| IwdError::BusUnavailable(error.to_string()))?;
    let reply = blocking_call(
        &connection,
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &(KESTREL_SERVICE,),
    )
    .map_err(|error| IwdError::BusUnavailable(error.to_string()))?;
    let running: bool = FromArgs::from_args(reply.body)
        .map_err(|error| IwdError::Protocol(format!("NameHasOwner: {error}")))?;
    if !running {
        return Err(IwdError::Unsupported(
            "Kestrel is not running on the system bus".to_owned(),
        ));
    }
    Ok(connection)
}

fn managed_objects(connection: &Connection) -> Result<ManagedObjects, IwdError> {
    let reply = blocking_call(
        connection,
        Some(KESTREL_SERVICE),
        KESTREL_ROOT,
        Some(OBJECT_MANAGER_IFACE),
        "GetManagedObjects",
        &(),
    )
    .map_err(|error| classify(error, "GetManagedObjects"))?;
    FromArgs::from_args(reply.body)
        .map_err(|error| IwdError::Protocol(format!("GetManagedObjects: {error}")))
}

fn find_p2p_device(objects: &ManagedObjects) -> Option<String> {
    let mut candidates: Vec<&str> = objects
        .iter()
        .filter_map(|(path, ifaces)| {
            if ifaces.contains_key(P2P_IFACE) {
                return Some(path.as_str());
            }
            if let Some(device) = ifaces.get(DEVICE_IFACE)
                && has_capability(device, "p2p")
            {
                return Some(path.as_str());
            }
            None
        })
        .collect();
    candidates.sort_unstable();
    candidates.first().map(|s| (*s).to_owned())
}

fn has_capability(props: &HashMap<String, Value>, cap: &str) -> bool {
    let Some(Value::Array(_, items)) = props.get("Capabilities").map(Value::unwrap_variant) else {
        return false;
    };
    items.iter().any(|item| item.as_str() == Some(cap))
}

fn ready_sessions_for_device(
    objects: &ManagedObjects,
    device_path: &str,
) -> HashMap<String, SessionLink> {
    let mut map = HashMap::new();
    for interfaces in objects.values() {
        let Some(props) = interfaces.get(SESSION_IFACE) else {
            continue;
        };
        let Some(dev) = props.get("Device").and_then(Value::as_str) else {
            continue;
        };
        if dev != device_path {
            continue;
        }
        let state = props.get("State").and_then(Value::as_str).unwrap_or("");
        if state != "ready" {
            continue;
        }
        let peer = props.get("Peer").and_then(Value::as_str).unwrap_or("");
        if peer.is_empty() {
            continue;
        }
        let group_interface = props
            .get("GroupInterface")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let peer_ip = props
            .get("PeerIp")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        map.insert(
            peer.to_ascii_lowercase(),
            SessionLink {
                group_interface,
                peer_ip,
            },
        );
    }
    map
}

fn decode_peer_tuple(raw: Value) -> Result<(String, String, String, Vec<u8>), IwdError> {
    let Value::Struct(fields) = raw.unwrap_variant().clone() else {
        return Err(IwdError::Protocol(
            "P2P.GetPeers: expected tuple struct".to_owned(),
        ));
    };
    let [raw_address, raw_name, raw_device_type, raw_wfd]: [Value; 4] =
        fields.try_into().map_err(|fields: Vec<Value>| {
            IwdError::Protocol(format!(
                "P2P.GetPeers: expected 4 fields, got {}",
                fields.len()
            ))
        })?;
    let address = String::from_value(raw_address)
        .map_err(|e| IwdError::Protocol(format!("P2P.GetPeers address: {e}")))?;
    let name = String::from_value(raw_name)
        .map_err(|e| IwdError::Protocol(format!("P2P.GetPeers name: {e}")))?;
    let device_type = String::from_value(raw_device_type)
        .map_err(|e| IwdError::Protocol(format!("P2P.GetPeers device_type: {e}")))?;
    let wfd = Vec::<u8>::from_value(raw_wfd)
        .map_err(|e| IwdError::Protocol(format!("P2P.GetPeers wfd: {e}")))?;
    Ok((address, name, device_type, wfd))
}

fn get<T: FromValue>(
    connection: &Connection,
    path: &str,
    interface: &str,
    name: &str,
) -> Result<T, IwdError> {
    let reply = blocking_call(
        connection,
        Some(KESTREL_SERVICE),
        path,
        Some(PROPERTIES_IFACE),
        "Get",
        &(interface, name),
    )
    .map_err(|error| classify(error, &format!("{interface}.{name}")))?;
    let value: Value = FromArgs::from_args(reply.body)
        .map_err(|error| IwdError::Protocol(format!("{interface}.{name}: {error}")))?;
    T::from_value(value).map_err(|error| IwdError::Protocol(format!("{interface}.{name}: {error}")))
}

fn classify(error: tessera_dbus::Error, operation: &str) -> IwdError {
    if let tessera_dbus::Error::MethodError { name, detail } = &error {
        let text = detail.as_deref().unwrap_or(name.as_str());
        return match name.as_str() {
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "com.hihusky.Kestrel.Error.Stopped" => {
                IwdError::Unsupported("Kestrel is not running on the system bus".to_owned())
            }
            "org.freedesktop.DBus.Error.AccessDenied"
            | "com.hihusky.Kestrel.Error.AccessDenied" => IwdError::NotAuthorized(text.to_owned()),
            "com.hihusky.Kestrel.Error.NotSupported"
            | "com.hihusky.Kestrel.Error.NoSuchDevice"
            | "com.hihusky.Kestrel.Error.NoSuchSession" => IwdError::Unsupported(text.to_owned()),
            "com.hihusky.Kestrel.Error.InvalidArgument" => {
                IwdError::InvalidArguments(text.to_owned())
            }
            _ => IwdError::Protocol(format!("{operation}: {error}")),
        };
    }
    IwdError::Protocol(format!("{operation}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_p2p_capable_device_and_extracts_ready_session_peer_ip() {
        let mut objects: ManagedObjects = HashMap::new();
        let dev_path = OwnedObjectPath::try_from("/com/hihusky/Kestrel/Device/wlan0").unwrap();
        let mut dev_ifaces = HashMap::new();
        let mut dev_props = HashMap::new();
        dev_props.insert(
            "Capabilities".to_owned(),
            Value::Variant(Box::new(Value::Array(
                "s".into(),
                vec![Value::String("station".into()), Value::String("p2p".into())],
            ))),
        );
        dev_ifaces.insert(DEVICE_IFACE.to_owned(), dev_props);
        dev_ifaces.insert(P2P_IFACE.to_owned(), HashMap::new());
        objects.insert(dev_path, dev_ifaces);

        let sess_path = OwnedObjectPath::try_from("/com/hihusky/Kestrel/Session/1").unwrap();
        let mut sess_ifaces = HashMap::new();
        let mut sess_props = HashMap::new();
        sess_props.insert(
            "Device".to_owned(),
            Value::Variant(Box::new(Value::ObjectPath(
                OwnedObjectPath::try_from("/com/hihusky/Kestrel/Device/wlan0").unwrap(),
            ))),
        );
        sess_props.insert(
            "Peer".to_owned(),
            Value::Variant(Box::new(Value::String("AA:BB:CC:DD:EE:FF".into()))),
        );
        sess_props.insert(
            "State".to_owned(),
            Value::Variant(Box::new(Value::String("ready".into()))),
        );
        sess_props.insert(
            "GroupInterface".to_owned(),
            Value::Variant(Box::new(Value::String("p2p-wlan0-0".into()))),
        );
        sess_props.insert(
            "PeerIp".to_owned(),
            Value::Variant(Box::new(Value::String("192.168.49.10".into()))),
        );
        sess_ifaces.insert(SESSION_IFACE.to_owned(), sess_props);
        objects.insert(sess_path, sess_ifaces);

        let found = find_p2p_device(&objects).expect("device");
        assert_eq!(found, "/com/hihusky/Kestrel/Device/wlan0");

        let sessions = ready_sessions_for_device(&objects, &found);
        let link = sessions.get("aa:bb:cc:dd:ee:ff").expect("session");
        assert_eq!(link.group_interface.as_deref(), Some("p2p-wlan0-0"));
        assert_eq!(link.peer_ip.as_deref(), Some("192.168.49.10"));
    }

    #[test]
    fn permission_error_classifies_as_not_authorized() {
        let error = tessera_dbus::Error::MethodError {
            name: "com.hihusky.Kestrel.Error.AccessDenied".to_owned(),
            detail: Some("polkit refused".to_owned()),
        };
        assert!(matches!(
            classify(error, "P2P.Connect"),
            IwdError::NotAuthorized(msg) if msg.contains("polkit refused")
        ));
    }
}
