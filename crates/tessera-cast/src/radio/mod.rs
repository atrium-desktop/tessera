//! The radio port: what a caster needs from a Wi-Fi Display P2P authority.
//!
//! The trait exists so the discovery and casting use cases can be driven by a
//! scripted authority in tests. The iwd implementation
//! ([`iwd`]) is the only production one, and it is `Unverified on hardware`.
//!
//! Nothing here owns the station link: the wireless subsystem does that
//! ([ADR-0190](../docs/adr/0190-wireless-backends-are-selected-and-capability-declared.md)).
//! The caster owns only the **P2P role** — a role iwd exposes separately, with
//! its own device object and its own connection budget.

pub mod iwd;

use crate::iwd::IwdError;

/// A peer's WiFi Display capability advertisement.
///
/// Every field is optional-in-spirit: a peer may advertise a Display interface
/// without a usable port, and a peer may not advertise Display at all. The
/// distinction between "not a sink" and "we could not read it" is kept by
/// [`DiscoveredPeer::display`] being `Option`, never a zeroed struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayCapabilities {
    pub source: bool,
    pub sink: bool,
    pub port: Option<u16>,
    pub has_audio: bool,
    pub has_uibc: bool,
    pub has_content_protection: bool,
}

impl DisplayCapabilities {
    /// What this caster advertises: a **source only**.
    ///
    /// Advertising single-role is deliberate, not an oversight: iwd refuses to
    /// connect a dual-role local device to a dual-role peer, and most Miracast
    /// sinks advertise both roles ([ADR-0192](../docs/adr/0192-first-party-miracast-caster.md) §4).
    pub fn source_only(port: u16) -> Self {
        Self {
            source: true,
            sink: false,
            port: Some(port),
            has_audio: false,
            has_uibc: false,
            has_content_protection: false,
        }
    }

    /// Whether this peer can receive our cast: it must advertise a sink and a
    /// usable RTSP port.
    ///
    /// A peer that advertises `sink` without a port is reported as not castable
    /// rather than attempted: the WFD session has nowhere to connect, and
    /// guessing a port would look like a radio failure.
    pub fn can_receive_cast(&self) -> bool {
        self.sink && self.port.is_some_and(|port| port != 0)
    }
}

/// A P2P peer the authority discovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    /// The authority's object path, used to connect to this peer.
    pub path: String,
    /// A human name, when the peer supplies one.
    pub name: Option<String>,
    /// The peer's device address, when the authority reports it.
    pub device_address: Option<String>,
    /// `None` when the peer advertises no Display interface at all (most
    /// non-Miracast P2P peers), which is not the same as advertising no sink.
    pub display: Option<DisplayCapabilities>,
    /// Whether this peer is currently connected in a P2P group.
    pub connected: bool,
    /// The network interface for the P2P group, if connected.
    pub connected_interface: Option<String>,
    /// The IP address of the peer reported by the authority, if connected.
    pub connected_ip: Option<String>,
}

impl DiscoveredPeer {
    /// A one-line reason this peer cannot receive a cast, or `None` when it can.
    ///
    /// Naming the reason is the point: "not in the list", "no Display
    /// interface", and "display but no sink" have completely different causes.
    pub fn cast_blocker(&self) -> Option<&'static str> {
        match &self.display {
            None => Some("advertises no Wi-Fi Display capability"),
            Some(display) if !display.sink => Some("advertises itself as a source, not a sink"),
            Some(display) if display.port.is_none_or(|port| port == 0) => {
                Some("advertises no RTSP port for a session")
            }
            Some(_) => None,
        }
    }
}

/// The radio operations a caster needs.
///
/// `enable_p2p` and `request_discovery` are **actions**, unlike the read-only
/// probe ([`crate::probe`]): a caller that only wants to know whether casting is
/// possible must not call them.
pub trait P2pRadio {
    /// Power the adapter's P2P role.
    fn enable_p2p(&self) -> Result<(), IwdError>;

    /// Register this caster as a WiFi Display source.
    ///
    /// iwd acknowledges with no value and unregisters per connection, so the
    /// port carries no object path: measured on the reference machine,
    /// `RegisterDisplayService(a{sv})` returns nothing and
    /// `UnregisterDisplayService()` takes no arguments.
    fn register_source(&self, advertisement: DisplayCapabilities) -> Result<(), IwdError>;

    /// Remove this connection's display-service registration.
    fn unregister_source(&self) -> Result<(), IwdError>;

    /// Begin P2P discovery.
    fn request_discovery(&self) -> Result<(), IwdError>;

    /// The P2P peers currently known to the authority.
    fn peers(&self) -> Result<Vec<DiscoveredPeer>, IwdError>;

    /// Form a P2P group with the chosen peer.
    ///
    /// Marked `Unverified on hardware`: introspected from the running iwd binary
    /// (`net.connman.iwd.SimpleConfiguration.PushButton()`), pending confirmation
    /// against a physical sink.
    fn form_group(&self, peer_path: &str) -> Result<(), IwdError>;

