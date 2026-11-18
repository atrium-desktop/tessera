//! A fake NetworkManager for casting tests, served on a private bus.
//!
//! It models exactly the slice of NetworkManager's D-Bus API that
//! `tessera_cast::radio::networkmanager::NmRadio` uses.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tessera_dbus::{
    Connection, FromValue, Message, ObjectHandler, OwnedObjectPath, ToValue, Value,
};

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
    pub settings: Option<HashMap<String, HashMap<String, Value>>>,
    pub options: Option<HashMap<String, Value>>,
}

pub type Shared = Arc<Mutex<FakeState>>;

/// A running fake: its bus, its connection, and its state.
pub struct FakeNetworkManager {
    pub state: Shared,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    _connection: Connection,
    bus: PrivateBus,
}

impl Drop for FakeNetworkManager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
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
        let peer = |name: &str, address: &str, wfd_ies: &[u8], strength: u8| {
            Arc::new(Peer {
                name: name.to_owned(),
                hw_address: address.to_owned(),
                wfd_ies: wfd_ies.to_vec(),
                strength,
            })
        };

        let connection = Connection::address(bus.address()).expect("connect fake NetworkManager");
        connection.register_object(
            "/org/freedesktop/NetworkManager",
            Arc::new(Manager {
                state: Arc::clone(&state),
            }),
        );
        connection.register_object(
            "/org/freedesktop/NetworkManager/Devices/3",
            Arc::new(Device {
                state: Arc::clone(&state),
                device_type: 2,
                device_state: 100,
                interface: "wlan0".to_owned(),
                carries_group: false,
                wifi_p2p: false,
            }),
        );
        connection.register_object(
            P2P_DEVICE,
            Arc::new(Device {
                state: Arc::clone(&state),
                device_type: 30,
                device_state: 30,
                interface: "p2p-wlan0-0".to_owned(),
                carries_group: true,
                wifi_p2p: true,
            }),
        );
        connection.register_object(TV_PEER, peer(TV_NAME, TV_ADDRESS, &TV_WFD_IES, 85));
        connection.register_object(
            "/org/freedesktop/NetworkManager/WifiP2PPeer/2",
            peer("DIRECT-e8-HP M233 LaserJet", "0A:A1:36:45:AE:E8", &[], 80),
        );
        connection.register_object(
            "/org/freedesktop/NetworkManager/WifiP2PPeer/3",
            peer("DIRECT-2F-EPSON-ECE1A4", "DE:CD:2F:EC:61:A4", &[], 74),
        );
        connection.register_object(
            ACTIVE,
            Arc::new(Active {
                state: Arc::clone(&state),
            }),
        );
        connection
            .request_name("org.freedesktop.NetworkManager")
            .expect("request NetworkManager name");

        let stop = Arc::new(AtomicBool::new(false));
        let worker_conn = connection.clone();
        let worker_stop = Arc::clone(&stop);
        let worker = std::thread::Builder::new()
            .name("fake-nm-dbus".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    if worker_conn
                        .process_once(Some(Duration::from_millis(20)))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("spawn fake-nm-dbus");

        Some(Self {
            state,
            stop,
            worker: Some(worker),
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
    pub fn setting(&self, section: &str, key: &str) -> Option<Value> {
        let state = self.state.lock().unwrap();
        state.settings.as_ref()?.get(section)?.get(key).cloned()
    }
}

fn record(state: &Shared, call: impl Into<String>) {
    state.lock().unwrap().calls.push(call.into());
}

fn path(value: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(value).expect("object path")
}

fn unknown_method(msg: &Message) -> (String, String) {
    (
        "org.freedesktop.DBus.Error.UnknownMethod".into(),
        format!("unknown method {:?}.{:?}", msg.interface, msg.member),
    )
}

fn unknown_prop(prop: &str) -> (String, String) {
    (
        "org.freedesktop.DBus.Error.UnknownProperty".into(),
        format!("unknown property {prop}"),
    )
}

struct Manager {
    state: Shared,
}

impl ObjectHandler for Manager {
    fn handle_method_call(
        &self,
        _conn: &Connection,
        msg: &Message,
    ) -> Result<Vec<Value>, (String, String)> {
        match (msg.interface.as_deref(), msg.member.as_deref()) {
            (Some("org.freedesktop.NetworkManager"), Some("GetDevices")) => Ok(vec![Value::Array(
                "o".into(),
                vec![
                    Value::ObjectPath(path("/org/freedesktop/NetworkManager/Devices/3")),
                    Value::ObjectPath(path(P2P_DEVICE)),
                ],
            )]),
            (Some("org.freedesktop.NetworkManager"), Some("AddAndActivateConnection2")) => {
                if msg.body.len() < 4 {
                    return Err((
                        "org.freedesktop.DBus.Error.InvalidArgs".into(),
                        "expected 4 args".into(),
                    ));
                }
                let settings =
                    HashMap::<String, HashMap<String, Value>>::from_value(msg.body[0].clone())
                        .map_err(|e| {
                            (
                                "org.freedesktop.DBus.Error.InvalidArgs".into(),
                                e.to_string(),
                            )
                        })?;
                let device = OwnedObjectPath::from_value(msg.body[1].clone()).map_err(|e| {
                    (
                        "org.freedesktop.DBus.Error.InvalidArgs".into(),
                        e.to_string(),
                    )
                })?;
                let specific_object =
                    OwnedObjectPath::from_value(msg.body[2].clone()).map_err(|e| {
                        (
                            "org.freedesktop.DBus.Error.InvalidArgs".into(),
                            e.to_string(),
                        )
                    })?;
                let options =
                    HashMap::<String, Value>::from_value(msg.body[3].clone()).map_err(|e| {
                        (
                            "org.freedesktop.DBus.Error.InvalidArgs".into(),
                            e.to_string(),
                        )
                    })?;

                let mut state = self.state.lock().unwrap();
                state.calls.push(format!(
                    "AddAndActivateConnection2 {} {}",
                    device.as_str(),
                    specific_object.as_str()
                ));
                if state.activation == Activation::Deny {
                    return Err((
                        "org.freedesktop.DBus.Error.AccessDenied".into(),
                        "Insufficient privileges".to_owned(),
                    ));
                }
                state.settings = Some(settings);
                state.options = Some(options);
                state.active_state = match state.activation {
                    Activation::Activate => 2,
                    _ => 4,
                };
                Ok(vec![
                    Value::ObjectPath(path("/org/freedesktop/NetworkManager/Settings/1")),
                    Value::ObjectPath(path(ACTIVE)),
                    HashMap::<String, Value>::new().to_value(),
                ])
            }
            (Some("org.freedesktop.NetworkManager"), Some("DeactivateConnection")) => {
                if let Some(active) = msg
                    .body
                    .first()
                    .and_then(|v| OwnedObjectPath::from_value(v.clone()).ok())
                {
                    let mut state = self.state.lock().unwrap();
                    state
                        .calls
                        .push(format!("DeactivateConnection {}", active.as_str()));
                    state.active_state = 4;
                }
                Ok(Vec::new())
            }
            _ => Err(unknown_method(msg)),
        }
    }
}

struct Device {
    state: Shared,
    device_type: u32,
    device_state: u32,
    interface: String,
    carries_group: bool,
    wifi_p2p: bool,
}

impl ObjectHandler for Device {
    fn handle_method_call(
        &self,
        _conn: &Connection,
        msg: &Message,
    ) -> Result<Vec<Value>, (String, String)> {
        match (msg.interface.as_deref(), msg.member.as_deref()) {
            (Some("org.freedesktop.DBus.Properties"), Some("Get")) => {
                let iface = msg.body.first().and_then(Value::as_str).unwrap_or("");
                let prop = msg.body.get(1).and_then(Value::as_str).unwrap_or("");
                let val = match (iface, prop) {
                    ("org.freedesktop.NetworkManager.Device", "DeviceType") => {
                        Value::Uint32(self.device_type)
                    }
                    ("org.freedesktop.NetworkManager.Device", "State") => {
                        Value::Uint32(self.device_state)
                    }
                    ("org.freedesktop.NetworkManager.Device", "ActiveConnection") => {
                        if self.carries_group && self.state.lock().unwrap().active_state == 2 {
                            Value::ObjectPath(path(ACTIVE))
                        } else {
                            Value::ObjectPath(path("/"))
                        }
                    }
                    ("org.freedesktop.NetworkManager.Device", "IpInterface") => {
                        let s =
                            if self.carries_group && self.state.lock().unwrap().active_state != 2 {
                                String::new()
                            } else {
                                self.interface.clone()
                            };
                        Value::String(s)
                    }
                    ("org.freedesktop.NetworkManager.Device.WifiP2P", "Peers") if self.wifi_p2p => {
                        let peers = (1..=3)
                            .map(|i| {
                                Value::ObjectPath(path(&format!(
                                    "/org/freedesktop/NetworkManager/WifiP2PPeer/{i}"
                                )))
                            })
                            .collect();
                        Value::Array("o".into(), peers)
                    }
                    _ => return Err(unknown_prop(prop)),
                };
                Ok(vec![Value::Variant(Box::new(val))])
            }
            (Some("org.freedesktop.NetworkManager.Device.WifiP2P"), Some("StartFind"))
                if self.wifi_p2p =>
            {
                let timeout = msg
                    .body
                    .first()
                    .and_then(|v| HashMap::<String, Value>::from_value(v.clone()).ok())
                    .and_then(|opts| opts.get("timeout").and_then(Value::as_i32));
                record(&self.state, format!("StartFind timeout={timeout:?}"));
                Ok(Vec::new())
            }
            (Some("org.freedesktop.NetworkManager.Device.WifiP2P"), Some("StopFind"))
                if self.wifi_p2p =>
            {
                record(&self.state, "StopFind");
                Ok(Vec::new())
            }
            _ => Err(unknown_method(msg)),
        }
    }
}

struct Peer {
    name: String,
    hw_address: String,
    wfd_ies: Vec<u8>,
    strength: u8,
}

impl ObjectHandler for Peer {
    fn handle_method_call(
        &self,
        _conn: &Connection,
        msg: &Message,
    ) -> Result<Vec<Value>, (String, String)> {
        match (msg.interface.as_deref(), msg.member.as_deref()) {
            (Some("org.freedesktop.DBus.Properties"), Some("Get")) => {
                let prop = msg.body.get(1).and_then(Value::as_str).unwrap_or("");
                let val = match prop {
                    "Name" => Value::String(self.name.clone()),
                    "HwAddress" => Value::String(self.hw_address.clone()),
                    "WfdIEs" => Value::Array(
                        "y".into(),
                        self.wfd_ies.iter().copied().map(Value::Byte).collect(),
                    ),
                    "Strength" => Value::Byte(self.strength),
                    _ => return Err(unknown_prop(prop)),
                };
                Ok(vec![Value::Variant(Box::new(val))])
            }
            _ => Err(unknown_method(msg)),
        }
    }
}

struct Active {
    state: Shared,
}

impl ObjectHandler for Active {
    fn handle_method_call(
        &self,
        _conn: &Connection,
        msg: &Message,
    ) -> Result<Vec<Value>, (String, String)> {
        match (msg.interface.as_deref(), msg.member.as_deref()) {
            (Some("org.freedesktop.DBus.Properties"), Some("Get")) => {
                let prop = msg.body.get(1).and_then(Value::as_str).unwrap_or("");
                let val = match prop {
                    "State" => Value::Uint32(self.state.lock().unwrap().active_state),
                    "SpecificObject" => Value::ObjectPath(path(TV_PEER)),
                    _ => return Err(unknown_prop(prop)),
                };
                Ok(vec![Value::Variant(Box::new(val))])
            }
            _ => Err(unknown_method(msg)),
        }
    }
}
