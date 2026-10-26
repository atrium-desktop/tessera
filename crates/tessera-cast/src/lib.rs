//! Miracast (Wi-Fi Display) caster for the Tessera desktop
//! ([ADR-0192](../../docs/adr/0192-first-party-miracast-caster.md)).
//!
//! Casting has two halves that come from different places, and a machine can have
//! one without the other:
//!
//! - the **radio** — a Wi-Fi Display P2P authority, from the system's Wi-Fi owner
//!   (iwd on the reference configuration);
//! - the **protocol** — the WFD session, from this crate.
//!
//! What lives here, and how each part earns its keep:
//!
//! | Module | Role | Evidence |
//! |--------|------|----------|
//! | [`probe`] / [`observe`] | read-only capability verdict | live iwd on the reference machine |
//! | [`radio`] | the P2P port | enable / register / discover verified live |
//! | [`discover`] | the discovery use case | scripted-radio tests |
//! | [`neighbours`] / [`endpoint`] | device address → reachable endpoint | fixtures + the live table |
//! | [`wfd`] | the protocol core | byte-level fixtures |
//! | [`cast`] | the bounded handshake loop | caster ↔ sink over a socketpair |
//! | [`sink`] | the other side, scripted | used by the tests and as a local harness |
//!
//! Two rules govern everything here, and they are not stylistic:
//!
//! 1. **Nothing is fabricated** (`[INV-ARCH-83]`). The probe reports observed,
//!    absent, or unverified — never "probably fine"; the formats tool annotates a
//!    value but never invents one; a refusal always names its own cause.
//! 2. **What cannot be verified offline is labelled.** iwd P2P group formation, VA-API
//!    encoding, PipeWire capture, and a real sink accepting the exchange are
//!    `Unverified on hardware` (ADR-0192 §7/§8). Nothing in this crate may be
//!    described as working before a real sink shows frames.

#![forbid(unsafe_code)]

pub mod cast;
pub mod discover;
pub mod endpoint;
pub mod iwd;
pub mod media;
pub mod neighbours;
pub mod observe;
pub mod probe;
pub mod radio;
pub mod sink;
pub mod wfd;
