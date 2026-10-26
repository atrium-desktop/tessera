//! Turning a discovered peer into the endpoint a socket needs.
//!
//! Discovery reports a device address and a WFD port; RTSP needs an IP. This module
//! is the join between the two, kept separate from both the radio and the socket so
//! the policy is testable: which peers can be reached, and *why not* when they
//! cannot.
//!
//! Every refusal names its own cause, because the fixes differ completely: a peer
//! that is not a sink needs a different device, an unresolved address needs the
//! group to settle, and an ambiguous one needs the operator to choose.

use crate::neighbours::{self, Neighbour, Resolution};
use crate::radio::DiscoveredPeer;

/// Where a cast would connect, or why it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// Reachable: `host:port` for RTSP.
    Ready { host: String, port: u16 },
    /// The peer advertises no Wi-Fi Display capability, so it has no RTSP port.
    NoDisplayInterface,
    /// The peer is a source, not a sink: it would not accept a cast.
    NotASink,
    /// The peer is a sink but advertises no usable port.
    NoPort,
    /// The radio reported no device address for the peer, so the neighbour table
    /// cannot be consulted.
    NoDeviceAddress,
    /// The device address is not (yet) in the neighbour table. The group has not
    /// settled, or the sink has not been seen at ARP level.
    Unresolved,
    /// A row exists but the kernel has not resolved it.
    Incomplete,
    /// Several addresses answer for this device; the operator must pick one.
    Ambiguous(Vec<String>),
    /// The device address could not be parsed.
    BadDeviceAddress(String),
    /// The neighbour table itself could not be read.
    TableUnavailable(String),
}

impl Endpoint {
    pub fn host(&self) -> Option<&str> {
        match self {
            Self::Ready { host, .. } => Some(host.as_str()),
            _ => None,
        }
    }

    pub fn port(&self) -> Option<u16> {
        match self {
            Self::Ready { port, .. } => Some(*port),
            _ => None,
        }
    }

    /// A one-line reason, for the report. `None` when the endpoint is ready.
    pub fn blocker(&self) -> Option<String> {
        match self {
            Self::Ready { .. } => None,
            Self::NoDisplayInterface => Some("advertises no Wi-Fi Display capability".to_owned()),
            Self::NotASink => Some("advertises itself as a source, not a sink".to_owned()),
            Self::NoPort => Some("advertises no usable RTSP port".to_owned()),
            Self::NoDeviceAddress => {
                Some("the radio reported no device address for this peer".to_owned())
            }
            Self::Unresolved => Some(
                "its address is not in the neighbour table yet — let the group settle, or \
                 pass --host explicitly"
                    .to_owned(),
            ),
            Self::Incomplete => Some(
                "the neighbour entry exists but is unresolved — let the group settle".to_owned(),
            ),
            Self::Ambiguous(addresses) => Some(format!(
                "several addresses answer for this device ({}); pass --host to choose",
                addresses.join(", ")
            )),
            Self::BadDeviceAddress(value) => Some(format!("device address {value:?} is not a MAC")),
            Self::TableUnavailable(reason) => {
                Some(format!("the neighbour table is unreadable: {reason}"))
            }
        }
    }
}

/// Resolve a peer's RTSP endpoint against a neighbour table.
///
/// `device` restricts the lookup to one interface (the P2P group's), which matters
/// when the same device is reachable on both the station link and the group.
pub fn resolve(peer: &DiscoveredPeer, table: &[Neighbour], device: Option<&str>) -> Endpoint {
    let Some(display) = peer.display else {
        return Endpoint::NoDisplayInterface;
    };
    if !display.sink {
        return Endpoint::NotASink;
    }
    let Some(port) = display.port.filter(|port| *port != 0) else {
        return Endpoint::NoPort;
    };
    if let Some(ip) = peer.connected_ip.as_deref().filter(|ip| !ip.is_empty()) {
        return Endpoint::Ready {
            host: ip.to_owned(),
            port,
        };
    }
    let Some(address) = peer.device_address.as_deref() else {
        return Endpoint::NoDeviceAddress;
    };
    let Some(mac) = neighbours::parse_mac(address) else {
        return Endpoint::BadDeviceAddress(address.to_owned());
    };
    match neighbours::resolve(&mac, table, device) {
        Resolution::Found(ip) => Endpoint::Ready {
            host: ip.to_string(),
            port,
        },
        Resolution::NotPresent => Endpoint::Unresolved,
        Resolution::Incomplete => Endpoint::Incomplete,
        Resolution::Ambiguous(addresses) => {
            Endpoint::Ambiguous(addresses.iter().map(ToString::to_string).collect())
        }
    }
}

