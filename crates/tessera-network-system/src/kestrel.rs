//! Kestrel (`com.hihusky.Kestrel`) backend over the system D-Bus.
//!
//! Kestrel is an out-of-process wireless control-plane orchestrator that drives
//! `wpa_supplicant` or `iwd` alongside `systemd-networkd`. Like the `iwd`
//! backend, Kestrel owns wireless links only; wired link detection remains
//! with the compositor's sysfs probe (`network_authoritative = false`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tessera_dbus::{
    Connection, FromArgs, FromValue, Message, MessageStream, OwnedObjectPath, ToArgs, Value,
};
use tessera_desktop::system::{NetworkState, WifiLinkState, WifiNetwork, WifiSecurity};
use tessera_network::{BackendKind, NetworkBackend, NetworkError, NetworkSnapshot};

const KESTREL_SERVICE: &str = "com.hihusky.Kestrel";
const KESTREL_ROOT: &str = "/com/hihusky/Kestrel";
const API_VERSION: &str = "1";

const MANAGER_IFACE: &str = "com.hihusky.Kestrel.Manager";
const DEVICE_IFACE: &str = "com.hihusky.Kestrel.Device";
const PROFILE_IFACE: &str = "com.hihusky.Kestrel.Profile";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_IFACE: &str = "org.freedesktop.DBus";

/// How long an initiated scan stays reflected as `Scanning` if the daemon has
/// not yet flipped `Device.Scanning` or emitted a signal.
const SCAN_VISIBLE: Duration = Duration::from_secs(6);
const RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default)]
struct KestrelManager {
    version: String,
    wireless_enabled: bool,
    wireless_hardware_enabled: bool,
}

#[derive(Debug, Clone, Default)]
struct KestrelDevice {
    path: String,
    interface: String,
    capabilities: Vec<String>,
    supported_security: Vec<String>,
    state: String,
    ssid: String,
    scanning: bool,
    autoconnect_held: bool,
}

#[derive(Debug, Clone)]
struct KestrelScanEntry {
    ssid: String,
    frequency_mhz: u32,
    signal_dbm: i16,
    security: String,
    profile_path: String,
}

#[derive(Debug, Clone)]
struct KestrelProfile {
    path: String,
    ssid: String,
    security: String,
    autoconnect: bool,
    priority: i32,
}

#[derive(Debug, Clone, Default)]
struct KestrelObjects {
    manager: Option<KestrelManager>,
    devices: Vec<KestrelDevice>,
    profiles: Vec<KestrelProfile>,
}

/// The narrow Kestrel D-Bus surface consumed by [`KestrelBackend`], split out
/// so state translation and command logic are unit-testable without a live bus.
trait KestrelApi: Send + Sync {
    fn managed_objects(&self) -> Result<KestrelObjects, NetworkError>;
    fn scan_results(&self, device_path: &str) -> Result<Vec<KestrelScanEntry>, NetworkError>;
    fn scan(&self, device_path: &str) -> Result<(), NetworkError>;
    fn connect(&self, device_path: &str, profile_path: &str) -> Result<(), NetworkError>;
    fn disconnect(&self, device_path: &str) -> Result<(), NetworkError>;
    fn set_autoconnect(&self, device_path: &str, enabled: bool) -> Result<(), NetworkError>;
    fn set_wireless_enabled(&self, enabled: bool) -> Result<(), NetworkError>;
    fn add_profile(
        &self,
        settings: HashMap<String, Value>,
        secret: &str,
    ) -> Result<String, NetworkError>;
    fn update_profile(
        &self,
        profile_path: &str,
        settings: HashMap<String, Value>,
        secret_action: &str,
        secret: &str,
    ) -> Result<(), NetworkError>;
    fn delete_profile(&self, profile_path: &str) -> Result<(), NetworkError>;
}

struct ZbusKestrelApi {
    connection: Connection,
}

impl KestrelApi for ZbusKestrelApi {
    fn managed_objects(&self) -> Result<KestrelObjects, NetworkError> {
        // Per the D-Bus ObjectManager specification, GetManagedObjects only
        // returns descendant paths strictly below the ObjectManager root, so
        // `com.hihusky.Kestrel.Manager` on `/com/hihusky/Kestrel` must be
        // queried directly through `org.freedesktop.DBus.Properties.GetAll`.
        let manager_reply = blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            KESTREL_ROOT,
            Some(PROPERTIES_IFACE),
            "GetAll",
            &(MANAGER_IFACE,),
        )
        .map_err(|error| map_call("Manager.GetAll", error))?;
        let manager_props: HashMap<String, Value> = FromArgs::from_args(manager_reply.body)
            .map_err(|error| NetworkError::Service(format!("Manager.GetAll body: {error}")))?;
        let manager = KestrelManager {
            version: get_str(&manager_props, "Version").unwrap_or_default(),
            wireless_enabled: get_bool(&manager_props, "WirelessEnabled").unwrap_or(true),
            wireless_hardware_enabled: get_bool(&manager_props, "WirelessHardwareEnabled")
                .unwrap_or(true),
        };

