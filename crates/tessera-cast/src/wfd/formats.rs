//! The `wfd_video_formats` value: a validated, reversible token sequence with
//! annotation.
//!
//! # The line this module will not cross
//!
//! `wfd_video_formats` is a space-separated list of hex tokens whose bit
//! positions pack native and preferred-display resolutions, micro-steps, latency,
//! slice settings and profile/level codes. This crate has no sink to check the
//! layout against, and a wrong bit makes a sink reject the session
//! indistinguishably from a radio failure
//! ([ADR-0192](../../../docs/adr/0192-first-party-miracast-caster.md) §7).
//!
//! So the value is treated as **opaque data with a validated surface form**:
//!
//! - [`VideoFormats`] round-trips a given value byte-exactly;
//! - [`VideoFormats::annotate`] presents each token with its offset, hex and bit
//!   positions, plus the *declared* field name from [`FIELD_SCHEMA`];
//! - the declared schema is marked unverified and is used **only for display**.
//!   No code path here constructs a value to send: a caster must be given one.
//!
//! That makes a real sink's value checkable in one place instead of guessed in
//! several.

use std::fmt;

/// Longest accepted value, matching the parameter-value ceiling.
const MAX_LEN: usize = 1024;
/// Longest accepted single token, in bytes (16 hex digits).
const MAX_TOKEN_BYTES: usize = 8;
/// Most tokens accepted; a real advertisement is well under this.
const MAX_TOKENS: usize = 32;
/// The literal a source sends when it supports no video format.
pub const NONE: &str = "none";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The value carries no bytes at all.
    Empty,
    /// A token was not an even-length run of hex digits.
    NotHex { token: String },
    /// A token is longer than one field can be.
    TokenTooLong { token: String, bytes: usize },
    /// Too many tokens for an advertisement.
    TooManyTokens(usize),
    /// The whole value exceeds the parameter ceiling.
    TooLong(usize),
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("wfd_video_formats is empty"),
            Self::NotHex { token } => write!(
                formatter,
                "token {token:?} is not an even-length run of hex digits"
            ),
            Self::TokenTooLong { token, bytes } => write!(
                formatter,
                "token {token:?} is {bytes} bytes (limit {MAX_TOKEN_BYTES})"
            ),
            Self::TooManyTokens(count) => {
                write!(formatter, "{count} tokens (limit {MAX_TOKENS})")
            }
            Self::TooLong(len) => write!(formatter, "{len} bytes (limit {MAX_LEN})"),
        }
    }
}

impl std::error::Error for FormatError {}

/// One token of the value.
///
/// A real advertisement mixes hex fields with literal `none` fields (the latency,
/// slice and frame-rate-control positions), so `none` is a token kind — not only a
/// whole-value literal. Rejecting it would refuse a value a sink actually sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Hex(Vec<u8>),
    None,
}

impl Token {
    /// The token's bytes, or `None` for the `none` literal.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Hex(bytes) => Some(bytes),
            Self::None => None,
        }
    }

    /// Lower-case hex for a hex token, or `none`.
    pub fn hex(&self) -> String {
        match self {
            Self::Hex(bytes) => bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
            Self::None => NONE.to_owned(),
        }
    }

    /// The bit string, most significant bit of the first byte first, or empty for
    /// `none`.
    ///
    /// A bit listing is what a maintainer compares against a specification table;
    /// decimal or hex alone hides which bit is at fault.
    pub fn bits(&self) -> String {
        match self {
            Self::Hex(bytes) => bytes
                .iter()
                .map(|byte| format!("{byte:08b}"))
                .collect::<Vec<_>>()
                .join(" "),
            Self::None => String::new(),
        }
    }
}

/// A validated `wfd_video_formats` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFormats {
    tokens: Vec<Token>,
    /// Whether the original value was the literal `none`.
    none: bool,
}

