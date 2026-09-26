//! D-Bus accessibility provider implementing `org.a11y.atspi.Accessible` and `org.a11y.atspi.Action`.
//!
//! Enforces:
//! - `[INV-A11Y-03] Zero Data Leak on Sensitive Fields`
//! - `[INV-A11Y-04] Kernel-Authenticated A11y Peers` via caller PID check (`SO_PEERCRED`).

use std::sync::{Arc, RwLock};
use zbus::fdo;
use zbus::message::Header;

use super::auth::PeerAuthenticator;
use super::tree::ShellSemanticTree;
use tessera_primitives::accessibility::AccessibleRole;

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
    async fn authenticate_caller(
        &self,
        conn: &zbus::Connection,
        header: &Header<'_>,
    ) -> fdo::Result<()> {
        let Some(sender) = header.sender() else {
            return Err(fdo::Error::AccessDenied("missing sender header".into()));
        };

        // Resolve Unix PID of sender via org.freedesktop.DBus
        let dbus_proxy = fdo::DBusProxy::new(conn).await?;
        let pid = dbus_proxy.get_connection_unix_process_id(sender.as_str().try_into()?).await?;

        if !self.authenticator.is_authorized_pid(pid) {
            return Err(fdo::Error::AccessDenied(
                "caller is not an authorized accessibility peer".into(),
            ));
        }

        Ok(())
    }
}

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl ShellA11yService {
    /// User-facing label or name. Redacts credentials per `[INV-A11Y-03]`.
    async fn get_name(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<String> {
        self.authenticate_caller(conn, &header).await?;

        let tree = self.tree.read().map_err(|_| fdo::Error::Failed("lock poisoned".into()))?;
        let root = tree.get_node(tree.root_id());
        Ok(root.and_then(|n| n.safe_name()).unwrap_or("").to_string())
    }

    /// User-facing description.
    async fn get_description(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<String> {
        self.authenticate_caller(conn, &header).await?;

        let tree = self.tree.read().map_err(|_| fdo::Error::Failed("lock poisoned".into()))?;
        let root = tree.get_node(tree.root_id());
        Ok(root.and_then(|n| n.description.as_deref()).unwrap_or("").to_string())
    }

    /// Mapped numeric AT-SPI role.
    async fn get_role(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<u32> {
        self.authenticate_caller(conn, &header).await?;

        let tree = self.tree.read().map_err(|_| fdo::Error::Failed("lock poisoned".into()))?;
        let role = tree.get_node(tree.root_id()).map(|n| n.role).unwrap_or(AccessibleRole::Unknown);
        Ok(role as u32)
    }

    /// Direct children count.
    async fn get_child_count(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<i32> {
        self.authenticate_caller(conn, &header).await?;

        let tree = self.tree.read().map_err(|_| fdo::Error::Failed("lock poisoned".into()))?;
        let count = tree.get_node(tree.root_id()).map(|n| n.children.len()).unwrap_or(0);
        Ok(count as i32)
    }
}

#[zbus::interface(name = "org.a11y.atspi.Action")]
impl ShellA11yService {
    /// Number of available actions on the root node.
    async fn get_n_actions(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<i32> {
        self.authenticate_caller(conn, &header).await?;
        Ok(1)
    }

    /// Name of action at index.
    async fn get_action_name(
        &self,
        index: i32,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<String> {
        self.authenticate_caller(conn, &header).await?;
        if index == 0 {
            Ok("activate".into())
        } else {
            Err(fdo::Error::InvalidArgs("action index out of range".into()))
        }
    }

    /// Perform action, guarded by focus and sensitivity barrier.
    async fn do_action(
        &self,
        index: i32,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<bool> {
        self.authenticate_caller(conn, &header).await?;

        if index != 0 {
            return Ok(false);
        }

        let tree = self.tree.read().map_err(|_| fdo::Error::Failed("lock poisoned".into()))?;
        let root_id = tree.root_id();

        // Enforce action barrier
        if !tree.can_execute_action(root_id) {
            return Err(fdo::Error::AccessDenied(
                "action barrier: node is disabled or sensitive".into(),
            ));
        }

        Ok(true)
    }
}
