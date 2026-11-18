//! Minimal synchronous D-Bus v1 wire-protocol client and object dispatcher.

pub mod transport;
pub mod wire;

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io;
pub use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use transport::{Transport, TransportError, authenticate, connect_address};
pub use wire::{
    FromArgs, FromValue, Message, MessageType, OwnedObjectPath, ToArgs, ToValue, Value, WireError,
};

pub type Result<T> = std::result::Result<T, Error>;

/// D-Bus error type.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Wire(WireError),
    MethodError {
        name: String,
        detail: Option<String>,
    },
    Disconnected,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "D-Bus I/O error: {e}"),
            Self::Wire(e) => write!(f, "D-Bus wire error: {e}"),
            Self::MethodError { name, detail } => match detail {
                Some(d) => write!(f, "D-Bus method error {name}: {d}"),
                None => write!(f, "D-Bus method error {name}"),
            },
            Self::Disconnected => write!(f, "D-Bus connection closed"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Wire(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<WireError> for Error {
    fn from(e: WireError) -> Self {
        Self::Wire(e)
    }
}

impl From<TransportError> for Error {
    fn from(e: TransportError) -> Self {
        match e {
            TransportError::Io(io) => Self::Io(io),
            TransportError::Wire(w) => Self::Wire(w),
        }
    }
}

/// Trait implemented by exported D-Bus objects.
pub trait ObjectHandler: Send + Sync {
    fn handle_method_call(
        &self,
        conn: &Connection,
        msg: &Message,
    ) -> std::result::Result<Vec<Value>, (String, String)>;
}

struct Inner {
    transport: Transport,
    pending_incoming: VecDeque<Message>,
    objects: HashMap<String, Arc<dyn ObjectHandler>>,
}

/// Thread-safe synchronous D-Bus connection.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Mutex<Inner>>,
    next_serial: Arc<AtomicU32>,
    unique_name: Arc<Mutex<Option<String>>>,
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

impl Connection {
    /// Connect to the session bus (`$DBUS_SESSION_BUS_ADDRESS`).
    pub fn session() -> Result<Self> {
        let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").map_err(|_| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "DBUS_SESSION_BUS_ADDRESS is not set",
            )
        })?;
        Self::address(&addr)
    }

    /// Connect to the system bus (`$DBUS_SYSTEM_BUS_ADDRESS` or `/run/dbus/system_bus_socket`).
    pub fn system() -> Result<Self> {
        let addr = std::env::var("DBUS_SYSTEM_BUS_ADDRESS")
            .unwrap_or_else(|_| "unix:path=/run/dbus/system_bus_socket".to_string());
        Self::address(&addr)
    }

    /// Connect to an explicit D-Bus address string.
    pub fn address(addr: &str) -> Result<Self> {
        let mut stream = connect_address(addr)?;
        authenticate(&mut stream)?;
        let conn = Self {
            inner: Arc::new(Mutex::new(Inner {
                transport: Transport::new(stream),
                pending_incoming: VecDeque::new(),
                objects: HashMap::new(),
            })),
            next_serial: Arc::new(AtomicU32::new(1)),
            unique_name: Arc::new(Mutex::new(None)),
        };
        let reply = conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "Hello",
            &(),
        )?;
        let name: String = FromArgs::from_args(reply.body)?;
        *lock_or_recover(&conn.unique_name) = Some(name);
        Ok(conn)
    }

    pub fn unique_name(&self) -> Option<String> {
        lock_or_recover(&self.unique_name).clone()
    }

    fn alloc_serial(&self) -> u32 {
        self.next_serial.fetch_add(1, Ordering::Relaxed)
    }

    /// Register an object handler at `path`.
    pub fn register_object(&self, path: impl Into<String>, handler: Arc<dyn ObjectHandler>) {
        lock_or_recover(&self.inner)
            .objects
            .insert(path.into(), handler);
    }

    /// Request a well-known bus name (`org.freedesktop.DBus.RequestName`).
    pub fn request_name(&self, name: &str) -> Result<u32> {
        let reply = self.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "RequestName",
            &(name, 0u32),
        )?;
        Ok(FromArgs::from_args(reply.body)?)
    }

    /// Emit a D-Bus signal.
    pub fn emit_signal<B: ToArgs>(
        &self,
        destination: Option<&str>,
        path: &str,
        interface: &str,
        member: &str,
        body: &B,
    ) -> Result<()> {
        let serial = self.alloc_serial();
        let msg = Message::signal(serial, destination, path, interface, member, body.to_args())?;
        let mut inner = lock_or_recover(&self.inner);
        inner.transport.send_message(&msg)?;
        Ok(())
    }

    /// Call a D-Bus method synchronously and wait for its `MethodReturn` or `Error`.
    pub fn call_method<B: ToArgs>(
        &self,
        destination: Option<&str>,
        path: &str,
        interface: Option<&str>,
        member: &str,
        body: &B,
    ) -> Result<Message> {
        let serial = self.alloc_serial();
        let msg =
            Message::method_call(serial, destination, path, interface, member, body.to_args())?;

        let mut inner = lock_or_recover(&self.inner);
        inner.transport.send_message(&msg)?;
        drop(inner);

        loop {
            let incoming = {
                let mut inner = lock_or_recover(&self.inner);
                inner.transport.read_message()?
            };
            if incoming.reply_serial == Some(serial) {
                return match incoming.msg_type {
                    MessageType::MethodReturn => Ok(incoming),
                    MessageType::Error => {
                        let name = incoming
                            .error_name
                            .unwrap_or_else(|| "org.freedesktop.DBus.Error.Failed".into());
                        let detail = incoming
                            .body
                            .first()
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        Err(Error::MethodError { name, detail })
                    }
                    _ => Err(Error::Wire(WireError::InvalidFormat(
                        "unexpected reply message type".into(),
                    ))),
                };
            }
            self.handle_unsolicited(incoming);
        }
    }

    fn handle_unsolicited(&self, msg: Message) {
        if msg.msg_type == MessageType::MethodCall {
            let handler = msg.path.as_ref().and_then(|p| {
                lock_or_recover(&self.inner)
                    .objects
                    .get(p.as_str())
                    .cloned()
            });
            if let Some(handler) = handler {
                let reply_serial = msg.serial;
                let dest = msg.sender.clone();
                let serial = self.alloc_serial();
                let response = match handler.handle_method_call(self, &msg) {
                    Ok(body) => Message::method_return(serial, reply_serial, dest.as_deref(), body),
                    Err((err_name, err_msg)) => Message::error_reply(
                        serial,
                        reply_serial,
                        dest.as_deref(),
                        &err_name,
                        &err_msg,
                    ),
                };
                let _ = lock_or_recover(&self.inner)
                    .transport
                    .send_message(&response);
                return;
            }
        }
        lock_or_recover(&self.inner).pending_incoming.push_back(msg);
    }

    /// Process one incoming message (blocking up to `timeout` if provided).
    pub fn process_once(&self, timeout: Option<Duration>) -> Result<bool> {
        if let Some(dur) = timeout {
            let raw_fd = lock_or_recover(&self.inner).transport.raw_fd();
            let mut pfd = libc::pollfd {
                fd: raw_fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = dur.as_millis().min(i32::MAX as u128) as libc::c_int;
            let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
            if rc == 0 {
                return Ok(false);
            }
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    return Ok(false);
                }
                return Err(Error::Io(err));
            }
        }

        let msg = {
            let mut inner = lock_or_recover(&self.inner);
            inner.transport.read_message()?
        };
        self.handle_unsolicited(msg);
        Ok(true)
    }
}

