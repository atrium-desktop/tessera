//! Geometry, hit-testing, and layout utility primitives.

use lens::{Align, LayoutOpts, Rect};

/// Return true if the rectangle contains the point `(x, y)`.
#[inline]
pub fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

/// A zero-padding, centered layout stretching to the rectangle's dimensions.
#[inline]
pub fn stretch(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        pad: 0.0,
        gap: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A zero-padding layout stretching horizontally and aligned to the start/top.
#[inline]
pub fn stretch_top(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        pad: 0.0,
        gap: 0.0,
        cross: Align::Start,
        ..Default::default()
    }
}

/// A centered layout stretching to the rectangle with explicit padding.
#[inline]
pub fn stretch_pad(rect: Rect, pad: f32) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        pad,
        gap: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A centered layout stretching to the rectangle with explicit item gap.
#[inline]
pub fn stretch_gap(rect: Rect, gap: f32) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        pad: 0.0,
        gap,
        cross: Align::Center,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains() {
        let r = Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 50.0,
        };
        assert!(contains(r, 10.0, 20.0));
        assert!(contains(r, 50.0, 45.0));
        assert!(contains(r, 109.9, 69.9));
        assert!(!contains(r, 9.9, 20.0));
        assert!(!contains(r, 110.0, 20.0));
        assert!(!contains(r, 50.0, 70.0));
    }
}
