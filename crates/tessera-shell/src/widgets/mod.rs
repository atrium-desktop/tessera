//! Composite UI patterns, modal dialog scaffolding, motion curves, and shared
//! layout primitives for Tessera chrome.
//!
//! # Architecture & Boundary
//!
//! This module builds on top of `optics-ui` (the immediate-mode UI engine) and `tessera-design`
//! (the product design tokens and materials). It provides reusable composite UI patterns
//! and enforces UI design consistency across Tessera compositor chrome components
//! without coupling to compositor server state.

#![forbid(unsafe_code)]

pub mod dialog;
pub mod geom;
pub mod icons;
pub mod menu;
pub mod motion;
pub mod picker;
pub mod shapes;
pub mod tiered;
