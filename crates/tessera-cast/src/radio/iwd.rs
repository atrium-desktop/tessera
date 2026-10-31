//! The iwd implementation of [`P2pRadio`].
//!
//! # What is verified, and against what
//!
//! `enable_p2p`, `register_source`, `request_discovery`, and `peers` are all
//! exercised against the live iwd on the reference machine
//! (`iwlwifi`, iwd exposing `net.connman.iwd.p2p.{ServiceManager,Device}`), and
//! the D-Bus contract below is what the daemon actually accepted — not what its
//! documentation suggests:
//!
//! - `RegisterDisplayService` takes `a{sv}` and returns **nothing**;
//! - `UnregisterDisplayService` takes **no arguments** (the registration belongs
//!   to the connection);
//! - iwd **rejects unknown keys** with `InvalidArguments`: `Source`, `Sink`,
//!   `Port`, `HasUIBC` and `HasContentProtection` are accepted, while `HasAudio`
//!   is refused whatever its type.
//!
//! # What is still unverified
//!
//! Reading a *real* peer's `p2p.Display` properties, and everything past peer
//! enumeration: group formation and the WFD session against a sink
//! ([ADR-0192](../../../docs/adr/0192-first-party-miracast-caster.md) §8). No sink
//! has been in range on the machine this was written on.

use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::{DiscoveredPeer, DisplayCapabilities, P2pRadio};
use crate::iwd::{
    self, IwdError, P2P_DEVICE, P2P_DISPLAY, P2P_PEER, P2P_SERVICE_MANAGER, SIMPLE_CONFIGURATION,
};

/// A caster's radio, backed by one iwd adapter.
pub struct IwdRadio {
    connection: Connection,
    adapter: String,
    /// Whether this connection currently holds a display-service registration.
    /// iwd's registration is per connection and unregisters without an argument,
    /// so a flag is the complete state.
    registered: std::sync::Mutex<bool>,
    /// The adapter's `Enabled` value before this caster touched it, when this
    /// caster is the one that flipped it. Restored on drop: a discovery run must
    /// not leave the radio in a different state than it found it.
    enabled_prior: std::sync::Mutex<Option<bool>>,
}

impl IwdRadio {
    /// Open the radio for the first adapter iwd reports.
    pub fn open() -> Result<Self, IwdError> {
        let connection = iwd::system_connection()?;
        let adapter = iwd::first_adapter(&connection)?;
        Ok(Self {
            connection,
            adapter,
            registered: std::sync::Mutex::new(false),
            enabled_prior: std::sync::Mutex::new(None),
        })
    }

    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// Build and send the display-service dictionary.
    ///
    /// `include_optional` is what makes the degradation explicit: the core keys
    /// (`Source`, `Sink`, `Port`) are always present, while the capability keys
    /// are only added on the first attempt.
    fn register_with(
        &self,
        advertisement: &DisplayCapabilities,
        include_optional: bool,
    ) -> Result<(), IwdError> {
        let mut properties: std::collections::HashMap<&str, Value<'_>> =
            std::collections::HashMap::new();
        // Source-only is the deliberate dual-role mitigation (ADR-0192 §4), so
        // `Sink` is sent as an explicit `false` rather than omitted — the core
        // keys are exactly what every iwd release accepts.
        properties.insert("Source", Value::Bool(advertisement.source));
        properties.insert("Sink", Value::Bool(advertisement.sink));
        if let Some(port) = advertisement.port {
            properties.insert("Port", Value::U16(port));
        }
        if include_optional {
            // Only advertised capabilities are named: a `false` is the daemon's
            // default, and an unrecognised key fails the whole call.
            if advertisement.has_audio {
                properties.insert("HasAudio", Value::Bool(true));
            }
            if advertisement.has_uibc {
                properties.insert("HasUIBC", Value::Bool(true));
            }
            if advertisement.has_content_protection {
                properties.insert("HasContentProtection", Value::Bool(true));
            }
        }

        self.connection
            .call_method(
                Some(iwd::IWD_SERVICE),
                iwd::IWD_ROOT,
                Some(P2P_SERVICE_MANAGER),
                "RegisterDisplayService",
                &properties,
            )
            .map_err(|error| iwd::classify(error, "RegisterDisplayService"))?;
        // Measured against iwd: the method returns no object path. The
        // registration is owned by this connection and unregistered without an
        // argument, so a flag is the whole state.
        *self.registered.lock().unwrap_or_else(|e| e.into_inner()) = true;
        Ok(())
    }

