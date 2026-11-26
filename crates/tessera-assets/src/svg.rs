//! In-process SVG parsing and rasterization via system `librsvg-2.0` and `cairo`.
//!
//! Why `librsvg` + `cairo` instead of a minimal subset parser:
//! - Desktop application icons, icon themes (Breeze, Papirus, Adwaita, hicolor),
//!   and third-party XDG cursor themes routinely rely on full SVG 1.1 / SVG 2
//!   features — `<style>` CSS stylesheets, `<use>` / `<symbol>`, `<clipPath>`,
//!   `<mask>`, `<filter>` blurs/shadows, `gradientTransform`, `<text>`, and
//!   embedded `<image>` elements.
//! - Modern `librsvg` is the Linux desktop reference implementation, executes
//!   in-process (no subprocess `fork` on asset paths), enforces XML safety
//!   limits by default, and blocks external filesystem/network references when
//!   loaded from memory with no base URI.
//! - Cairo's `CAIRO_FORMAT_ARGB32` is native-endian premultiplied `0xAARRGGBB`,
//!   which on little-endian Linux is already **premultiplied BGRA8** — the exact
//!   layout `optics-gfx` samples through `Bgra8Unorm`.

use std::borrow::Cow;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::{self, NonNull};

use crate::{DecodeError, MAX_EXTENT, RasterImage};

/// Hard ceiling on uncompressed SVG input bytes handed to `librsvg`.
///
/// Matches [`crate::vector`]'s decompression ceiling so direct `.svg` reads and
/// `.svgz` expansions obey the same untrusted-input budget.
const MAX_SVG_INPUT_BYTES: usize = 16 * 1024 * 1024;

/// Standard CSS reference DPI (96 DPI rather than librsvg's historical 90 DPI).
const CSS_REFERENCE_DPI: f64 = 96.0;

/// User-origin stylesheet setting the fallback `currentColor` to opaque white.
///
/// The `:not([color])` guard ensures a `color="..."` presentation attribute on
/// the root `<svg>` still wins (librsvg evaluates presentation hints below
/// stylesheet rules), while Author-origin `<style>` sheets, inline `style=""`,
/// and descendant `color` attributes naturally override the User-origin rule.
const DEFAULT_CURRENT_COLOR_CSS: &[u8] = b"svg:not([color]) { color: #ffffff; }";

const CAIRO_FORMAT_ARGB32: c_int = 0;
const CAIRO_STATUS_SUCCESS: c_int = 0;

#[repr(C)]
struct RsvgHandle {
    _private: [u8; 0],
}

#[repr(C)]
struct CairoSurface {
    _private: [u8; 0],
}

#[repr(C)]
struct CairoContext {
    _private: [u8; 0],
}

#[repr(C)]
struct GError {
    domain: u32,
    code: c_int,
    message: *const c_char,
}

#[repr(C)]
#[derive(Default)]
struct RsvgRectangle {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Default)]
struct RsvgLength {
    length: f64,
    unit: c_int,
}

unsafe extern "C" {
    fn rsvg_handle_new_from_data(
        data: *const u8,
        data_len: usize,
        error: *mut *mut GError,
    ) -> *mut RsvgHandle;
    fn rsvg_handle_set_dpi(handle: *mut RsvgHandle, dpi: f64);
    fn rsvg_handle_set_stylesheet(
        handle: *mut RsvgHandle,
        css: *const u8,
        css_len: usize,
        error: *mut *mut GError,
    ) -> c_int;
    fn rsvg_handle_get_intrinsic_dimensions(
        handle: *mut RsvgHandle,
        out_has_width: *mut c_int,
        out_width: *mut RsvgLength,
        out_has_height: *mut c_int,
        out_height: *mut RsvgLength,
        out_has_viewbox: *mut c_int,
        out_viewbox: *mut RsvgRectangle,
    );
    fn rsvg_handle_get_intrinsic_size_in_pixels(
        handle: *mut RsvgHandle,
        out_width: *mut f64,
        out_height: *mut f64,
    ) -> c_int;
    fn rsvg_handle_render_document(
        handle: *mut RsvgHandle,
        cr: *mut CairoContext,
        viewport: *const RsvgRectangle,
        error: *mut *mut GError,
    ) -> c_int;

    fn g_object_unref(object: *mut c_void);
    fn g_error_free(error: *mut GError);

    fn cairo_image_surface_create(format: c_int, width: c_int, height: c_int) -> *mut CairoSurface;
    fn cairo_surface_status(surface: *mut CairoSurface) -> c_int;
    fn cairo_surface_flush(surface: *mut CairoSurface);
    fn cairo_image_surface_get_data(surface: *mut CairoSurface) -> *mut u8;
    fn cairo_image_surface_get_stride(surface: *mut CairoSurface) -> c_int;
    fn cairo_surface_destroy(surface: *mut CairoSurface);

    fn cairo_create(target: *mut CairoSurface) -> *mut CairoContext;
    fn cairo_status(cr: *mut CairoContext) -> c_int;
    fn cairo_destroy(cr: *mut CairoContext);
}

