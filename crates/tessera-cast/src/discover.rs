//! The discovery use case: enable P2P, advertise as a source, look for sinks.
//!
//! Split from the radio so the *policy* — what order to do things in, which
//! peers count as castable, how to report a partial failure — is unit-testable
//! against a scripted authority. The radio itself needs hardware
//! ([`crate::radio::iwd`], `Unverified on hardware`).
//!
//! Failure policy, and why each choice is what it is:
//!
//! - **Enabling P2P is fatal if it fails.** Nothing downstream can work, and the
//!   error is almost always authorization, which the operator can fix.
//! - **Registering the display service is fatal.** Without it this machine is not
//!   discoverable as a source, so "no peers" would be a misleading success.
//! - **Requesting discovery is fatal.** A stale peer list is worse than none.
//! - **Reading one peer is not fatal.** A peer that vanished mid-enumeration
//!   must not discard the peers that were read: the report says so instead.

use std::time::Duration;

use crate::iwd::IwdError;
use crate::radio::{DiscoveredPeer, DisplayCapabilities, P2pRadio, sort_peers};

/// What a discovery run learned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryReport {
    /// What this machine advertised as.
    pub advertisement: DisplayCapabilities,
    /// The peers the authority knew, castable first.
    pub peers: Vec<DiscoveredPeer>,
    /// Problems that did not stop the run, in the order encountered.
    pub warnings: Vec<String>,
}

impl DiscoveryReport {
    /// Peers that could receive a cast.
    pub fn castable(&self) -> impl Iterator<Item = &DiscoveredPeer> {
        self.peers
            .iter()
            .filter(|peer| peer.cast_blocker().is_none())
    }

    /// Render the report for a terminal.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "advertising as: source (port {}, audio {}, uibc {}, content protection {})\n",
            self.advertisement
                .port
                .map(|port| port.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            if self.advertisement.has_audio {
                "yes"
            } else {
                "no"
            },
            if self.advertisement.has_uibc {
                "yes"
            } else {
                "no"
            },
            if self.advertisement.has_content_protection {
                "yes"
            } else {
                "no"
            },
        ));
        out.push_str(
            "advertising source-only: a dual-role local device is refused by iwd when the \
             peer is also dual-role\n\n",
        );
        if self.peers.is_empty() {
            out.push_str("peers: none discovered\n");
            out.push_str(
                "  if a sink is powered on and in range, its Wi-Fi Display / Screen Mirroring \
                 mode may need to be opened first\n",
            );
        } else {
            out.push_str(&format!("peers: {}\n", self.peers.len()));
            for peer in &self.peers {
                let label = peer
                    .name
                    .as_deref()
                    .or(peer.device_address.as_deref())
                    .unwrap_or("(unnamed)");
                match peer.cast_blocker() {
                    None => {
                        let display = peer.display.expect("castable peers advertise display");
                        out.push_str(&format!(
                            "  [castable] {label}  port {}{}{}{}\n",
                            display.port.unwrap_or(0),
                            if display.has_audio { "  audio" } else { "" },
                            if display.has_uibc { "  uibc" } else { "" },
                            if display.has_content_protection {
                                "  hdcp"
                            } else {
                                ""
                            },
                        ));
                        out.push_str(&format!("             {}\n", peer.path));
                    }
                    Some(reason) => {
                        out.push_str(&format!("  [skipped]  {label}  — {reason}\n"));
                        out.push_str(&format!("             {}\n", peer.path));
                    }
                }
            }
        }
        for warning in &self.warnings {
            out.push_str(&format!("warning: {warning}\n"));
        }
        out
    }
}

