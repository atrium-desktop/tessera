//! Stable shared value types without platform or storage dependencies.

#![forbid(unsafe_code)]

mod geometry;
mod identity;
pub mod input;
pub use geometry::{Point, Rect, Size, Transform};
pub use identity::*;
