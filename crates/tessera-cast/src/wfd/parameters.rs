//! WFD session parameters and their wire validation.
//!
//! # What this module deliberately does *not* do
//!
//! It does not encode `wfd_video_formats`' bitfields. That layout packs native
//! and preferred-display resolutions, micro-steps, latency, slice settings and
//! profile/level codes into a long hex string; getting one bit wrong makes a
//! sink reject the session with no diagnostic that distinguishes it from a radio
//! failure. Without a real sink trace to check against, inventing those bits
//! would be fabrication, not implementation ([ADR-0192](../../../docs/adr/0192-first-party-miracast-caster.md) §7).
//!
//! So each parameter is a **caller-supplied string** whose *surface form* is
//! validated here. The one validation that is not cosmetic is injection
//! defence: the body is line-based, so a `\r`, `\n` or `NUL` inside a value
//! would let it forge a further parameter and impersonate the peer's own
//! negotiation. That is rejected unconditionally.

use std::collections::BTreeMap;

use super::rtsp::write_parameters;

/// Longest accepted parameter value, bytes. Generous for a video-format list,
/// far short of anything that could be mistaken for a payload.
pub const MAX_VALUE_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterError {
    /// The value contains a byte that would break the line-based body.
    InjectedByte { name: &'static str, byte: u8 },
    /// The value is not printable ASCII (the media type is `text/parameters`).
    NotAscii { name: &'static str },
    /// The value is longer than [`MAX_VALUE_LEN`].
    TooLong { name: &'static str, len: usize },
}

impl std::fmt::Display for ParameterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InjectedByte { name, byte } => write!(
                formatter,
                "{name} contains byte 0x{byte:02x}, which would break the parameter body"
            ),
            Self::NotAscii { name } => write!(formatter, "{name} is not printable ASCII"),
            Self::TooLong { name, len } => {
                write!(formatter, "{name} is {len} bytes (limit {MAX_VALUE_LEN})")
            }
        }
    }
}

impl std::error::Error for ParameterError {}

/// The parameters a WFD **source** advertises in M4, plus the presentation
/// identity a sink uses for `SETUP`.
///
/// Defaults describe a source that sends H.264 video only, with no content
/// protection, no UIBC, and no audio — the non-goals recorded in ADR-0192. The
/// video format list has no default: it cannot be invented, so the caller must
/// supply it (from configuration, or from a captured sink negotiation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceParameters {
    /// e.g. `rtsp://192.168.49.1/wfd1.0/streamid=0`. Empty until the radio
    /// layer knows the P2P interface address.
    pub presentation_url: String,
    /// UDP port the source receives RTP on, e.g. `19000`.
    pub rtp_port: u16,
    /// The `wfd_video_formats` value. Required; never defaulted.
    pub video_formats: String,
}

impl SourceParameters {
    pub fn new(video_formats: impl Into<String>) -> Self {
        Self {
            presentation_url: String::new(),
            rtp_port: 19_000,
            video_formats: video_formats.into(),
        }
    }

    /// Set the presentation URL the sink will `SETUP`.
    pub fn with_presentation_url(mut self, url: impl Into<String>) -> Self {
        self.presentation_url = url.into();
        self
    }

    /// Set the RTP receive port.
    pub fn with_rtp_port(mut self, port: u16) -> Self {
        self.rtp_port = port;
        self
    }

    /// The parameter map sent as the M4 body.
    ///
    /// `wfd_client_rtp_ports` uses the WFD form
    /// `RTP/AVP/UDP;unicast <port> 0 mode=play`. `wfd_audio_codecs` and
    /// `wfd_uibc_capability` state `none` rather than being omitted: a sink must
    /// be able to read a refusal, not infer one from silence.
    pub fn to_map(&self) -> Result<BTreeMap<String, String>, ParameterError> {
        let mut map = BTreeMap::new();
        map.insert("wfd_video_formats".to_owned(), self.video_formats.clone());
        map.insert("wfd_audio_codecs".to_owned(), "none".to_owned());
        map.insert("wfd_uibc_capability".to_owned(), "none".to_owned());
        map.insert("wfd_content_protection".to_owned(), "none".to_owned());
        map.insert(
            "wfd_client_rtp_ports".to_owned(),
            format!("RTP/AVP/UDP;unicast {} 0 mode=play", self.rtp_port),
        );
        if !self.presentation_url.is_empty() {
            map.insert(
                "wfd_presentation_URL".to_owned(),
                format!("{} none", self.presentation_url),
            );
        }
        for (name, value) in &map {
            validate_value(name, value)?;
        }
        Ok(map)
    }

    /// The M4 body bytes.
    pub fn to_body(&self) -> Result<Vec<u8>, ParameterError> {
        Ok(write_parameters(&self.to_map()?))
    }
}

/// Validate one parameter value against the line-based body's constraints.
pub fn validate_value(name: &str, value: &str) -> Result<(), ParameterError> {
    if value.len() > MAX_VALUE_LEN {
        return Err(ParameterError::TooLong {
            name: leak(name),
            len: value.len(),
        });
    }
    for byte in value.bytes() {
        if matches!(byte, b'\r' | b'\n' | 0) {
            return Err(ParameterError::InjectedByte {
                name: leak(name),
                byte,
            });
        }
        if !(0x20..=0x7e).contains(&byte) {
            return Err(ParameterError::NotAscii { name: leak(name) });
        }
    }
    Ok(())
}