        let reply = blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            KESTREL_ROOT,
            Some(OBJECT_MANAGER_IFACE),
            "GetManagedObjects",
            &(),
        )
        .map_err(|error| map_call("GetManagedObjects", error))?;

        type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, Value>>>;
        let objects: ManagedObjects = FromArgs::from_args(reply.body)
            .map_err(|error| NetworkError::Service(format!("GetManagedObjects body: {error}")))?;

        let mut parsed = KestrelObjects {
            manager: Some(manager),
            devices: Vec::new(),
            profiles: Vec::new(),
        };
        for (path, interfaces) in objects {
            let path_str = path.into_inner();
            if let Some(props) = interfaces.get(DEVICE_IFACE) {
                parsed.devices.push(KestrelDevice {
                    path: path_str.clone(),
                    interface: get_str(props, "Interface").unwrap_or_default(),
                    capabilities: get_strings(props, "Capabilities"),
                    supported_security: get_strings(props, "SupportedSecurity"),
                    state: get_str(props, "State").unwrap_or_default(),
                    ssid: get_str(props, "Ssid").unwrap_or_default(),
                    scanning: get_bool(props, "Scanning").unwrap_or(false),
                    autoconnect_held: get_bool(props, "AutoconnectHeld").unwrap_or(false),
                });
            }
            if let Some(props) = interfaces.get(PROFILE_IFACE)
                && let Some(settings) = get_dict(props, "Settings")
                && let Some(ssid) = get_str(&settings, "ssid")
            {
                parsed.profiles.push(KestrelProfile {
                    path: path_str,
                    ssid,
                    security: get_str(&settings, "security").unwrap_or_default(),
                    autoconnect: get_bool(&settings, "autoconnect").unwrap_or(true),
                    priority: get_i32(&settings, "priority").unwrap_or(0),
                });
            }
        }
        parsed.devices.sort_by(|a, b| a.interface.cmp(&b.interface));
        parsed.profiles.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(parsed)
    }

    fn scan_results(&self, device_path: &str) -> Result<Vec<KestrelScanEntry>, NetworkError> {
        let reply = blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            device_path,
            Some(DEVICE_IFACE),
            "GetScanResults",
            &(),
        )
        .map_err(|error| map_call("GetScanResults", error))?;

        let raw: Vec<Value> = FromArgs::from_args(reply.body)
            .map_err(|error| NetworkError::Service(format!("GetScanResults body: {error}")))?;
        let mut entries = Vec::with_capacity(raw.len());
        for item in raw {
            let Value::Struct(fields) = item.unwrap_variant().clone() else {
                return Err(NetworkError::Service(
                    "GetScanResults: expected tuple struct".to_string(),
                ));
            };
            let [
                raw_ssid,
                raw_bssid,
                raw_freq,
                raw_signal,
                raw_security,
                raw_profile,
            ]: [Value; 6] = fields.try_into().map_err(|fields: Vec<Value>| {
                NetworkError::Service(format!(
                    "GetScanResults: expected 6 fields, got {}",
                    fields.len()
                ))
            })?;
            let ssid = String::from_value(raw_ssid)
                .map_err(|e| NetworkError::Service(format!("GetScanResults ssid: {e}")))?;
            let _bssid = String::from_value(raw_bssid)
                .map_err(|e| NetworkError::Service(format!("GetScanResults bssid: {e}")))?;
            let frequency_mhz = u32::from_value(raw_freq)
                .map_err(|e| NetworkError::Service(format!("GetScanResults freq: {e}")))?;
            let signal_dbm = i16::from_value(raw_signal)
                .map_err(|e| NetworkError::Service(format!("GetScanResults signal: {e}")))?;
            let security = String::from_value(raw_security)
                .map_err(|e| NetworkError::Service(format!("GetScanResults security: {e}")))?;
            let profile_path = OwnedObjectPath::from_value(raw_profile)
                .map_err(|e| NetworkError::Service(format!("GetScanResults profile: {e}")))?
                .into_inner();
            entries.push(KestrelScanEntry {
                ssid,
                frequency_mhz,
                signal_dbm,
                security,
                profile_path,
            });
        }
        Ok(entries)
    }

    fn scan(&self, device_path: &str) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            device_path,
            Some(DEVICE_IFACE),
            "Scan",
            &(),
        )
        .map_err(|error| map_call("Scan", error))?;
        Ok(())
    }

    fn connect(&self, device_path: &str, profile_path: &str) -> Result<(), NetworkError> {
        let profile = object_path(profile_path)?;
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            device_path,
            Some(DEVICE_IFACE),
            "Connect",
            &(profile,),
        )
        .map_err(|error| map_call("Connect", error))?;
        Ok(())
    }

    fn disconnect(&self, device_path: &str) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            device_path,
            Some(DEVICE_IFACE),
            "Disconnect",
            &(),
        )
        .map_err(|error| map_call("Disconnect", error))?;
        Ok(())
    }

    fn set_autoconnect(&self, device_path: &str, enabled: bool) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            device_path,
            Some(DEVICE_IFACE),
            "SetAutoconnect",
            &(enabled,),
        )
        .map_err(|error| map_call("SetAutoconnect", error))?;
        Ok(())
    }

    fn set_wireless_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            KESTREL_ROOT,
            Some(MANAGER_IFACE),
            "SetWirelessEnabled",
            &(enabled,),
        )
        .map_err(|error| map_call("SetWirelessEnabled", error))?;
        Ok(())
    }

    fn add_profile(
        &self,
        settings: HashMap<String, Value>,
        secret: &str,
    ) -> Result<String, NetworkError> {
        let secret_map = secret_bundle(secret);
        let reply = blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            KESTREL_ROOT,
            Some(MANAGER_IFACE),
            "AddProfile",
            &(settings, secret_map),
        )
        .map_err(|error| map_call("AddProfile", error))?;
        let path: OwnedObjectPath = FromArgs::from_args(reply.body)
            .map_err(|error| NetworkError::Service(format!("AddProfile body: {error}")))?;
        Ok(path.into_inner())
    }

    fn update_profile(
        &self,
        profile_path: &str,
        settings: HashMap<String, Value>,
        secret_action: &str,
        secret: &str,
    ) -> Result<(), NetworkError> {
        let secret_map = secret_bundle(secret);
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            profile_path,
            Some(PROFILE_IFACE),
            "Update",
            &(settings, secret_action, secret_map),
        )
        .map_err(|error| map_call("Profile.Update", error))?;
        Ok(())
    }

    fn delete_profile(&self, profile_path: &str) -> Result<(), NetworkError> {
        blocking_call(
            &self.connection,
            Some(KESTREL_SERVICE),
            profile_path,
            Some(PROFILE_IFACE),
            "Delete",
            &(),
        )
        .map_err(|error| map_call("Profile.Delete", error))?;
        Ok(())
    }
}