    /// The `p2p_peers` object path for this adapter, where peers appear.
    fn peers_path(&self) -> String {
        format!("{}/p2p_peers", self.adapter)
    }

    /// Read one peer's `p2p.Display` advertisement, `None` when the peer has no
    /// Display interface.
    fn read_display(&self, peer_path: &str) -> Result<Option<DisplayCapabilities>, IwdError> {
        // A peer without the interface is normal — most P2P peers are not
        // display devices — so absence is `None`, not an error.
        if !iwd::has_interface(&self.connection, peer_path, P2P_DISPLAY)? {
            return Ok(None);
        }
        let flag = |name: &str| -> bool {
            iwd::bool_property(&self.connection, peer_path, P2P_DISPLAY, name).unwrap_or(false)
        };
        let port = iwd::u16_property(&self.connection, peer_path, P2P_DISPLAY, "Port").ok();
        Ok(Some(DisplayCapabilities {
            source: flag("Source"),
            sink: flag("Sink"),
            port,
            has_audio: flag("HasAudio"),
            has_uibc: flag("HasUIBC"),
            has_content_protection: flag("HasContentProtection"),
        }))
    }
}

impl P2pRadio for IwdRadio {
    fn enable_p2p(&self) -> Result<(), IwdError> {
        if !iwd::has_interface(&self.connection, &self.adapter, P2P_DEVICE)? {
            return Err(IwdError::Unsupported(format!(
                "{P2P_DEVICE} on {}",
                self.adapter
            )));
        }
        if iwd::bool_property(&self.connection, &self.adapter, P2P_DEVICE, "Enabled")? {
            return Ok(());
        }
        iwd::set_bool_property(&self.connection, &self.adapter, P2P_DEVICE, "Enabled", true)?;
        // Remember that *this* run flipped it, so drop can put it back: a run
        // must not leave the radio in a state it did not find.
        *self.enabled_prior.lock().unwrap_or_else(|e| e.into_inner()) = Some(false);
        Ok(())
    }

    fn register_source(&self, advertisement: DisplayCapabilities) -> Result<(), IwdError> {
        if !iwd::has_interface(&self.connection, iwd::IWD_ROOT, P2P_SERVICE_MANAGER)? {
            return Err(IwdError::Unsupported(
                "iwd's P2P service manager is not present".to_owned(),
            ));
        }
        // iwd rejects a display-service dictionary containing a key it does not
        // know, with `InvalidArguments` — and which keys it knows varies between
        // releases. Measured on the reference machine: `Source`, `Sink`, `Port`,
        // `HasUIBC` and `HasContentProtection` are accepted; `HasAudio` is
        // rejected whatever its type. So a richer advertisement is attempted
        // first and degraded to the universally-accepted core, and the
        // degradation is reported rather than hidden.
        match self.register_with(&advertisement, true) {
            Ok(()) => Ok(()),
            Err(IwdError::InvalidArguments(reason)) => {
                log::warn!(
                    "radio: iwd rejected the full display advertisement ({reason}); \
                     retrying with only Source/Sink/Port"
                );
                self.register_with(&advertisement, false)
            }
            Err(error) => Err(error),
        }
    }

    fn unregister_source(&self) -> Result<(), IwdError> {
        // Measured against iwd: this method takes no arguments and returns
        // nothing; the registration belongs to this connection.
        let result = self
            .connection
            .call_method(
                Some(iwd::IWD_SERVICE),
                iwd::IWD_ROOT,
                Some(P2P_SERVICE_MANAGER),
                "UnregisterDisplayService",
                &(),
            )
            .map(|_| ())
            .map_err(|error| iwd::classify(error, "UnregisterDisplayService"));
        if result.is_ok() {
            *self.registered.lock().unwrap_or_else(|e| e.into_inner()) = false;
        }
        result
    }