    /// Disconnect an active P2P group with the chosen peer.
    fn disconnect_peer(&self, peer_path: &str) -> Result<(), IwdError>;
}

/// Sort peers for display: castable peers first, then by name, then by path.
///
/// Deterministic output matters because a trace of two runs must be comparable,
/// and because an operator picks a peer from the list by position.
pub fn sort_peers(peers: &mut [DiscoveredPeer]) {
    peers.sort_by(|left, right| {
        let left_castable = left.cast_blocker().is_none();
        let right_castable = right.cast_blocker().is_none();
        right_castable
            .cmp(&left_castable)
            .then_with(|| {
                left.name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(right.name.as_deref().unwrap_or(""))
            })
            .then_with(|| left.path.cmp(&right.path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(
        path: &str,
        name: Option<&str>,
        display: Option<DisplayCapabilities>,
    ) -> DiscoveredPeer {
        DiscoveredPeer {
            path: path.to_owned(),
            name: name.map(str::to_owned),
            device_address: None,
            display,
            connected: false,
            connected_interface: None,
            connected_ip: None,
        }
    }

    #[test]
    fn a_source_only_advertisement_is_the_dual_role_mitigation() {
        let advertisement = DisplayCapabilities::source_only(19_000);
        assert!(advertisement.source);
        assert!(
            !advertisement.sink,
            "advertising sink as well would make the local device dual-role, \
             which iwd refuses to pair with a dual-role peer"
        );
        assert_eq!(advertisement.port, Some(19_000));
        assert!(
            !advertisement.can_receive_cast(),
            "a source does not receive"
        );
    }

    #[test]
    fn each_peer_blocker_names_its_own_cause() {
        assert_eq!(
            peer("/p/1", None, None).cast_blocker(),
            Some("advertises no Wi-Fi Display capability")
        );
        let source_only = DisplayCapabilities {
            source: true,
            sink: false,
            port: Some(7236),
            has_audio: false,
            has_uibc: false,
            has_content_protection: false,
        };
        assert_eq!(
            peer("/p/2", None, Some(source_only)).cast_blocker(),
            Some("advertises itself as a source, not a sink")
        );
        let no_port = DisplayCapabilities {
            source: false,
            sink: true,
            port: None,
            has_audio: false,
            has_uibc: false,
            has_content_protection: false,
        };
        assert_eq!(
            peer("/p/3", None, Some(no_port)).cast_blocker(),
            Some("advertises no RTSP port for a session")
        );
        let zero_port = DisplayCapabilities {
            port: Some(0),
            ..no_port
        };
        assert_eq!(
            peer("/p/4", None, Some(zero_port)).cast_blocker(),
            Some("advertises no RTSP port for a session"),
            "port 0 is not a usable session endpoint"
        );
    }

    #[test]
    fn a_real_sink_has_no_blocker() {
        let sink = DisplayCapabilities {
            source: false,
            sink: true,
            port: Some(7236),
            has_audio: true,
            has_uibc: true,
            has_content_protection: false,
        };
        let peer = peer("/p/1", Some("TV"), Some(sink));
        assert!(peer.cast_blocker().is_none());
        assert!(sink.can_receive_cast());
    }

    #[test]
    fn peers_sort_castable_first_then_by_name_then_by_path() {
        let sink = DisplayCapabilities {
            source: false,
            sink: true,
            port: Some(7236),
            has_audio: false,
            has_uibc: false,
            has_content_protection: false,
        };
        let mut peers = vec![
            peer("/p/z", Some("Zeta"), Some(sink)),
            peer("/p/a", None, None),
            peer("/p/b", Some("Beta"), Some(sink)),
            peer("/p/0", None, Some(sink)),
        ];
        sort_peers(&mut peers);
        let order: Vec<&str> = peers.iter().map(|peer| peer.path.as_str()).collect();
        assert_eq!(
            order,
            vec!["/p/0", "/p/b", "/p/z", "/p/a"],
            "castable first (unnamed, then Beta, then Zeta), then the non-castable"
        );
    }

    #[test]
    fn sorting_is_stable_across_runs() {
        let sink = DisplayCapabilities {
            source: false,
            sink: true,
            port: Some(1),
            has_audio: false,
            has_uibc: false,
            has_content_protection: false,
        };
        let make = || {
            vec![
                peer("/p/2", Some("Same"), Some(sink)),
                peer("/p/1", Some("Same"), Some(sink)),
                peer("/p/3", None, None),
            ]
        };
        let mut first = make();
        sort_peers(&mut first);
        let mut second = make();
        sort_peers(&mut second);
        assert_eq!(first, second);
        assert_eq!(first[0].path, "/p/1", "equal names fall back to the path");
    }
}
