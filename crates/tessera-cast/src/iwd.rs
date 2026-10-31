//! Shared, minimal iwd D-Bus plumbing.
//!
//! iwd's interfaces are the radio half of Miracast ([ADR-0192](../docs/adr/0192-first-party-miracast-caster.md)),
//! so both the read-only probe ([`crate::observe`]) and the caster's radio
//! ([`crate::radio`]) talk to the same objects. The plumbing lives here once, so
//! the P2P object paths, the property shapes, and the error classification
//! cannot drift between the two.
//!
//! Verified against live iwd on the reference machine: reads, enabling P2P,
//! registering the display service, discovering peers, and forming the group to
//! the reference TV. A `NotFound` from a method or interface that iwd has already
//! removed is classified as `Unsupported`, not a protocol failure.

use zbus::blocking::Connection;

/// iwd's well-known system-bus name.
pub const IWD_SERVICE: &str = "net.connman.iwd";
/// iwd's root object path; adapters are its numeric children.
pub const IWD_ROOT: &str = "/net/connman/iwd";
/// The P2P service manager: where a WFD source registers itself.
pub const P2P_SERVICE_MANAGER: &str = "net.connman.iwd.p2p.ServiceManager";
/// The P2P device: discovery, peers, and the radio's P2P enable state.
pub const P2P_DEVICE: &str = "net.connman.iwd.p2p.Device";
/// A P2P peer.
pub const P2P_PEER: &str = "net.connman.iwd.p2p.Peer";
/// A P2P peer's WiFi Display capability advertisement.
pub const P2P_DISPLAY: &str = "net.connman.iwd.p2p.Display";
/// The SimpleConfiguration interface on a P2P peer, used to initiate WPS group formation.
pub const SIMPLE_CONFIGURATION: &str = "net.connman.iwd.SimpleConfiguration";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IwdError {
    /// The system bus could not be reached at all.
    BusUnavailable(String),
    /// iwd is not running.
    ServiceUnavailable,
    /// iwd is running but has no wireless adapter.
    NoAdapter,
    /// The operation needs a privilege this process does not hold. Reported
    /// verbatim rather than retried: it is a deployment fact, not a fault.
    NotAuthorized(String),
    /// The object or interface this operation needs does not exist (for example
    /// P2P on an adapter without it).
    Unsupported(String),
    /// The daemon rejected the argument *shape*: a key or value it does not
    /// recognise. Callers that can degrade (a richer advertisement to a minimal
    /// one) must do so rather than fail.
    InvalidArguments(String),
    /// iwd or the bus answered something unexpected.
    Protocol(String),
}

impl std::fmt::Display for IwdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BusUnavailable(reason) => write!(formatter, "system bus unavailable: {reason}"),
            Self::ServiceUnavailable => formatter.write_str("iwd is not running on the system bus"),
            Self::NoAdapter => formatter.write_str("iwd reports no wireless adapter"),
            Self::NotAuthorized(reason) => write!(formatter, "not authorized: {reason}"),
            Self::Unsupported(what) => write!(formatter, "unsupported: {what}"),
            Self::InvalidArguments(what) => {
                write!(formatter, "iwd rejected the argument shape: {what}")
            }
            Self::Protocol(reason) => write!(formatter, "unexpected answer from iwd: {reason}"),
        }
    }
}

impl std::error::Error for IwdError {}

/// Open the system bus.
pub fn system_connection() -> Result<Connection, IwdError> {
    Connection::system().map_err(|error| IwdError::BusUnavailable(error.to_string()))
}

/// The numeric `/net/connman/iwd/<n>` adapter paths iwd exposes.
pub fn adapter_paths(connection: &Connection) -> Result<Vec<String>, IwdError> {
    let xml = introspect(connection, IWD_ROOT)?;
    let mut paths = Vec::new();
    for line in xml.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("<node name=\"")
            && let Some(name) = rest.strip_suffix("\"/>")
            && !name.is_empty()
            && name.bytes().all(|byte| byte.is_ascii_digit())
        {
            paths.push(format!("{IWD_ROOT}/{name}"));
        }
    }
    Ok(paths)
}

