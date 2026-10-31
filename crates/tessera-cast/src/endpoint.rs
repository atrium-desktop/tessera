//! Finding a discovered peer by a human-provided needle.
//!
//! The old endpoint resolver assumed the *source* connects to the sink. Miracast
//! specifies the opposite: the source is the RTSP server and the sink connects to
//! it, so the sink's address arrives with the accepted connection and never has
//! to be resolved from the neighbour table. All that is left here is matching a
//! peer by name, address, or D-Bus path.

use crate::radio::DiscoveredPeer;

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

/// Find a peer in `peers` by matching `needle` against its path, name, or device
/// address. Matching is a case-insensitive substring search.
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
    use crate::radio::DisplayCapabilities;

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

    fn peer(path: &str, name: &str, address: &str) -> DiscoveredPeer {
        DiscoveredPeer {
            path: path.to_owned(),
            name: Some(name.to_owned()),
            device_address: Some(address.to_owned()),
            display: sink(None),
            connected: false,
            connected_interface: None,
            connected_ip: None,
        }
    }

    #[test]
    fn match_peer_finds_by_name_path_or_mac_case_insensitively() {
        let p1 = peer(
            "/net/connman/iwd/0/p2p_peers/11_22_33_44_55_66",
            "Living Room TV",
            "11:22:33:44:55:66",
        );
        let p2 = peer(
            "/net/connman/iwd/0/p2p_peers/aa_bb_cc_dd_ee_ff",
            "Bedroom Display",
            "aa:bb:cc:dd:ee:ff",
        );
        let peers = vec![p1, p2];

        assert_eq!(
            match_peer(&peers, "living"),
            PeerMatch::ExactlyOne(&peers[0])
        );
        assert_eq!(
            match_peer(&peers, "BB:CC"),
            PeerMatch::ExactlyOne(&peers[1])
        );
        assert_eq!(
            match_peer(&peers, "11_22_33"),
            PeerMatch::ExactlyOne(&peers[0])
        );
        assert_eq!(match_peer(&peers, "kitchen"), PeerMatch::None);
        assert_eq!(
            match_peer(&peers, "p2p_peers"),
            PeerMatch::Ambiguous(vec![&peers[0], &peers[1]])
        );
    }
}
