//! D-Bus accessibility provider implementing `org.a11y.atspi.Accessible` and `org.a11y.atspi.Action`.
//!
//! Enforces:
//! - `[INV-A11Y-03] Zero Data Leak on Sensitive Fields`
//! - `[INV-A11Y-04] Kernel-Authenticated A11y Peers` via caller PID check (`SO_PEERCRED`).

use std::sync::{Arc, RwLock};

use tessera_dbus::{Connection, FromArgs, FromValue, Message, ObjectHandler, Value};
use tessera_primitives::accessibility::AccessibleRole;

use super::auth::PeerAuthenticator;
use super::tree::ShellSemanticTree;

/// Core D-Bus accessible object for the shell hierarchy.
pub struct ShellA11yService {
    tree: Arc<RwLock<ShellSemanticTree>>,
    authenticator: Arc<PeerAuthenticator>,
}

impl ShellA11yService {
    /// Create a new A11y D-Bus service backed by the shared semantic tree.
    #[must_use]
    pub fn new(
        tree: Arc<RwLock<ShellSemanticTree>>,
        authenticator: Arc<PeerAuthenticator>,
    ) -> Self {
        Self {
            tree,
            authenticator,
        }
    }

    /// Authenticate the caller using D-Bus sender's kernel PID (`SO_PEERCRED`).
    fn authenticate_caller(
        &self,
        conn: &Connection,
        msg: &Message,
    ) -> Result<(), (String, String)> {
        let Some(sender) = msg.sender.as_deref() else {
            return Err((
                "org.freedesktop.DBus.Error.AccessDenied".into(),
                "missing sender header".into(),
            ));
        };

        let reply = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetConnectionUnixProcessID",
                &(sender,),
            )
            .map_err(|e| ("org.freedesktop.DBus.Error.Failed".into(), e.to_string()))?;
        let pid: u32 = FromArgs::from_args(reply.body)
            .map_err(|e| ("org.freedesktop.DBus.Error.Failed".into(), e.to_string()))?;

        if !self.authenticator.is_authorized_pid(pid) {
            return Err((
                "org.freedesktop.DBus.Error.AccessDenied".into(),
                "caller is not an authorized accessibility peer".into(),
            ));
        }

        Ok(())
    }
}

impl ObjectHandler for ShellA11yService {
    fn handle_method_call(
        &self,
        conn: &Connection,
        msg: &Message,
    ) -> Result<Vec<Value>, (String, String)> {
        self.authenticate_caller(conn, msg)?;

        match (msg.interface.as_deref(), msg.member.as_deref()) {
            (Some("org.a11y.atspi.Accessible"), Some("GetName")) => {
                let tree = self.tree.read().map_err(|_| {
                    (
                        "org.freedesktop.DBus.Error.Failed".into(),
                        "lock poisoned".into(),
                    )
                })?;
                let root = tree.get_node(tree.root_id());
                Ok(vec![Value::String(
                    root.and_then(|n| n.safe_name()).unwrap_or("").to_string(),
                )])
            }
            (Some("org.a11y.atspi.Accessible"), Some("GetDescription")) => {
                let tree = self.tree.read().map_err(|_| {
                    (
                        "org.freedesktop.DBus.Error.Failed".into(),
                        "lock poisoned".into(),
                    )
                })?;
                let root = tree.get_node(tree.root_id());
                Ok(vec![Value::String(
                    root.and_then(|n| n.description.as_deref())
                        .unwrap_or("")
                        .to_string(),
                )])
            }
            (Some("org.a11y.atspi.Accessible"), Some("GetRole")) => {
                let tree = self.tree.read().map_err(|_| {
                    (
                        "org.freedesktop.DBus.Error.Failed".into(),
                        "lock poisoned".into(),
                    )
                })?;
                let role = tree
                    .get_node(tree.root_id())
                    .map(|n| n.role)
                    .unwrap_or(AccessibleRole::Unknown);
                Ok(vec![Value::Uint32(role as u32)])
            }
            (Some("org.a11y.atspi.Accessible"), Some("GetChildCount")) => {
                let tree = self.tree.read().map_err(|_| {
                    (
                        "org.freedesktop.DBus.Error.Failed".into(),
                        "lock poisoned".into(),
                    )
                })?;
                let count = tree
                    .get_node(tree.root_id())
                    .map(|n| n.children.len())
                    .unwrap_or(0);
                Ok(vec![Value::Int32(count as i32)])
            }
            (Some("org.a11y.atspi.Action"), Some("GetNActions")) => Ok(vec![Value::Int32(1)]),
            (Some("org.a11y.atspi.Action"), Some("GetActionName")) => {
                let index = msg
                    .body
                    .first()
                    .cloned()
                    .ok_or_else(|| {
                        (
                            "org.freedesktop.DBus.Error.InvalidArgs".into(),
                            "missing index".into(),
                        )
                    })
                    .and_then(|v| {
                        i32::from_value(v).map_err(|e| {
                            (
                                "org.freedesktop.DBus.Error.InvalidArgs".into(),
                                e.to_string(),
                            )
                        })
                    })?;
                if index == 0 {
                    Ok(vec![Value::String("activate".into())])
                } else {
                    Err((
                        "org.freedesktop.DBus.Error.InvalidArgs".into(),
                        "action index out of range".into(),
                    ))
                }
            }
            (Some("org.a11y.atspi.Action"), Some("DoAction")) => {
                let index = msg
                    .body
                    .first()
                    .cloned()
                    .ok_or_else(|| {
                        (
                            "org.freedesktop.DBus.Error.InvalidArgs".into(),
                            "missing index".into(),
                        )
                    })
                    .and_then(|v| {
                        i32::from_value(v).map_err(|e| {
                            (
                                "org.freedesktop.DBus.Error.InvalidArgs".into(),
                                e.to_string(),
                            )
                        })
                    })?;
                if index != 0 {
                    return Ok(vec![Value::Bool(false)]);
                }
                let tree = self.tree.read().map_err(|_| {
                    (
                        "org.freedesktop.DBus.Error.Failed".into(),
                        "lock poisoned".into(),
                    )
                })?;
                let root_id = tree.root_id();
                if !tree.can_execute_action(root_id) {
                    return Err((
                        "org.freedesktop.DBus.Error.AccessDenied".into(),
                        "action barrier: node is disabled or sensitive".into(),
                    ));
                }
                Ok(vec![Value::Bool(true)])
            }
            _ => Err((
                "org.freedesktop.DBus.Error.UnknownMethod".into(),
                format!("unknown method {:?}.{:?}", msg.interface, msg.member),
            )),
        }
    }
}