/// Resolve using the live neighbour table.
pub fn resolve_live(peer: &DiscoveredPeer, device: Option<&str>) -> Endpoint {
    match neighbours::read_arp() {
        Ok(table) => resolve(peer, &table, device),
        Err(error) => Endpoint::TableUnavailable(error.to_string()),
    }
}

/// What happened when searching for a peer by needle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerMatch<'a> {
    /// Exactly one peer matched.
    ExactlyOne(&'a DiscoveredPeer),
    /// No peer matched the needle.
    None,
    /// Several peers matched; the operator must choose.
    Ambiguous(Vec<&'a DiscoveredPeer>),
}

/// Find a peer in `peers` by matching `needle` against its path, name, or device address.
/// Matching is case-insensitive substring search.
pub fn match_peer<'a>(peers: &'a [DiscoveredPeer], needle: &str) -> PeerMatch<'a> {
    let needle_lower = needle.to_lowercase();
    let matches: Vec<&'a DiscoveredPeer> = peers
        .iter()
        .filter(|peer| {
            peer.path.to_lowercase().contains(&needle_lower)
                || peer
                    .name
                    .as_deref()
                    .is_some_and(|name| name.to_lowercase().contains(&needle_lower))
                || peer
                    .device_address
                    .as_deref()
                    .is_some_and(|addr| addr.to_lowercase().contains(&needle_lower))
        })
        .collect();
    match matches.len() {
        0 => PeerMatch::None,
        1 => PeerMatch::ExactlyOne(matches[0]),
        _ => PeerMatch::Ambiguous(matches),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neighbours::parse_arp;
    use crate::radio::DisplayCapabilities;
    use std::net::Ipv4Addr;

    const TABLE: &str = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.49.1     0x1         0x2         0a:11:22:33:44:55     *        p2p-wlan0-0
10.0.0.9         0x1         0x2         0a:11:22:33:44:55     *        wlan0
192.168.49.8     0x1         0x0         0b:11:22:33:44:55     *        p2p-wlan0-0
";

    fn table() -> Vec<Neighbour> {
        parse_arp(TABLE).expect("parse")
    }

    fn peer(display: Option<DisplayCapabilities>, device_address: Option<&str>) -> DiscoveredPeer {
        DiscoveredPeer {
            path: "/net/connman/iwd/0/p2p_peers/peer1".to_owned(),
            name: Some("Living Room TV".to_owned()),
            device_address: device_address.map(str::to_owned),
            display,
            connected: false,
            connected_interface: None,
            connected_ip: None,
        }
    }

    fn sink(port: Option<u16>) -> Option<DisplayCapabilities> {
        Some(DisplayCapabilities {
            source: false,
            sink: true,
            port,
            has_audio: false,
            has_uibc: false,
            has_content_protection: false,
        })
    }

    #[test]
    fn a_sink_with_a_resolved_address_produces_a_usable_endpoint() {
        let endpoint = resolve(
            &peer(sink(Some(7236)), Some("0a:11:22:33:44:55")),
            &table(),
            Some("p2p-wlan0-0"),
        );
        assert_eq!(
            endpoint,
            Endpoint::Ready {
                host: "192.168.49.1".to_owned(),
                port: 7236
            }
        );
        assert_eq!(endpoint.host(), Some("192.168.49.1"));
        assert_eq!(endpoint.port(), Some(7236));
        assert_eq!(endpoint.blocker(), None);
    }

    #[test]
    fn the_group_interface_is_preferred_over_the_station_link() {
        // The same device is reachable on both; a cast must use the group address.
        let peer = peer(sink(Some(7236)), Some("0a:11:22:33:44:55"));
        assert_eq!(
            resolve(&peer, &table(), Some("p2p-wlan0-0")),
            Endpoint::Ready {
                host: "192.168.49.1".to_owned(),
                port: 7236
            }
        );
        assert!(matches!(
            resolve(&peer, &table(), None),
            Endpoint::Ambiguous(_)
        ));
    }

    #[test]
    fn each_refusal_names_a_different_cause() {
        assert_eq!(
            resolve(&peer(None, Some("0a:11:22:33:44:55")), &table(), None),
            Endpoint::NoDisplayInterface
        );
        let source_only = Some(DisplayCapabilities {
            source: true,
            sink: false,
            port: Some(7236),
            has_audio: false,
            has_uibc: false,
            has_content_protection: false,
        });
        assert_eq!(
            resolve(
                &peer(source_only, Some("0a:11:22:33:44:55")),
                &table(),
                None
            ),
            Endpoint::NotASink
        );
        assert_eq!(
            resolve(&peer(sink(None), Some("0a:11:22:33:44:55")), &table(), None),
            Endpoint::NoPort
        );
        assert_eq!(
            resolve(
                &peer(sink(Some(0)), Some("0a:11:22:33:44:55")),
                &table(),
                None
            ),
            Endpoint::NoPort,
            "port 0 is not a usable session endpoint"
        );
        assert_eq!(
            resolve(&peer(sink(Some(7236)), None), &table(), None),
            Endpoint::NoDeviceAddress
        );
        assert_eq!(
            resolve(&peer(sink(Some(7236)), Some("not-a-mac")), &table(), None),
            Endpoint::BadDeviceAddress("not-a-mac".to_owned())
        );
        assert_eq!(
            resolve(
                &peer(sink(Some(7236)), Some("ff:ff:ff:ff:ff:ff")),
                &table(),
                None
            ),
            Endpoint::Unresolved
        );
        assert_eq!(
            resolve(
                &peer(sink(Some(7236)), Some("0b:11:22:33:44:55")),
                &table(),
                None
            ),
            Endpoint::Incomplete,
            "a row without ATF_COM is not an address"
        );
    }

    #[test]
    fn a_blocker_names_the_operator_action_not_just_the_failure() {
        let unresolved = Endpoint::Unresolved.blocker().expect("blocker");
        assert!(unresolved.contains("let the group settle"), "{unresolved}");
        assert!(unresolved.contains("--host"), "{unresolved}");
        let ambiguous = Endpoint::Ambiguous(vec![
            Ipv4Addr::new(10, 0, 0, 9).to_string(),
            Ipv4Addr::new(192, 168, 49, 1).to_string(),
        ])
        .blocker()
        .expect("blocker");
        assert!(ambiguous.contains("10.0.0.9, 192.168.49.1"), "{ambiguous}");
        assert!(ambiguous.contains("--host"), "{ambiguous}");
    }

    #[test]
    fn a_ready_endpoint_has_no_blocker() {
        assert_eq!(
            Endpoint::Ready {
                host: "192.168.49.1".to_owned(),
                port: 7236
            }
            .blocker(),
            None
        );
    }

    #[test]
    fn a_peer_with_connected_ip_is_ready_without_arp_table() {
        let mut p = peer(sink(Some(7236)), None);
        p.connected = true;
        p.connected_ip = Some("192.168.49.2".to_owned());
        let endpoint = resolve(&p, &[], None);
        assert_eq!(
            endpoint,
            Endpoint::Ready {
                host: "192.168.49.2".to_owned(),
                port: 7236
            }
        );
    }

    #[test]
    fn match_peer_finds_by_name_path_or_mac_case_insensitively() {
        let p1 = DiscoveredPeer {
            path: "/net/connman/iwd/0/p2p_peers/11_22_33_44_55_66".to_owned(),
            name: Some("Living Room TV".to_owned()),
            device_address: Some("11:22:33:44:55:66".to_owned()),
            display: sink(Some(7236)),
            connected: false,
            connected_interface: None,
            connected_ip: None,
        };
        let p2 = DiscoveredPeer {
            path: "/net/connman/iwd/0/p2p_peers/aa_bb_cc_dd_ee_ff".to_owned(),
            name: Some("Bedroom Display".to_owned()),
            device_address: Some("aa:bb:cc:dd:ee:ff".to_owned()),
            display: sink(Some(7236)),
            connected: false,
            connected_interface: None,
            connected_ip: None,
        };
        let peers = vec![p1, p2];

        // Match by name
        assert_eq!(
            match_peer(&peers, "living"),
            PeerMatch::ExactlyOne(&peers[0])
        );
        // Match by address
        assert_eq!(
            match_peer(&peers, "BB:CC"),
            PeerMatch::ExactlyOne(&peers[1])
        );
        // Match by path
        assert_eq!(
            match_peer(&peers, "11_22_33"),
            PeerMatch::ExactlyOne(&peers[0])
        );
        // None
        assert_eq!(match_peer(&peers, "kitchen"), PeerMatch::None);
        // Ambiguous (both have "peers" in path)
        assert_eq!(
            match_peer(&peers, "p2p_peers"),
            PeerMatch::Ambiguous(vec![&peers[0], &peers[1]])
        );
    }
}