pub struct KestrelBackend {
    api: Arc<dyn KestrelApi>,
    device: Mutex<String>,
    cache: Mutex<NetworkSnapshot>,
    scan_security: Mutex<HashMap<String, String>>,
    dirty: Arc<AtomicBool>,
    scan_until: Mutex<Option<Instant>>,
    retry_at: Mutex<Option<Instant>>,
}

impl KestrelBackend {
    /// Open Kestrel over the system D-Bus and take one live snapshot.
    ///
    /// Kestrel must already own its bus name. Selection never D-Bus-activates
    /// it: a stopped service is an operator decision.
    pub fn open() -> Result<Self, NetworkError> {
        let connection = Connection::system().map_err(|_| NetworkError::Unavailable)?;
        if !name_has_owner(&connection, KESTREL_SERVICE)? {
            return Err(NetworkError::Unavailable);
        }
        let api: Arc<dyn KestrelApi> = Arc::new(ZbusKestrelApi {
            connection: connection.clone(),
        });
        // Once Kestrel owns the bus name, any failure to answer GetManagedObjects
        // is "present but unusable", never "absent": falling back to NetworkManager
        // or iwd while Kestrel is running would contend for the same adapter.
        let objects = api
            .managed_objects()
            .map_err(|error| NetworkError::Service(format!("Kestrel did not answer: {error}")))?;
        let manager = objects.manager.as_ref().ok_or_else(|| {
            NetworkError::Service("Kestrel did not export Manager interface".to_string())
        })?;
        check_api_version(&manager.version)?;
        let device = select_station_device(&objects.devices, None)?.path.clone();
        let backend = Self::assemble(api, device);
        backend.refresh()?;
        if start_signal_thread(connection, backend.dirty.clone()) {
            log::debug!("network service: Kestrel signal subscription active");
        } else {
            log::warn!(
                "network service: Kestrel signal subscription did not start; \
                 state will update on command and retry only"
            );
        }
        Ok(backend)
    }

