//! NetworkManager backend over the system D-Bus (ADR-0242).
//!
//! This is the production default. It reads NetworkManager's own device,
//! access-point, active-connection, and settings objects and translates them
//! once into [`NetworkSnapshot`]. No NetworkManager interface name, object
//! path, or state value crosses this module.
//!
//! The D-Bus surface is intentionally narrow: the compositor's existing
//! station verbs (`scan`, `connect`, `disconnect`, `enable`, `forget`,
//! `auto-connect`) and the inspections they need. Wired configuration, VPNs,
//! DNS, and IP routing are not modelled because no consumer asks for them;
//! the coarse link state still reports a wired primary connection.

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tessera_desktop::system::{NetworkState, WifiLinkState, WifiNetwork, WifiSecurity};
use tessera_network::{BackendKind, NetworkBackend, NetworkError, NetworkSnapshot};
use zbus::Connection;
use zbus::{ObjectPath, OwnedObjectPath, OwnedVariant, Str, Variant};

const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
/// `Settings.ListConnections` lives on the dedicated Settings object, not on
/// the manager object.
const NM_SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";
const ACTIVE_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION_IFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_IFACE: &str = "org.freedesktop.DBus";

const DEVICE_TYPE_ETHERNET: u32 = 1;
const DEVICE_TYPE_WIFI: u32 = 2;
const DEVICE_STATE_ACTIVATED: u32 = 100;

const AP_FLAG_PRIVACY: u32 = 0x1;
const AP_SEC_KEY_MGMT_PSK: u32 = 0x100;
const AP_SEC_KEY_MGMT_802_1X: u32 = 0x200;
const AP_SEC_KEY_MGMT_SAE: u32 = 0x400;
const AP_SEC_KEY_MGMT_OWE: u32 = 0x800;
const AP_SEC_KEY_MGMT_EAP_SUITE_B_192: u32 = 0x2000;

/// How long a requested scan keeps the neutral link state in `Scanning`.
const SCAN_VISIBLE: Duration = Duration::from_secs(5);
/// How long a failed refresh waits before the worker retries it.
const RETRY_DELAY: Duration = Duration::from_secs(2);

type NmSettings = HashMap<String, HashMap<String, OwnedVariant>>;

#[derive(Debug, Clone)]
struct NmManager {
    networking_enabled: bool,
    wireless_enabled: bool,
    wireless_hardware_enabled: bool,
    primary_connection: String,
    primary_connection_type: String,
    devices: Vec<String>,
}

#[derive(Debug, Clone)]
struct NmDevice {
    interface: String,
    device_type: u32,
    state: u32,
}

#[derive(Debug, Clone)]
struct NmWireless {
    access_points: Vec<String>,
    active_access_point: String,
}

#[derive(Debug, Clone)]
struct NmAccessPoint {
    ssid: Vec<u8>,
    strength: u8,
    frequency: u32,
    flags: u32,
    wpa_flags: u32,
    rsn_flags: u32,
}

#[derive(Debug, Clone)]
struct NmActiveConnection {
    devices: Vec<String>,
}

/// The AP detail the neutral `WifiSecurity` enum intentionally omits.
///
/// The user-facing model only needs "open" versus "secured", but the
/// connection settings need the actual key-management method: WPA3-SAE and
/// OWE are not interchangeable with WPA2-PSK on the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct NmSecurity {
    psk: bool,
    sae: bool,
    owe: bool,
}

impl NmSecurity {
    fn from_access_point(access_point: &NmAccessPoint) -> Self {
        let security = access_point.wpa_flags | access_point.rsn_flags;
        Self {
            psk: security & AP_SEC_KEY_MGMT_PSK != 0,
            sae: security & AP_SEC_KEY_MGMT_SAE != 0,
            owe: security & AP_SEC_KEY_MGMT_OWE != 0,
        }
    }
}

#[derive(Debug, Clone)]
struct NmProfile {
    path: String,
    ssid: String,
    auto_connect: bool,
}

/// The narrow NetworkManager surface this backend consumes. Split from the
/// implementation so the translation and command policy are testable without
/// a bus.
trait NmApi: Send + Sync {
    fn manager(&self) -> Result<NmManager, NetworkError>;
    fn device(&self, path: &str) -> Result<NmDevice, NetworkError>;
    fn wireless(&self, path: &str) -> Result<NmWireless, NetworkError>;
    fn access_point(&self, path: &str) -> Result<NmAccessPoint, NetworkError>;
    fn active_connection(&self, path: &str) -> Result<NmActiveConnection, NetworkError>;
    fn list_connections(&self) -> Result<Vec<String>, NetworkError>;
    fn connection_settings(&self, path: &str) -> Result<NmSettings, NetworkError>;
    fn request_scan(&self, device: &str) -> Result<(), NetworkError>;
    fn set_networking_enabled(&self, enabled: bool) -> Result<(), NetworkError>;
    fn set_wireless_enabled(&self, enabled: bool) -> Result<(), NetworkError>;
    fn activate_connection(
        &self,
        profile: &str,
        device: &str,
        specific_object: &str,
    ) -> Result<(), NetworkError>;
    fn add_and_activate(
        &self,
        settings: NmSettings,
        device: &str,
        specific_object: &str,
    ) -> Result<(), NetworkError>;
    fn disconnect_device(&self, device: &str) -> Result<(), NetworkError>;
    fn update_connection(&self, path: &str, settings: NmSettings) -> Result<(), NetworkError>;
    fn delete_connection(&self, path: &str) -> Result<(), NetworkError>;
}

pub struct NetworkManagerBackend {
    api: Arc<dyn NmApi>,
    /// The current Wi-Fi device object path. Re-resolved when the device
    /// disappears (USB adapter removal, driver reload, NetworkManager restart),
    /// so a hotplug does not leave the service permanently unavailable.
    device: Mutex<String>,
    cache: Mutex<NetworkSnapshot>,
    ap_security: Mutex<HashMap<String, NmSecurity>>,
    dirty: Arc<AtomicBool>,
    scan_until: Mutex<Option<Instant>>,
    retry_at: Mutex<Option<Instant>>,
}

impl NetworkManagerBackend {
    /// Open NetworkManager, find a Wi-Fi device, and take one live reading.
    ///
    /// NetworkManager must already own its bus name. Selection never
    /// D-Bus-activates it: on an iwd host an activation would start a second
    /// radio manager, and a stopped system service is an operator decision.
    pub fn open() -> Result<Self, NetworkError> {
        let connection =
            zbus::block_on(Connection::system()).map_err(|_| NetworkError::Unavailable)?;
        if !name_has_owner(&connection, NM_SERVICE)? {
            return Err(NetworkError::Unavailable);
        }
        let api: Arc<dyn NmApi> = Arc::new(ZbusNmApi {
            connection: connection.clone(),
        });
        // Once NetworkManager owns the name, every failure below is "present
        // but unusable", never "absent": falling back to iwd here would start
        // a second radio manager.
        let manager = api.manager().map_err(|error| {
            NetworkError::Service(format!("NetworkManager did not answer: {error}"))
        })?;
        let device = find_wifi_device(&api, &manager)?;
        let backend = Self::assemble(api, device);
        backend.refresh()?;
        if start_signal_thread(connection, backend.dirty.clone()) {
            log::debug!("network service: NetworkManager signal subscription active");
        } else {
            log::warn!(
                "network service: NetworkManager signal subscription did not start; \
                 state will update on command and retry only"
            );
        }
        Ok(backend)
    }

