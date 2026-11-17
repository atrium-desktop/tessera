//! The Wi-Fi Display Information Element and its subelement codec.
//!
//! Wire shape (Wi-Fi Display Technical Specification, "WFD IE"):
//!
//! ```text
//! Element ID   0xDD                 (vendor specific)
//! Length       1 byte               (bytes after this field)
//! OUI          0x50 0x6F 0x9A       (Wi-Fi Alliance)
//! OUI Type     0x0A                 (Wi-Fi Display)
//! Subelements  repeated: ID (1) | Length (2, big-endian) | Body
//! ```
//!
//! # Why this module preserves unknown bits and unknown subelements
//!
//! A WFD peer advertises capability in packed bitfields, and peers disagree on
//! the bits nobody checks. Two rules follow, and both are
//! load-bearing ([ADR-0192](../../../docs/adr/0192-first-party-miracast-caster.md) §7):
//!
//! 1. **Unknown subelements are carried verbatim.** A decode/encode round trip
//!    must not drop what it did not understand.
//! 2. **Only the fields this source needs are named**, and each accessor states
//!    the bit position it reads. The remaining bits stay in a raw array, so a
//!    wrong assumption about an unused bit cannot corrupt a peer's advertised
//!    capability.
//!
//! # Hardware evidence
//!
//! The reference Skyworth A3F_99E advertises, through NetworkManager's
//! `WifiP2PPeer.WfdIEs` (subelements only, no element header):
//!
//! ```text
//! 00 0006 0011 1c44 0032
//! ^  ^    ^    ^    ^ max throughput 50 Mbit/s
//! |  |    |    | control port 7236
//! |  |    | device information: bits 1:0 = 01 primary sink, bits 5:4 = 01 available
//! |  | length 6, big-endian
//! | subelement 0, WFD Device Information
//! ```
//!
//! An earlier revision read lengths little-endian and the role from the
//! wrong bits; it would have rejected this sink. The fixture is kept in the
//! tests so that cannot come back.

/// Vendor-specific element ID for the WFD IE.
pub const WFD_ELEMENT_ID: u8 = 0xdd;
/// Wi-Fi Alliance OUI, big-endian on the wire.
pub const WFD_OUI: [u8; 3] = [0x50, 0x6f, 0x9a];
/// WFD OUI type discriminator.
pub const WFD_OUI_TYPE: u8 = 0x0a;

/// WFD subelement ID for WFD Device Information.
pub const SUBELEMENT_DEVICE_INFORMATION: u8 = 0;
/// WFD subelement ID for Associated BSSID.
pub const SUBELEMENT_ASSOCIATED_BSSID: u8 = 1;
/// WFD subelement ID for WFD Session Information.
pub const SUBELEMENT_SESSION_INFORMATION: u8 = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IeError {
    /// The element ended before the declared length did.
    Truncated { wanted: usize, got: usize },
    /// The element id was not the vendor-specific id.
    NotVendorSpecific(u8),
    /// The OUI or OUI type was not Wi-Fi Display.
    NotWfd { oui: [u8; 3], oui_type: u8 },
    /// A subelement's declared length exceeded the remaining bytes.
    BadSubelementLength {
        id: u8,
        declared: usize,
        available: usize,
    },
    /// A subelement's declared length is impossible for its ID.
    BadSubelementBody { id: u8, reason: &'static str },
}

impl std::fmt::Display for IeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated { wanted, got } => {
                write!(
                    formatter,
                    "truncated WFD IE: wanted {wanted} bytes, got {got}"
                )
            }
            Self::NotVendorSpecific(id) => write!(formatter, "element id 0x{id:02x} is not 0xdd"),
            Self::NotWfd { oui, oui_type } => write!(
                formatter,
                "not a WFD IE: OUI {oui:02x?} type 0x{oui_type:02x}"
            ),
            Self::BadSubelementLength {
                id,
                declared,
                available,
            } => write!(
                formatter,
                "subelement {id} declares {declared} bytes, {available} remain"
            ),
            Self::BadSubelementBody { id, reason } => {
                write!(formatter, "subelement {id}: {reason}")
            }
        }
    }
}

impl std::error::Error for IeError {}

/// One WFD subelement. Unknown ids are preserved byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subelement {
    pub id: u8,
    pub body: Vec<u8>,
}

/// A decoded WFD information element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WfdIe {
    pub subelements: Vec<Subelement>,
}

impl WfdIe {
    /// Find the first subelement with `id`.
    pub fn subelement(&self, id: u8) -> Option<&Subelement> {
        self.subelements.iter().find(|sub| sub.id == id)
    }

    /// The WFD Device Information subelement, if present and well-formed.
    pub fn device_information(&self) -> Option<DeviceInformation> {
        DeviceInformation::decode(&self.subelement(SUBELEMENT_DEVICE_INFORMATION)?.body).ok()
    }

