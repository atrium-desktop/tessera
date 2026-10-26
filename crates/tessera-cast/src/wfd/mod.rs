//! Wi-Fi Display (Miracast) protocol core.
//!
//! Two kinds of code live here, and the difference is load-bearing
//! ([ADR-0192](../../../docs/adr/0192-first-party-miracast-caster.md) §7):
//!
//! **Pure byte transformations** — values in, bytes out, and back. Each is proven
//! by fixtures without a radio, a GPU, or a sink:
//!
//! | Module | Provenance |
//! |--------|-----------|
//! | [`rtsp`] | RFC 2326 message framing + `wfd_*` parameter bodies |
//! | [`ie`] | Wi-Fi Display Information Element and subelement codec |
//! | [`parameters`] | The advertised parameter set, with injection defence |
//! | [`rtp`] | RFC 6184 H.264 payloads over RTP (RFC 3550 headers) |
//! | [`ts`] | ISO/IEC 13818-1 transport stream: PAT, PMT, PES, PCR |
//!
//! **Stateful, but driven only by messages** — no radio, clock, or socket, so a
//! scripted sink can exercise them end to end:
//!
//! | Module | Provenance |
//! |--------|-----------|
//! | [`session`] | The sink-driven WFD source session (permissive responder) |
//! | [`trace`] | Two-directional message trace with hex dumps |
//!
//! What is *not* here, deliberately: anything that needs a radio, a GPU, or a
//! sink. The sink still decides whether any of this is correct in the field; these
//! modules only guarantee that the bytes are what the specifications say and that
//! the responder answers what it was asked.

pub mod formats;
pub mod ie;
pub mod parameters;
pub mod rtp;
pub mod rtsp;
pub mod session;
pub mod trace;
pub mod transport;
pub mod ts;