    fn request_discovery(&self) -> Result<(), IwdError> {
        self.connection
            .call_method(
                Some(iwd::IWD_SERVICE),
                self.adapter.as_str(),
                Some(P2P_DEVICE),
                "RequestDiscovery",
                &(),
            )
            .map(|_| ())
            .map_err(|error| iwd::classify(error, "RequestDiscovery"))
    }

    fn peers(&self) -> Result<Vec<DiscoveredPeer>, IwdError> {
        let path = self.peers_path();
        // A fresh adapter with no discovery run has no peers object yet; that is
        // an empty list, not a failure.
        let xml = match iwd::introspect(&self.connection, &path) {
            Ok(xml) => xml,
            Err(IwdError::Unsupported(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut peers = Vec::new();
        for line in xml.lines() {
            let trimmed = line.trim();
            let Some(rest) = trimmed.strip_prefix("<node name=\"") else {
                continue;
            };
            let Some(name) = rest.strip_suffix("\"/>") else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let peer_path = format!("{path}/{name}");
            let display = self.read_display(&peer_path)?;
            let address = iwd::string_property(&self.connection, &peer_path, P2P_PEER, "Address")
                .or_else(|_| {
                    iwd::string_property(&self.connection, &peer_path, P2P_PEER, "DeviceAddress")
                })
                .ok();
            let connected = iwd::bool_property(&self.connection, &peer_path, P2P_PEER, "Connected")
                .unwrap_or(false);
            let connected_interface =
                iwd::string_property(&self.connection, &peer_path, P2P_PEER, "ConnectedInterface")
                    .ok();
            let connected_ip =
                iwd::string_property(&self.connection, &peer_path, P2P_PEER, "ConnectedIP").ok();
            peers.push(DiscoveredPeer {
                name: iwd::string_property(&self.connection, &peer_path, P2P_PEER, "Name").ok(),
                device_address: address,
                path: peer_path,
                display,
                connected,
                connected_interface,
                connected_ip,
            });
        }
        Ok(peers)
    }

    fn form_group(&self, peer_path: &str) -> Result<(), IwdError> {
        self.connection
            .call_method(
                Some(iwd::IWD_SERVICE),
                peer_path,
                Some(SIMPLE_CONFIGURATION),
                "PushButton",
                &(),
            )
            .map(|_| ())
            .map_err(|error| iwd::classify(error, "PushButton"))
    }

    fn disconnect_peer(&self, peer_path: &str) -> Result<(), IwdError> {
        self.connection
            .call_method(
                Some(iwd::IWD_SERVICE),
                peer_path,
                Some(P2P_PEER),
                "Disconnect",
                &(),
            )
            .map(|_| ())
            .map_err(|error| iwd::classify(error, "Disconnect"))
    }
}

impl Drop for IwdRadio {
    fn drop(&mut self) {
        // Leaving a display service registered would keep advertising this
        // machine as a source after the caster exits.
        let registered = self.registered.lock().map(|slot| *slot).unwrap_or(false);
        if registered && let Err(error) = self.unregister_source() {
            if matches!(error, crate::iwd::IwdError::Unsupported(_)) {
                log::debug!("radio: display service was already gone: {error}");
            } else {
                log::warn!("radio: could not unregister the display service: {error}");
            }
        }
        // Put the P2P switch back if this run was the one that flipped it, so a
        // diagnostic does not leave the radio changed.
        let prior = self.enabled_prior.lock().ok().and_then(|slot| *slot);
        if let Some(prior) = prior
            && let Err(error) = iwd::set_bool_property(
                &self.connection,
                &self.adapter,
                P2P_DEVICE,
                "Enabled",
                prior,
            )
        {
            if matches!(error, crate::iwd::IwdError::Unsupported(_)) {
                log::debug!("radio: P2P device was already gone: {error}");
            } else {
                log::warn!("radio: could not restore P2P Enabled={prior}: {error}");
            }
        }
    }
}