    fn assemble(api: Arc<dyn NmApi>, device: String) -> Self {
        Self {
            api,
            device: Mutex::new(device),
            cache: Mutex::new(NetworkSnapshot::unavailable(
                BackendKind::NetworkManager,
                "NetworkManager has not answered yet",
            )),
            ap_security: Mutex::new(HashMap::new()),
            dirty: Arc::new(AtomicBool::new(false)),
            scan_until: Mutex::new(None),
            retry_at: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_api(api: Arc<dyn NmApi>, device: &str) -> Self {
        let backend = Self::assemble(api, device.to_string());
        backend.refresh().expect("test NetworkManager refresh");
        backend
    }

    fn device_path(&self) -> String {
        self.device
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// The tracked device, or the first Wi-Fi device NetworkManager now
    /// manages when the tracked one vanished.
    ///
    /// A `Device` object path is not stable across a USB unplug/replug, a
    /// driver reload, or a NetworkManager restart. Treating the old path as
    /// permanent would make every refresh fail until the session restarted,
    /// even after the adapter came back.
    fn current_device(&self, manager: &NmManager) -> Result<(String, NmDevice), NetworkError> {
        let path = self.device_path();
        // A stale object path can be reused by another device after a
        // hotplug. Accept it only while it is still a Wi-Fi device; otherwise
        // re-resolve instead of reading a wireless interface that no longer
        // belongs to this path.
        if let Ok(device) = self.api.device(&path)
            && device.device_type == DEVICE_TYPE_WIFI
        {
            return Ok((path, device));
        }
        let path = find_wifi_device(&self.api, manager)?;
        let device = self.api.device(&path)?;
        log::info!("network service: adopted Wi-Fi device {path}");
        *self
            .device
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = path.clone();
        Ok((path, device))
    }

    fn resolve_device(&self) -> Result<String, NetworkError> {
        let manager = self.api.manager()?;
        self.current_device(&manager).map(|(path, _)| path)
    }

    fn profiles(&self) -> Result<Vec<NmProfile>, NetworkError> {
        let mut profiles = Vec::new();
        for path in self.api.list_connections()? {
            let Ok(settings) = self.api.connection_settings(&path) else {
                // One malformed profile must not hide every other network.
                continue;
            };
            let Some(ssid) = settings_ssid(&settings) else {
                continue;
            };
            profiles.push(NmProfile {
                path,
                ssid,
                auto_connect: settings_auto_connect(&settings).unwrap_or(true),
            });
        }
        profiles.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(profiles)
    }

    fn read_snapshot(&self) -> Result<NetworkSnapshot, NetworkError> {
        let manager = self.api.manager()?;
        let (device_path, device) = self.current_device(&manager)?;
        let wireless = self.api.wireless(&device_path)?;
        let profiles = self.profiles()?;
        let wireless_enabled = manager.networking_enabled
            && manager.wireless_enabled
            && manager.wireless_hardware_enabled;
        let active_ap = wireless.active_access_point.as_str();
        let scan_active = self
            .scan_until
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some_and(|deadline| Instant::now() < deadline);

        let mut access_points: BTreeMap<String, WifiNetwork> = BTreeMap::new();
        let mut ap_security: HashMap<String, NmSecurity> = HashMap::new();
        for path in &wireless.access_points {
            let Ok(access_point) = self.api.access_point(path) else {
                // A peer that vanished between the list and the read is not an
                // error; the next refresh will see the new list.
                continue;
            };
            let ssid = String::from_utf8_lossy(&access_point.ssid).into_owned();
            if ssid.is_empty() {
                continue;
            }
            let connected = path == active_ap;
            let security = access_point_security(&access_point);
            let detail = NmSecurity::from_access_point(&access_point);
            let bars = signal_bars(access_point.strength);
            let profile = profiles.iter().find(|profile| profile.ssid == ssid);
            let network = WifiNetwork {
                ssid: ssid.clone(),
                signal_bars: bars,
                security,
                is_connected: connected,
                is_saved: profile.is_some(),
                auto_connect: profile.is_some_and(|profile| profile.auto_connect),
                frequency_mhz: (access_point.frequency > 0).then_some(access_point.frequency),
            };
            match access_points.get_mut(&ssid) {
                Some(existing) => {
                    let was_connected = existing.is_connected;
                    let stronger = network.signal_bars > existing.signal_bars;
                    existing.is_connected |= network.is_connected;
                    existing.security = merge_security(existing.security, network.security);
                    if stronger {
                        existing.signal_bars = network.signal_bars;
                        existing.frequency_mhz = network.frequency_mhz;
                    }
                    // The connected AP is the one a profile must match; among
                    // equal candidates, the strongest AP supplies the detail.
                    if network.is_connected || (stronger && !was_connected) {
                        ap_security.insert(ssid.clone(), detail);
                    }
                }
                None => {
                    access_points.insert(ssid.clone(), network);
                    ap_security.insert(ssid, detail);
                }
            }
        }

        *self
            .ap_security
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = ap_security;

        let mut networks: Vec<WifiNetwork> = access_points.into_values().collect();
        networks.sort_by(|left, right| {
            right
                .is_connected
                .cmp(&left.is_connected)
                .then_with(|| right.signal_bars.cmp(&left.signal_bars))
                .then_with(|| left.ssid.cmp(&right.ssid))
        });
        let active_ssid = wireless_enabled
            .then(|| {
                networks
                    .iter()
                    .find(|network| network.is_connected)
                    .map(|network| network.ssid.clone())
            })
            .flatten();

        let wifi_state = link_state(
            wireless_enabled,
            device.state,
            active_ssid.is_some(),
            scan_active,
        );
        let network = if manager.networking_enabled {
            primary_network(&manager, wifi_state)
        } else {
            NetworkState::Offline
        };
        let network_interface = match network {
            NetworkState::Wifi => device.interface,
            NetworkState::Wired => self.primary_interface(&manager),
            NetworkState::Offline => String::new(),
        };

        Ok(NetworkSnapshot {
            available: true,
            backend: BackendKind::NetworkManager,
            network,
            network_authoritative: true,
            network_interface,
            wifi_enabled: Some(wireless_enabled),
            wifi_state,
            wifi_ssid: active_ssid,
            wifi_networks: networks,
            last_error: None,
        })
    }

    fn primary_interface(&self, manager: &NmManager) -> String {
        if !manager.primary_connection.is_empty()
            && let Ok(active) = self.api.active_connection(&manager.primary_connection)
            && let Some(device) = active.devices.first()
            && let Ok(device) = self.api.device(device)
        {
            return device.interface;
        }
        for path in &manager.devices {
            if let Ok(device) = self.api.device(path)
                && device.device_type == DEVICE_TYPE_ETHERNET
                && device.state == DEVICE_STATE_ACTIVATED
            {
                return device.interface;
            }
        }
        String::new()
    }

    fn matching_profiles(&self, ssid: &str) -> Result<Vec<NmProfile>, NetworkError> {
        Ok(self
            .profiles()?
            .into_iter()
            .filter(|profile| profile.ssid == ssid)
            .collect())
    }

    fn update_profile_security(
        &self,
        path: &str,
        security: WifiSecurity,
        passphrase: Option<&str>,
        detail: NmSecurity,
    ) -> Result<(), NetworkError> {
        let mut settings = self.api.connection_settings(path)?;
        apply_security(&mut settings, security, passphrase, detail)?;
        self.api.update_connection(path, settings)
    }
}

impl NetworkBackend for NetworkManagerBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::NetworkManager
    }

    fn snapshot(&self) -> NetworkSnapshot {
        self.cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn refresh(&self) -> Result<(), NetworkError> {
        match self.read_snapshot() {
            Ok(snapshot) => {
                *self.cache.lock().unwrap_or_else(|error| error.into_inner()) = snapshot;
                *self
                    .retry_at
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = None;
                Ok(())
            }
            Err(error) => {
                let mut snapshot = self.snapshot();
                snapshot.available = false;
                snapshot.wifi_enabled = None;
                snapshot.wifi_state = WifiLinkState::Disconnected;
                snapshot.wifi_ssid = None;
                snapshot.wifi_networks.clear();
                snapshot.network = NetworkState::Offline;
                snapshot.network_interface.clear();
                snapshot.last_error = Some(error.to_string());
                *self.cache.lock().unwrap_or_else(|error| error.into_inner()) = snapshot;
                *self
                    .retry_at
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) =
                    Some(Instant::now() + RETRY_DELAY);
                Err(error)
            }
        }
    }

    fn poll(&self) -> bool {
        let dirty = self.dirty.swap(false, Ordering::Relaxed);
        let scan_finished = {
            let mut scan_until = self
                .scan_until
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match *scan_until {
                Some(deadline) if Instant::now() >= deadline => {
                    *scan_until = None;
                    true
                }
                _ => false,
            }
        };
        let retry_due = self
            .retry_at
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some_and(|deadline| Instant::now() >= deadline);
        if dirty || scan_finished || retry_due {
            let _ = self.refresh();
            true
        } else {
            false
        }
    }

    fn scan(&self) -> Result<(), NetworkError> {
        let device = self.resolve_device()?;
        self.api.request_scan(&device)?;
        *self
            .scan_until
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Instant::now() + SCAN_VISIBLE);
        let _ = self.refresh();
        Ok(())
    }