struct OwnedRsvgHandle(NonNull<RsvgHandle>);

impl Drop for OwnedRsvgHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is an owned GObject reference from `rsvg_handle_new_from_data`.
        unsafe { g_object_unref(self.0.as_ptr().cast()) };
    }
}

struct OwnedCairoSurface(NonNull<CairoSurface>);

impl Drop for OwnedCairoSurface {
    fn drop(&mut self) {
        // SAFETY: `self.0` was created by `cairo_image_surface_create`.
        unsafe { cairo_surface_destroy(self.0.as_ptr()) };
    }
}

struct OwnedCairoContext(NonNull<CairoContext>);

impl Drop for OwnedCairoContext {
    fn drop(&mut self) {
        // SAFETY: `self.0` was created by `cairo_create`.
        unsafe { cairo_destroy(self.0.as_ptr()) };
    }
}

/// Extract a human-readable message from a nullable `GError*` and free it.
unsafe fn take_gerror(err: *mut GError, fallback: &str) -> String {
    if err.is_null() {
        return fallback.to_owned();
    }
    // SAFETY: `err` is a valid non-null `GError` allocated by GLib/librsvg.
    let msg = unsafe {
        let ptr = (*err).message;
        if ptr.is_null() {
            fallback.to_owned()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    };
    // SAFETY: caller transfers ownership of `err` to `take_gerror`.
    unsafe { g_error_free(err) };
    msg
}

/// Metadata scraped from the root `<svg>` start tag.
///
/// `librsvg` does not expose arbitrary `data-*` XML attributes through its C
/// API, so cursor hotspot metadata (`data-hotspot-x` / `data-hotspot-y`) is
/// read once from the root tag alongside checking whether `xmlns` was omitted.
#[derive(Debug, Default)]
struct RootSvgMeta {
    /// Byte offset right after `<svg` where ` xmlns="..."` can be inserted if
    /// the root tag omitted the SVG namespace.
    insert_xmlns_at: Option<usize>,
    hotspot_x: Option<f32>,
    hotspot_y: Option<f32>,
}

/// A parsed SVG/SVGZ document backed by `librsvg`.
pub(crate) struct SvgDocument {
    handle: OwnedRsvgHandle,
    viewport: [f32; 4],
    hotspot_x: Option<f32>,
    hotspot_y: Option<f32>,
}

impl SvgDocument {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.is_empty() {
            return Err(DecodeError::Vector("empty SVG document".into()));
        }
        if bytes.len() > MAX_SVG_INPUT_BYTES {
            return Err(DecodeError::Vector(format!(
                "SVG document exceeds {} byte ceiling",
                MAX_SVG_INPUT_BYTES
            )));
        }

        let meta = inspect_root_svg_tag(bytes);
        let normalized: Cow<'_, [u8]> = match meta.insert_xmlns_at {
            Some(pos) if pos <= bytes.len() => {
                const XMLNS: &[u8] = b" xmlns=\"http://www.w3.org/2000/svg\"";
                let mut patched = Vec::with_capacity(bytes.len() + XMLNS.len());
                patched.extend_from_slice(&bytes[..pos]);
                patched.extend_from_slice(XMLNS);
                patched.extend_from_slice(&bytes[pos..]);
                Cow::Owned(patched)
            }
            _ => Cow::Borrowed(bytes),
        };

        let mut err: *mut GError = ptr::null_mut();
        // SAFETY: `normalized` is a valid byte slice; `rsvg_handle_new_from_data`
        // creates a handle with `base_uri = NULL` and default XML security limits.
        let raw_handle =
            unsafe { rsvg_handle_new_from_data(normalized.as_ptr(), normalized.len(), &mut err) };
        let Some(non_null) = NonNull::new(raw_handle) else {
            let msg = unsafe { take_gerror(err, "librsvg rejected SVG document") };
            return Err(DecodeError::Vector(msg));
        };
        // Free any stray warning/error even on success.
        if !err.is_null() {
            unsafe { g_error_free(err) };
        }
        let handle = OwnedRsvgHandle(non_null);

        // SAFETY: `handle.0` is a fully-loaded, valid `RsvgHandle`.
        unsafe { rsvg_handle_set_dpi(handle.0.as_ptr(), CSS_REFERENCE_DPI) };

        let mut css_err: *mut GError = ptr::null_mut();
        // SAFETY: `DEFAULT_CURRENT_COLOR_CSS` is valid UTF-8 CSS.
        let css_ok = unsafe {
            rsvg_handle_set_stylesheet(
                handle.0.as_ptr(),
                DEFAULT_CURRENT_COLOR_CSS.as_ptr(),
                DEFAULT_CURRENT_COLOR_CSS.len(),
                &mut css_err,
            )
        };
        if css_ok == 0 {
            let msg = unsafe { take_gerror(css_err, "failed to apply default SVG stylesheet") };
            return Err(DecodeError::Vector(msg));
        } else if !css_err.is_null() {
            unsafe { g_error_free(css_err) };
        }

        let viewport = query_viewport(handle.0.as_ptr());

        Ok(Self {
            handle,
            viewport,
            hotspot_x: meta.hotspot_x,
            hotspot_y: meta.hotspot_y,
        })
    }

    pub(crate) fn viewport(&self) -> [f32; 4] {
        self.viewport
    }

    pub(crate) fn hotspot_x(&self) -> Option<f32> {
        self.hotspot_x
    }

    pub(crate) fn hotspot_y(&self) -> Option<f32> {
        self.hotspot_y
    }

    pub(crate) fn rasterize(&self, extent: u32) -> Result<RasterImage, DecodeError> {
        let extent = extent.clamp(1, MAX_EXTENT);
        let dim = c_int::try_from(extent).map_err(|_| DecodeError::Extent {
            width: extent,
            height: extent,
            len: 0,
        })?;

        // SAFETY: `dim` is in `1..=MAX_EXTENT`, and Cairo zero-initializes the
        // image surface to transparent black premultiplied ARGB32.
        let raw_surface = unsafe { cairo_image_surface_create(CAIRO_FORMAT_ARGB32, dim, dim) };
        let Some(surface_ptr) = NonNull::new(raw_surface) else {
            return Err(DecodeError::Vector(
                "cairo surface allocation failed".into(),
            ));
        };
        let surface = OwnedCairoSurface(surface_ptr);
        if unsafe { cairo_surface_status(surface.0.as_ptr()) } != CAIRO_STATUS_SUCCESS {
            return Err(DecodeError::Vector(
                "cairo surface entered error state".into(),
            ));
        }

        let raw_cr = unsafe { cairo_create(surface.0.as_ptr()) };
        let Some(cr_ptr) = NonNull::new(raw_cr) else {
            return Err(DecodeError::Vector(
                "cairo context allocation failed".into(),
            ));
        };
        let cr = OwnedCairoContext(cr_ptr);
        if unsafe { cairo_status(cr.0.as_ptr()) } != CAIRO_STATUS_SUCCESS {
            return Err(DecodeError::Vector(
                "cairo context entered error state".into(),
            ));
        }

        let viewport = RsvgRectangle {
            x: 0.0,
            y: 0.0,
            width: f64::from(extent),
            height: f64::from(extent),
        };
        let mut err: *mut GError = ptr::null_mut();
        // SAFETY: `handle`, `cr`, and `viewport` are valid; `render_document`
        // proportionally fits and centers the SVG inside `viewport`.
        let ok = unsafe {
            rsvg_handle_render_document(self.handle.0.as_ptr(), cr.0.as_ptr(), &viewport, &mut err)
        };
        if ok == 0 {
            let msg = unsafe { take_gerror(err, "librsvg failed to render document") };
            return Err(DecodeError::Vector(msg));
        }
        if !err.is_null() {
            unsafe { g_error_free(err) };
        }

        // Release the context before flushing and reading back the surface.
        drop(cr);
        unsafe { cairo_surface_flush(surface.0.as_ptr()) };

        let data_ptr = unsafe { cairo_image_surface_get_data(surface.0.as_ptr()) };
        let stride = unsafe { cairo_image_surface_get_stride(surface.0.as_ptr()) };
        if data_ptr.is_null() || stride <= 0 {
            return Err(DecodeError::Vector("cairo surface data unavailable".into()));
        }

        let stride = stride as usize;
        let row_bytes = (extent as usize)
            .checked_mul(4)
            .ok_or(DecodeError::Extent {
                width: extent,
                height: extent,
                len: 0,
            })?;
        if stride < row_bytes {
            return Err(DecodeError::Extent {
                width: extent,
                height: extent,
                len: stride,
            });
        }

        let total_surface_bytes =
            stride
                .checked_mul(extent as usize)
                .ok_or(DecodeError::Extent {
                    width: extent,
                    height: extent,
                    len: 0,
                })?;
        // SAFETY: Cairo guarantees `stride * height` readable bytes after flush.
        let src = unsafe { std::slice::from_raw_parts(data_ptr, total_surface_bytes) };

        let bgra = if stride == row_bytes {
            src.to_vec()
        } else {
            let expected = row_bytes
                .checked_mul(extent as usize)
                .ok_or(DecodeError::Extent {
                    width: extent,
                    height: extent,
                    len: 0,
                })?;
            let mut packed = Vec::with_capacity(expected);
            for row in 0..extent as usize {
                let start = row * stride;
                let end = start + row_bytes;
                let Some(slice) = src.get(start..end) else {
                    return Err(DecodeError::Extent {
                        width: extent,
                        height: extent,
                        len: src.len(),
                    });
                };
                packed.extend_from_slice(slice);
            }
            packed
        };

        #[cfg(target_endian = "big")]
        let mut bgra = bgra;
        // Cairo ARGB32 is native-endian premultiplied 0xAARRGGBB:
        // - Little-endian: [B, G, R, A] (already premultiplied BGRA8).
        // - Big-endian: [A, R, G, B] -> reverse each pixel to [B, G, R, A].
        #[cfg(target_endian = "big")]
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.reverse();
        }

        RasterImage::new(extent, extent, bgra)
    }
}