/// Run one discovery pass.
///
/// `settle` is how long to let the authority accumulate peers after
/// `RequestDiscovery`: discovery is asynchronous, so reading immediately reports
/// the previous run's list. It is a parameter rather than a constant so a test
/// can pass zero.
pub fn discover(
    radio: &dyn P2pRadio,
    port: u16,
    settle: Duration,
) -> Result<DiscoveryReport, IwdError> {
    let advertisement = DisplayCapabilities::source_only(port);
    radio.enable_p2p()?;
    radio.register_source(advertisement)?;
    radio.request_discovery()?;
    if !settle.is_zero() {
        std::thread::sleep(settle);
    }

    let mut warnings = Vec::new();
    let mut peers = Vec::new();
    match radio.peers() {
        Ok(found) => peers = found,
        Err(error) => {
            // A failed enumeration after a successful request is reported as a
            // warning with an empty list: the run itself worked, the answer is
            // "nothing readable", and that is what the operator must see.
            warnings.push(format!("could not enumerate peers: {error}"));
        }
    }
    sort_peers(&mut peers);
    Ok(DiscoveryReport {
        advertisement,
        peers,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A scripted authority: records calls, returns prepared answers.
    struct FakeRadio {
        calls: RefCell<Vec<&'static str>>,
        enable: Result<(), IwdError>,
        register: Result<(), IwdError>,
        discovery: Result<(), IwdError>,
        peers: Result<Vec<DiscoveredPeer>, IwdError>,
    }

    impl FakeRadio {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                enable: Ok(()),
                register: Ok(()),
                discovery: Ok(()),
                peers: Ok(Vec::new()),
            }
        }
    }

    impl P2pRadio for FakeRadio {
        fn enable_p2p(&self) -> Result<(), IwdError> {
            self.calls.borrow_mut().push("enable");
            self.enable.clone()
        }
        fn register_source(&self, advertisement: DisplayCapabilities) -> Result<(), IwdError> {
            assert!(advertisement.source, "must register as a source");
            assert!(!advertisement.sink, "must not register as dual-role");
            self.calls.borrow_mut().push("register");
            self.register.clone()
        }
        fn unregister_source(&self) -> Result<(), IwdError> {
            self.calls.borrow_mut().push("unregister");
            Ok(())
        }
        fn request_discovery(&self) -> Result<(), IwdError> {
            self.calls.borrow_mut().push("discover");
            self.discovery.clone()
        }
        fn peers(&self) -> Result<Vec<DiscoveredPeer>, IwdError> {
            self.calls.borrow_mut().push("peers");
            self.peers.clone()
        }
        fn form_group(&self, _peer_path: &str) -> Result<(), IwdError> {
            self.calls.borrow_mut().push("form_group");
            Ok(())
        }
        fn disconnect_peer(&self, _peer_path: &str) -> Result<(), IwdError> {
            self.calls.borrow_mut().push("disconnect");
            Ok(())
        }
    }

    fn sink_peer(path: &str, name: &str) -> DiscoveredPeer {
        DiscoveredPeer {
            path: path.to_owned(),
            name: Some(name.to_owned()),
            device_address: None,
            display: Some(DisplayCapabilities {
                source: false,
                sink: true,
                port: Some(7236),
                has_audio: true,
                has_uibc: false,
                has_content_protection: false,
            }),
            connected: false,
            connected_interface: None,
            connected_ip: None,
        }
    }

    #[test]
    fn the_run_order_is_enable_then_register_then_discover_then_read() {
        let radio = FakeRadio {
            peers: Ok(vec![sink_peer("/p/1", "TV")]),
            ..FakeRadio::new()
        };
        let report = discover(&radio, 19_000, Duration::ZERO).expect("discover");
        assert_eq!(
            *radio.calls.borrow(),
            vec!["enable", "register", "discover", "peers"]
        );
        assert_eq!(report.advertisement.port, Some(19_000));
        assert_eq!(report.castable().count(), 1);
    }

    #[test]
    fn a_failed_enable_stops_before_registering() {
        let radio = FakeRadio {
            enable: Err(IwdError::NotAuthorized("polkit denied".into())),
            ..FakeRadio::new()
        };
        let error = discover(&radio, 19_000, Duration::ZERO).expect_err("must fail");
        assert!(matches!(error, IwdError::NotAuthorized(_)));
        assert_eq!(
            *radio.calls.borrow(),
            vec!["enable"],
            "nothing may be attempted after P2P cannot be enabled"
        );
    }

    #[test]
    fn a_failed_registration_stops_before_discovery() {
        let radio = FakeRadio {
            register: Err(IwdError::Unsupported("no service manager".into())),
            ..FakeRadio::new()
        };
        let error = discover(&radio, 19_000, Duration::ZERO).expect_err("must fail");
        assert!(matches!(error, IwdError::Unsupported(_)));
        assert_eq!(*radio.calls.borrow(), vec!["enable", "register"]);
    }

    #[test]
    fn a_failed_discovery_request_stops_before_reading_peers() {
        let radio = FakeRadio {
            discovery: Err(IwdError::Protocol("busy".into())),
            ..FakeRadio::new()
        };
        assert!(discover(&radio, 19_000, Duration::ZERO).is_err());
        assert_eq!(
            *radio.calls.borrow(),
            vec!["enable", "register", "discover"]
        );
    }

    #[test]
    fn a_failed_peer_enumeration_is_a_warning_not_a_discarded_run() {
        let radio = FakeRadio {
            peers: Err(IwdError::Protocol("one peer vanished".into())),
            ..FakeRadio::new()
        };
        let report = discover(&radio, 19_000, Duration::ZERO).expect("the run itself worked");
        assert!(report.peers.is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("one peer vanished"));
        assert!(
            report
                .render()
                .contains("warning: could not enumerate peers")
        );
    }

    #[test]
    fn the_report_separates_castable_peers_from_skipped_ones_with_reasons() {
        let radio = FakeRadio {
            peers: Ok(vec![
                DiscoveredPeer {
                    path: "/p/other".to_owned(),
                    name: Some("Printer".to_owned()),
                    device_address: None,
                    display: None,
                    connected: false,
                    connected_interface: None,
                    connected_ip: None,
                },
                sink_peer("/p/tv", "Living Room TV"),
            ]),
            ..FakeRadio::new()
        };
        let report = discover(&radio, 19_000, Duration::ZERO).expect("discover");
        assert_eq!(report.peers.len(), 2);
        assert_eq!(
            report.peers[0].path, "/p/tv",
            "a castable sink sorts ahead of a non-display peer"
        );
        let rendered = report.render();
        assert!(
            rendered.contains("[castable] Living Room TV  port 7236  audio"),
            "{rendered}"
        );
        assert!(
            rendered.contains("[skipped]  Printer  — advertises no Wi-Fi Display capability"),
            "{rendered}"
        );
        assert!(rendered.contains("advertising source-only"), "{rendered}");
    }

    #[test]
    fn an_empty_result_suggests_the_likely_cause_instead_of_just_saying_none() {
        let radio = FakeRadio::new();
        let report = discover(&radio, 19_000, Duration::ZERO).expect("discover");
        assert!(report.peers.is_empty());
        let rendered = report.render();
        assert!(rendered.contains("peers: none discovered"), "{rendered}");
        assert!(
            rendered.contains("Screen Mirroring"),
            "an empty list must suggest the sink-side action, not just report emptiness: {rendered}"
        );
    }
}
