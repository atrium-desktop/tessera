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
//! | [`endpoint`] | peer matching for `--peer` | fixtures |
//! | [`firewall`] | inbound RTSP reachability | the reference nftables configuration |
//! | [`wfd`] | the protocol core | byte-level fixtures |
//! | [`cast`] | the source-side server: accept, M1–M6 + SETUP/PLAY, media | source ↔ sink over a socketpair |
//! | [`session`] | one session from P2P group to media | shared by the CLI and the service |
//! | [`daemon`] | the long-lived service and its wire protocol | framing fixtures + a live smoke run |
//! | [`sink`] | the sink side, scripted | used by the tests and as a local harness |
//!
//! The direction is not optional: Miracast makes the **source** the RTSP server
//! and the **sink** the client. The source listens on its control port (7236 by
//! default), the sink connects, and the sink sends the first `OPTIONS`. An
//! implementation that dials the sink is implementing a different protocol.
//!
//! Two rules govern everything here, and they are not stylistic:
//!
//! 1. **Nothing is fabricated** (`[INV-ARCH-83]`). The probe reports observed,
//!    absent, or unverified — never "probably fine"; the formats tool annotates a
//!    value but never invents one; a refusal always names its own cause.
//! 2. **Hardware evidence is recorded, not assumed.** The reference TV completed
//!    the session, received 300 frames, and displayed the test pattern
//!    (ADR-0192 §8); desktop capture and the compositor UI click are the
//!    remaining checks.

#![forbid(unsafe_code)]

pub mod cast;
pub mod daemon;
pub mod discover;
pub mod endpoint;
pub mod firewall;
pub mod iwd;
pub mod media;
pub mod observe;
pub mod probe;
pub mod radio;
pub mod session;
pub mod sink;
pub mod wfd;