    fn connect(&self, ssid: &str, passphrase: Option<&str>) -> Result<(), NetworkError> {
        self.refresh()?;
        let device = self.resolve_device()?;
        let snapshot = self.snapshot();
        let target = snapshot
            .wifi_networks
            .iter()
            .find(|network| network.ssid == ssid)
            .cloned()
            .ok_or_else(|| NetworkError::NetworkNotFound(ssid.to_string()))?;

        let detail = self
            .ap_security
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(ssid)
            .copied()
            .unwrap_or_default();
        let profiles = self.matching_profiles(ssid)?;
        if let Some(profile) = profiles.first() {
            if passphrase.is_some() {
                self.update_profile_security(&profile.path, target.security, passphrase, detail)?;
            }
            self.api.activate_connection(&profile.path, &device, "/")?;
        } else {
            let settings = connection_settings(&target, passphrase, detail)?;
            self.api.add_and_activate(settings, &device, "/")?;
        }
        let _ = self.refresh();
        Ok(())
    }

    fn disconnect(&self) -> Result<(), NetworkError> {
        let device = self.resolve_device()?;
        self.api.disconnect_device(&device)?;
        let _ = self.refresh();
        Ok(())
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
        if enabled {
            let manager = self.api.manager()?;
            if !manager.networking_enabled {
                // A Wi-Fi toggle on an airplane-mode/global-off host must
                // actually turn networking on; otherwise the radio flips its
                // software switch while no connection can ever activate.
                self.api.set_networking_enabled(true)?;
            }
        }
        self.api.set_wireless_enabled(enabled)?;
        let _ = self.refresh();
        Ok(())
    }

    fn forget(&self, ssid: &str) -> Result<(), NetworkError> {
        let profiles = self.matching_profiles(ssid)?;
        if profiles.is_empty() {
            return Err(NetworkError::NetworkNotFound(ssid.to_string()));
        }
        for profile in profiles {
            self.api.delete_connection(&profile.path)?;
        }
        let _ = self.refresh();
        Ok(())
    }

    fn set_auto_connect(&self, ssid: &str, auto_connect: bool) -> Result<(), NetworkError> {
        let profiles = self.matching_profiles(ssid)?;
        if profiles.is_empty() {
            return Err(NetworkError::NetworkNotFound(ssid.to_string()));
        }
        for profile in profiles {
            let mut settings = self.api.connection_settings(&profile.path)?;
            setting_mut(&mut settings, "connection")
                .insert("autoconnect".to_string(), OwnedVariant::from(auto_connect));
            self.api.update_connection(&profile.path, settings)?;
        }
        let _ = self.refresh();
        Ok(())
    }
}

fn blocking_call<B>(
    connection: &Connection,
    destination: Option<&str>,
    path: &str,
    interface: Option<&str>,
    method: &str,
    body: &B,
) -> zbus::Result<zbus::Message>
where
    B: zbus::export::serde::ser::Serialize + zbus::DynamicType,
{
    zbus::block_on(connection.call_method(destination, path, interface, method, body))
}

fn name_has_owner(connection: &Connection, name: &str) -> Result<bool, NetworkError> {
    let reply = blocking_call(
        connection,
        Some(DBUS_SERVICE),
        DBUS_PATH,
        Some(DBUS_IFACE),
        "NameHasOwner",
        &(name,),
    )
    .map_err(|error| NetworkError::Service(format!("NameHasOwner failed: {error}")))?;
    reply
        .body()
        .deserialize()
        .map_err(|error| NetworkError::Service(format!("NameHasOwner body: {error}")))
}

fn find_wifi_device(api: &Arc<dyn NmApi>, manager: &NmManager) -> Result<String, NetworkError> {
    for path in &manager.devices {
        if let Ok(device) = api.device(path)
            && device.device_type == DEVICE_TYPE_WIFI
        {
            return Ok(path.clone());
        }
    }
    Err(NetworkError::Unsupported(
        "NetworkManager reported no Wi-Fi device".to_string(),
    ))
}

fn start_signal_thread(connection: Connection, dirty: Arc<AtomicBool>) -> bool {
    std::thread::Builder::new()
        .name("tessera-nm-signals".into())
        .spawn(move || {
            tessera_sched::mark_background();
            let mut stream = match zbus::block_on(zbus::MessageStream::for_match_rule(
                "type='signal',sender='org.freedesktop.NetworkManager'",
                &connection,
                NonZeroUsize::new(64),
            )) {
                Ok(stream) => stream,
                Err(error) => {
                    log::warn!(
                        "network service: could not subscribe to NetworkManager signals: {error}"
                    );
                    return;
                }
            };
            // Keep the thread inside `block_on` so the connection's zruntime
            // keeps delivering signals; the stream ends when the connection
            // closes.
            zbus::block_on(async {
                while let Some(message) = stream.next().await {
                    if message.is_ok() {
                        dirty.store(true, Ordering::Relaxed);
                    }
                }
            });
        })
        .is_ok()
}

fn link_state(
    wireless_enabled: bool,
    device_state: u32,
    connected: bool,
    scanning: bool,
) -> WifiLinkState {
    if !wireless_enabled {
        return WifiLinkState::Disabled;
    }
    match device_state {
        DEVICE_STATE_ACTIVATED if connected => WifiLinkState::Connected,
        40 | 50 | 60 | 70 | 80 | 90 => WifiLinkState::Connecting,
        _ if scanning => WifiLinkState::Scanning,
        _ => WifiLinkState::Disconnected,
    }
}

