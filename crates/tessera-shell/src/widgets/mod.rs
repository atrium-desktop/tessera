//! Composite UI patterns, modal dialog scaffolding, settings controls, motion curves, and shared layout primitives for Tessera chrome.
//!
//! # Architecture & Boundary
//!
//! This module builds on top of `lens` (the immediate-mode UI engine) and `tessera-design`
//! (the product design tokens and materials). It provides reusable composite UI patterns
//! and enforces UI design consistency across Tessera compositor chrome components
//! without coupling to compositor server state.

#![forbid(unsafe_code)]
#![allow(dead_code)]

pub mod chip;
pub mod dialog;
pub mod geom;
pub mod menu;
pub mod motion;
pub mod picker;
pub mod settings;
pub mod shapes;