/// Query the SVG's intrinsic viewport in user units, guaranteed non-zero.
fn query_viewport(handle: *mut RsvgHandle) -> [f32; 4] {
    let mut has_width: c_int = 0;
    let mut width = RsvgLength::default();
    let mut has_height: c_int = 0;
    let mut height = RsvgLength::default();
    let mut has_viewbox: c_int = 0;
    let mut viewbox = RsvgRectangle::default();

    // SAFETY: `handle` is a fully-loaded `RsvgHandle`.
    unsafe {
        rsvg_handle_get_intrinsic_dimensions(
            handle,
            &mut has_width,
            &mut width,
            &mut has_height,
            &mut height,
            &mut has_viewbox,
            &mut viewbox,
        );
    }

    if has_viewbox != 0
        && viewbox.width.is_finite()
        && viewbox.height.is_finite()
        && viewbox.width > 0.0
        && viewbox.height > 0.0
    {
        return [
            viewbox.x as f32,
            viewbox.y as f32,
            viewbox.width as f32,
            viewbox.height as f32,
        ];
    }

    let mut px_w = 0.0f64;
    let mut px_h = 0.0f64;
    // SAFETY: `handle` is a fully-loaded `RsvgHandle`.
    let has_px = unsafe { rsvg_handle_get_intrinsic_size_in_pixels(handle, &mut px_w, &mut px_h) };
    if has_px != 0 && px_w.is_finite() && px_h.is_finite() && px_w > 0.0 && px_h > 0.0 {
        return [0.0, 0.0, px_w as f32, px_h as f32];
    }

    [0.0, 0.0, 1.0, 1.0]
}