    /// Decode a whole information element from its bytes, including the
    /// element id and length.
    pub fn decode(bytes: &[u8]) -> Result<Self, IeError> {
        if bytes.len() < 2 {
            return Err(IeError::Truncated {
                wanted: 2,
                got: bytes.len(),
            });
        }
        if bytes[0] != WFD_ELEMENT_ID {
            return Err(IeError::NotVendorSpecific(bytes[0]));
        }
        let declared = usize::from(bytes[1]);
        let total = 2 + declared;
        if bytes.len() < total {
            return Err(IeError::Truncated {
                wanted: total,
                got: bytes.len(),
            });
        }
        let body = &bytes[2..total];
        if body.len() < 4 {
            return Err(IeError::NotWfd {
                oui: [0, 0, 0],
                oui_type: 0,
            });
        }
        let oui = [body[0], body[1], body[2]];
        let oui_type = body[3];
        if oui != WFD_OUI || oui_type != WFD_OUI_TYPE {
            return Err(IeError::NotWfd { oui, oui_type });
        }
        Self::decode_subelements(&body[4..])
    }

    /// Decode a bare subelement list, the shape wpa_supplicant and
    /// NetworkManager use (`WfdIEs`, `wifi-p2p.wfd-ies`): no element id, no
    /// length octet, no OUI.
    pub fn decode_subelements(mut bytes: &[u8]) -> Result<Self, IeError> {
        let mut subelements = Vec::new();
        while !bytes.is_empty() {
            if bytes.len() < 3 {
                return Err(IeError::Truncated {
                    wanted: 3,
                    got: bytes.len(),
                });
            }
            let id = bytes[0];
            let length = usize::from(u16::from_be_bytes([bytes[1], bytes[2]]));
            let start = 3;
            let end = start + length;
            if bytes.len() < end {
                return Err(IeError::BadSubelementLength {
                    id,
                    declared: length,
                    available: bytes.len() - start,
                });
            }
            subelements.push(Subelement {
                id,
                body: bytes[start..end].to_vec(),
            });
            bytes = &bytes[end..];
        }
        Ok(Self { subelements })
    }

    /// Encode the bare subelement list (see [`Self::decode_subelements`]).
    pub fn encode_subelements(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for subelement in &self.subelements {
            out.push(subelement.id);
            let length = u16::try_from(subelement.body.len()).unwrap_or(u16::MAX);
            out.extend_from_slice(&length.to_be_bytes());
            out.extend_from_slice(&subelement.body);
        }
        out
    }

    /// Encode the element, recomputing the length field.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&WFD_OUI);
        body.push(WFD_OUI_TYPE);
        body.extend_from_slice(&self.encode_subelements());
        let mut out = vec![WFD_ELEMENT_ID, u8::try_from(body.len()).unwrap_or(u8::MAX)];
        out.extend_from_slice(&body);
        out
    }
}

/// The device role a peer advertises: bits 1:0 of WFD Device Information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// `00`
    Source,
    /// `01`
    PrimarySink,
    /// `10`
    SecondarySink,
    /// `11` — source and primary sink.
    DualRole,
}

impl DeviceType {
    fn from_bits(bits: u16) -> Self {
        match bits & 0b11 {
            0b00 => Self::Source,
            0b01 => Self::PrimarySink,
            0b10 => Self::SecondarySink,
            _ => Self::DualRole,
        }
    }

    fn bits(self) -> u16 {
        match self {
            Self::Source => 0b00,
            Self::PrimarySink => 0b01,
            Self::SecondarySink => 0b10,
            Self::DualRole => 0b11,
        }
    }

    /// Whether this role can act as a source.
    pub fn is_source(self) -> bool {
        matches!(self, Self::Source | Self::DualRole)
    }

    /// Whether this role can receive a cast.
    pub fn is_sink(self) -> bool {
        !matches!(self, Self::Source)
    }
}

/// Bits 5:4 of WFD Device Information: `01` is "available for a session".
const SESSION_AVAILABILITY_SHIFT: u16 = 4;
const SESSION_AVAILABLE: u16 = 0b01;

/// The WFD Device Information subelement.
///
/// The first two octets are a big-endian 16-bit bitmap; only the device type
/// (bits 1:0) and session availability (bits 5:4) are named. Every other bit is
/// preserved in [`Self::raw`]. A full six-octet body also carries the RTSP
/// control port and the maximum throughput, both big-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceInformation {
    raw: [u8; 2],
    control_port: Option<u16>,
    max_throughput_mbps: Option<u16>,
}