    fn assemble(api: Arc<dyn KestrelApi>, device: String) -> Self {
        Self {
            api,
            device: Mutex::new(device),
            cache: Mutex::new(NetworkSnapshot::unavailable(
                BackendKind::Kestrel,
                "Kestrel has not answered yet",
            )),
            scan_security: Mutex::new(HashMap::new()),
            dirty: Arc::new(AtomicBool::new(false)),
            scan_until: Mutex::new(None),
            retry_at: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_api(api: Arc<dyn KestrelApi>, device: &str) -> Self {
        let backend = Self::assemble(api, device.to_string());
        backend.refresh().expect("test Kestrel refresh");
        backend
    }

    fn device_path(&self) -> String {
        self.device
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn read_snapshot(&self) -> Result<NetworkSnapshot, NetworkError> {
        let objects = self.api.managed_objects()?;
        let manager = objects.manager.as_ref().ok_or_else(|| {
            NetworkError::Service("Kestrel did not export Manager interface".to_string())
        })?;
        check_api_version(&manager.version)?;

        let tracked = self.device_path();
        let device = select_station_device(&objects.devices, Some(&tracked))?;
        if device.path != tracked {
            log::info!("network service: adopted Kestrel device {}", device.path);
            *self
                .device
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = device.path.clone();
        }

        let scan_entries = if manager.wireless_enabled && manager.wireless_hardware_enabled {
            self.api.scan_results(&device.path)?
        } else {
            Vec::new()
        };

        let wifi_enabled = manager.wireless_enabled && manager.wireless_hardware_enabled;
        let scan_active = device.scanning
            || self
                .scan_until
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_some_and(|deadline| Instant::now() < deadline);

        let wifi_state = map_link_state(wifi_enabled, &device.state, scan_active);
        let active_ssid = (wifi_enabled && !device.ssid.is_empty()).then(|| device.ssid.clone());

        let mut profiles_by_ssid: HashMap<&str, Vec<&KestrelProfile>> = HashMap::new();
        for profile in &objects.profiles {
            profiles_by_ssid
                .entry(profile.ssid.as_str())
                .or_default()
                .push(profile);
        }

        let mut networks_by_ssid: HashMap<String, (WifiNetwork, String, i16)> = HashMap::new();
        for entry in scan_entries {
            if entry.ssid.is_empty() {
                continue;
            }
            let matching = profiles_by_ssid.get(entry.ssid.as_str());
            let is_saved = matching.is_some_and(|list| !list.is_empty())
                || (entry.profile_path != "/" && !entry.profile_path.is_empty());
            let auto_connect = matching.is_some_and(|list| list.iter().any(|p| p.autoconnect));
            let is_connected = wifi_state == WifiLinkState::Connected
                && active_ssid.as_deref() == Some(entry.ssid.as_str());
            let signal_bars = dbm_to_bars(entry.signal_dbm);
            let security = map_bss_security(&entry.security);
            let candidate = WifiNetwork {
                ssid: entry.ssid.clone(),
                signal_bars,
                security,
                is_connected,
                is_saved,
                auto_connect,
                frequency_mhz: (entry.frequency_mhz > 0).then_some(entry.frequency_mhz),
            };
            match networks_by_ssid.get_mut(&entry.ssid) {
                Some((existing, existing_raw_sec, best_dbm)) => {
                    existing.security = merge_security(existing.security, candidate.security);
                    existing.is_connected |= candidate.is_connected;
                    existing.is_saved |= candidate.is_saved;
                    existing.auto_connect |= candidate.auto_connect;
                    if entry.signal_dbm > *best_dbm {
                        existing.signal_bars = candidate.signal_bars;
                        existing.frequency_mhz = candidate.frequency_mhz;
                        *existing_raw_sec = entry.security;
                        *best_dbm = entry.signal_dbm;
                    }
                }
                None => {
                    networks_by_ssid
                        .insert(entry.ssid, (candidate, entry.security, entry.signal_dbm));
                }
            }
        }

        let mut raw_security = HashMap::with_capacity(networks_by_ssid.len());
        let mut networks = Vec::with_capacity(networks_by_ssid.len());
        for (ssid, (network, sec, _)) in networks_by_ssid {
            raw_security.insert(ssid, sec);
            networks.push(network);
        }
        networks.sort_by(|left, right| {
            right
                .is_connected
                .cmp(&left.is_connected)
                .then_with(|| right.signal_bars.cmp(&left.signal_bars))
                .then_with(|| left.ssid.cmp(&right.ssid))
        });

        *self
            .scan_security
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = raw_security;

        let network = if wifi_state == WifiLinkState::Connected {
            NetworkState::Wifi
        } else {
            NetworkState::Offline
        };

        Ok(NetworkSnapshot {
            available: true,
            backend: BackendKind::Kestrel,
            network,
            // Kestrel manages wireless interfaces only; the compositor keeps
            // its sysfs link probe for coarse wired/offline status.
            network_authoritative: false,
            network_interface: device.interface.clone(),
            wifi_enabled: Some(wifi_enabled),
            wifi_state,
            wifi_ssid: if wifi_state == WifiLinkState::Connected {
                active_ssid
            } else {
                None
            },
            wifi_networks: networks,
            last_error: None,
        })
    }

    fn resolve_state(&self) -> Result<(KestrelDevice, Vec<KestrelProfile>), NetworkError> {
        let objects = self.api.managed_objects()?;
        let manager = objects.manager.as_ref().ok_or_else(|| {
            NetworkError::Service("Kestrel did not export Manager interface".to_string())
        })?;
        check_api_version(&manager.version)?;
        let tracked = self.device_path();
        let device = select_station_device(&objects.devices, Some(&tracked))?.clone();
        if device.path != tracked {
            *self
                .device
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = device.path.clone();
        }
        Ok((device, objects.profiles))
    }
}

impl NetworkBackend for KestrelBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Kestrel
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
        let (device, _) = self.resolve_state()?;
        self.api.scan(&device.path)?;
        *self
            .scan_until
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Instant::now() + SCAN_VISIBLE);
        let _ = self.refresh();
        Ok(())
    }

