//! The Wi-Fi Display Information Element and its subelement codec.
//!
//! Wire shape (Wi-Fi Display Technical Specification, "WFD IE"):
//!
//! ```text
//! Element ID   0xDD                 (vendor specific)
//! Length       1 byte               (bytes after this field)
//! OUI          0x50 0x6F 0x9A       (Wi-Fi Alliance)
//! OUI Type     0x0A                 (Wi-Fi Display)
//! Subelements  repeated: ID (1) | Length (2, little-endian) | Body
//! ```
//!
//! # Why this module preserves unknown bits and unknown subelements
//!
//! A WFD peer advertises capability in packed bitfields, and this crate is
//! written without a sink to validate against. Two rules follow, and both are
//! load-bearing ([ADR-0192](../../../docs/adr/0192-first-party-miracast-caster.md) §7):
//!
//! 1. **Unknown subelements are carried verbatim.** A decode/encode round trip
//!    must not drop what it did not understand.
//! 2. **Only the fields this source needs are named**, and each accessor states
//!    the bit position it reads. The remaining bits stay in a raw array, so a
//!    wrong assumption about an unused bit cannot corrupt a peer's advertised
//!    capability.
//!
//! The named positions below are the ones to re-check first against a real
//! sink's M1/M3 exchange.

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
        Ok(Self {
            subelements: decode_subelements(&body[4..])?,
        })
    }

    /// Encode the element, recomputing the length field.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&WFD_OUI);
        body.push(WFD_OUI_TYPE);
        for subelement in &self.subelements {
            body.push(subelement.id);
            let length = u16::try_from(subelement.body.len()).unwrap_or(u16::MAX);
            body.extend_from_slice(&length.to_le_bytes());
            body.extend_from_slice(&subelement.body);
        }
        let mut out = vec![WFD_ELEMENT_ID, u8::try_from(body.len()).unwrap_or(u8::MAX)];
        out.extend_from_slice(&body);
        out
    }
}

fn decode_subelements(mut bytes: &[u8]) -> Result<Vec<Subelement>, IeError> {
    let mut subelements = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < 3 {
            return Err(IeError::Truncated {
                wanted: 3,
                got: bytes.len(),
            });
        }
        let id = bytes[0];
        let length = usize::from(u16::from_le_bytes([bytes[1], bytes[2]]));
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
    Ok(subelements)
}

/// The device role a peer advertises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    PrimarySink,
    SecondarySink,
    Source,
    Reserved(u8),
}

impl DeviceType {
    fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0b01 => Self::PrimarySink,
            0b10 => Self::SecondarySink,
            0b11 => Self::Source,
            other => Self::Reserved(other),
        }
    }

    fn bits(self) -> u8 {
        match self {
            Self::PrimarySink => 0b01,
            Self::SecondarySink => 0b10,
            Self::Source => 0b11,
            Self::Reserved(bits) => bits & 0x03,
        }
    }
}

/// The WFD Device Information subelement.
///
/// Only two bitfields are named. Both live in the first octet:
/// `device_type` at bits 1–2 and `session_availability` at bits 3–4. **These
/// positions are the first thing to re-check against a real sink**, because a
/// mis-set role is rejected by the peer with no diagnostic that distinguishes it
/// from a radio failure. Every other bit is preserved in [`Self::raw`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceInformation {
    raw: [u8; 2],
}

impl DeviceInformation {
    /// Two octets of raw subelement body.
    pub fn new(raw: [u8; 2]) -> Self {
        Self { raw }
    }

    /// Build a device information body from the two fields this crate sets.
    pub fn from_role(device_type: DeviceType, session_available: bool) -> Self {
        let mut octet = (device_type.bits() << 1) & 0x06;
        if session_available {
            octet |= 0b0000_1000;
        }
        Self { raw: [octet, 0] }
    }

    pub fn raw(&self) -> [u8; 2] {
        self.raw
    }

    /// Bits 1–2 of the first octet.
    pub fn device_type(&self) -> DeviceType {
        DeviceType::from_bits((self.raw[0] >> 1) & 0x03)
    }

    /// Bits 3–4 of the first octet.
    pub fn session_available(&self) -> bool {
        self.raw[0] & 0b0000_1000 != 0
    }

    pub fn decode(body: &[u8]) -> Result<Self, IeError> {
        if body.len() < 2 {
            return Err(IeError::BadSubelementBody {
                id: SUBELEMENT_DEVICE_INFORMATION,
                reason: "WFD Device Information needs two octets",
            });
        }
        Ok(Self {
            raw: [body[0], body[1]],
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        self.raw.to_vec()
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

    #[test]
    fn a_source_only_device_information_round_trips() {
        let device = DeviceInformation::from_role(DeviceType::Source, true);
        let ie = WfdIe {
            subelements: vec![device.subelement()],
        };
        let bytes = ie.encode();
        assert_eq!(bytes[0], WFD_ELEMENT_ID);
        assert_eq!(&bytes[2..5], &WFD_OUI);
        assert_eq!(bytes[5], WFD_OUI_TYPE);

        let decoded = WfdIe::decode(&bytes).expect("decode");
        assert_eq!(decoded, ie);
        let information = decoded.device_information().expect("device information");
        assert_eq!(information.device_type(), DeviceType::Source);
        assert!(information.session_available());
    }

    #[test]
    fn device_type_bits_match_the_declared_positions() {
        // Bits 1-2 of the first octet.
        assert_eq!(DeviceType::from_bits(0b01), DeviceType::PrimarySink);
        assert_eq!(DeviceType::from_bits(0b10), DeviceType::SecondarySink);
        assert_eq!(DeviceType::from_bits(0b11), DeviceType::Source);
        let sink = DeviceInformation::new([0b0000_0010, 0]);
        assert_eq!(sink.device_type(), DeviceType::PrimarySink);
        let source = DeviceInformation::new([0b0000_0110, 0]);
        assert_eq!(source.device_type(), DeviceType::Source);
        // Session availability is bit 3.
        assert!(!DeviceInformation::new([0b0000_0110, 0]).session_available());
        assert!(DeviceInformation::new([0b0000_1110, 0]).session_available());
    }

    #[test]
    fn unknown_bits_and_unknown_subelements_survive_a_round_trip() {
        // A peer's advertisement with bits this crate does not interpret, plus
        // an id it does not know.
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
        let other_oui = [
            WFD_ELEMENT_ID,
            7,
            0x00,
            0x11,
            0x22, // not the Wi-Fi Alliance OUI
            WFD_OUI_TYPE,
            0,
            0,
            0,
        ];
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
    fn a_subelement_overrunning_the_element_is_an_error() {
        // A well-formed element boundary whose subelement claims more bytes than
        // remain inside it.
        let mut bytes = vec![WFD_ELEMENT_ID, 7];
        bytes.extend_from_slice(&WFD_OUI);
        bytes.push(WFD_OUI_TYPE);
        bytes.push(SUBELEMENT_DEVICE_INFORMATION);
        bytes.extend_from_slice(&99u16.to_le_bytes()); // claims 99 bytes
        assert_eq!(bytes.len(), 9, "declared length 7 covers exactly the body");
        assert!(matches!(
            WfdIe::decode(&bytes),
            Err(IeError::BadSubelementLength {
                declared: 99,
                available: 0,
                ..
            })
        ));
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
