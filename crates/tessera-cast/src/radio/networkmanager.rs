//! The NetworkManager implementation of [`P2pRadio`].
//!
//! Hosts that run NetworkManager with wpa_supplicant have no iwd, so the iwd
//! radio reports "no P2P authority" even though the adapter can do Wi-Fi
//! Direct. NetworkManager exposes that role as a separate `wifi-p2p` device
//! (`p2p-dev-wlan0`), and this module drives it:
//!
//! | Port operation | NetworkManager call |
//! |----------------|---------------------|
//! | `enable_p2p` | find the `wifi-p2p` device and check it is not unavailable |
//! | `register_source` | remember our WFD IEs; NetworkManager pushes them to wpa_supplicant at activation |
//! | `request_discovery` | `Device.WifiP2P.StartFind({timeout})` |
//! | `peers` | `Device.WifiP2P.Peers` → `WifiP2PPeer.{Name,HwAddress,WfdIEs}` |
//! | `form_group` | `AddAndActivateConnection2` with a **volatile** `wifi-p2p` profile, then wait for `Activated` |
//! | `disconnect_peer` | `DeactivateConnection` (the volatile profile is deleted with it) |
//!
//! The station link is never touched: NetworkManager owns it, and the P2P role
//! is a separate device with its own state (ADR-0191, ADR-0242).
//!
//! Peer WFD IEs arrive as a **bare subelement list** (no `0xDD`/OUI header),
//! decoded with [`WfdIe::decode_subelements`]. The mapping from NetworkManager's
//! shapes to the port's vocabulary is pure and unit-tested below; only the bus
//! calls are in the [`NmRadio`] shell.
//!
//! NetworkManager is only used when it already owns its bus name. A method call
//! alone can D-Bus-activate it, so every probe asks `NameHasOwner` first; a
//! stopped NetworkManager is reported absent rather than started from a
//! read-only command (the same rule the iwd probe follows).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tessera_dbus::{Connection, FromArgs, FromValue, OwnedObjectPath, Value};

use super::{DiscoveredPeer, DisplayCapabilities, P2pRadio};
use crate::iwd::{IwdError, blocking_call};
use crate::wfd::ie::{DeviceInformation, DeviceType, WfdIe};

pub const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
pub const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_MANAGER: &str = "org.freedesktop.NetworkManager";
const NM_DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const NM_WIFI_P2P: &str = "org.freedesktop.NetworkManager.Device.WifiP2P";
const NM_P2P_PEER: &str = "org.freedesktop.NetworkManager.WifiP2PPeer";
const NM_ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// `NM_DEVICE_TYPE_WIFI_P2P`.
const DEVICE_TYPE_WIFI_P2P: u32 = 30;
/// `NM_DEVICE_STATE_UNMANAGED` / `_UNAVAILABLE`: below this the device cannot act.
const DEVICE_STATE_DISCONNECTED: u32 = 30;

/// How long NetworkManager may keep discovery running after one request.
const FIND_TIMEOUT_SECONDS: i32 = 30;
/// How long group formation (GO negotiation + WPS + DHCP) may take. The
/// reference TV forms its group in about six seconds; DHCP can add a few more.
pub const DEFAULT_ACTIVATION_TIMEOUT: Duration = Duration::from_secs(45);

/// The maximum throughput this source advertises, in Mbit/s.
const ADVERTISED_THROUGHPUT_MBPS: u16 = 50;

/// `NMActiveConnectionState`, reduced to what group formation decides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationState {
    Pending,
    Activated,
    Failed,
}

impl ActivationState {
    /// Interpret `Connection.Active.State`; a vanished object is a failure.
    pub fn from_nm(state: Option<u32>) -> Self {
        match state {
            Some(0 | 1) => Self::Pending,
            Some(2) => Self::Activated,
            _ => Self::Failed,
        }
    }
}

