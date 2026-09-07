//! Pure presentation-domain value layer for tessera (ADR-0039/0077/0101
//! damage and presentation pipeline).
//!
//! This crate is the presentation pipeline's **value layer**: frame damage
//! verdicts, the per-consumer damage assessment split, swapchain slot-ring
//! repaint history, surface damage baselines, and the logical→physical
//! damage mapping. Everything here is a pure function or value over
//! `tessera-model` geometry types — no compositor state, no IPC, no engine
//! handles, no side effects.
//!
//! # Boundary
//!
//! The orchestrating damage assessment (change-signal sampling across the
//! server, shell, notifications, and wallpaper; the decision of when a
//! frame is presented at all) lives in the composition root; it consumes
//! this crate's value types and feeds client surfaces to
//! [`ClientDamageTracker`] through the narrow [`SurfaceDamageFrame`]
//! observation seam. That split follows the same principle as
//! `tessera-capture`: facts live in a shared crate, decisions live with the
//! process that owns them.

mod damage;

pub use damage::{
    composite_repaint_for_slot, logical_rects_to_frame, logical_to_physical,
    record_composite_present, union_frame_damage, AssessedFrameDamage, ClientDamage,
    ClientDamageTracker, DamageAssessment, FrameDamage, SurfaceDamageFrame,
};