fn primary_network(manager: &NmManager, wifi_state: WifiLinkState) -> NetworkState {
    if wifi_state == WifiLinkState::Disabled {
        // A disabled radio cannot own the primary link, even if NetworkManager
        // has not cleared its last `PrimaryConnectionType` yet.
        return if manager.primary_connection_type.is_empty()
            || manager.primary_connection_type == "802-11-wireless"
        {
            NetworkState::Offline
        } else {
            NetworkState::Wired
        };
    }
    // The primary connection wins: both Ethernet and Wi-Fi can be up, and the
    // snapshot must not relabel a wired default as Wi-Fi just because a station
    // link also exists. `wifi_ssid` remains independent telemetry.
    if manager.primary_connection_type == "802-11-wireless" {
        NetworkState::Wifi
    } else if !manager.primary_connection_type.is_empty() {
        NetworkState::Wired
    } else if wifi_state == WifiLinkState::Connected {
        NetworkState::Wifi
    } else {
        NetworkState::Offline
    }
}

fn access_point_security(access_point: &NmAccessPoint) -> WifiSecurity {
    security_from_flags(
        access_point.flags,
        access_point.wpa_flags | access_point.rsn_flags,
    )
}

fn security_from_flags(flags: u32, security: u32) -> WifiSecurity {
    if flags & AP_FLAG_PRIVACY == 0 && security == 0 {
        WifiSecurity::Open
    } else if security & (AP_SEC_KEY_MGMT_802_1X | AP_SEC_KEY_MGMT_EAP_SUITE_B_192) != 0 {
        WifiSecurity::Enterprise
    } else {
        // WPA/WPA2/WPA3 personal, OWE, and WEP all live in this bucket: the
        // neutral model has no finer distinction, and a secured AP is never
        // presented as open. WPA3-SAE (0x400) and OWE land here too.
        WifiSecurity::WpaPsk
    }
}

fn merge_security(existing: WifiSecurity, incoming: WifiSecurity) -> WifiSecurity {
    fn rank(security: WifiSecurity) -> u8 {
        match security {
            WifiSecurity::Open => 0,
            WifiSecurity::WpaPsk => 1,
            WifiSecurity::Enterprise => 2,
        }
    }
    if rank(incoming) > rank(existing) {
        incoming
    } else {
        existing
    }
}

fn signal_bars(strength: u8) -> u8 {
    u16::from(strength).div_ceil(25).min(4) as u8
}

fn own(value: &str) -> OwnedVariant {
    OwnedVariant::from(Str::from(value))
}

fn bytes(value: &[u8]) -> Result<OwnedVariant, NetworkError> {
    OwnedVariant::try_from(Variant::Array(zbus::Array::from(value.to_vec())))
        .map_err(|error| NetworkError::Service(format!("could not encode SSID bytes: {error}")))
}

fn setting_mut<'a>(
    settings: &'a mut NmSettings,
    name: &str,
) -> &'a mut HashMap<String, OwnedVariant> {
    settings.entry(name.to_string()).or_default()
}

fn settings_ssid(settings: &NmSettings) -> Option<String> {
    let value = settings.get("802-11-wireless")?.get("ssid")?;
    let value: &Variant = Borrow::<Variant>::borrow(value);
    value_bytes(value).map(|ssid| String::from_utf8_lossy(&ssid).into_owned())
}

fn settings_auto_connect(settings: &NmSettings) -> Option<bool> {
    let value = settings.get("connection")?.get("autoconnect")?;
    let value: &Variant = Borrow::<Variant>::borrow(value);
    value.downcast_ref::<bool>().ok()
}

fn connection_settings(
    network: &WifiNetwork,
    passphrase: Option<&str>,
    detail: NmSecurity,
) -> Result<NmSettings, NetworkError> {
    let mut settings = NmSettings::new();
    let connection = setting_mut(&mut settings, "connection");
    connection.insert("id".to_string(), own(&network.ssid));
    connection.insert("type".to_string(), own("802-11-wireless"));
    connection.insert("autoconnect".to_string(), OwnedVariant::from(true));

    let wireless = setting_mut(&mut settings, "802-11-wireless");
    wireless.insert("ssid".to_string(), bytes(network.ssid.as_bytes())?);
    wireless.insert("mode".to_string(), own("infrastructure"));
    apply_security(&mut settings, network.security, passphrase, detail)?;

    setting_mut(&mut settings, "ipv4").insert("method".to_string(), own("auto"));
    setting_mut(&mut settings, "ipv6").insert("method".to_string(), own("auto"));
    Ok(settings)
}

fn apply_security(
    settings: &mut NmSettings,
    security: WifiSecurity,
    passphrase: Option<&str>,
    detail: NmSecurity,
) -> Result<(), NetworkError> {
    match security {
        WifiSecurity::Open => {
            settings.remove("802-11-wireless-security");
            if let Some(wireless) = settings.get_mut("802-11-wireless") {
                wireless.remove("security");
            }
            Ok(())
        }
        WifiSecurity::WpaPsk => {
            setting_mut(settings, "802-11-wireless")
                .insert("security".to_string(), own("802-11-wireless-security"));
            let security = setting_mut(settings, "802-11-wireless-security");
            if detail.owe {
                // Enhanced Open carries no passphrase.
                security.insert("key-mgmt".to_string(), own("owe"));
                return Ok(());
            }
            let passphrase = passphrase.ok_or_else(|| {
                NetworkError::Refused("a passphrase is required for this network".to_string())
            })?;
            // A WPA3 transition AP advertises both PSK and SAE; prefer the
            // WPA2 path there. A WPA3-only AP advertises SAE alone.
            security.insert(
                "key-mgmt".to_string(),
                own(if detail.sae && !detail.psk {
                    "sae"
                } else {
                    "wpa-psk"
                }),
            );
            security.insert("psk".to_string(), own(passphrase));
            Ok(())
        }
        WifiSecurity::Enterprise => Err(NetworkError::Unsupported(
            "enterprise Wi-Fi needs an 802.1X identity, which the neutral connect verb does not carry"
                .to_string(),
        )),
    }
}

struct ZbusNmApi {
    connection: Connection,
}

impl NmApi for ZbusNmApi {
    fn manager(&self) -> Result<NmManager, NetworkError> {
        let properties = get_all(&self.connection, NM_PATH, NM_IFACE)?;
        Ok(NmManager {
            networking_enabled: get_bool(&properties, "NetworkingEnabled").unwrap_or(true),
            wireless_enabled: get_bool(&properties, "WirelessEnabled").unwrap_or(false),
            wireless_hardware_enabled: get_bool(&properties, "WirelessHardwareEnabled")
                .unwrap_or(true),
            primary_connection: get_object_path(&properties, "PrimaryConnection")
                .unwrap_or_default(),
            primary_connection_type: get_str(&properties, "PrimaryConnectionType")
                .unwrap_or_default(),
            devices: get_object_paths(&properties, "Devices"),
        })
    }

    fn device(&self, path: &str) -> Result<NmDevice, NetworkError> {
        let properties = get_all(&self.connection, path, DEVICE_IFACE)?;
        Ok(NmDevice {
            interface: get_str(&properties, "Interface").unwrap_or_default(),
            device_type: get_u32(&properties, "DeviceType").unwrap_or_default(),
            state: get_u32(&properties, "State").unwrap_or_default(),
        })
    }

    fn wireless(&self, path: &str) -> Result<NmWireless, NetworkError> {
        let properties = get_all(&self.connection, path, WIRELESS_IFACE)?;
        Ok(NmWireless {
            access_points: get_object_paths(&properties, "AccessPoints"),
            active_access_point: get_object_path(&properties, "ActiveAccessPoint")
                .unwrap_or_default(),
        })
    }