/// Decode a peer's `WfdIEs` into the port's capability vocabulary.
///
/// An empty list (a printer, most phones in idle) is `None`: no Display
/// interface at all, which the port keeps distinct from "display but no sink".
/// A list that does not decode is also `None`, with the reason logged, because
/// a peer whose advertisement cannot be read must not be offered as castable.
pub fn display_from_wfd_ies(bytes: &[u8]) -> Option<DisplayCapabilities> {
    if bytes.is_empty() {
        return None;
    }
    let information = match WfdIe::decode_subelements(bytes) {
        Ok(ie) => ie.device_information()?,
        Err(error) => {
            log::warn!("radio: unreadable peer WFD IEs {bytes:02x?}: {error}");
            return None;
        }
    };
    let role = information.device_type();
    Some(DisplayCapabilities {
        source: role.is_source(),
        sink: role.is_sink(),
        port: information.control_port().filter(|port| *port != 0),
        // Audio/UIBC/content protection live in other subelements (extended
        // capability, content protection) that sinks rarely put in a probe
        // response; the session's M3/M4 exchange is where they are negotiated.
        has_audio: false,
        has_uibc: false,
        has_content_protection: false,
    })
}

/// The WFD IEs this source advertises during group formation: Device
/// Information only, role **source**, session available, our control port.
pub fn source_wfd_ies(advertisement: &DisplayCapabilities) -> Vec<u8> {
    let role = if advertisement.sink {
        DeviceType::DualRole
    } else {
        DeviceType::Source
    };
    let information = DeviceInformation::from_role(role, true).with_endpoint(
        advertisement
            .port
            .unwrap_or(crate::firewall::WFD_CONTROL_PORT),
        ADVERTISED_THROUGHPUT_MBPS,
    );
    WfdIe {
        subelements: vec![information.subelement()],
    }
    .encode_subelements()
}

/// The connection profile for one P2P group.
///
/// `ipv4.method=auto`: the sink is group owner and runs DHCP (the reference TV
/// hands out `192.168.49.x`). IPv6 is disabled because WFD sessions are IPv4
/// and router-solicitation timeouts only delay activation. The profile is
/// **not** autoconnecting and is added volatile by the caller, so a cast never
/// leaves a saved connection behind.
///
/// `owner` makes the profile **user-owned** (`connection.permissions =
/// ["user:<owner>"]`). Adding a system-wide profile needs
/// `settings.modify.system`, which a desktop user normally holds only after
/// an admin prompt; a user-owned one needs `settings.modify.own`, which an
/// active session holds. Measured on the NetworkManager host: without an
/// owner, `AddAndActivateConnection2` fails with
/// `Settings.PermissionDenied: Insufficient privileges`.
fn var(v: Value) -> Value {
    Value::Variant(Box::new(v))
}

pub fn group_settings(
    peer_address: &str,
    wfd_ies: &[u8],
    owner: Option<&str>,
) -> HashMap<&'static str, HashMap<&'static str, Value>> {
    let mut connection = HashMap::new();
    connection.insert("type", var(Value::String("wifi-p2p".into())));
    connection.insert("id", var(Value::String("Tessera cast".into())));
    connection.insert("autoconnect", var(Value::Bool(false)));
    if let Some(owner) = owner {
        connection.insert(
            "permissions",
            var(Value::Array(
                "s".into(),
                vec![Value::String(format!("user:{owner}"))],
            )),
        );
    }

    let mut p2p = HashMap::new();
    p2p.insert("peer", var(Value::String(peer_address.to_string())));
    p2p.insert(
        "wfd-ies",
        var(Value::Array(
            "y".into(),
            wfd_ies.iter().copied().map(Value::Byte).collect(),
        )),
    );

    let mut ipv4 = HashMap::new();
    ipv4.insert("method", var(Value::String("auto".into())));
    let mut ipv6 = HashMap::new();
    ipv6.insert("method", var(Value::String("disabled".into())));

    let mut settings = HashMap::new();
    settings.insert("connection", connection);
    settings.insert("wifi-p2p", p2p);
    settings.insert("ipv4", ipv4);
    settings.insert("ipv6", ipv6);
    settings
}

