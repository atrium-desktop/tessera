//! Capability verdict algebra and the probe's report shape.
//!
//! Everything here is pure: values in, values out, no OS calls. The observing
//! layer ([`crate::observe`]) produces these values; this module decides what
//! they *mean*, so the meaning is unit-testable without a radio, a GPU, or a
//! session bus.

use std::fmt;

/// One observed fact, with the three-state honesty the probe requires.
///
/// `Unverified` is not "probably fine": it means the probe could not look —
/// a permission wall, a missing tool, an unsupported interface — and the answer
/// must be reported as unknown rather than assumed (`[INV-ARCH-83]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict<T> {
    /// The fact was observed.
    Observed(T),
    /// The fact was conclusively not present.
    Absent,
    /// The probe could not determine the fact, with the reason.
    Unverified(String),
}

impl<T> Verdict<T> {
    pub fn is_observed(&self) -> bool {
        matches!(self, Self::Observed(_))
    }

    /// Whether the probe conclusively found the fact absent (as opposed to
    /// never managing to look).
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    /// A one-line human description, for the report.
    pub fn describe(&self, present: &str, missing: &str) -> String {
        match self {
            Self::Observed(_) => present.to_owned(),
            Self::Absent => missing.to_owned(),
            Self::Unverified(reason) => format!("unverified: {reason}"),
        }
    }
}

impl<T: fmt::Display> fmt::Display for Verdict<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observed(value) => write!(formatter, "{value}"),
            Self::Absent => formatter.write_str("absent"),
            Self::Unverified(reason) => write!(formatter, "unverified ({reason})"),
        }
    }
}

/// The radio half: a P2P authority that can form a Wi-Fi Display group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadioReport {
    /// iwd's `net.connman.iwd.p2p.ServiceManager`, the WFD registration point.
    pub wfd_registration: Verdict<()>,
    /// iwd's `net.connman.iwd.p2p.Device` on at least one adapter.
    pub p2p_device: Verdict<P2pDeviceFacts>,
}

/// Facts read from `net.connman.iwd.p2p.Device`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2pDeviceFacts {
    /// Whether P2P is currently enabled on the adapter.
    pub enabled: bool,
    /// Concurrent P2P connections the adapter supports. A caster needs at
    /// least one while the station link stays up.
    pub available_connections: u16,
}

/// The protocol half: the encoder and frame source this crate needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineReport {
    /// A hardware H.264 encoder entry point (`VAEntrypointEncSlice`).
    pub h264_encoder: Verdict<()>,
    /// A PipeWire daemon socket the caster can consume frames from.
    pub pipewire: Verdict<()>,
    /// The session bus's `org.freedesktop.portal.ScreenCast`, which is how the
    /// caster obtains a virtual source without a compositor scope
    /// ([ADR-0191](../../docs/adr/0191-casting-p2p-belongs-to-the-system-wifi-authority.md)).
    pub screencast_portal: Verdict<()>,
}

/// The probe's complete, ordered verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastReport {
    pub radio: RadioReport,
    pub pipeline: PipelineReport,
}

/// Which half blocks casting, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    /// No P2P authority: nothing can form a Wi-Fi Display group.
    Radio,
    /// No encoder or no frame source: the session could form but carry nothing.
    Pipeline,
    /// Both halves are present.
    None,
    /// A half could not be inspected, so nothing can be promised either way.
    Unknown,
}

impl CastReport {
    /// Resolve the blocking half.
    ///
    /// Ordering is deliberate: a definite absence in either half is reported
    /// ahead of an unknown in the other, because a definite absence is
    /// actionable and an unknown is not. Only when nothing is definitely
    /// absent does an unknown make the whole answer unknown.
    pub fn blocker(&self) -> Blocker {
        let radio_absent =
            self.radio.wfd_registration.is_absent() || self.radio.p2p_device.is_absent();
        let pipeline_absent = self.pipeline.h264_encoder.is_absent()
            || self.pipeline.pipewire.is_absent()
            || self.pipeline.screencast_portal.is_absent();
        if radio_absent {
            return Blocker::Radio;
        }
        if pipeline_absent {
            return Blocker::Pipeline;
        }
        let unknown = matches!(self.radio.wfd_registration, Verdict::Unverified(_))
            || matches!(self.radio.p2p_device, Verdict::Unverified(_))
            || matches!(self.pipeline.h264_encoder, Verdict::Unverified(_))
            || matches!(self.pipeline.pipewire, Verdict::Unverified(_))
            || matches!(self.pipeline.screencast_portal, Verdict::Unverified(_));
        if unknown {
            Blocker::Unknown
        } else {
            Blocker::None
        }
    }

