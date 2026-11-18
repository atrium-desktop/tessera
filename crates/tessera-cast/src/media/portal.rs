//! Client for `org.freedesktop.portal.ScreenCast` over D-Bus.
//!
//! Per [ADR-0191](../../docs/adr/0191-casting-p2p-belongs-to-the-system-wifi-authority.md),
//! the caster holds **no compositor IPC scope**: frame capture passes through the
//! standard desktop portal so that user consent and seat security are honored.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};

use tessera_dbus::{Connection, FromArgs, FromValue, MessageStream, OwnedObjectPath, Proxy, Value};

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
    Bus(tessera_dbus::Error),
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

impl From<tessera_dbus::Error> for PortalError {
    fn from(err: tessera_dbus::Error) -> Self {
        Self::Bus(err)
    }
}

impl From<tessera_dbus::WireError> for PortalError {
    fn from(err: tessera_dbus::WireError) -> Self {
        Self::Bus(tessera_dbus::Error::Wire(err))
    }
}

fn receive_response_stream(
    connection: &Connection,
    req_path: &str,
) -> Result<MessageStream, PortalError> {
    let rule =
        format!("type='signal',interface='{REQUEST_IFACE}',member='Response',path='{req_path}'");
    Ok(MessageStream::for_match_rule(connection, &rule)?)
}

fn decode_response(
    msg: tessera_dbus::Message,
) -> Result<(u32, HashMap<String, Value>), PortalError> {
    let mut it = msg.body.into_iter();
    let (Some(code_val), Some(results_val)) = (it.next(), it.next()) else {
        return Err(PortalError::Protocol(
            "portal Response missing arguments".into(),
        ));
    };
    let code = u32::from_value(code_val)?;
    let results = HashMap::<String, Value>::from_value(results_val)?;
    Ok((code, results))
}

fn variant(v: Value) -> Value {
    Value::Variant(Box::new(v))
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
            .trim_start_matches(':')
            .replace('.', "_");

        let id = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();

        let session_token = format!("tessera_cast_sess_{pid}_{id}");
        let create_token = format!("tessera_cast_create_{pid}_{id}");
        let select_token = format!("tessera_cast_select_{pid}_{id}");
        let start_token = format!("tessera_cast_start_{pid}_{id}");

        let screencast = Proxy::new(connection, PORTAL_BUS, PORTAL_PATH, SCREENCAST_IFACE);

        // 1. CreateSession
        let create_req_path =
            format!("/org/freedesktop/portal/desktop/request/{sender}/{create_token}");
        let create_signals = receive_response_stream(connection, &create_req_path)?;

        let mut create_options: HashMap<String, Value> = HashMap::new();
        create_options.insert(
            "session_handle_token".into(),
            variant(Value::String(session_token)),
        );
        create_options.insert("handle_token".into(), variant(Value::String(create_token)));

        let _req_path: OwnedObjectPath = screencast.call("CreateSession", &(create_options,))?;
        let res_msg = create_signals.next().ok_or_else(|| {
            PortalError::Protocol("portal closed before CreateSession response".into())
        })?;
        let (code, create_results) = decode_response(res_msg)?;
        if code != 0 {
            return Err(PortalError::Denied(format!(
                "CreateSession returned code {code}"
            )));
        }

        let session_handle_val = create_results.get("session_handle").ok_or_else(|| {
            PortalError::Protocol("missing session_handle in CreateSession response".into())
        })?;
        let session_handle_str = match session_handle_val.unwrap_variant() {
            Value::String(s) => s.clone(),
            Value::ObjectPath(p) => p.to_string(),
            other => {
                return Err(PortalError::Protocol(format!(
                    "unexpected session_handle type: {}",
                    other.signature()
                )));
            }
        };

        // 2. SelectSources
        let select_req_path =
            format!("/org/freedesktop/portal/desktop/request/{sender}/{select_token}");
        let select_signals = receive_response_stream(connection, &select_req_path)?;

        let mut select_options: HashMap<String, Value> = HashMap::new();
        select_options.insert("handle_token".into(), variant(Value::String(select_token)));
        let available_types = screencast
            .get_property::<u32>("AvailableSourceTypes")
            .unwrap_or(1);
        let requested_type = if virtual_source && (available_types & 4 != 0) {
            4u32 // SOURCE_TYPE_VIRTUAL
        } else {
            1u32 // SOURCE_TYPE_MONITOR
        };
        select_options.insert("types".into(), variant(Value::Uint32(requested_type)));
        select_options.insert("multiple".into(), variant(Value::Bool(false)));
        let cursor_modes = screencast
            .get_property::<u32>("AvailableCursorModes")
            .unwrap_or(1);
        let cursor_mode = if cursor_modes & 2 != 0 { 2u32 } else { 1u32 };
        select_options.insert("cursor_mode".into(), variant(Value::Uint32(cursor_mode)));

        let session_path = OwnedObjectPath::try_from(session_handle_str.as_str())
            .map_err(|err| PortalError::Protocol(format!("invalid session handle path: {err}")))?;

        let _req_path: OwnedObjectPath =
            screencast.call("SelectSources", &(session_path.clone(), select_options))?;
        let res_msg = select_signals.next().ok_or_else(|| {
            PortalError::Protocol("portal closed before SelectSources response".into())
        })?;
        let (code, _) = decode_response(res_msg)?;
        if code != 0 {
            return Err(PortalError::Denied(format!(
                "SelectSources returned code {code}"
            )));
        }

        // 3. Start
        let start_req_path =
            format!("/org/freedesktop/portal/desktop/request/{sender}/{start_token}");
        let start_signals = receive_response_stream(connection, &start_req_path)?;

        let mut start_options: HashMap<String, Value> = HashMap::new();
        start_options.insert("handle_token".into(), variant(Value::String(start_token)));

        let _req_path: OwnedObjectPath =
            screencast.call("Start", &(session_path.clone(), "", start_options))?;
        let res_msg = start_signals
            .next()
            .ok_or_else(|| PortalError::Protocol("portal closed before Start response".into()))?;
        let (code, start_results) = decode_response(res_msg)?;
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
        let fd_options: HashMap<String, Value> = HashMap::new();
        let reply = connection.call_method(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            Some(SCREENCAST_IFACE),
            "OpenPipeWireRemote",
            &(session_path, fd_options),
        )?;
        let pipewire_fd: OwnedFd = FromArgs::from_args(reply.body)?;

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
        let _ = self.connection.call_method(
            Some(PORTAL_BUS),
            self.session_handle.as_str(),
            Some("org.freedesktop.portal.Session"),
            "Close",
            &(),
        );
    }
}