/// The login name of this process's real uid.
///
/// `USER`/`LOGNAME` are not reliable (a service or agent environment may have
/// neither), so the uid from `/proc/self/status` is resolved through
/// `/etc/passwd` when they are empty.
fn current_user_name() -> Option<String> {
    for key in ["USER", "LOGNAME"] {
        if let Ok(name) = std::env::var(key)
            && !name.is_empty()
        {
            return Some(name);
        }
    }
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    user_name_for(&status, &passwd)
}

/// Resolve the real uid in a `/proc/<pid>/status` text against a passwd file.
fn user_name_for(status: &str, passwd: &str) -> Option<String> {
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?;
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _password = fields.next()?;
        (fields.next()? == uid).then(|| name.to_owned())
    })
}

/// Map a NetworkManager D-Bus failure into the port's error vocabulary,
/// naming NetworkManager rather than iwd.
fn classify(error: tessera_dbus::Error, what: &str) -> IwdError {
    if let Some((name, detail)) = error_name(&error) {
        return match name.as_str() {
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => {
                IwdError::Unsupported("NetworkManager is not running".to_owned())
            }
            name if name == "org.freedesktop.DBus.Error.AccessDenied"
                || name.ends_with(".PermissionDenied") =>
            {
                IwdError::NotAuthorized(format!("NetworkManager {what}: {detail}"))
            }
            "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownProperty" => {
                IwdError::Unsupported(format!("{what}: {detail}"))
            }
            "org.freedesktop.DBus.Error.InvalidArgs"
            | "org.freedesktop.NetworkManager.Settings.Connection.InvalidProperty"
            | "org.freedesktop.NetworkManager.Settings.Connection.MissingProperty" => {
                IwdError::InvalidArguments(format!("{what}: {detail}"))
            }
            _ => IwdError::Protocol(format!("NetworkManager {what}: {name}: {detail}")),
        };
    }
    IwdError::Protocol(format!("NetworkManager {what}: {error}"))
}

fn error_name(error: &tessera_dbus::Error) -> Option<(String, String)> {
    match error {
        tessera_dbus::Error::MethodError { name, detail } => {
            Some((name.clone(), detail.clone().unwrap_or_default()))
        }
        _ => None,
    }
}

/// Read-only: whether NetworkManager already owns its bus name.
///
/// A method call to `org.freedesktop.NetworkManager` is enough to D-Bus
/// activate it. On an iwd-only host where NetworkManager is installed but
/// intentionally stopped, that would start a second radio manager from a
/// read-only probe. `NameHasOwner` never activates anything.
fn name_has_owner(connection: &Connection, name: &str) -> Result<bool, IwdError> {
    let reply = blocking_call(
        connection,
        Some(DBUS_SERVICE),
        DBUS_PATH,
        Some(DBUS_SERVICE),
        "NameHasOwner",
        &(name,),
    )
    .map_err(|error| classify(error, "NameHasOwner"))?;
    FromArgs::from_args(reply.body)
        .map_err(|error| IwdError::Protocol(format!("NameHasOwner: {error}")))
}