/// Inspect the root `<svg ...>` start tag (skipping XML prolog, comments, and
/// DOCTYPE) to read `data-hotspot-x` / `data-hotspot-y` and detect an absent
/// `xmlns` attribute.
fn inspect_root_svg_tag(bytes: &[u8]) -> RootSvgMeta {
    let mut meta = RootSvgMeta::default();
    let mut i = if bytes.starts_with(b"\xEF\xBB\xBF") {
        3
    } else {
        0
    };

    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'<' {
            return meta;
        }
        if bytes[i..].starts_with(b"<!--") {
            let Some(rel) = find_bytes(&bytes[i + 4..], b"-->") else {
                return meta;
            };
            i += 4 + rel + 3;
            continue;
        }
        if bytes[i..].starts_with(b"<?") {
            let Some(rel) = find_bytes(&bytes[i + 2..], b"?>") else {
                return meta;
            };
            i += 2 + rel + 2;
            continue;
        }
        if bytes[i..].starts_with(b"<!") {
            // Skip DOCTYPE including any internal `[ ... ]` subset.
            i += 2;
            let mut in_subset = false;
            let mut quote: Option<u8> = None;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if let Some(q) = quote {
                    if b == q {
                        quote = None;
                    }
                    continue;
                }
                match b {
                    b'"' | b'\'' => quote = Some(b),
                    b'[' => in_subset = true,
                    b']' => in_subset = false,
                    b'>' if !in_subset => break,
                    _ => {}
                }
            }
            continue;
        }

        // First real element start tag.
        let name_start = i + 1;
        let mut pos = name_start;
        while pos < bytes.len()
            && !bytes[pos].is_ascii_whitespace()
            && bytes[pos] != b'>'
            && bytes[pos] != b'/'
        {
            pos += 1;
        }
        let tag_name = &bytes[name_start..pos];
        let is_unprefixed_svg = tag_name == b"svg";
        let is_prefixed_svg = tag_name.ends_with(b":svg");
        if !is_unprefixed_svg && !is_prefixed_svg {
            return meta;
        }
        let after_name = pos;
        let mut has_xmlns = false;

        // Parse attributes up to the closing `>` or `/>`.
        while pos < bytes.len() {
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if pos >= bytes.len() || bytes[pos] == b'>' {
                break;
            }
            if bytes[pos] == b'/' {
                break;
            }

            let attr_start = pos;
            while pos < bytes.len()
                && !bytes[pos].is_ascii_whitespace()
                && bytes[pos] != b'='
                && bytes[pos] != b'>'
                && bytes[pos] != b'/'
            {
                pos += 1;
            }
            let attr_name = &bytes[attr_start..pos];
            if attr_name.is_empty() {
                break;
            }
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if pos >= bytes.len() || bytes[pos] != b'=' {
                continue;
            }
            pos += 1;
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if pos >= bytes.len() {
                break;
            }
            let quote = bytes[pos];
            if quote != b'"' && quote != b'\'' {
                break;
            }
            pos += 1;
            let val_start = pos;
            while pos < bytes.len() && bytes[pos] != quote {
                pos += 1;
            }
            if pos >= bytes.len() {
                break;
            }
            let attr_val = &bytes[val_start..pos];
            pos += 1;

            if attr_name == b"xmlns" {
                has_xmlns = true;
            } else if attr_name == b"data-hotspot-x" {
                meta.hotspot_x = std::str::from_utf8(attr_val)
                    .ok()
                    .and_then(|s| s.trim().parse::<f32>().ok())
                    .filter(|v| v.is_finite());
            } else if attr_name == b"data-hotspot-y" {
                meta.hotspot_y = std::str::from_utf8(attr_val)
                    .ok()
                    .and_then(|s| s.trim().parse::<f32>().ok())
                    .filter(|v| v.is_finite());
            }
        }

        if is_unprefixed_svg && !has_xmlns {
            meta.insert_xmlns_at = Some(after_name);
        }
        return meta;
    }

    meta
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
