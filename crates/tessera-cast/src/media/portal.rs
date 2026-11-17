//! Client for `org.freedesktop.portal.ScreenCast` over D-Bus.
//!
//! Per [ADR-0191](../../docs/adr/0191-casting-p2p-belongs-to-the-system-wifi-authority.md),
//! the caster holds **no compositor IPC scope**: frame capture passes through the
//! standard desktop portal so that user consent and seat security are honored.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::StreamExt;
use zbus::{Connection, OwnedObjectPath, OwnedVariant, Proxy, Variant};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST_IFACE: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// One PipeWire stream exposed by the ScreenCast portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeWireStreamInfo {
    pub node_id: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug)]
pub enum PortalError {
    Bus(zbus::Error),
    Denied(String),
    Protocol(String),
}

impl std::fmt::Display for PortalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bus(err) => write!(f, "portal D-Bus error: {err}"),
            Self::Denied(reason) => write!(f, "portal consent denied: {reason}"),
            Self::Protocol(reason) => write!(f, "portal protocol error: {reason}"),
        }
    }
}

impl std::error::Error for PortalError {}

impl From<zbus::Error> for PortalError {
    fn from(err: zbus::Error) -> Self {
        Self::Bus(err)
    }
}

fn proxy<'a>(
    connection: &'a Connection,
    destination: &'a str,
    path: &'a str,
    interface: &'a str,
) -> Result<Proxy<'a>, PortalError> {
    Ok(zbus::block_on(Proxy::new(
        connection,
        destination,
        path,
        interface,
    ))?)
}

/// An active ScreenCast portal session.
pub struct ScreenCastSession {
    connection: Connection,
    session_handle: String,
    pub streams: Vec<PipeWireStreamInfo>,
    pub pipewire_fd: OwnedFd,
}

impl ScreenCastSession {
    /// Open a new ScreenCast portal session requesting virtual display or monitor capture.
    pub fn open(connection: &Connection, virtual_source: bool) -> Result<Self, PortalError> {
        let sender = connection
            .unique_name()
            .ok_or_else(|| PortalError::Protocol("connection has no unique name".into()))?
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");

        let id = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();

        let session_token = format!("tessera_cast_sess_{pid}_{id}");
        let create_token = format!("tessera_cast_create_{pid}_{id}");
        let select_token = format!("tessera_cast_select_{pid}_{id}");
        let start_token = format!("tessera_cast_start_{pid}_{id}");

        let screencast = proxy(connection, PORTAL_BUS, PORTAL_PATH, SCREENCAST_IFACE)?;

        // 1. CreateSession
        let create_req_path =
            format!("/org/freedesktop/portal/desktop/request/{sender}/{create_token}");
        let create_req = proxy(
            connection,
            PORTAL_BUS,
            create_req_path.as_str(),
            REQUEST_IFACE,
        )?;
        let mut create_signals = zbus::block_on(create_req.receive_signal("Response"))?;

        let mut create_options: HashMap<&str, Variant<'_>> = HashMap::new();
        create_options.insert(
            "session_handle_token",
            Variant::from(session_token.as_str()),
        );
        create_options.insert("handle_token", Variant::from(create_token.as_str()));

        let _req_path: OwnedObjectPath =
            zbus::block_on(screencast.call("CreateSession", &(create_options,)))?;
        let res_msg = zbus::block_on(create_signals.next()).ok_or_else(|| {
            PortalError::Protocol("portal closed before CreateSession response".into())
        })?;
        let (code, create_results): (u32, HashMap<String, OwnedVariant>) =
            res_msg.body().deserialize()?;
        if code != 0 {
            return Err(PortalError::Denied(format!(
                "CreateSession returned code {code}"
            )));
        }

        let session_handle_val = create_results.get("session_handle").ok_or_else(|| {
            PortalError::Protocol("missing session_handle in CreateSession response".into())
        })?;
        let session_handle_str = match &**session_handle_val {
            Variant::Str(s) => s.to_string(),
            Variant::ObjectPath(p) => p.to_string(),
            other => {
                return Err(PortalError::Protocol(format!(
                    "unexpected session_handle type: {}",
                    other.value_signature()
                )));
            }
        };

        // 2. SelectSources
        let select_req_path =
            format!("/org/freedesktop/portal/desktop/request/{sender}/{select_token}");
        let select_req = proxy(
            connection,
            PORTAL_BUS,
            select_req_path.as_str(),
            REQUEST_IFACE,
        )?;
        let mut select_signals = zbus::block_on(select_req.receive_signal("Response"))?;

        let mut select_options: HashMap<&str, Variant<'_>> = HashMap::new();
        select_options.insert("handle_token", Variant::from(select_token.as_str()));
        // Query portal capability: prefer virtual extend output (4) if requested & supported,
        // otherwise fall back to screen mirroring (1 = Monitor).
        let available_types =
            zbus::block_on(screencast.get_property::<u32>("AvailableSourceTypes")).unwrap_or(1);
        let requested_type = if virtual_source && (available_types & 4 != 0) {
            4u32 // SOURCE_TYPE_VIRTUAL
        } else {
            1u32 // SOURCE_TYPE_MONITOR
        };
        select_options.insert("types", Variant::from(requested_type));
        select_options.insert("multiple", Variant::from(false));
        // Match the portal's AvailableCursorModes (2 = Embedded, 1 = Hidden)
        let cursor_modes =
            zbus::block_on(screencast.get_property::<u32>("AvailableCursorModes")).unwrap_or(1);
        let cursor_mode = if cursor_modes & 2 != 0 { 2u32 } else { 1u32 };
        select_options.insert("cursor_mode", Variant::from(cursor_mode));

        let session_path = zbus::ObjectPath::try_from(session_handle_str.as_str())
            .map_err(|err| PortalError::Protocol(format!("invalid session handle path: {err}")))?;

        let _req_path: OwnedObjectPath =
            zbus::block_on(screencast.call("SelectSources", &(&session_path, select_options)))?;
        let res_msg = zbus::block_on(select_signals.next()).ok_or_else(|| {
            PortalError::Protocol("portal closed before SelectSources response".into())
        })?;
        let (code, _): (u32, HashMap<String, OwnedVariant>) = res_msg.body().deserialize()?;
        if code != 0 {
            return Err(PortalError::Denied(format!(
                "SelectSources returned code {code}"
            )));
        }

        // 3. Start
        let start_req_path =
            format!("/org/freedesktop/portal/desktop/request/{sender}/{start_token}");
        let start_req = proxy(
            connection,
            PORTAL_BUS,
            start_req_path.as_str(),
            REQUEST_IFACE,
        )?;
        let mut start_signals = zbus::block_on(start_req.receive_signal("Response"))?;

        let mut start_options: HashMap<&str, Variant<'_>> = HashMap::new();
        start_options.insert("handle_token", Variant::from(start_token.as_str()));

        let _req_path: OwnedObjectPath =
            zbus::block_on(screencast.call("Start", &(&session_path, "", start_options)))?;
        let res_msg = zbus::block_on(start_signals.next())
            .ok_or_else(|| PortalError::Protocol("portal closed before Start response".into()))?;
        let (code, start_results): (u32, HashMap<String, OwnedVariant>) =
            res_msg.body().deserialize()?;
        if code != 0 {
            return Err(PortalError::Denied(format!("Start returned code {code}")));
        }

        let streams = parse_streams(&start_results)?;
        if streams.is_empty() {
            return Err(PortalError::Protocol(
                "ScreenCast returned no PipeWire streams".into(),
            ));
        }

        // 4. OpenPipeWireRemote
        let fd_options: HashMap<&str, Variant<'_>> = HashMap::new();
        let reply = zbus::block_on(
            screencast.call_method("OpenPipeWireRemote", &(&session_path, fd_options)),
        )?;
        let fd: zbus::OwnedFd = reply.body().deserialize()?;
        let pipewire_fd = fd.into();

        Ok(Self {
            connection: connection.clone(),
            session_handle: session_handle_str,
            streams,
            pipewire_fd,
        })
    }
}