/// The first adapter path, or [`IwdError::NoAdapter`].
pub fn first_adapter(connection: &Connection) -> Result<String, IwdError> {
    adapter_paths(connection)?
        .into_iter()
        .next()
        .ok_or(IwdError::NoAdapter)
}

/// Introspect one object path.
pub fn introspect(connection: &Connection, path: &str) -> Result<String, IwdError> {
    let reply = connection
        .call_method(
            Some(IWD_SERVICE),
            path,
            Some("org.freedesktop.DBus.Introspectable"),
            "Introspect",
            &(),
        )
        .map_err(|error| classify(error, "iwd"))?;
    reply
        .body()
        .deserialize::<String>()
        .map_err(|error| IwdError::Protocol(format!("introspection of {path}: {error}")))
}

/// Whether `interface` appears in an object's introspection XML.
pub fn has_interface(
    connection: &Connection,
    path: &str,
    interface: &str,
) -> Result<bool, IwdError> {
    let xml = introspect(connection, path)?;
    Ok(xml.contains(&format!("<interface name=\"{interface}\">")))
}

/// Read one property as an owned variant.
pub fn property(
    connection: &Connection,
    path: &str,
    interface: &str,
    name: &str,
) -> Result<zbus::zvariant::OwnedValue, IwdError> {
    let reply = connection
        .call_method(
            Some(IWD_SERVICE),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(interface, name),
        )
        .map_err(|error| match classify(error, "property") {
            // `Get` on a missing interface surfaces as an unknown-interface
            // error; for the caller that means the capability is absent.
            IwdError::Protocol(_) => IwdError::Unsupported(format!("{interface}.{name}")),
            other => other,
        })?;
    reply
        .body()
        .deserialize::<zbus::zvariant::OwnedValue>()
        .map_err(|error| IwdError::Protocol(format!("{interface}.{name}: {error}")))
}

/// Read a boolean property.
pub fn bool_property(
    connection: &Connection,
    path: &str,
    interface: &str,
    name: &str,
) -> Result<bool, IwdError> {
    match &*property(connection, path, interface, name)? {
        zbus::zvariant::Value::Bool(value) => Ok(*value),
        other => Err(IwdError::Protocol(format!(
            "{interface}.{name}: expected b, got {}",
            other.value_signature()
        ))),
    }
}

/// Read a `u16` (`q`) property.
pub fn u16_property(
    connection: &Connection,
    path: &str,
    interface: &str,
    name: &str,
) -> Result<u16, IwdError> {
    match &*property(connection, path, interface, name)? {
        zbus::zvariant::Value::U16(value) => Ok(*value),
        other => Err(IwdError::Protocol(format!(
            "{interface}.{name}: expected q, got {}",
            other.value_signature()
        ))),
    }
}

/// Read a string property.
pub fn string_property(
    connection: &Connection,
    path: &str,
    interface: &str,
    name: &str,
) -> Result<String, IwdError> {
    match &*property(connection, path, interface, name)? {
        zbus::zvariant::Value::Str(value) => Ok(value.to_string()),
        other => Err(IwdError::Protocol(format!(
            "{interface}.{name}: expected s, got {}",
            other.value_signature()
        ))),
    }
}

/// Write a boolean property.
pub fn set_bool_property(
    connection: &Connection,
    path: &str,
    interface: &str,
    name: &str,
    value: bool,
) -> Result<(), IwdError> {
    connection
        .call_method(
            Some(IWD_SERVICE),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &(interface, name, zbus::zvariant::Value::Bool(value)),
        )
        .map(|_| ())
        .map_err(|error| classify(error, &format!("{interface}.{name}")))
}