impl DeviceInformation {
    /// Two octets of raw device-information bitmap, with no port or throughput.
    pub fn new(raw: [u8; 2]) -> Self {
        Self {
            raw,
            control_port: None,
            max_throughput_mbps: None,
        }
    }

    /// Build a device information body from the fields this crate sets.
    pub fn from_role(device_type: DeviceType, session_available: bool) -> Self {
        let mut bits = device_type.bits();
        if session_available {
            bits |= SESSION_AVAILABLE << SESSION_AVAILABILITY_SHIFT;
        }
        Self::new(bits.to_be_bytes())
    }

    /// Add the RTSP control port and maximum throughput (the six-octet form
    /// every peer in the field sends).
    pub fn with_endpoint(mut self, control_port: u16, max_throughput_mbps: u16) -> Self {
        self.control_port = Some(control_port);
        self.max_throughput_mbps = Some(max_throughput_mbps);
        self
    }

    pub fn raw(&self) -> [u8; 2] {
        self.raw
    }

    fn bits(&self) -> u16 {
        u16::from_be_bytes(self.raw)
    }

    /// Bits 1:0.
    pub fn device_type(&self) -> DeviceType {
        DeviceType::from_bits(self.bits())
    }

    /// Bits 5:4 equal `01`.
    pub fn session_available(&self) -> bool {
        (self.bits() >> SESSION_AVAILABILITY_SHIFT) & 0b11 == SESSION_AVAILABLE
    }

    /// The peer's advertised RTSP control port, when the body carries one.
    pub fn control_port(&self) -> Option<u16> {
        self.control_port
    }

    /// The peer's advertised maximum throughput in Mbit/s, when present.
    pub fn max_throughput_mbps(&self) -> Option<u16> {
        self.max_throughput_mbps
    }