    fn connect(&self, ssid: &str, passphrase: Option<&str>) -> Result<(), NetworkError> {
        self.refresh()?;
        let (device, profiles) = self.resolve_state()?;

        let mut matching: Vec<KestrelProfile> =
            profiles.into_iter().filter(|p| p.ssid == ssid).collect();
        matching.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.path.cmp(&b.path))
        });

        if let Some(profile) = matching.first() {
            if let Some(secret) = passphrase
                && !secret.is_empty()
            {
                let mut updates = HashMap::new();
                if profile.security == "open" || profile.security == "owe" {
                    updates.insert(
                        "security".to_string(),
                        Value::Variant(Box::new(Value::String("wpa-personal".to_string()))),
                    );
                }
                self.api
                    .update_profile(&profile.path, updates, "set", secret)?;
            }
            self.api.connect(&device.path, &profile.path)?;
            let _ = self.refresh();
            return Ok(());
        }

        let raw_sec = self
            .scan_security
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(ssid)
            .cloned()
            .ok_or_else(|| NetworkError::NetworkNotFound(ssid.to_string()))?;

        let profile_security = profile_security_for_bss(&raw_sec, &device.supported_security)?;
        let secret = if profile_security == "open" || profile_security == "owe" {
            ""
        } else {
            let secret = passphrase.unwrap_or("");
            if secret.is_empty() {
                return Err(NetworkError::Refused(format!(
                    "passphrase required for {ssid}"
                )));
            }
            secret
        };

        let mut settings = HashMap::new();
        settings.insert(
            "ssid".to_string(),
            Value::Variant(Box::new(Value::String(ssid.to_string()))),
        );
        settings.insert(
            "security".to_string(),
            Value::Variant(Box::new(Value::String(profile_security.to_string()))),
        );
        let profile_path = self.api.add_profile(settings, secret)?;
        self.api.connect(&device.path, &profile_path)?;
        let _ = self.refresh();
        Ok(())
    }

    fn disconnect(&self) -> Result<(), NetworkError> {
        let (device, _) = self.resolve_state()?;
        self.api.disconnect(&device.path)?;
        let _ = self.refresh();
        Ok(())
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
        self.api.set_wireless_enabled(enabled)?;
        let _ = self.refresh();
        Ok(())
    }

    fn forget(&self, ssid: &str) -> Result<(), NetworkError> {
        let (_, profiles) = self.resolve_state()?;
        let matching: Vec<KestrelProfile> =
            profiles.into_iter().filter(|p| p.ssid == ssid).collect();
        if matching.is_empty() {
            return Err(NetworkError::NetworkNotFound(ssid.to_string()));
        }
        for profile in matching {
            self.api.delete_profile(&profile.path)?;
        }
        let _ = self.refresh();
        Ok(())
    }

    fn set_auto_connect(&self, ssid: &str, auto_connect: bool) -> Result<(), NetworkError> {
        let (device, profiles) = self.resolve_state()?;
        let matching: Vec<KestrelProfile> =
            profiles.into_iter().filter(|p| p.ssid == ssid).collect();
        if matching.is_empty() {
            return Err(NetworkError::NetworkNotFound(ssid.to_string()));
        }
        for profile in matching {
            let mut settings = HashMap::new();
            settings.insert(
                "autoconnect".to_string(),
                Value::Variant(Box::new(Value::Bool(auto_connect))),
            );
            self.api
                .update_profile(&profile.path, settings, "keep", "")?;
        }
        if auto_connect && device.autoconnect_held && device.ssid == ssid {
            let _ = self.api.set_autoconnect(&device.path, true);
        }
        let _ = self.refresh();
        Ok(())
    }
}

fn check_api_version(version: &str) -> Result<(), NetworkError> {
    if version == API_VERSION {
        Ok(())
    } else {
        Err(NetworkError::Service(format!(
            "unsupported Kestrel API version '{version}' (expected '{API_VERSION}')"
        )))
    }
}

fn select_station_device<'a>(
    devices: &'a [KestrelDevice],
    preferred_path: Option<&str>,
) -> Result<&'a KestrelDevice, NetworkError> {
    if let Some(preferred) = preferred_path
        && let Some(device) = devices
            .iter()
            .find(|d| d.path == preferred && d.capabilities.iter().any(|c| c == "station"))
    {
        return Ok(device);
    }
    devices
        .iter()
        .find(|d| d.capabilities.iter().any(|c| c == "station"))
        .ok_or_else(|| {
            NetworkError::Unsupported("Kestrel reported no wireless station device".to_string())
        })
}

fn map_link_state(wifi_enabled: bool, state: &str, scan_active: bool) -> WifiLinkState {
    if !wifi_enabled || state == "disabled" {
        return WifiLinkState::Disabled;
    }
    match state {
        "connected" => WifiLinkState::Connected,
        "associating" | "configuring-l3" | "reconnecting" => WifiLinkState::Connecting,
        _ if scan_active => WifiLinkState::Scanning,
        _ => WifiLinkState::Disconnected,
    }
}

fn dbm_to_bars(dbm: i16) -> u8 {
    match dbm {
        -55..=i16::MAX => 4,
        -67..=-56 => 3,
        -77..=-68 => 2,
        -88..=-78 => 1,
        _ => 0,
    }
}