/// Read-only: whether NetworkManager has a `wifi-p2p` device that is ready.
///
/// `Ok(false)` covers "NetworkManager is not running" and "no such device":
/// both are conclusive absences. Only a bus failure is an error. The probe
/// never starts NetworkManager.
pub fn observe_p2p_device() -> Result<bool, IwdError> {
    match NmRadio::open() {
        Ok(radio) => Ok(radio.device_state()? >= DEVICE_STATE_DISCONNECTED),
        Err(IwdError::Unsupported(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// A caster's radio, backed by NetworkManager's `wifi-p2p` device.
pub struct NmRadio {
    connection: Connection,
    device: OwnedObjectPath,
    /// Our WFD IEs, set by `register_source` and sent at activation.
    wfd_ies: Mutex<Option<Vec<u8>>>,
    /// Active connections this radio created, by peer path.
    groups: Mutex<HashMap<String, OwnedObjectPath>>,
    finding: Mutex<bool>,
    activation_timeout: Duration,
}

impl NmRadio {
    /// Open the first `wifi-p2p` device NetworkManager manages.
    ///
    /// NetworkManager must already be running: the caster never activates it,
    /// for the same reason the probe never activates iwd.
    pub fn open() -> Result<Self, IwdError> {
        let connection =
            Connection::system().map_err(|error| IwdError::BusUnavailable(error.to_string()))?;
        if !name_has_owner(&connection, NM_SERVICE)? {
            return Err(IwdError::Unsupported(
                "NetworkManager is not running".to_owned(),
            ));
        }
        let reply = blocking_call(
            &connection,
            Some(NM_SERVICE),
            NM_PATH,
            Some(NM_MANAGER),
            "GetDevices",
            &(),
        )
        .map_err(|error| classify(error, "GetDevices"))?;
        let devices: Vec<OwnedObjectPath> = FromArgs::from_args(reply.body)
            .map_err(|error| IwdError::Protocol(format!("GetDevices: {error}")))?;
        for device in devices {
            let kind: Option<u32> = get(&connection, device.as_str(), NM_DEVICE, "DeviceType").ok();
            if kind == Some(DEVICE_TYPE_WIFI_P2P) {
                return Ok(Self {
                    connection,
                    device,
                    wfd_ies: Mutex::new(None),
                    groups: Mutex::new(HashMap::new()),
                    finding: Mutex::new(false),
                    activation_timeout: DEFAULT_ACTIVATION_TIMEOUT,
                });
            }
        }
        Err(IwdError::Unsupported(
            "NetworkManager manages no wifi-p2p device".to_owned(),
        ))
    }

    /// The `wifi-p2p` device's object path.
    pub fn device(&self) -> &str {
        self.device.as_str()
    }

    fn device_state(&self) -> Result<u32, IwdError> {
        get(&self.connection, self.device.as_str(), NM_DEVICE, "State")
    }

    /// The device's active connection and its group interface, when the
    /// active connection belongs to `peer_path`.
    fn connected_group(&self, peer_path: &str) -> Option<String> {
        let active: OwnedObjectPath = get(
            &self.connection,
            self.device.as_str(),
            NM_DEVICE,
            "ActiveConnection",
        )
        .ok()?;
        if active.as_str() == "/" {
            return None;
        }
        let specific: OwnedObjectPath = get(
            &self.connection,
            active.as_str(),
            NM_ACTIVE,
            "SpecificObject",
        )
        .ok()?;
        let state: Option<u32> = get(&self.connection, active.as_str(), NM_ACTIVE, "State").ok();
        if specific.as_str() != peer_path
            || ActivationState::from_nm(state) != ActivationState::Activated
        {
            return None;
        }
        get::<String>(
            &self.connection,
            self.device.as_str(),
            NM_DEVICE,
            "IpInterface",
        )
        .ok()
        .filter(|name| !name.is_empty())
    }

    fn deactivate(&self, active: &OwnedObjectPath) -> Result<(), IwdError> {
        let path = OwnedObjectPath::try_from(active.as_str())
            .map_err(|error| IwdError::Protocol(error.to_string()))?;
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            NM_PATH,
            Some(NM_MANAGER),
            "DeactivateConnection",
            &(path,),
        )
        .map(|_| ())
        .map_err(|error| classify(error, "DeactivateConnection"))
    }
}

/// Read one property and convert it.
fn get<T: FromValue>(
    connection: &Connection,
    path: &str,
    interface: &str,
    name: &str,
) -> Result<T, IwdError> {
    let reply = blocking_call(
        connection,
        Some(NM_SERVICE),
        path,
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &(interface, name),
    )
    .map_err(|error| classify(error, &format!("{interface}.{name}")))?;
    let value: Value = FromArgs::from_args(reply.body)
        .map_err(|error| IwdError::Protocol(format!("{interface}.{name}: {error}")))?;
    T::from_value(value).map_err(|error| IwdError::Protocol(format!("{interface}.{name}: {error}")))
}

impl P2pRadio for NmRadio {
    fn authority(&self) -> &'static str {
        "NetworkManager"
    }

    fn enable_p2p(&self) -> Result<(), IwdError> {
        // NetworkManager owns the P2P device's power together with Wi-Fi; a
        // caster has no business flipping the radio. Below `disconnected` the
        // device is unmanaged or unavailable (Wi-Fi off, rfkill).
        let state = self.device_state()?;
        if state < DEVICE_STATE_DISCONNECTED {
            return Err(IwdError::Unsupported(format!(
                "NetworkManager's wifi-p2p device is not ready (state {state}); is Wi-Fi enabled?"
            )));
        }
        Ok(())
    }

    fn register_source(&self, advertisement: DisplayCapabilities) -> Result<(), IwdError> {
        *self.wfd_ies.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(source_wfd_ies(&advertisement));
        Ok(())
    }

    fn unregister_source(&self) -> Result<(), IwdError> {
        *self.wfd_ies.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }

    fn request_discovery(&self) -> Result<(), IwdError> {
        let mut options: HashMap<&str, Value> = HashMap::new();
        options.insert("timeout", var(Value::Int32(FIND_TIMEOUT_SECONDS)));
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            self.device.as_str(),
            Some(NM_WIFI_P2P),
            "StartFind",
            &(options,),
        )
        .map_err(|error| classify(error, "StartFind"))?;
        *self.finding.lock().unwrap_or_else(|e| e.into_inner()) = true;
        Ok(())
    }

    fn peers(&self) -> Result<Vec<DiscoveredPeer>, IwdError> {
        let paths: Vec<OwnedObjectPath> =
            get(&self.connection, self.device.as_str(), NM_WIFI_P2P, "Peers")?;
        let mut peers = Vec::with_capacity(paths.len());
        for path in paths {
            let path = path.as_str();
            // A peer can vanish between `Peers` and its own reads; skip it
            // rather than fail the enumeration.
            let Ok(address) = get::<String>(&self.connection, path, NM_P2P_PEER, "HwAddress")
            else {
                continue;
            };
            let name = get::<String>(&self.connection, path, NM_P2P_PEER, "Name")
                .ok()
                .filter(|name| !name.trim().is_empty());
            let wfd_ies: Vec<u8> =
                get(&self.connection, path, NM_P2P_PEER, "WfdIEs").unwrap_or_default();
            let interface = self.connected_group(path);
            peers.push(DiscoveredPeer {
                path: path.to_owned(),
                name,
                device_address: Some(address),
                display: display_from_wfd_ies(&wfd_ies),
                connected: interface.is_some(),
                connected_interface: interface,
                // NetworkManager reports our lease, not the peer's address;
                // the sink's address arrives with its RTSP connection.
                connected_ip: None,
            });
        }
        Ok(peers)
    }

    fn form_group(&self, peer_path: &str) -> Result<(), IwdError> {
        let address: String = get(&self.connection, peer_path, NM_P2P_PEER, "HwAddress")?;
        let wfd_ies = self
            .wfd_ies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| {
                source_wfd_ies(&DisplayCapabilities::source_only(
                    crate::firewall::WFD_CONTROL_PORT,
                ))
            });
        let owner = current_user_name();
        let settings = group_settings(&address, &wfd_ies, owner.as_deref());
        let device = OwnedObjectPath::try_from(self.device.as_str())
            .map_err(|error| IwdError::Protocol(error.to_string()))?;
        let peer = OwnedObjectPath::try_from(peer_path)
            .map_err(|error| IwdError::Protocol(error.to_string()))?;
        let mut options: HashMap<&str, Value> = HashMap::new();
        options.insert("persist", var(Value::String("volatile".into())));
        let reply = blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            NM_PATH,
            Some(NM_MANAGER),
            "AddAndActivateConnection2",
            &(settings, device, peer, options),
        )
        .map_err(|error| classify(error, "AddAndActivateConnection2"))?;
        if reply.body.len() < 2 {
            return Err(IwdError::Protocol(
                "AddAndActivateConnection2: missing object paths in reply".into(),
            ));
        }
        let active = OwnedObjectPath::from_value(reply.body[1].clone())
            .map_err(|error| IwdError::Protocol(format!("AddAndActivateConnection2: {error}")))?;

        let deadline = Instant::now() + self.activation_timeout;
        loop {
            let state = get::<u32>(&self.connection, active.as_str(), NM_ACTIVE, "State").ok();
            match ActivationState::from_nm(state) {
                ActivationState::Activated => break,
                ActivationState::Failed => {
                    // The volatile profile should be discarded with the failed
                    // activation, but never leave an active connection behind
                    // if NetworkManager kept one.
                    let _ = self.deactivate(&active);
                    return Err(IwdError::Protocol(format!(
                        "NetworkManager could not form the P2P group with {address} \
                         (active connection state {state:?})"
                    )));
                }
                ActivationState::Pending if Instant::now() >= deadline => {
                    let _ = self.deactivate(&active);
                    return Err(IwdError::Protocol(format!(
                        "P2P group with {address} did not activate within {}s",
                        self.activation_timeout.as_secs()
                    )));
                }
                ActivationState::Pending => std::thread::sleep(Duration::from_millis(250)),
            }
        }
        let interface = get::<String>(
            &self.connection,
            self.device.as_str(),
            NM_DEVICE,
            "IpInterface",
        )
        .unwrap_or_default();
        log::info!("radio: P2P group with {address} is up on {interface}");
        self.groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer_path.to_owned(), active);
        Ok(())
    }

    fn disconnect_peer(&self, peer_path: &str) -> Result<(), IwdError> {
        // Keep the entry until deactivation succeeds: `Drop` is the retry of
        // last resort if a transient bus failure leaves the group up here.
        let active = self
            .groups
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(peer_path)
            .cloned();
        let Some(active) = active else {
            // A group this radio did not form is not ours to tear down.
            return Ok(());
        };
        self.deactivate(&active)?;
        self.groups
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(peer_path);
        Ok(())
    }
}