    pub fn decode(body: &[u8]) -> Result<Self, IeError> {
        if body.len() < 2 {
            return Err(IeError::BadSubelementBody {
                id: SUBELEMENT_DEVICE_INFORMATION,
                reason: "WFD Device Information needs two octets",
            });
        }
        let word = |at: usize| {
            body.get(at..at + 2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        };
        Ok(Self {
            raw: [body[0], body[1]],
            control_port: word(2),
            max_throughput_mbps: word(4),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.raw.to_vec();
        // The body is 2, 4, or 6 octets: the bitmap, an optional control port,
        // and an optional maximum throughput. A peer that sends only the port
        // must keep it on a decode/encode round trip.
        if let Some(port) = self.control_port {
            out.extend_from_slice(&port.to_be_bytes());
        }
        if let Some(throughput) = self.max_throughput_mbps {
            out.extend_from_slice(&throughput.to_be_bytes());
        }
        out
    }

    /// This subelement, wrapped for inclusion in a [`WfdIe`].
    pub fn subelement(&self) -> Subelement {
        Subelement {
            id: SUBELEMENT_DEVICE_INFORMATION,
            body: self.encode(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference Skyworth A3F_99E's `WfdIEs`, read live from
    /// NetworkManager on 2026-11-17.
    const SKYWORTH_WFD_IES: [u8; 9] = [0x00, 0x00, 0x06, 0x00, 0x11, 0x1c, 0x44, 0x00, 0x32];

    #[test]
    fn the_reference_sink_decodes_as_an_available_primary_sink_on_7236() {
        let ie = WfdIe::decode_subelements(&SKYWORTH_WFD_IES).expect("decode");
        let info = ie.device_information().expect("device information");
        assert_eq!(info.device_type(), DeviceType::PrimarySink);
        assert!(info.device_type().is_sink());
        assert!(!info.device_type().is_source());
        assert!(info.session_available());
        assert_eq!(info.control_port(), Some(7236));
        assert_eq!(info.max_throughput_mbps(), Some(50));
        assert_eq!(ie.encode_subelements(), SKYWORTH_WFD_IES, "round trip");
    }

    #[test]
    fn a_source_advertisement_has_the_spec_bit_layout() {
        let info = DeviceInformation::from_role(DeviceType::Source, true).with_endpoint(7236, 50);
        assert_eq!(
            info.raw(),
            [0x00, 0x10],
            "type 00, availability 01 at bits 5:4"
        );
        let ie = WfdIe {
            subelements: vec![info.subelement()],
        };
        assert_eq!(
            ie.encode_subelements(),
            vec![0x00, 0x00, 0x06, 0x00, 0x10, 0x1c, 0x44, 0x00, 0x32]
        );
    }

    #[test]
    fn device_type_bits_follow_the_specification() {
        let of = |word: u16| DeviceInformation::new(word.to_be_bytes()).device_type();
        assert_eq!(of(0b00), DeviceType::Source);
        assert_eq!(of(0b01), DeviceType::PrimarySink);
        assert_eq!(of(0b10), DeviceType::SecondarySink);
        assert_eq!(of(0b11), DeviceType::DualRole);
        assert!(DeviceType::DualRole.is_sink() && DeviceType::DualRole.is_source());
        let available = |word: u16| DeviceInformation::new(word.to_be_bytes()).session_available();
        assert!(available(0b01_0000));
        assert!(!available(0b00_0000), "00 is not available");
        assert!(!available(0b10_0000), "10 is reserved, not available");
        assert!(
            !available(0b0000_1000),
            "bit 3 is coupled-sink support, not availability"
        );
    }

    #[test]
    fn a_full_element_round_trips_with_its_header() {
        let device = DeviceInformation::from_role(DeviceType::Source, true);
        let ie = WfdIe {
            subelements: vec![device.subelement()],
        };
        let bytes = ie.encode();
        assert_eq!(bytes[0], WFD_ELEMENT_ID);
        assert_eq!(&bytes[2..5], &WFD_OUI);
        assert_eq!(bytes[5], WFD_OUI_TYPE);
        assert_eq!(
            &bytes[6..9],
            &[0x00, 0x00, 0x02],
            "big-endian subelement length"
        );
        let decoded = WfdIe::decode(&bytes).expect("decode");
        assert_eq!(decoded, ie);
        let information = decoded.device_information().expect("device information");
        assert_eq!(information.device_type(), DeviceType::Source);
        assert!(information.session_available());
        assert_eq!(information.control_port(), None);
    }

    #[test]
    fn a_port_only_device_information_keeps_its_port_on_a_round_trip() {
        // A legal four-octet body: bitmap plus control port, no throughput.
        let bytes = [0x00, 0x00, 0x04, 0x00, 0x11, 0x1c, 0x44];
        let ie = WfdIe::decode_subelements(&bytes).expect("decode");
        let information = ie.device_information().expect("device information");
        assert_eq!(information.control_port(), Some(7236));
        assert_eq!(information.max_throughput_mbps(), None);
        assert_eq!(ie.encode_subelements(), bytes);
    }

    #[test]
    fn unknown_bits_and_unknown_subelements_survive_a_round_trip() {
        let raw = DeviceInformation::new([0b1111_0110, 0b1010_0101]);
        let ie = WfdIe {
            subelements: vec![
                raw.subelement(),
                Subelement {
                    id: 0x42,
                    body: vec![0xde, 0xad, 0xbe, 0xef],
                },
            ],
        };
        let decoded = WfdIe::decode(&ie.encode()).expect("decode");
        assert_eq!(
            decoded
                .device_information()
                .expect("device information")
                .raw(),
            [0b1111_0110, 0b1010_0101],
            "uninterpreted bits must be preserved verbatim"
        );
        assert_eq!(
            decoded.subelement(0x42).map(|sub| sub.body.clone()),
            Some(vec![0xde, 0xad, 0xbe, 0xef]),
            "unknown subelements must be preserved verbatim"
        );
    }

    #[test]
    fn a_non_wfd_vendor_element_is_refused_rather_than_misread() {
        let other_oui = [WFD_ELEMENT_ID, 7, 0x00, 0x11, 0x22, WFD_OUI_TYPE, 0, 0, 0];
        assert!(matches!(
            WfdIe::decode(&other_oui),
            Err(IeError::NotWfd { .. })
        ));
        assert!(matches!(
            WfdIe::decode(&[0x00, 5, 1, 2, 3, 4, 5]),
            Err(IeError::NotVendorSpecific(0x00))
        ));
    }

    #[test]
    fn a_subelement_overrunning_the_list_is_an_error() {
        let mut bytes = vec![SUBELEMENT_DEVICE_INFORMATION];
        bytes.extend_from_slice(&99u16.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x11]);
        assert!(matches!(
            WfdIe::decode_subelements(&bytes),
            Err(IeError::BadSubelementLength {
                declared: 99,
                available: 2,
                ..
            })
        ));
        assert!(matches!(
            WfdIe::decode_subelements(&[0x00, 0x00]),
            Err(IeError::Truncated { wanted: 3, got: 2 })
        ));
        assert_eq!(
            WfdIe::decode_subelements(&[]).expect("empty").subelements,
            vec![],
            "an empty list is a peer without WFD, not an error"
        );
    }

    #[test]
    fn a_truncated_element_is_an_error() {
        assert!(matches!(
            WfdIe::decode(&[WFD_ELEMENT_ID, 40, 1, 2]),
            Err(IeError::Truncated { wanted: 42, got: 4 })
        ));
    }

    #[test]
    fn a_short_device_information_body_is_refused() {
        assert!(matches!(
            DeviceInformation::decode(&[0x06]),
            Err(IeError::BadSubelementBody { .. })
        ));
    }
}
