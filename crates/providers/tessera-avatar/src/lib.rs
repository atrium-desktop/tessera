//! Personalized avatar runtime for tessera shell surfaces.
//!
//! This crate owns the persona domain (ADR-0080/0096/0097): the local
//! account defaults presented by shell surfaces, and — with the `persona`
//! feature — the shared still/VRM portrait, motion, and live-reload pipeline.
//!
//! Security principals and authentication remain in `tessera-security` and
//! the lock-screen authentication boundary; this crate is presentation-side
//! only.

pub mod persona;