impl VideoFormats {
    /// Parse a value. `none` is a legal advertisement; an empty string is not.
    pub fn parse(value: &str) -> Result<Self, FormatError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(FormatError::Empty);
        }
        if trimmed.len() > MAX_LEN {
            return Err(FormatError::TooLong(trimmed.len()));
        }
        if trimmed.eq_ignore_ascii_case(NONE) {
            return Ok(Self {
                tokens: Vec::new(),
                none: true,
            });
        }
        let mut tokens = Vec::new();
        for raw in trimmed.split_whitespace() {
            if raw.eq_ignore_ascii_case(NONE) {
                tokens.push(Token::None);
                continue;
            }
            if raw.len() % 2 != 0 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(FormatError::NotHex {
                    token: raw.to_owned(),
                });
            }
            let bytes = raw.len() / 2;
            if bytes > MAX_TOKEN_BYTES {
                return Err(FormatError::TokenTooLong {
                    token: raw.to_owned(),
                    bytes,
                });
            }
            let mut decoded = Vec::with_capacity(bytes);
            for index in 0..bytes {
                let pair = &raw[index * 2..index * 2 + 2];
                decoded.push(
                    u8::from_str_radix(pair, 16).map_err(|_| FormatError::NotHex {
                        token: raw.to_owned(),
                    })?,
                );
            }
            tokens.push(Token::Hex(decoded));
        }
        if tokens.len() > MAX_TOKENS {
            return Err(FormatError::TooManyTokens(tokens.len()));
        }
        Ok(Self {
            tokens,
            none: false,
        })
    }

    pub fn is_none(&self) -> bool {
        self.none
    }

    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    /// The canonical wire form: lower-case hex, single spaces, or `none`.
    pub fn to_wire(&self) -> String {
        if self.none {
            return NONE.to_owned();
        }
        self.tokens
            .iter()
            .map(Token::hex)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Annotate the value: one line per token with its index, offset in the wire
    /// form, hex, bits, and the *declared* field name.
    ///
    /// The walk is width-aware. A token whose length does not match the declared
    /// field is reported as a mismatch rather than given that field's name, and a
    /// token past the end of the schema is reported as unrecognised. Both make the
    /// annotation a check rather than a decoration: pasting a real sink's value
    /// shows immediately whether the assumed layout holds.
    pub fn annotate(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.none {
            lines.push("none — this source advertises no video format".to_owned());
            return lines;
        }
        let mut offset = 0;
        for (index, token) in self.tokens.iter().enumerate() {
            let width = token.bytes().map_or(0, <[u8]>::len);
            let declared = match (FIELD_SCHEMA.get(index), token) {
                // A `none` token is a literal, not a width mismatch: the field is
                // absent from this value rather than the wrong size.
                (Some(field), Token::None) => {
                    format!(
                        "  declared (unverified): {} is `none` in this value",
                        field.name
                    )
                }
                (Some(field), _) if field.bytes == width => {
                    format!("  declared (unverified): {}", field.name)
                }
                (Some(field), _) => format!(
                    "  declared (unverified): {} expects {} byte(s), this token is {width} — \
                     the value does not match the assumed layout",
                    field.name, field.bytes
                ),
                (None, _) => format!(
                    "  declared (unverified): unrecognised position (the assumed layout has {} \
                     field(s))",
                    FIELD_SCHEMA.len()
                ),
            };
            lines.push(format!(
                "[{index}] offset {offset:>3}  {}  {}\n{declared}",
                token.hex(),
                token.bits()
            ));
            offset += token.hex().len() + usize::from(index + 1 < self.tokens.len());
        }
        if self.tokens.len() < FIELD_SCHEMA.len() {
            lines.push(format!(
                "note: {} declared field(s) carry no token in this value",
                FIELD_SCHEMA.len() - self.tokens.len()
            ));
        }
        lines
    }
}

impl fmt::Display for VideoFormats {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_wire())
    }
}

/// A declared field position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: &'static str,
    /// Bytes this field occupies in the wire form.
    pub bytes: usize,
}

