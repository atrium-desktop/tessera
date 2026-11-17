//! A fake NetworkManager for casting tests, served on a private bus.
//!
//! It models exactly the slice of NetworkManager's D-Bus API that
//! `tessera_cast::radio::networkmanager::NmRadio` uses, with the peers that
//! were in range of the reference host on 2026-11-17: the Skyworth TV (its real
//! `WfdIEs`) and two printers that advertise no Wi-Fi Display at all. Group
//! formation is scripted per test through [`Activation`], and every call is
//! recorded so a test can assert what the caster asked for.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::{OwnedObjectPath, OwnedVariant};

pub use tessera_test_support::dbus::PrivateBus;

pub const TV_NAME: &str = "创维电视A3F_99E";
pub const TV_ADDRESS: &str = "9E:1F:E6:7C:8B:CE";
/// The TV's `WfdIEs`, as the real NetworkManager reported them.
pub const TV_WFD_IES: [u8; 9] = [0x00, 0x00, 0x06, 0x00, 0x11, 0x1c, 0x44, 0x00, 0x32];

pub const P2P_DEVICE: &str = "/org/freedesktop/NetworkManager/Devices/7";
pub const TV_PEER: &str = "/org/freedesktop/NetworkManager/WifiP2PPeer/1";
const ACTIVE: &str = "/org/freedesktop/NetworkManager/ActiveConnection/1";

/// How the fake answers `AddAndActivateConnection2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// The group comes up (`Activated`).
    Activate,
    /// The call succeeds but the activation fails (`Deactivated`), as the
    /// real NetworkManager reports a WPS/supplicant timeout.
    Fail,
    /// The call is refused, as a profile without `settings.modify.*` is.
    Deny,
}

/// What the fake has seen and what it will do next.
#[derive(Debug)]
pub struct FakeState {
    pub activation: Activation,
    /// `NMActiveConnectionState` of the one active connection; 0 = none.
    pub active_state: u32,
    pub calls: Vec<String>,
    pub settings: Option<HashMap<String, HashMap<String, OwnedVariant>>>,
    pub options: Option<HashMap<String, OwnedVariant>>,
}

pub type Shared = Arc<Mutex<FakeState>>;

/// A running fake: its bus, its connection, and its state.
pub struct FakeNetworkManager {
    pub state: Shared,
    // Field order matters: the connection must close before the bus dies.
    _connection: zbus::Connection,
    bus: PrivateBus,
}

impl FakeNetworkManager {
    /// Start a private bus and serve the fake on it, or `None` when
    /// `dbus-daemon` is unavailable.
    pub fn start(activation: Activation) -> Option<Self> {
        let bus = PrivateBus::start()?;
        let state = Arc::new(Mutex::new(FakeState {
            activation,
            active_state: 0,
            calls: Vec::new(),
            settings: None,
            options: None,
        }));
        let peer = |name: &str, address: &str, wfd_ies: &[u8], strength: u8| Peer {
            name: name.to_owned(),
            hw_address: address.to_owned(),
            wfd_ies: wfd_ies.to_vec(),
            strength,
        };
        let builder = zbus::connection::Builder::address(bus.address())
            .serve_at(
                "/org/freedesktop/NetworkManager",
                Manager {
                    state: Arc::clone(&state),
                },
            )
            .serve_at(
                "/org/freedesktop/NetworkManager/Devices/3",
                Device {
                    state: Arc::clone(&state),
                    device_type: 2,
                    device_state: 100,
                    interface: "wlan0".to_owned(),
                    carries_group: false,
                },
            )
            .serve_at(
                P2P_DEVICE,
                Device {
                    state: Arc::clone(&state),
                    device_type: 30,
                    device_state: 30,
                    interface: "p2p-wlan0-0".to_owned(),
                    carries_group: true,
                },
            )
            .serve_at(
                P2P_DEVICE,
                WifiP2P {
                    state: Arc::clone(&state),
                },
            )
            .serve_at(TV_PEER, peer(TV_NAME, TV_ADDRESS, &TV_WFD_IES, 85))
            .serve_at(
                "/org/freedesktop/NetworkManager/WifiP2PPeer/2",
                peer("DIRECT-e8-HP M233 LaserJet", "0A:A1:36:45:AE:E8", &[], 80),
            )
            .serve_at(
                "/org/freedesktop/NetworkManager/WifiP2PPeer/3",
                peer("DIRECT-2F-EPSON-ECE1A4", "DE:CD:2F:EC:61:A4", &[], 74),
            )
            .serve_at(
                ACTIVE,
                Active {
                    state: Arc::clone(&state),
                },
            )
            .name("org.freedesktop.NetworkManager");
        let connection = zbus::block_on(builder.build()).expect("serve the fake NetworkManager");
        Some(Self {
            state,
            _connection: connection,
            bus,
        })
    }