/// Parse streams from Start response dictionary.
fn parse_streams(results: &HashMap<String, Value>) -> Result<Vec<PipeWireStreamInfo>, PortalError> {
    let mut out = Vec::new();
    let Some(streams_val) = results.get("streams") else {
        return Ok(out);
    };

    if let Value::Array(_, arr) = streams_val.unwrap_variant() {
        for elem in arr {
            if let Value::Struct(fields) = elem.unwrap_variant()
                && fields.len() >= 2
                && let Some(node_id) = fields[0].as_u32()
            {
                let mut width = None;
                let mut height = None;
                if let Value::Array(_, entries) = fields[1].unwrap_variant() {
                    for entry in entries {
                        if let Value::DictEntry(k, v) = entry
                            && k.as_str() == Some("size")
                            && let Value::Struct(sz_fields) = v.unwrap_variant()
                            && sz_fields.len() >= 2
                            && let (Some(w), Some(h)) =
                                (sz_fields[0].as_i32(), sz_fields[1].as_i32())
                        {
                            width = Some(w as u32);
                            height = Some(h as u32);
                        }
                    }
                }
                out.push(PipeWireStreamInfo {
                    node_id,
                    width,
                    height,
                });
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
        let size_struct = Value::Struct(vec![Value::Int32(1920), Value::Int32(1080)]);
        let dict = Value::Array(
            "{sv}".into(),
            vec![Value::DictEntry(
                Box::new(Value::String("size".into())),
                Box::new(Value::Variant(Box::new(size_struct))),
            )],
        );
        let stream_struct = Value::Struct(vec![Value::Uint32(42), dict]);
        let arr = Value::Array("(ua{sv})".into(), vec![stream_struct]);

        let mut results = HashMap::new();
        results.insert("streams".to_string(), Value::Variant(Box::new(arr)));

        let parsed = parse_streams(&results).expect("parse streams");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].node_id, 42);
        assert_eq!(parsed[0].width, Some(1920));
        assert_eq!(parsed[0].height, Some(1080));
    }
}
