//! Foundational physical, geometric, color, identity, and buffer primitives for Tessera.
#![forbid(unsafe_code)]

pub mod color;
pub mod dmabuf;
pub mod edid;
pub mod geometry;
pub mod identity;
pub mod input;
pub mod stream;

pub use geometry::{
    ClosingGhostView, MinimizeWarp, Point, Rect, Size, SurfaceDmabuf, SurfaceGeometry,
    SurfacePixels, Transform,
};
pub use identity::*;