    fn access_point(&self, path: &str) -> Result<NmAccessPoint, NetworkError> {
        let properties = get_all(&self.connection, path, AP_IFACE)?;
        Ok(NmAccessPoint {
            ssid: get_bytes(&properties, "Ssid").unwrap_or_default(),
            strength: get_u8(&properties, "Strength").unwrap_or_default(),
            frequency: get_u32(&properties, "Frequency").unwrap_or_default(),
            flags: get_u32(&properties, "Flags").unwrap_or_default(),
            wpa_flags: get_u32(&properties, "WpaFlags").unwrap_or_default(),
            rsn_flags: get_u32(&properties, "RsnFlags").unwrap_or_default(),
        })
    }

    fn active_connection(&self, path: &str) -> Result<NmActiveConnection, NetworkError> {
        let properties = get_all(&self.connection, path, ACTIVE_IFACE)?;
        Ok(NmActiveConnection {
            devices: get_object_paths(&properties, "Devices"),
        })
    }

    fn list_connections(&self) -> Result<Vec<String>, NetworkError> {
        let reply = blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            NM_SETTINGS_PATH,
            Some(SETTINGS_IFACE),
            "ListConnections",
            &(),
        )
        .map_err(|error| map_call("ListConnections", error))?;
        let paths: Vec<OwnedObjectPath> = reply
            .body()
            .deserialize()
            .map_err(|error| NetworkError::Service(format!("ListConnections body: {error}")))?;
        Ok(paths.into_iter().map(|path| path.to_string()).collect())
    }

    fn connection_settings(&self, path: &str) -> Result<NmSettings, NetworkError> {
        let reply = blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            path,
            Some(CONNECTION_IFACE),
            "GetSettings",
            &(),
        )
        .map_err(|error| map_call("GetSettings", error))?;
        reply
            .body()
            .deserialize()
            .map_err(|error| NetworkError::Service(format!("GetSettings body: {error}")))
    }

    fn request_scan(&self, device: &str) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            device,
            Some(WIRELESS_IFACE),
            "RequestScan",
            &(HashMap::<String, OwnedVariant>::new(),),
        )
        .map_err(|error| map_call("RequestScan", error))?;
        Ok(())
    }

    fn set_networking_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            NM_PATH,
            Some(PROPERTIES_IFACE),
            "Set",
            &(NM_IFACE, "NetworkingEnabled", Variant::Bool(enabled)),
        )
        .map_err(|error| map_call("Set NetworkingEnabled", error))?;
        Ok(())
    }

    fn set_wireless_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            NM_PATH,
            Some(PROPERTIES_IFACE),
            "Set",
            &(NM_IFACE, "WirelessEnabled", Variant::Bool(enabled)),
        )
        .map_err(|error| map_call("Set WirelessEnabled", error))?;
        Ok(())
    }

    fn activate_connection(
        &self,
        profile: &str,
        device: &str,
        specific_object: &str,
    ) -> Result<(), NetworkError> {
        let profile = object_path(profile)?;
        let device = object_path(device)?;
        let specific = object_path(specific_object)?;
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            NM_PATH,
            Some(NM_IFACE),
            "ActivateConnection",
            &(profile, device, specific),
        )
        .map_err(|error| map_call("ActivateConnection", error))?;
        Ok(())
    }

    fn add_and_activate(
        &self,
        settings: NmSettings,
        device: &str,
        specific_object: &str,
    ) -> Result<(), NetworkError> {
        let device = object_path(device)?;
        let specific = object_path(specific_object)?;
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            NM_PATH,
            Some(NM_IFACE),
            "AddAndActivateConnection",
            &(settings, device, specific),
        )
        .map_err(|error| map_call("AddAndActivateConnection", error))?;
        Ok(())
    }

    fn disconnect_device(&self, device: &str) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            device,
            Some(DEVICE_IFACE),
            "Disconnect",
            &(),
        )
        .map_err(|error| map_call("Disconnect", error))?;
        Ok(())
    }

    fn update_connection(&self, path: &str, settings: NmSettings) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            path,
            Some(CONNECTION_IFACE),
            "Update",
            &(settings,),
        )
        .map_err(|error| map_call("Update", error))?;
        Ok(())
    }

    fn delete_connection(&self, path: &str) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(NM_SERVICE),
            path,
            Some(CONNECTION_IFACE),
            "Delete",
            &(),
        )
        .map_err(|error| map_call("Delete", error))?;
        Ok(())
    }
}

fn get_all(
    connection: &Connection,
    path: &str,
    interface: &str,
) -> Result<HashMap<String, OwnedVariant>, NetworkError> {
    let reply = blocking_call(
        connection,
        Some(NM_SERVICE),
        path,
        Some(PROPERTIES_IFACE),
        "GetAll",
        &(interface,),
    )
    .map_err(|error| map_call("GetAll", error))?;
    reply
        .body()
        .deserialize()
        .map_err(|error| NetworkError::Service(format!("GetAll body: {error}")))
}

fn map_call(operation: &str, error: zbus::Error) -> NetworkError {
    if let Some((name, _)) = error_name(&error) {
        return match name.as_str() {
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => NetworkError::Unavailable,
            "org.freedesktop.DBus.Error.AccessDenied" => {
                NetworkError::Refused(operation.to_string())
            }
            _ => NetworkError::Service(format!("{operation}: {error}")),
        };
    }
    NetworkError::Service(format!("{operation}: {error}"))
}

/// The D-Bus error name and description, including errors zbus promotes to
/// `fdo::Error` (`AccessDenied`, `ServiceUnknown`, …).
fn error_name(error: &zbus::Error) -> Option<(String, String)> {
    use zbus::DBusError as _;
    match error {
        zbus::Error::MethodError(name, detail, _) => {
            Some((name.to_string(), detail.clone().unwrap_or_default()))
        }
        zbus::Error::FDO(error) => Some((
            error.name().to_string(),
            error.description().unwrap_or_default().to_owned(),
        )),
        _ => None,
    }
}

fn object_path(value: &str) -> Result<ObjectPath<'_>, NetworkError> {
    ObjectPath::try_from(value)
        .map_err(|error| NetworkError::Service(format!("invalid object path '{value}': {error}")))
}

fn get_str(properties: &HashMap<String, OwnedVariant>, key: &str) -> Option<String> {
    let value: &Variant = Borrow::<Variant>::borrow(properties.get(key)?);
    value
        .downcast_ref::<&str>()
        .ok()
        .map(std::string::ToString::to_string)
}

fn get_bool(properties: &HashMap<String, OwnedVariant>, key: &str) -> Option<bool> {
    let value: &Variant = Borrow::<Variant>::borrow(properties.get(key)?);
    value.downcast_ref::<bool>().ok()
}

fn get_u32(properties: &HashMap<String, OwnedVariant>, key: &str) -> Option<u32> {
    let value: &Variant = Borrow::<Variant>::borrow(properties.get(key)?);
    value.downcast_ref::<u32>().ok()
}

fn get_u8(properties: &HashMap<String, OwnedVariant>, key: &str) -> Option<u8> {
    let value: &Variant = Borrow::<Variant>::borrow(properties.get(key)?);
    value.downcast_ref::<u8>().ok()
}

fn get_bytes(properties: &HashMap<String, OwnedVariant>, key: &str) -> Option<Vec<u8>> {
    let value: &Variant = Borrow::<Variant>::borrow(properties.get(key)?);
    value_bytes(value)
}

fn value_bytes(value: &Variant<'_>) -> Option<Vec<u8>> {
    let Variant::Array(array) = value else {
        return None;
    };
    array
        .iter()
        .map(|element| element.downcast_ref::<u8>().ok())
        .collect()
}

fn get_object_path(properties: &HashMap<String, OwnedVariant>, key: &str) -> Option<String> {
    let value: &Variant = Borrow::<Variant>::borrow(properties.get(key)?);
    value
        .downcast_ref::<ObjectPath>()
        .ok()
        .map(|path| path.to_string())
}