impl Drop for ScreenCastSession {
    fn drop(&mut self) {
        let session_proxy = proxy(
            &self.connection,
            PORTAL_BUS,
            self.session_handle.as_str(),
            "org.freedesktop.portal.Session",
        );
        if let Ok(proxy) = session_proxy {
            let _ = zbus::block_on(proxy.call_method("Close", &()));
        }
    }
}

/// Parse streams from Start response dictionary.
fn parse_streams(
    results: &HashMap<String, OwnedVariant>,
) -> Result<Vec<PipeWireStreamInfo>, PortalError> {
    let mut out = Vec::new();
    let Some(streams_val) = results.get("streams") else {
        return Ok(out);
    };

    if let Variant::Array(arr) = &**streams_val {
        for elem in arr.iter() {
            if let Variant::Structure(st) = elem {
                let fields = st.fields();
                if fields.len() >= 2
                    && let Variant::U32(node_id) = &fields[0]
                {
                    let mut width = None;
                    let mut height = None;
                    if let Variant::Dict(dict) = &fields[1] {
                        for (k, v) in dict.iter() {
                            if let Variant::Str(key) = k
                                && key.as_str() == "size"
                            {
                                let inner = match v {
                                    Variant::Variant(val) => &**val,
                                    other => other,
                                };
                                if let Variant::Structure(sz) = inner {
                                    let sz_fields = sz.fields();
                                    if sz_fields.len() >= 2
                                        && let (Variant::I32(w), Variant::I32(h)) =
                                            (&sz_fields[0], &sz_fields[1])
                                    {
                                        width = Some(*w as u32);
                                        height = Some(*h as u32);
                                    }
                                }
                            }
                        }
                    }
                    out.push(PipeWireStreamInfo {
                        node_id: *node_id,
                        width,
                        height,
                    });
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_streams_extracts_node_id_and_size() {
        use zbus::{Array, Signature, Structure};

        let size_struct = Structure::from((1920i32, 1080i32));
        let mut dict: HashMap<String, Variant<'_>> = HashMap::new();
        dict.insert("size".to_string(), Variant::new(size_struct));

        let stream_struct = Structure::from((42u32, dict));

        let elem_sig = Signature::try_from("(ua{sv})").unwrap();
        let mut arr = Array::new(&elem_sig);
        arr.append(Variant::from(stream_struct)).unwrap();

        let mut results = HashMap::new();
        results.insert(
            "streams".to_string(),
            OwnedVariant::try_from(Variant::from(arr)).unwrap(),
        );

        let parsed = parse_streams(&results).expect("parse streams");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].node_id, 42);
        assert_eq!(parsed[0].width, Some(1920));
        assert_eq!(parsed[0].height, Some(1080));
    }
}