impl Drop for NmRadio {
    fn drop(&mut self) {
        let groups: Vec<OwnedObjectPath> = {
            let mut groups = self
                .groups
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            groups.drain().map(|(_, active)| active).collect()
        };
        for active in groups {
            if let Err(error) = self.deactivate(&active) {
                log::debug!("radio: P2P group was already gone: {error}");
            }
        }
        let finding = *self
            .finding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if finding
            && let Err(error) = blocking_call(
                &self.connection,
                Some(NM_SERVICE),
                self.device.as_str(),
                Some(NM_WIFI_P2P),
                "StopFind",
                &(),
            )
        {
            log::debug!("radio: StopFind: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference Skyworth A3F_99E, as NetworkManager reported it.
    const SKYWORTH: [u8; 9] = [0x00, 0x00, 0x06, 0x00, 0x11, 0x1c, 0x44, 0x00, 0x32];

    #[test]
    fn the_reference_tv_is_a_castable_sink() {
        let display = display_from_wfd_ies(&SKYWORTH).expect("a display peer");
        assert!(display.sink);
        assert!(!display.source);
        assert_eq!(display.port, Some(7236));
        assert!(display.can_receive_cast());
    }

    #[test]
    fn a_peer_without_wfd_ies_has_no_display_interface() {
        // The HP and Epson printers next to the TV report `WfdIEs: ay 0`.
        assert_eq!(display_from_wfd_ies(&[]), None);
    }

    #[test]
    fn an_unreadable_advertisement_is_not_offered_as_castable() {
        assert_eq!(display_from_wfd_ies(&[0x00, 0x00, 0x09, 0x00]), None);
        // Subelements without Device Information carry no role at all.
        assert_eq!(display_from_wfd_ies(&[0x07, 0x00, 0x02, 0x00, 0x01]), None);
    }

    #[test]
    fn a_source_peer_is_a_display_but_not_castable() {
        let phone = [0x00, 0x00, 0x06, 0x00, 0x10, 0x1c, 0x44, 0x00, 0x32];
        let display = display_from_wfd_ies(&phone).expect("display");
        assert!(display.source && !display.sink);
    }

    #[test]
    fn our_advertisement_is_a_source_with_our_port() {
        let ies = source_wfd_ies(&DisplayCapabilities::source_only(7236));
        assert_eq!(
            ies,
            vec![0x00, 0x00, 0x06, 0x00, 0x10, 0x1c, 0x44, 0x00, 0x32]
        );
        let decoded = display_from_wfd_ies(&ies).expect("decodes");
        assert!(decoded.source && !decoded.sink);
    }

    #[test]
    fn the_group_profile_is_a_dhcp_client_p2p_connection() {
        let ies = source_wfd_ies(&DisplayCapabilities::source_only(7236));
        let settings = group_settings("9E:1F:E6:7C:8B:CE", &ies, Some("ming"));
        assert_eq!(
            settings["connection"]["type"],
            var(Value::String("wifi-p2p".into()))
        );
        assert_eq!(
            settings["connection"]["autoconnect"],
            var(Value::Bool(false))
        );
        assert_eq!(
            settings["wifi-p2p"]["peer"],
            var(Value::String("9E:1F:E6:7C:8B:CE".into()))
        );
        assert_eq!(
            settings["wifi-p2p"]["wfd-ies"],
            var(Value::Array(
                "y".into(),
                ies.iter().copied().map(Value::Byte).collect()
            ))
        );
        assert_eq!(
            settings["ipv4"]["method"],
            var(Value::String("auto".into()))
        );
        assert_eq!(
            settings["ipv6"]["method"],
            var(Value::String("disabled".into()))
        );
        assert_eq!(
            settings["connection"]["permissions"],
            var(Value::Array(
                "s".into(),
                vec![Value::String("user:ming".to_owned())]
            )),
            "a user-owned profile needs only settings.modify.own"
        );
        let system = group_settings("9E:1F:E6:7C:8B:CE", &ies, None);
        assert!(!system["connection"].contains_key("permissions"));
    }

    #[test]
    fn the_profile_owner_is_resolved_from_the_real_uid() {
        let status = "Name:\ttessera-cast\nUid:\t1000\t1000\t1000\t1000\n";
        let passwd = "root:x:0:0::/root:/bin/sh\nming:x:1000:1000::/home/ming:/bin/zsh\n";
        assert_eq!(user_name_for(status, passwd), Some("ming".to_owned()));
        assert_eq!(user_name_for("Uid:\t4242\t4242\n", passwd), None);
        assert_eq!(user_name_for("Name: x\n", passwd), None);
    }

    #[test]
    fn permission_failures_are_authorization_not_protocol() {
        let error = tessera_dbus::Error::MethodError {
            name: "org.freedesktop.NetworkManager.Settings.PermissionDenied".to_owned(),
            detail: Some("Insufficient privileges".to_owned()),
        };
        assert!(matches!(
            classify(error, "AddAndActivateConnection2"),
            IwdError::NotAuthorized(reason) if reason.contains("Insufficient privileges")
        ));
    }

    #[test]
    fn activation_states_map_to_pending_activated_failed() {
        assert_eq!(ActivationState::from_nm(Some(0)), ActivationState::Pending);
        assert_eq!(ActivationState::from_nm(Some(1)), ActivationState::Pending);
        assert_eq!(
            ActivationState::from_nm(Some(2)),
            ActivationState::Activated
        );
        assert_eq!(ActivationState::from_nm(Some(3)), ActivationState::Failed);
        assert_eq!(ActivationState::from_nm(Some(4)), ActivationState::Failed);
        assert_eq!(
            ActivationState::from_nm(None),
            ActivationState::Failed,
            "a vanished active connection is a failed activation"
        );
    }
}