fn map_bss_security(security: &str) -> WifiSecurity {
    match security {
        // OWE (Enhanced Open) is encrypted: it keeps no passphrase, but it is
        // not an open network and must never be presented as one.
        "open" => WifiSecurity::Open,
        "enterprise" | "wpa3-enterprise" => WifiSecurity::Enterprise,
        "owe" | "wpa-psk" | "wpa2-psk" | "wpa2-wpa3" | "wpa3-sae" | "wep" => WifiSecurity::WpaPsk,
        // An unknown token is reported as secured. Guessing "open" would
        // understate an AP's protection; the connect path still refuses what
        // it cannot build.
        _ => WifiSecurity::WpaPsk,
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

fn profile_security_for_bss(
    bss_security: &str,
    supported: &[String],
) -> Result<&'static str, NetworkError> {
    let supports = |mode: &str| supported.is_empty() || supported.iter().any(|s| s == mode);
    match bss_security {
        "open" => Ok("open"),
        "owe" => {
            if supports("owe") {
                Ok("owe")
            } else {
                Err(NetworkError::Unsupported(
                    "Kestrel's active backend does not support OWE".to_string(),
                ))
            }
        }
        "wpa-psk" | "wpa2-psk" | "wpa2-wpa3" => Ok("wpa-personal"),
        "wpa3-sae" => {
            if supports("wpa3-personal") {
                Ok("wpa3-personal")
            } else if supports("wpa-personal") {
                Ok("wpa-personal")
            } else {
                Err(NetworkError::Unsupported(
                    "Kestrel's active backend does not support WPA3-Personal".to_string(),
                ))
            }
        }
        "enterprise" | "wpa3-enterprise" => Err(NetworkError::Unsupported(
            "802.1X enterprise networks must be configured via Kestrel".to_string(),
        )),
        other => Err(NetworkError::Unsupported(format!(
            "unsupported wireless security '{other}'"
        ))),
    }
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
    FromArgs::from_args(reply.body)
        .map_err(|error| NetworkError::Service(format!("NameHasOwner body: {error}")))
}

fn start_signal_thread(connection: Connection, dirty: Arc<AtomicBool>) -> bool {
    std::thread::Builder::new()
        .name("tessera-kestrel-signals".into())
        .spawn(move || {
            let stream = match MessageStream::for_match_rule(
                &connection,
                "type='signal',sender='com.hihusky.Kestrel'",
            ) {
                Ok(stream) => stream,
                Err(error) => {
                    log::warn!("network service: could not subscribe to Kestrel signals: {error}");
                    return;
                }
            };
            while stream.next().is_some() {
                dirty.store(true, Ordering::Relaxed);
            }
        })
        .is_ok()
}

fn map_call(operation: &str, error: tessera_dbus::Error) -> NetworkError {
    if let tessera_dbus::Error::MethodError { name, detail } = &error {
        let msg = detail.as_deref().unwrap_or(name.as_str());
        return match name.as_str() {
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "com.hihusky.Kestrel.Error.Stopped" => NetworkError::Unavailable,
            "org.freedesktop.DBus.Error.AccessDenied"
            | "com.hihusky.Kestrel.Error.AccessDenied" => NetworkError::Refused(msg.to_string()),
            "com.hihusky.Kestrel.Error.NotSupported" => NetworkError::Unsupported(msg.to_string()),
            "com.hihusky.Kestrel.Error.InvalidArgument"
            | "com.hihusky.Kestrel.Error.WirelessDisabled" => {
                NetworkError::Refused(msg.to_string())
            }
            "com.hihusky.Kestrel.Error.NoSuchProfile"
            | "com.hihusky.Kestrel.Error.NoSuchDevice" => {
                NetworkError::NetworkNotFound(msg.to_string())
            }
            _ => NetworkError::Service(format!("{operation}: {error}")),
        };
    }
    NetworkError::Service(format!("{operation}: {error}"))
}

fn secret_bundle(secret: &str) -> HashMap<&'static str, &str> {
    let mut map = HashMap::new();
    if !secret.is_empty() {
        map.insert("passphrase", secret);
    }
    map
}

fn object_path(value: &str) -> Result<OwnedObjectPath, NetworkError> {
    OwnedObjectPath::try_from(value)
        .map_err(|error| NetworkError::Service(format!("invalid object path '{value}': {error}")))
}

fn get_str(properties: &HashMap<String, Value>, key: &str) -> Option<String> {
    properties.get(key)?.as_str().map(str::to_string)
}

fn get_bool(properties: &HashMap<String, Value>, key: &str) -> Option<bool> {
    properties.get(key)?.as_bool()
}

fn get_i32(properties: &HashMap<String, Value>, key: &str) -> Option<i32> {
    properties.get(key)?.as_i32()
}