    /// The address to export as `DBUS_SYSTEM_BUS_ADDRESS`.
    pub fn address(&self) -> &str {
        self.bus.address()
    }

    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }

    pub fn group_is_up(&self) -> bool {
        self.state.lock().unwrap().active_state == 2
    }

    /// A value from the last activated profile, as `section.key`.
    pub fn setting(&self, section: &str, key: &str) -> Option<OwnedVariant> {
        let state = self.state.lock().unwrap();
        let value = state.settings.as_ref()?.get(section)?.get(key)?;
        value.try_clone().ok()
    }
}

fn record(state: &Shared, call: impl Into<String>) {
    state.lock().unwrap().calls.push(call.into());
}

fn path(value: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(value).expect("object path")
}

struct Manager {
    state: Shared,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager")]
impl Manager {
    fn get_devices(&self) -> Vec<OwnedObjectPath> {
        vec![
            path("/org/freedesktop/NetworkManager/Devices/3"),
            path(P2P_DEVICE),
        ]
    }

    #[zbus(name = "AddAndActivateConnection2")]
    fn add_and_activate_connection2(
        &self,
        settings: HashMap<String, HashMap<String, OwnedVariant>>,
        device: OwnedObjectPath,
        specific_object: OwnedObjectPath,
        options: HashMap<String, OwnedVariant>,
    ) -> zbus::fdo::Result<(
        OwnedObjectPath,
        OwnedObjectPath,
        HashMap<String, OwnedVariant>,
    )> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(format!(
            "AddAndActivateConnection2 {} {}",
            device.as_str(),
            specific_object.as_str()
        ));
        if state.activation == Activation::Deny {
            return Err(zbus::fdo::Error::AccessDenied(
                "Insufficient privileges".to_owned(),
            ));
        }
        state.settings = Some(settings);
        state.options = Some(options);
        state.active_state = match state.activation {
            Activation::Activate => 2,
            _ => 4,
        };
        Ok((
            path("/org/freedesktop/NetworkManager/Settings/1"),
            path(ACTIVE),
            HashMap::new(),
        ))
    }

    fn deactivate_connection(&self, active: OwnedObjectPath) {
        let mut state = self.state.lock().unwrap();
        state
            .calls
            .push(format!("DeactivateConnection {}", active.as_str()));
        state.active_state = 4;
    }
}

struct Device {
    state: Shared,
    device_type: u32,
    device_state: u32,
    interface: String,
    /// Whether an activated group shows up on this device.
    carries_group: bool,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device")]
impl Device {
    #[zbus(property)]
    fn device_type(&self) -> u32 {
        self.device_type
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        self.device_state
    }

    #[zbus(property)]
    fn active_connection(&self) -> OwnedObjectPath {
        if self.carries_group && self.state.lock().unwrap().active_state == 2 {
            path(ACTIVE)
        } else {
            path("/")
        }
    }

    #[zbus(property)]
    fn ip_interface(&self) -> String {
        if self.carries_group && self.state.lock().unwrap().active_state != 2 {
            String::new()
        } else {
            self.interface.clone()
        }
    }
}

struct WifiP2P {
    state: Shared,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.WifiP2P")]
impl WifiP2P {
    fn start_find(&self, options: HashMap<String, OwnedVariant>) {
        let timeout = options
            .get("timeout")
            .and_then(|value| i32::try_from(value).ok());
        record(&self.state, format!("StartFind timeout={timeout:?}"));
    }

    fn stop_find(&self) {
        record(&self.state, "StopFind");
    }

    #[zbus(property)]
    fn peers(&self) -> Vec<OwnedObjectPath> {
        (1..=3)
            .map(|index| {
                path(&format!(
                    "/org/freedesktop/NetworkManager/WifiP2PPeer/{index}"
                ))
            })
            .collect()
    }
}

struct Peer {
    name: String,
    hw_address: String,
    wfd_ies: Vec<u8>,
    strength: u8,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.WifiP2PPeer")]
impl Peer {
    #[zbus(property)]
    fn name(&self) -> String {
        self.name.clone()
    }

    #[zbus(property)]
    fn hw_address(&self) -> String {
        self.hw_address.clone()
    }

    #[zbus(property, name = "WfdIEs")]
    fn wfd_ies(&self) -> Vec<u8> {
        self.wfd_ies.clone()
    }

    #[zbus(property)]
    fn strength(&self) -> u8 {
        self.strength
    }
}

struct Active {
    state: Shared,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Connection.Active")]
impl Active {
    #[zbus(property)]
    fn state(&self) -> u32 {
        self.state.lock().unwrap().active_state
    }

    #[zbus(property)]
    fn specific_object(&self) -> OwnedObjectPath {
        path(TV_PEER)
    }
}