fn get_object_paths(properties: &HashMap<String, OwnedVariant>, key: &str) -> Vec<String> {
    let Some(value) = properties.get(key) else {
        return Vec::new();
    };
    let value: &Variant = Borrow::<Variant>::borrow(value);
    let Variant::Array(array) = value else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|element| {
            element
                .downcast_ref::<ObjectPath>()
                .ok()
                .map(|path| path.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeState {
        manager: Option<NmManager>,
        devices: HashMap<String, NmDevice>,
        wireless: Option<NmWireless>,
        access_points: HashMap<String, NmAccessPoint>,
        active_connections: HashMap<String, NmActiveConnection>,
        profiles: HashMap<String, NmSettings>,
        scans: Vec<String>,
        activated: Vec<(String, String)>,
        added: Vec<NmSettings>,
        deleted: Vec<String>,
        updated: Vec<(String, NmSettings)>,
        networking_enabled: Option<bool>,
        wireless_enabled: Option<bool>,
        disconnected: Vec<String>,
    }

    #[derive(Default)]
    struct FakeNm {
        state: Mutex<FakeState>,
    }

    impl FakeNm {
        fn with_state(state: FakeState) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(state),
            })
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.state.lock().unwrap_or_else(|error| error.into_inner())
        }
    }

    impl NmApi for FakeNm {
        fn manager(&self) -> Result<NmManager, NetworkError> {
            let state = self.lock();
            let mut manager = state
                .manager
                .clone()
                .ok_or_else(|| NetworkError::Service("no manager".to_string()))?;
            if let Some(enabled) = state.networking_enabled {
                manager.networking_enabled = enabled;
            }
            if let Some(enabled) = state.wireless_enabled {
                manager.wireless_enabled = enabled;
            }
            Ok(manager)
        }

        fn device(&self, path: &str) -> Result<NmDevice, NetworkError> {
            self.lock()
                .devices
                .get(path)
                .cloned()
                .ok_or_else(|| NetworkError::Service(format!("no device {path}")))
        }

        fn wireless(&self, _path: &str) -> Result<NmWireless, NetworkError> {
            self.lock()
                .wireless
                .clone()
                .ok_or_else(|| NetworkError::Service("no wireless".to_string()))
        }

        fn access_point(&self, path: &str) -> Result<NmAccessPoint, NetworkError> {
            self.lock()
                .access_points
                .get(path)
                .cloned()
                .ok_or_else(|| NetworkError::Service(format!("no AP {path}")))
        }

        fn active_connection(&self, path: &str) -> Result<NmActiveConnection, NetworkError> {
            self.lock()
                .active_connections
                .get(path)
                .cloned()
                .ok_or_else(|| NetworkError::Service(format!("no active connection {path}")))
        }

        fn list_connections(&self) -> Result<Vec<String>, NetworkError> {
            Ok(self.lock().profiles.keys().cloned().collect())
        }

        fn connection_settings(&self, path: &str) -> Result<NmSettings, NetworkError> {
            self.lock()
                .profiles
                .get(path)
                .cloned()
                .ok_or_else(|| NetworkError::Service(format!("no profile {path}")))
        }

        fn request_scan(&self, device: &str) -> Result<(), NetworkError> {
            self.lock().scans.push(device.to_string());
            Ok(())
        }

        fn set_networking_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
            self.lock().networking_enabled = Some(enabled);
            Ok(())
        }

        fn set_wireless_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
            self.lock().wireless_enabled = Some(enabled);
            Ok(())
        }

        fn activate_connection(
            &self,
            profile: &str,
            device: &str,
            _specific_object: &str,
        ) -> Result<(), NetworkError> {
            self.lock()
                .activated
                .push((profile.to_string(), device.to_string()));
            Ok(())
        }

        fn add_and_activate(
            &self,
            settings: NmSettings,
            _device: &str,
            _specific_object: &str,
        ) -> Result<(), NetworkError> {
            self.lock().added.push(settings);
            Ok(())
        }

        fn disconnect_device(&self, device: &str) -> Result<(), NetworkError> {
            self.lock().disconnected.push(device.to_string());
            Ok(())
        }

        fn update_connection(&self, path: &str, settings: NmSettings) -> Result<(), NetworkError> {
            self.lock().updated.push((path.to_string(), settings));
            Ok(())
        }

        fn delete_connection(&self, path: &str) -> Result<(), NetworkError> {
            self.lock().deleted.push(path.to_string());
            Ok(())
        }
    }

    fn profile(ssid: &str, auto_connect: bool) -> NmSettings {
        let mut settings = NmSettings::new();
        let connection = setting_mut(&mut settings, "connection");
        connection.insert("autoconnect".to_string(), OwnedVariant::from(auto_connect));
        setting_mut(&mut settings, "802-11-wireless").insert(
            "ssid".to_string(),
            bytes(ssid.as_bytes()).expect("test SSID bytes"),
        );
        settings
    }

    fn ap(
        path: &str,
        ssid: &str,
        strength: u8,
        flags: u32,
        security: u32,
    ) -> (String, NmAccessPoint) {
        (
            path.to_string(),
            NmAccessPoint {
                ssid: ssid.as_bytes().to_vec(),
                strength,
                frequency: 5180,
                flags,
                wpa_flags: security,
                rsn_flags: 0,
            },
        )
    }

    fn basic_state() -> FakeState {
        let mut devices = HashMap::new();
        devices.insert(
            "/devices/1".to_string(),
            NmDevice {
                interface: "wlan0".to_string(),
                device_type: DEVICE_TYPE_WIFI,
                state: DEVICE_STATE_ACTIVATED,
            },
        );
        let mut access_points = HashMap::new();
        let (path, point) = ap("/aps/home", "Home-WiFi-5G", 82, AP_FLAG_PRIVACY, 0x400);
        access_points.insert(path, point);
        let (path, point) = ap("/aps/cafe", "Cafe-Guest-Open", 40, 0, 0);
        access_points.insert(path, point);
        let mut profiles = HashMap::new();
        profiles.insert("/profiles/home".to_string(), profile("Home-WiFi-5G", true));
        FakeState {
            manager: Some(NmManager {
                networking_enabled: true,
                wireless_enabled: true,
                wireless_hardware_enabled: true,
                primary_connection: String::new(),
                primary_connection_type: "802-11-wireless".to_string(),
                devices: vec!["/devices/1".to_string()],
            }),
            devices,
            wireless: Some(NmWireless {
                access_points: vec!["/aps/home".to_string(), "/aps/cafe".to_string()],
                active_access_point: "/aps/home".to_string(),
            }),
            access_points,
            active_connections: HashMap::new(),
            profiles,
            ..FakeState::default()
        }
    }

    #[test]
    fn networkmanager_truth_becomes_one_neutral_snapshot() {
        let api = FakeNm::with_state(basic_state());
        let backend = NetworkManagerBackend::with_api(api, "/devices/1");
        let snapshot = backend.snapshot();
        assert!(snapshot.available);
        assert_eq!(snapshot.backend, BackendKind::NetworkManager);
        assert_eq!(snapshot.network, NetworkState::Wifi);
        assert_eq!(snapshot.network_interface, "wlan0");
        assert_eq!(snapshot.wifi_enabled, Some(true));
        assert_eq!(snapshot.wifi_state, WifiLinkState::Connected);
        assert_eq!(snapshot.wifi_ssid.as_deref(), Some("Home-WiFi-5G"));
        assert_eq!(snapshot.wifi_networks.len(), 2);
        let home = snapshot
            .wifi_networks
            .iter()
            .find(|network| network.ssid == "Home-WiFi-5G")
            .expect("home");
        assert!(home.is_connected);
        assert!(home.is_saved);
        assert!(home.auto_connect);
        assert_eq!(home.security, WifiSecurity::WpaPsk);
        assert_eq!(home.signal_bars, 4);
        assert_eq!(home.frequency_mhz, Some(5180));
    }

    #[test]
    fn duplicate_ssids_collapse_to_one_row_with_the_strongest_signal() {
        let mut state = basic_state();
        let (path, point) = ap("/aps/home-2", "Home-WiFi-5G", 95, AP_FLAG_PRIVACY, 0x400);
        state.access_points.insert(path, point);
        state
            .wireless
            .as_mut()
            .expect("wireless")
            .access_points
            .push("/aps/home-2".to_string());
        let api = FakeNm::with_state(state);
        let backend = NetworkManagerBackend::with_api(api, "/devices/1");
        let snapshot = backend.snapshot();
        let home: Vec<_> = snapshot
            .wifi_networks
            .iter()
            .filter(|network| network.ssid == "Home-WiFi-5G")
            .collect();
        assert_eq!(home.len(), 1);
        assert_eq!(home[0].signal_bars, 4);
        assert!(home[0].is_connected, "the active AP's row stays connected");
    }

    #[test]
    fn an_ssid_with_open_and_secured_aps_is_presented_secured() {
        let mut state = basic_state();
        let (path, point) = ap("/aps/home-open", "Home-WiFi-5G", 99, 0, 0);
        state.access_points.insert(path, point);
        state
            .wireless
            .as_mut()
            .expect("wireless")
            .access_points
            .push("/aps/home-open".to_string());
        let api = FakeNm::with_state(state);
        let backend = NetworkManagerBackend::with_api(api, "/devices/1");
        let home = backend
            .snapshot()
            .wifi_networks
            .into_iter()
            .find(|network| network.ssid == "Home-WiFi-5G")
            .expect("home");
        assert_eq!(
            home.security,
            WifiSecurity::WpaPsk,
            "an evil-twin open AP must never downgrade a secured SSID to open"
        );
    }

    #[test]
    fn a_wpa3_access_point_uses_sae_key_management() {
        let mut state = basic_state();
        let (path, point) = ap(
            "/aps/wpa3",
            "WPA3-Only",
            70,
            AP_FLAG_PRIVACY,
            AP_SEC_KEY_MGMT_SAE,
        );
        state.access_points.insert(path, point);
        state
            .wireless
            .as_mut()
            .expect("wireless")
            .access_points
            .push("/aps/wpa3".to_string());
        let api = FakeNm::with_state(state);
        let backend = NetworkManagerBackend::with_api(api.clone(), "/devices/1");
        backend
            .connect("WPA3-Only", Some("secret"))
            .expect("connect");
        let added = api.lock().added.clone();
        assert_eq!(added.len(), 1);
        let key_mgmt = added[0]
            .get("802-11-wireless-security")
            .and_then(|security| security.get("key-mgmt"))
            .and_then(|value| {
                let value: &Variant = Borrow::<Variant>::borrow(value);
                value.downcast_ref::<&str>().ok()
            });
        assert_eq!(key_mgmt, Some("sae"));
    }

    #[test]
    fn security_flags_never_present_a_secured_network_as_open() {
        assert_eq!(security_from_flags(0, 0), WifiSecurity::Open);
        assert_eq!(
            security_from_flags(AP_FLAG_PRIVACY, 0x100),
            WifiSecurity::WpaPsk
        );
        assert_eq!(
            security_from_flags(AP_FLAG_PRIVACY, 0x400),
            WifiSecurity::WpaPsk
        );
        assert_eq!(
            security_from_flags(AP_FLAG_PRIVACY, AP_SEC_KEY_MGMT_802_1X),
            WifiSecurity::Enterprise
        );
        assert_eq!(
            security_from_flags(AP_FLAG_PRIVACY, AP_SEC_KEY_MGMT_EAP_SUITE_B_192),
            WifiSecurity::Enterprise
        );
        assert_eq!(
            security_from_flags(AP_FLAG_PRIVACY, 0),
            WifiSecurity::WpaPsk,
            "WEP and privacy-only APs are secured, not open"
        );
    }

    #[test]
    fn signal_strength_maps_to_four_bars() {
        assert_eq!(signal_bars(0), 0);
        assert_eq!(signal_bars(1), 1);
        assert_eq!(signal_bars(25), 1);
        assert_eq!(signal_bars(26), 2);
        assert_eq!(signal_bars(50), 2);
        assert_eq!(signal_bars(51), 3);
        assert_eq!(signal_bars(75), 3);
        assert_eq!(signal_bars(76), 4);
        assert_eq!(signal_bars(100), 4);
    }

    #[test]
    fn a_disabled_radio_reports_disabled_and_no_link() {
        let mut state = basic_state();
        state.manager.as_mut().expect("manager").wireless_enabled = false;
        let api = FakeNm::with_state(state);
        let backend = NetworkManagerBackend::with_api(api, "/devices/1");
        let snapshot = backend.snapshot();
        assert_eq!(snapshot.wifi_enabled, Some(false));
        assert_eq!(snapshot.wifi_state, WifiLinkState::Disabled);
        assert_eq!(snapshot.wifi_ssid, None);
        assert_eq!(snapshot.network, NetworkState::Offline);
    }

    #[test]
    fn the_primary_connection_wins_when_wifi_also_has_a_link() {
        let manager = NmManager {
            networking_enabled: true,
            wireless_enabled: true,
            wireless_hardware_enabled: true,
            primary_connection: String::new(),
            primary_connection_type: "802-3-ethernet".to_string(),
            devices: Vec::new(),
        };
        assert_eq!(
            primary_network(&manager, WifiLinkState::Connected),
            NetworkState::Wired,
            "a wired default must not be relabelled Wi-Fi because a station link also exists"
        );
    }

    #[test]
    fn enabling_wifi_also_enables_global_networking() {
        let mut state = basic_state();
        state.manager.as_mut().expect("manager").networking_enabled = false;
        state.networking_enabled = Some(false);
        let api = FakeNm::with_state(state);
        let backend = NetworkManagerBackend::with_api(api.clone(), "/devices/1");
        assert_eq!(backend.snapshot().wifi_enabled, Some(false));
        assert_eq!(backend.snapshot().network, NetworkState::Offline);

        backend.set_enabled(true).expect("enable");
        assert_eq!(api.lock().networking_enabled, Some(true));
        assert_eq!(api.lock().wireless_enabled, Some(true));
        assert_eq!(backend.snapshot().wifi_enabled, Some(true));
    }

    #[test]
    fn a_hardware_blocked_radio_reads_as_disabled() {
        let mut state = basic_state();
        let manager = state.manager.as_mut().expect("manager");
        manager.wireless_enabled = true;
        manager.wireless_hardware_enabled = false;
        let api = FakeNm::with_state(state);
        let backend = NetworkManagerBackend::with_api(api, "/devices/1");
        let snapshot = backend.snapshot();
        assert_eq!(snapshot.wifi_enabled, Some(false));
        assert_eq!(snapshot.wifi_state, WifiLinkState::Disabled);
        assert_eq!(snapshot.wifi_ssid, None);
    }

    #[test]
    fn object_path_arrays_are_read_without_leaking_zbus_types() {
        let mut properties = HashMap::new();
        let paths = vec![
            ObjectPath::try_from("/devices/1").expect("path"),
            ObjectPath::try_from("/devices/2").expect("path"),
        ];
        let value = Variant::Array(zbus::Array::from(paths));
        properties.insert(
            "Devices".to_string(),
            OwnedVariant::try_from(value).expect("owned array"),
        );
        assert_eq!(
            get_object_paths(&properties, "Devices"),
            vec!["/devices/1".to_string(), "/devices/2".to_string()]
        );
    }

    #[test]
    fn connect_activates_a_saved_profile_and_adds_an_unknown_one() {
        let mut state = basic_state();
        let (path, point) = ap("/aps/new", "New-Network", 55, AP_FLAG_PRIVACY, 0x100);
        state.access_points.insert(path, point);
        state
            .wireless
            .as_mut()
            .expect("wireless")
            .access_points
            .push("/aps/new".to_string());
        let api = FakeNm::with_state(state);
        let backend = NetworkManagerBackend::with_api(api.clone(), "/devices/1");
        backend
            .connect("Home-WiFi-5G", Some("secret"))
            .expect("activate saved");
        assert_eq!(
            api.lock().activated,
            vec![("/profiles/home".to_string(), "/devices/1".to_string())]
        );
        assert!(api.lock().added.is_empty());

        backend
            .connect("New-Network", Some("secret"))
            .expect("add new");
        let added = api.lock().added.clone();
        assert_eq!(added.len(), 1);
        assert_eq!(settings_ssid(&added[0]).as_deref(), Some("New-Network"));
        assert_eq!(
            added[0]
                .get("802-11-wireless-security")
                .and_then(|security| security.get("key-mgmt"))
                .and_then(|value| {
                    let value: &Variant = Borrow::<Variant>::borrow(value);
                    value.downcast_ref::<&str>().ok()
                }),
            Some("wpa-psk")
        );
    }

    #[test]
    fn enterprise_connect_is_refused_because_the_verb_carries_no_identity() {
        let mut state = basic_state();
        let (path, point) = ap(
            "/aps/enterprise",
            "Enterprise-Office",
            60,
            AP_FLAG_PRIVACY,
            AP_SEC_KEY_MGMT_802_1X,
        );
        state.access_points.insert(path, point);
        state
            .wireless
            .as_mut()
            .expect("wireless")
            .access_points
            .push("/aps/enterprise".to_string());
        let api = FakeNm::with_state(state);
        let backend = NetworkManagerBackend::with_api(api, "/devices/1");
        let error = backend
            .connect("Enterprise-Office", Some("secret"))
            .expect_err("enterprise needs identity");
        assert!(matches!(error, NetworkError::Unsupported(_)));
    }

    #[test]
    fn auto_connect_updates_every_profile_and_forget_deletes_every_profile() {
        let mut state = basic_state();
        state.profiles.insert(
            "/profiles/home-2".to_string(),
            profile("Home-WiFi-5G", false),
        );
        let api = FakeNm::with_state(state);
        let backend = NetworkManagerBackend::with_api(api.clone(), "/devices/1");
        backend
            .set_auto_connect("Home-WiFi-5G", false)
            .expect("update");
        assert_eq!(api.lock().updated.len(), 2);

        backend.forget("Home-WiFi-5G").expect("forget");
        let mut deleted = api.lock().deleted.clone();
        deleted.sort();
        assert_eq!(
            deleted,
            vec!["/profiles/home".to_string(), "/profiles/home-2".to_string()]
        );
    }

    #[test]
    fn a_failed_refresh_reports_unavailable_instead_of_last_known_state() {
        let api = FakeNm::with_state(FakeState::default());
        let backend = NetworkManagerBackend::assemble(api, "/devices/1".to_string());
        assert!(backend.refresh().is_err());
        let snapshot = backend.snapshot();
        assert!(!snapshot.available);
        assert_eq!(snapshot.wifi_enabled, None);
        assert!(snapshot.wifi_networks.is_empty());
        assert!(snapshot.last_error.is_some());
    }

    #[test]
    fn scan_and_disconnect_reach_only_the_wifi_device() {
        let api = FakeNm::with_state(basic_state());
        let backend = NetworkManagerBackend::with_api(api.clone(), "/devices/1");
        backend.scan().expect("scan");
        backend.disconnect().expect("disconnect");
        assert_eq!(api.lock().scans, vec!["/devices/1".to_string()]);
        assert_eq!(api.lock().disconnected, vec!["/devices/1".to_string()]);
    }

    #[test]
    fn a_replaced_wifi_device_is_adopted_without_a_restart() {
        let api = FakeNm::with_state(basic_state());
        let backend = NetworkManagerBackend::with_api(api.clone(), "/devices/1");
        assert_eq!(backend.device_path(), "/devices/1");

        // The USB adapter is unplugged and re-enumerated as `/devices/2`;
        // its old object path is reused by an Ethernet device.
        {
            let mut state = api.lock();
            state.devices.insert(
                "/devices/1".to_string(),
                NmDevice {
                    interface: "enp0s31f6".to_string(),
                    device_type: DEVICE_TYPE_ETHERNET,
                    state: DEVICE_STATE_ACTIVATED,
                },
            );
            state.devices.insert(
                "/devices/2".to_string(),
                NmDevice {
                    interface: "wlan1".to_string(),
                    device_type: DEVICE_TYPE_WIFI,
                    state: DEVICE_STATE_ACTIVATED,
                },
            );
            state.manager.as_mut().expect("manager").devices =
                vec!["/devices/1".to_string(), "/devices/2".to_string()];
        }

        backend.refresh().expect("adopt the replacement device");
        let snapshot = backend.snapshot();
        assert!(snapshot.available);
        assert_eq!(snapshot.network_interface, "wlan1");
        assert_eq!(backend.device_path(), "/devices/2");
    }

    #[test]
    #[ignore = "requires NetworkManager and permission to create a transient profile"]
    fn live_networkmanager_profile_commands_update_and_delete() {
        use std::process::Command;

        let name = "tessera-live-test";
        let ssid = "tessera-live-test";
        let nmcli = |args: &[&str]| {
            Command::new("nmcli")
                .args(args)
                .output()
                .expect("nmcli must be installed")
        };

        struct Cleanup;
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = Command::new("nmcli")
                    .args(["connection", "delete", "id", "tessera-live-test"])
                    .output();
            }
        }
        let _cleanup = Cleanup;

        let created = nmcli(&[
            "connection",
            "add",
            "type",
            "wifi",
            "con-name",
            name,
            "ssid",
            ssid,
            "autoconnect",
            "no",
        ]);
        if !created.status.success() {
            let stderr = String::from_utf8_lossy(&created.stderr);
            if stderr.contains("Insufficient privileges") {
                eprintln!("no polkit agent/permission to create a profile; skipping");
                return;
            }
            panic!("could not create the transient profile: {stderr}");
        }

        let backend = NetworkManagerBackend::open().expect("live NetworkManager");
        backend
            .set_auto_connect(ssid, true)
            .expect("update the transient profile");
        let updated = nmcli(&[
            "-g",
            "connection.autoconnect",
            "connection",
            "show",
            "id",
            name,
        ]);
        assert_eq!(
            String::from_utf8_lossy(&updated.stdout).trim(),
            "yes",
            "real Update did not set autoconnect"
        );

        backend.forget(ssid).expect("delete the transient profile");
        let listed = nmcli(&["-g", "NAME", "connection", "show"]);
        let names = String::from_utf8_lossy(&listed.stdout);
        assert!(
            !names.lines().any(|line| line == name),
            "real Delete left the transient profile behind: {names}"
        );
    }
}