fn get_strings(properties: &HashMap<String, Value>, key: &str) -> Vec<String> {
    let Some(value) = properties.get(key) else {
        return Vec::new();
    };
    let Value::Array(_, items) = value.unwrap_variant() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

fn get_dict(properties: &HashMap<String, Value>, key: &str) -> Option<HashMap<String, Value>> {
    let value = properties.get(key)?.unwrap_variant().clone();
    HashMap::<String, Value>::from_value(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeState {
        manager: KestrelManager,
        devices: Vec<KestrelDevice>,
        scan_results: Vec<KestrelScanEntry>,
        profiles: Vec<KestrelProfile>,
        scans: Vec<String>,
        connects: Vec<(String, String)>,
        disconnects: Vec<String>,
        autoconnect_resumes: Vec<(String, bool)>,
        wireless_toggles: Vec<bool>,
        added: Vec<(HashMap<String, Value>, String)>,
        updated: Vec<(String, HashMap<String, Value>, String, String)>,
        deleted: Vec<String>,
        next_profile_id: u32,
    }

    struct FakeKestrel {
        state: Mutex<FakeState>,
    }

    impl FakeKestrel {
        fn new(state: FakeState) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(state),
            })
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.state.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    impl KestrelApi for FakeKestrel {
        fn managed_objects(&self) -> Result<KestrelObjects, NetworkError> {
            let state = self.lock();
            Ok(KestrelObjects {
                manager: Some(state.manager.clone()),
                devices: state.devices.clone(),
                profiles: state.profiles.clone(),
            })
        }

        fn scan_results(&self, _device_path: &str) -> Result<Vec<KestrelScanEntry>, NetworkError> {
            Ok(self.lock().scan_results.clone())
        }

        fn scan(&self, device_path: &str) -> Result<(), NetworkError> {
            self.lock().scans.push(device_path.to_string());
            Ok(())
        }

        fn connect(&self, device_path: &str, profile_path: &str) -> Result<(), NetworkError> {
            self.lock()
                .connects
                .push((device_path.to_string(), profile_path.to_string()));
            Ok(())
        }

        fn disconnect(&self, device_path: &str) -> Result<(), NetworkError> {
            self.lock().disconnects.push(device_path.to_string());
            Ok(())
        }

        fn set_autoconnect(&self, device_path: &str, enabled: bool) -> Result<(), NetworkError> {
            self.lock()
                .autoconnect_resumes
                .push((device_path.to_string(), enabled));
            Ok(())
        }

        fn set_wireless_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
            let mut state = self.lock();
            state.wireless_toggles.push(enabled);
            state.manager.wireless_enabled = enabled;
            Ok(())
        }

        fn add_profile(
            &self,
            settings: HashMap<String, Value>,
            secret: &str,
        ) -> Result<String, NetworkError> {
            let mut state = self.lock();
            state.next_profile_id += 1;
            let path = format!("/com/hihusky/Kestrel/Profile/{}", state.next_profile_id);
            let ssid = get_str(&settings, "ssid").unwrap_or_default();
            let security = get_str(&settings, "security").unwrap_or_default();
            let autoconnect = get_bool(&settings, "autoconnect").unwrap_or(true);
            state.profiles.push(KestrelProfile {
                path: path.clone(),
                ssid,
                security,
                autoconnect,
                priority: 0,
            });
            state.added.push((settings, secret.to_string()));
            Ok(path)
        }

        fn update_profile(
            &self,
            profile_path: &str,
            settings: HashMap<String, Value>,
            secret_action: &str,
            secret: &str,
        ) -> Result<(), NetworkError> {
            let mut state = self.lock();
            if let Some(profile) = state.profiles.iter_mut().find(|p| p.path == profile_path) {
                if let Some(ac) = get_bool(&settings, "autoconnect") {
                    profile.autoconnect = ac;
                }
                if let Some(sec) = get_str(&settings, "security") {
                    profile.security = sec;
                }
            }
            state.updated.push((
                profile_path.to_string(),
                settings,
                secret_action.to_string(),
                secret.to_string(),
            ));
            Ok(())
        }

        fn delete_profile(&self, profile_path: &str) -> Result<(), NetworkError> {
            let mut state = self.lock();
            state.profiles.retain(|p| p.path != profile_path);
            state.deleted.push(profile_path.to_string());
            Ok(())
        }
    }

    fn base_state() -> FakeState {
        FakeState {
            manager: KestrelManager {
                version: API_VERSION.to_string(),
                wireless_enabled: true,
                wireless_hardware_enabled: true,
            },
            devices: vec![KestrelDevice {
                path: "/com/hihusky/Kestrel/Device/wlan0".to_string(),
                interface: "wlan0".to_string(),
                capabilities: vec!["station".to_string(), "p2p".to_string()],
                supported_security: vec![
                    "open".to_string(),
                    "owe".to_string(),
                    "wpa-personal".to_string(),
                    "wpa3-personal".to_string(),
                ],
                state: "connected".to_string(),
                ssid: "Home".to_string(),
                scanning: false,
                autoconnect_held: false,
            }],
            scan_results: vec![
                KestrelScanEntry {
                    ssid: "Home".to_string(),
                    frequency_mhz: 5180,
                    signal_dbm: -48,
                    security: "wpa2-wpa3".to_string(),
                    profile_path: "/com/hihusky/Kestrel/Profile/home".to_string(),
                },
                KestrelScanEntry {
                    ssid: "Home".to_string(),
                    frequency_mhz: 2412,
                    signal_dbm: -72,
                    security: "wpa2-psk".to_string(),
                    profile_path: "/com/hihusky/Kestrel/Profile/home".to_string(),
                },
                KestrelScanEntry {
                    ssid: "Cafe".to_string(),
                    frequency_mhz: 2437,
                    signal_dbm: -64,
                    security: "open".to_string(),
                    profile_path: "/".to_string(),
                },
                KestrelScanEntry {
                    ssid: "Office".to_string(),
                    frequency_mhz: 5220,
                    signal_dbm: -60,
                    security: "wpa3-sae".to_string(),
                    profile_path: "/".to_string(),
                },
            ],
            profiles: vec![KestrelProfile {
                path: "/com/hihusky/Kestrel/Profile/home".to_string(),
                ssid: "Home".to_string(),
                security: "wpa-personal".to_string(),
                autoconnect: true,
                priority: 10,
            }],
            ..FakeState::default()
        }
    }

    #[test]
    fn snapshot_translates_device_profiles_and_deduplicates_scan_results() {
        let api = FakeKestrel::new(base_state());
        let backend = KestrelBackend::with_api(api, "/com/hihusky/Kestrel/Device/wlan0");
        let snapshot = backend.snapshot();

        assert!(snapshot.available);
        assert_eq!(snapshot.backend, BackendKind::Kestrel);
        assert_eq!(snapshot.network, NetworkState::Wifi);
        assert!(!snapshot.network_authoritative);
        assert_eq!(snapshot.network_interface, "wlan0");
        assert_eq!(snapshot.wifi_enabled, Some(true));
        assert_eq!(snapshot.wifi_state, WifiLinkState::Connected);
        assert_eq!(snapshot.wifi_ssid.as_deref(), Some("Home"));
        assert_eq!(snapshot.wifi_networks.len(), 3);

        let home = &snapshot.wifi_networks[0];
        assert_eq!(home.ssid, "Home");
        assert!(home.is_connected);
        assert!(home.is_saved);
        assert!(home.auto_connect);
        assert_eq!(home.signal_bars, 4);
        assert_eq!(home.frequency_mhz, Some(5180));
        assert_eq!(home.security, WifiSecurity::WpaPsk);

        let cafe = snapshot
            .wifi_networks
            .iter()
            .find(|n| n.ssid == "Cafe")
            .unwrap();
        assert_eq!(cafe.security, WifiSecurity::Open);
        assert!(!cafe.is_saved);
    }

    #[test]
    fn connect_reuses_saved_profile_or_creates_new_profile() {
        let api = FakeKestrel::new(base_state());
        let backend = KestrelBackend::with_api(api.clone(), "/com/hihusky/Kestrel/Device/wlan0");

        backend.connect("Home", None).unwrap();
        assert_eq!(
            api.lock().connects,
            vec![(
                "/com/hihusky/Kestrel/Device/wlan0".to_string(),
                "/com/hihusky/Kestrel/Profile/home".to_string()
            )]
        );
        assert!(api.lock().added.is_empty());

        backend.connect("Office", Some("secret-pass")).unwrap();
        let state = api.lock();
        assert_eq!(state.added.len(), 1);
        assert_eq!(state.added[0].1, "secret-pass");
        assert_eq!(
            get_str(&state.added[0].0, "security").as_deref(),
            Some("wpa3-personal")
        );
        assert_eq!(state.connects.len(), 2);
    }

    #[test]
    fn forget_and_set_auto_connect_operate_on_matching_profiles() {
        let mut initial = base_state();
        initial.devices[0].autoconnect_held = true;
        let api = FakeKestrel::new(initial);
        let backend = KestrelBackend::with_api(api.clone(), "/com/hihusky/Kestrel/Device/wlan0");

        backend.set_auto_connect("Home", false).unwrap();
        assert!(!backend.snapshot().wifi_networks[0].auto_connect);

        backend.set_auto_connect("Home", true).unwrap();
        assert_eq!(
            api.lock().autoconnect_resumes,
            vec![("/com/hihusky/Kestrel/Device/wlan0".to_string(), true)]
        );

        backend.forget("Home").unwrap();
        assert_eq!(
            api.lock().deleted,
            vec!["/com/hihusky/Kestrel/Profile/home".to_string()]
        );
        assert_eq!(
            backend.forget("Missing"),
            Err(NetworkError::NetworkNotFound("Missing".to_string()))
        );
    }

    #[test]
    fn hardware_rfkill_marks_wifi_disabled() {
        let mut initial = base_state();
        initial.manager.wireless_hardware_enabled = false;
        let api = FakeKestrel::new(initial);
        let backend = KestrelBackend::with_api(api, "/com/hihusky/Kestrel/Device/wlan0");
        let snapshot = backend.snapshot();
        assert_eq!(snapshot.wifi_enabled, Some(false));
        assert_eq!(snapshot.wifi_state, WifiLinkState::Disabled);
        assert_eq!(snapshot.network, NetworkState::Offline);
        assert!(snapshot.wifi_networks.is_empty());
    }

    #[test]
    fn unexpected_api_version_is_rejected() {
        assert!(check_api_version("1").is_ok());
        let error = check_api_version("2").expect_err("version 2 must be rejected");
        assert!(error.to_string().contains("expected '1'"), "{error}");
    }

    #[test]
    #[ignore = "requires a running Kestrel daemon with a Wi-Fi device"]
    fn live_kestrel_backend_reads_real_snapshot() {
        let backend = KestrelBackend::open().expect("open live Kestrel backend");
        let snapshot = backend.snapshot();
        eprintln!("live Kestrel snapshot: {snapshot:?}");
        assert!(
            snapshot.available,
            "live snapshot unavailable: {snapshot:?}"
        );
        assert_eq!(snapshot.backend, BackendKind::Kestrel);
        assert!(snapshot.wifi_enabled.is_some());
        assert!(!snapshot.network_interface.is_empty());
    }
}