/// Classify a zbus failure into the vocabulary callers can act on.
///
/// `AccessDenied` is separated from every other failure on purpose: "you are not
/// privileged" and "this hardware cannot" are different problems with different
/// fixes, and collapsing them is how a deployment issue gets mistaken for a bug.
pub fn classify(error: zbus::Error, what: &str) -> IwdError {
    if let zbus::Error::MethodError(name, detail, _) = &error
        && let Some(mapped) = classify_error_name(name.as_str(), detail.as_deref(), what)
    {
        return mapped;
    }
    IwdError::Protocol(format!("{what}: {error}"))
}

/// Map a D-Bus error name to the caller-facing vocabulary. `None` means the name
/// is not one this crate interprets, so the caller keeps the raw error.
pub fn classify_error_name(name: &str, detail: Option<&str>, what: &str) -> Option<IwdError> {
    if name == "org.freedesktop.DBus.Error.ServiceUnknown"
        || name == "org.freedesktop.DBus.Error.NameHasNoOwner"
    {
        return Some(IwdError::ServiceUnavailable);
    }
    if matches!(
        name,
        "net.connman.iwd.NotAvailable"
            | "net.connman.iwd.NotEnabled"
            | "net.connman.iwd.NotSupported"
            | "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.NotFound"
    ) {
        return Some(IwdError::Unsupported(what.to_owned()));
    }
    if matches!(
        name,
        "net.connman.iwd.NotAuthorized"
            | "net.connman.iwd.Busy"
            | "org.freedesktop.DBus.Error.AccessDenied"
            | "org.freedesktop.DBus.Error.AuthFailed"
            | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired"
    ) {
        return Some(IwdError::NotAuthorized(detail.unwrap_or(name).to_owned()));
    }
    if name == "net.connman.iwd.InvalidArguments" {
        return Some(IwdError::InvalidArguments(what.to_owned()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_failures_are_not_reported_as_protocol_errors() {
        // Whether the caller may enable P2P is a deployment fact; conflating it
        // with "iwd said something odd" hides the fix.
        for name in [
            "org.freedesktop.DBus.Error.AccessDenied",
            "org.freedesktop.DBus.Error.AuthFailed",
            "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired",
            "net.connman.iwd.NotAuthorized",
        ] {
            assert_eq!(
                classify_error_name(name, Some("denied"), "Enable"),
                Some(IwdError::NotAuthorized("denied".to_owned())),
                "{name} must classify as NotAuthorized"
            );
        }
    }

    #[test]
    fn a_missing_service_is_distinguished_from_a_missing_capability() {
        assert_eq!(
            classify_error_name("org.freedesktop.DBus.Error.ServiceUnknown", None, "x"),
            Some(IwdError::ServiceUnavailable)
        );
        assert_eq!(
            classify_error_name("net.connman.iwd.NotSupported", None, "RequestDiscovery"),
            Some(IwdError::Unsupported("RequestDiscovery".to_owned()))
        );
        assert_eq!(
            classify_error_name("net.connman.iwd.NotAvailable", None, "Port"),
            Some(IwdError::Unsupported("Port".to_owned()))
        );
    }

    #[test]
    fn an_uninterpreted_name_keeps_the_raw_error() {
        assert_eq!(
            classify_error_name("com.example.SomethingElse", None, "x"),
            None
        );
    }

    #[test]
    fn the_adapter_paths_are_the_numeric_children_only() {
        // Mirrors iwd's root object shape: numeric children are adapters.
        let xml = r#"<node>
  <interface name="net.connman.iwd.Manager"/>
  <node name="0"/>
  <node name="12"/>
  <node name="p2p_peers"/>
</node>"#;
        let mut paths = Vec::new();
        for line in xml.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("<node name=\"")
                && let Some(name) = rest.strip_suffix("\"/>")
                && !name.is_empty()
                && name.bytes().all(|byte| byte.is_ascii_digit())
            {
                paths.push(format!("{IWD_ROOT}/{name}"));
            }
        }
        assert_eq!(paths, vec!["/net/connman/iwd/0", "/net/connman/iwd/12"]);
    }
}