/// Ergonomic proxy bound to `(destination, path, interface)`.
#[derive(Clone)]
pub struct Proxy<'a> {
    conn: &'a Connection,
    destination: String,
    path: String,
    interface: String,
}

impl<'a> Proxy<'a> {
    pub fn new(
        conn: &'a Connection,
        destination: impl Into<String>,
        path: impl Into<String>,
        interface: impl Into<String>,
    ) -> Self {
        Self {
            conn,
            destination: destination.into(),
            path: path.into(),
            interface: interface.into(),
        }
    }

    pub fn call<B: ToArgs, R: FromArgs>(&self, method: &str, body: &B) -> Result<R> {
        let reply = self.conn.call_method(
            Some(&self.destination),
            &self.path,
            Some(&self.interface),
            method,
            body,
        )?;
        Ok(R::from_args(reply.body)?)
    }

    pub fn get_property<T: FromValue>(&self, name: &str) -> Result<T> {
        let reply = self.conn.call_method(
            Some(&self.destination),
            &self.path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(self.interface.as_str(), name),
        )?;
        let val: Value = FromArgs::from_args(reply.body)?;
        Ok(T::from_value(val)?)
    }

    pub fn set_property<T: ToValue>(&self, name: &str, value: T) -> Result<()> {
        let variant = Value::Variant(Box::new(value.to_value()));
        self.conn.call_method(
            Some(&self.destination),
            &self.path,
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &vec![
                Value::String(self.interface.clone()),
                Value::String(name.to_string()),
                variant,
            ],
        )?;
        Ok(())
    }