    /// Render the report for a terminal. Every line states what was *observed*;
    /// nothing is summarised into a capability the probe did not see.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("radio (Wi-Fi Display P2P authority)\n");
        out.push_str(&format!(
            "  iwd WFD registration   {}\n",
            self.radio
                .wfd_registration
                .describe("available", "no P2P service manager")
        ));
        out.push_str(&format!(
            "  iwd P2P device         {}\n",
            match &self.radio.p2p_device {
                Verdict::Observed(facts) => format!(
                    "available (enabled: {}, concurrent connections: {})",
                    facts.enabled, facts.available_connections
                ),
                Verdict::Absent => "no P2P device".to_owned(),
                Verdict::Unverified(reason) => format!("unverified: {reason}"),
            }
        ));
        out.push_str("pipeline (session + frames)\n");
        out.push_str(&format!(
            "  H.264 hardware encoder {}\n",
            self.pipeline
                .h264_encoder
                .describe("available", "no VAEntrypointEncSlice")
        ));
        out.push_str(&format!(
            "  PipeWire               {}\n",
            self.pipeline
                .pipewire
                .describe("socket present", "no daemon socket")
        ));
        out.push_str(&format!(
            "  ScreenCast portal      {}\n",
            self.pipeline
                .screencast_portal
                .describe("available", "not on the session bus")
        ));
        out.push_str(&format!(
            "verdict                    {}\n",
            match self.blocker() {
                Blocker::None => "both halves present",
                Blocker::Radio => "blocked: no Wi-Fi Display P2P authority",
                Blocker::Pipeline => "blocked: no encoder or frame source",
                Blocker::Unknown => "indeterminate: something could not be inspected",
            }
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed_radio() -> RadioReport {
        RadioReport {
            wfd_registration: Verdict::Observed(()),
            p2p_device: Verdict::Observed(P2pDeviceFacts {
                enabled: false,
                available_connections: 1,
            }),
        }
    }

    fn observed_pipeline() -> PipelineReport {
        PipelineReport {
            h264_encoder: Verdict::Observed(()),
            pipewire: Verdict::Observed(()),
            screencast_portal: Verdict::Observed(()),
        }
    }

    fn report(radio: RadioReport, pipeline: PipelineReport) -> CastReport {
        CastReport { radio, pipeline }
    }

    #[test]
    fn test_inv_arch_83_a_complete_probe_reports_no_blocker() {
        let report = report(observed_radio(), observed_pipeline());
        assert_eq!(report.blocker(), Blocker::None);
        assert!(report.render().contains("both halves present"));
    }

    #[test]
    fn test_inv_arch_83_absent_radio_blocks_even_with_a_full_pipeline() {
        let report = report(
            RadioReport {
                wfd_registration: Verdict::Absent,
                p2p_device: Verdict::Absent,
            },
            observed_pipeline(),
        );
        assert_eq!(report.blocker(), Blocker::Radio);
        assert!(report.render().contains("no Wi-Fi Display P2P authority"));
    }

    #[test]
    fn test_inv_arch_83_missing_encoder_blocks_the_pipeline() {
        let mut pipeline = observed_pipeline();
        pipeline.h264_encoder = Verdict::Absent;
        let report = report(observed_radio(), pipeline);
        assert_eq!(report.blocker(), Blocker::Pipeline);
    }

    #[test]
    fn test_inv_arch_83_an_uninspected_half_is_never_reported_as_capable() {
        // A permission wall on the radio must not read as "can cast".
        let report = report(
            RadioReport {
                wfd_registration: Verdict::Unverified("insufficient privilege".into()),
                p2p_device: Verdict::Unverified("insufficient privilege".into()),
            },
            observed_pipeline(),
        );
        assert_eq!(
            report.blocker(),
            Blocker::Unknown,
            "an unverified half must never collapse into 'no blocker'"
        );
        let rendered = report.render();
        assert!(rendered.contains("indeterminate"), "{rendered}");
        assert!(!rendered.contains("both halves present"), "{rendered}");
        assert!(
            rendered.contains("unverified: insufficient privilege"),
            "{rendered}"
        );
    }

    #[test]
    fn test_inv_arch_83_a_definite_absence_outranks_an_unknown() {
        // The radio is definitely absent and the encoder is merely unverified:
        // the actionable answer is the absence, not "indeterminate".
        let mut pipeline = observed_pipeline();
        pipeline.h264_encoder = Verdict::Unverified("vainfo missing".into());
        let report = report(
            RadioReport {
                wfd_registration: Verdict::Absent,
                p2p_device: Verdict::Absent,
            },
            pipeline,
        );
        assert_eq!(report.blocker(), Blocker::Radio);
    }
}