/// Widest relevant WFD trigger methods, as sent in `wfd_trigger_method`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerMethod {
    Setup,
    Teardown,
    Play,
    Pause,
}

impl TriggerMethod {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "SETUP" => Some(Self::Setup),
            "TEARDOWN" => Some(Self::Teardown),
            "PLAY" => Some(Self::Play),
            "PAUSE" => Some(Self::Pause),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Setup => "SETUP",
            Self::Teardown => "TEARDOWN",
            Self::Play => "PLAY",
            Self::Pause => "PAUSE",
        }
    }
}

/// Parameter names are `&'static str` at every call site; this keeps the error
/// type allocation-free without widening its signature.
fn leak(name: &str) -> &'static str {
    match name {
        "wfd_video_formats" => "wfd_video_formats",
        "wfd_audio_codecs" => "wfd_audio_codecs",
        "wfd_uibc_capability" => "wfd_uibc_capability",
        "wfd_content_protection" => "wfd_content_protection",
        "wfd_client_rtp_ports" => "wfd_client_rtp_ports",
        "wfd_presentation_URL" => "wfd_presentation_URL",
        _ => "parameter",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wfd::rtsp::parse_parameters;

    #[test]
    fn the_advertised_set_states_its_refusals_explicitly() {
        let parameters = SourceParameters::new("00 00 02 10")
            .with_presentation_url("rtsp://192.168.49.1/wfd1.0/streamid=0")
            .with_rtp_port(19_000);
        let map = parameters.to_map().expect("map");
        assert_eq!(map["wfd_audio_codecs"], "none");
        assert_eq!(map["wfd_uibc_capability"], "none");
        assert_eq!(map["wfd_content_protection"], "none");
        assert_eq!(
            map["wfd_client_rtp_ports"],
            "RTP/AVP/UDP;unicast 19000 0 mode=play"
        );
        assert_eq!(
            map["wfd_presentation_URL"],
            "rtsp://192.168.49.1/wfd1.0/streamid=0 none"
        );
    }

    #[test]
    fn the_body_round_trips_through_the_parameters_parser() {
        let parameters = SourceParameters::new("00 00 02 10")
            .with_presentation_url("rtsp://10.0.0.1/wfd1.0/streamid=0");
        let body = parameters.to_body().expect("body");
        let parsed = parse_parameters(&body).expect("parse");
        assert_eq!(parsed, parameters.to_map().expect("map"));
    }

    #[test]
    fn an_unset_presentation_url_is_omitted_not_blank() {
        let parameters = SourceParameters::new("00 00 02 10");
        let map = parameters.to_map().expect("map");
        assert!(
            !map.contains_key("wfd_presentation_URL"),
            "a blank URL must be absent, so a sink never SETUPs to nothing"
        );
    }

    #[test]
    fn a_value_carrying_a_line_break_is_rejected_as_injection() {
        // The attack: the video-format value forges a second parameter, making
        // the source look like it negotiated content protection it refuses.
        let attack = SourceParameters::new("00 00 02 10\r\nwfd_content_protection: HDCP2.2");
        assert!(matches!(
            attack.to_map(),
            Err(ParameterError::InjectedByte {
                name: "wfd_video_formats",
                byte: b'\r'
            })
        ));

        let attack = SourceParameters::new("00 00 02 10\nwfd_audio_codecs: AAC");
        assert!(matches!(
            attack.to_map(),
            Err(ParameterError::InjectedByte { byte: b'\n', .. })
        ));
    }

    #[test]
    fn a_url_carrying_a_nul_or_non_ascii_is_rejected() {
        let with_nul = SourceParameters::new("00").with_presentation_url("rtsp://x/\0streamid=0");
        assert!(matches!(
            with_nul.to_map(),
            Err(ParameterError::InjectedByte { byte: 0, .. })
        ));
        let with_non_ascii = SourceParameters::new("00").with_presentation_url("rtsp://ünicode/x");
        assert!(matches!(
            with_non_ascii.to_map(),
            Err(ParameterError::NotAscii { .. })
        ));
    }

    #[test]
    fn an_oversized_value_is_rejected_before_it_reaches_the_wire() {
        let oversized = SourceParameters::new("00".repeat(MAX_VALUE_LEN));
        assert!(matches!(
            oversized.to_map(),
            Err(ParameterError::TooLong { len, .. }) if len > MAX_VALUE_LEN
        ));
    }

    #[test]
    fn trigger_methods_parse_exactly() {
        assert_eq!(TriggerMethod::parse("SETUP"), Some(TriggerMethod::Setup));
        assert_eq!(
            TriggerMethod::parse(" TEARDOWN "),
            Some(TriggerMethod::Teardown)
        );
        assert_eq!(
            TriggerMethod::parse("setup"),
            None,
            "the wire value is upper-case"
        );
        assert_eq!(TriggerMethod::parse("REBOOT"), None);
        assert_eq!(TriggerMethod::Setup.as_str(), "SETUP");
    }
}