/// The declared field order, for annotation only.
///
/// **Unverified.** These names and widths document the layout this crate assumes
/// when it reads a value; nothing constructs a value from them, and
/// [`VideoFormats::annotate`] reports a width mismatch instead of borrowing a
/// name for a token that does not fit. Correct this table against a real sink's
/// `wfd_video_formats` and the annotation becomes exact.
pub const FIELD_SCHEMA: &[FieldSchema] = &[
    FieldSchema {
        name: "native (screen) resolution bitmap",
        bytes: 1,
    },
    FieldSchema {
        name: "preferred-display-mode-supported",
        bytes: 1,
    },
    FieldSchema {
        name: "H.264 codec profile",
        bytes: 1,
    },
    FieldSchema {
        name: "H.264 level and CEA resolution bitmap",
        bytes: 1,
    },
    FieldSchema {
        name: "CEA resolution bitmap",
        bytes: 4,
    },
    FieldSchema {
        name: "VESA resolution bitmap",
        bytes: 4,
    },
    FieldSchema {
        name: "HH resolution bitmap",
        bytes: 1,
    },
    FieldSchema {
        name: "latency",
        bytes: 2,
    },
    FieldSchema {
        name: "min-slice-size / slice-enc-params",
        bytes: 2,
    },
    FieldSchema {
        name: "frame-rate-control-support / max-hres / max-vres",
        bytes: 1,
    },
    FieldSchema {
        name: "profile-level (H.264)",
        bytes: 4,
    },
    FieldSchema {
        name: "profile-level (other codecs)",
        bytes: 4,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typical_value_round_trips_byte_exactly() {
        let wire = "00 00 02 10 00000000 00000000 00 0000 0000 00 none none";
        let formats = VideoFormats::parse(wire).expect("parse");
        assert_eq!(formats.to_wire(), wire);
        assert!(!formats.is_none());
    }

    #[test]
    fn upper_case_input_normalises_to_lower_case_wire_form() {
        let formats = VideoFormats::parse("00 00 0A FF").expect("parse");
        assert_eq!(formats.to_wire(), "00 00 0a ff");
        // And the normalised form parses to the same value.
        assert_eq!(
            VideoFormats::parse(&formats.to_wire()).expect("reparse"),
            formats
        );
    }

    #[test]
    fn the_none_literal_is_a_legal_advertisement_that_round_trips() {
        let formats = VideoFormats::parse("NONE").expect("parse");
        assert!(formats.is_none());
        assert_eq!(formats.to_wire(), "none");
        assert!(formats.annotate()[0].contains("no video format"));
    }

    #[test]
    fn an_empty_value_is_refused_rather_than_sent_as_nothing() {
        assert_eq!(VideoFormats::parse("   "), Err(FormatError::Empty));
    }

    #[test]
    fn an_odd_length_or_non_hex_token_is_refused() {
        assert!(matches!(
            VideoFormats::parse("0 00"),
            Err(FormatError::NotHex { .. })
        ));
        assert!(matches!(
            VideoFormats::parse("00 zz"),
            Err(FormatError::NotHex { .. })
        ));
    }

    #[test]
    fn an_oversized_token_or_value_is_refused() {
        assert!(matches!(
            VideoFormats::parse("00112233445566778899"),
            Err(FormatError::TokenTooLong { bytes: 10, .. })
        ));
        let many = vec!["00"; MAX_TOKENS + 1].join(" ");
        assert!(matches!(
            VideoFormats::parse(&many),
            Err(FormatError::TooManyTokens(_))
        ));
        let long = vec!["00"; 600].join(" ");
        assert!(matches!(
            VideoFormats::parse(&long),
            Err(FormatError::TooLong(_))
        ));
    }

    #[test]
    fn annotation_shows_offsets_bits_and_an_explicitly_unverified_name() {
        let formats = VideoFormats::parse("00 01 0a").expect("parse");
        let lines = formats.annotate();
        // Three token lines, then a note naming how many declared fields have no
        // token in this value.
        assert_eq!(lines.len(), 4, "{lines:#?}");
        assert!(lines[0].starts_with("[0] offset   0  00"), "{}", lines[0]);
        assert!(lines[0].contains("00000000"), "{}", lines[0]);
        assert!(
            lines[0].contains("declared (unverified)"),
            "annotation must never read as verified: {}",
            lines[0]
        );
        // Offsets account for the separating spaces.
        assert!(lines[1].starts_with("[1] offset   3  01"), "{}", lines[1]);
        assert!(lines[2].starts_with("[2] offset   6  0a"), "{}", lines[2]);
        assert!(
            lines[3].contains("declared field(s) carry no token"),
            "{}",
            lines[3]
        );
    }

    #[test]
    fn a_token_whose_width_disagrees_with_the_schema_is_flagged_not_named() {
        // The schema declares a 1-byte native bitmap; a 4-byte token there means
        // the assumed layout is wrong, and the annotation must say so instead of
        // labelling it "native resolution".
        let formats = VideoFormats::parse("00000000 00").expect("parse");
        let lines = formats.annotate();
        assert!(
            lines[0].contains("expects 1 byte(s), this token is 4"),
            "{}",
            lines[0]
        );
        assert!(
            lines[0].contains("does not match the assumed layout"),
            "{}",
            lines[0]
        );
        assert!(
            !lines[0].contains("native (screen) resolution bitmap  "),
            "a mismatched token must not be presented as that field: {}",
            lines[0]
        );
    }

    #[test]
    fn a_token_past_the_declared_schema_says_so_instead_of_inventing_a_name() {
        let tokens: Vec<String> = (0..FIELD_SCHEMA.len() + 1)
            .map(|_| "00".to_owned())
            .collect();
        let formats = VideoFormats::parse(&tokens.join(" ")).expect("parse");
        let lines = formats.annotate();
        // Widths differ from the schema, but the last token is past its end, so
        // its note is about recognition, not width.
        let last = lines
            .iter()
            .rev()
            .find(|line| line.contains("offset"))
            .expect("a last token line");
        assert!(
            last.contains("unrecognised position"),
            "an unknown position must not borrow a name: {last}"
        );
    }

    #[test]
    fn annotation_never_changes_the_value() {
        // The tool may describe a value; it must not rewrite it.
        let wire = "00 00 02 10 00000000";
        let formats = VideoFormats::parse(wire).expect("parse");
        let before = formats.to_wire();
        let _ = formats.annotate();
        assert_eq!(formats.to_wire(), before);
        assert_eq!(before, wire);
    }
    #[test]
    fn bit_listing_groups_by_byte_with_the_high_bit_first() {
        // One two-byte token: no space, or it would parse as two tokens.
        let formats = VideoFormats::parse("0180").expect("parse");
        let token = &formats.tokens()[0];
        assert_eq!(token.bits(), "00000001 10000000");
        assert_eq!(token.hex(), "0180");
        assert_eq!(token.bytes(), Some(&[0x01, 0x80][..]));
    }

    #[test]
    fn a_none_field_inside_a_real_value_is_a_token_not_a_parse_error() {
        // A real advertisement ends with latency / slice / frame-rate-control
        // positions that are the literal `none`.
        let wire = "00 00 02 10 00000000 00000000 00 0000 0000 00 none none";
        let formats = VideoFormats::parse(wire).expect("a real value must parse");
        assert_eq!(formats.to_wire(), wire, "and round-trip exactly");
        assert!(!formats.is_none(), "the whole value is not `none`");
        let last = formats.tokens().last().expect("a last token");
        assert_eq!(last, &Token::None);
        assert_eq!(last.bytes(), None);
        assert_eq!(last.bits(), "", "`none` has no bits to list");
        assert!(formats.annotate().last().expect("line").contains("none"));
        // A `none` token is a literal, not a width mismatch.
        let lines = formats.annotate();
        let none_line = lines
            .iter()
            .find(|line| line.contains("in this value"))
            .expect("a `none` annotation");
        assert!(
            !none_line.contains("does not match the assumed layout"),
            "a literal `none` is not the wrong size: {none_line}"
        );
    }
}