    pub fn get_all_properties(&self) -> Result<HashMap<String, Value>> {
        let reply = self.conn.call_method(
            Some(&self.destination),
            &self.path,
            Some("org.freedesktop.DBus.Properties"),
            "GetAll",
            &(self.interface.as_str(),),
        )?;
        Ok(FromArgs::from_args(reply.body)?)
    }
}

/// Synchronous signal stream filtered by interface/member/path.
pub struct MessageStream {
    conn: Connection,
    interface: Option<String>,
    member: Option<String>,
    path: Option<String>,
}

impl MessageStream {
    /// Subscribe using a D-Bus match rule and return a stream.
    pub fn for_match_rule(conn: &Connection, rule: &str) -> Result<Self> {
        conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &(rule,),
        )?;
        let mut interface = None;
        let mut member = None;
        let mut path = None;
        for part in rule.split(',') {
            let part = part.trim();
            if let Some(v) = part.strip_prefix("interface=") {
                interface = Some(v.trim_matches('\'').to_string());
            } else if let Some(v) = part.strip_prefix("member=") {
                member = Some(v.trim_matches('\'').to_string());
            } else if let Some(v) = part.strip_prefix("path=") {
                path = Some(v.trim_matches('\'').to_string());
            }
        }
        Ok(Self {
            conn: conn.clone(),
            interface,
            member,
            path,
        })
    }

    fn matches(&self, msg: &Message) -> bool {
        if msg.msg_type != MessageType::Signal {
            return false;
        }
        if let Some(ref iface) = self.interface
            && msg.interface.as_deref() != Some(iface.as_str())
        {
            return false;
        }
        if let Some(ref mem) = self.member
            && msg.member.as_deref() != Some(mem.as_str())
        {
            return false;
        }
        if let Some(ref p) = self.path
            && msg.path.as_ref().map(OwnedObjectPath::as_str) != Some(p.as_str())
        {
            return false;
        }
        true
    }

    /// Block until the next matching signal arrives (or connection closes).
    pub fn next(&self) -> Option<Message> {
        loop {
            {
                let mut inner = lock_or_recover(&self.conn.inner);
                if let Some(pos) = inner.pending_incoming.iter().position(|m| self.matches(m)) {
                    return inner.pending_incoming.remove(pos);
                }
            }
            let msg = {
                let mut inner = lock_or_recover(&self.conn.inner);
                inner.transport.read_message().ok()?
            };
            if self.matches(&msg) {
                return Some(msg);
            }
            self.conn.handle_unsolicited(msg);
        }
    }
}

/// Convenience wrapper for `org.freedesktop.DBus` daemon calls.
pub mod fdo {
    use super::*;

    pub fn list_names(conn: &Connection) -> Result<Vec<String>> {
        let reply = conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        )?;
        Ok(FromArgs::from_args(reply.body)?)
    }

    pub fn get_name_owner(conn: &Connection, name: &str) -> Result<String> {
        let reply = conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(name,),
        )?;
        Ok(FromArgs::from_args(reply.body)?)
    }
}
