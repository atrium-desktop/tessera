//! Minimal Linux image decode, encode, and resize crate.
//!
//! - PNG: `libpng16` simplified `png_image` API
//! - JPEG: `libturbojpeg`
//! - WebP (static + animated): `libwebp` + `libwebpdemux`
//! - GIF (static + animated): pure-Rust GIF87a/89a + LZW

use std::ffi::{CStr, c_char, c_int, c_uchar, c_void};
use std::path::Path;
use std::ptr;
use std::time::Duration;

mod gif;

const DEFAULT_MAX_EDGE: u32 = 16384;
const DEFAULT_MAX_ALLOC: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Triangle,
    Lanczos3,
}

#[derive(Debug)]
pub enum ImageError {
    Io(std::io::Error),
    UnsupportedFormat,
    Dimensions(u32, u32),
    Decode(String),
    Encode(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "image I/O error: {err}"),
            Self::UnsupportedFormat => f.write_str("unsupported or unrecognized image format"),
            Self::Dimensions(w, h) => write!(f, "invalid or oversized image dimensions {w}x{h}"),
            Self::Decode(msg) => write!(f, "image decode failed: {msg}"),
            Self::Encode(msg) => write!(f, "image encode failed: {msg}"),
        }
    }
}

impl std::error::Error for ImageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ImageError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl RgbaImage {
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize) * (height as usize) * 4;
        Self {
            width,
            height,
            data: vec![0; len],
        }
    }

    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        let expected = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
            .and_then(|px| px.checked_mul(4))?;
        if data.len() != expected {
            return None;
        }
        Some(Self {
            width,
            height,
            data,
        })
    }

    pub fn from_pixel(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let count = (width as usize) * (height as usize);
        let mut data = Vec::with_capacity(count * 4);
        for _ in 0..count {
            data.extend_from_slice(&rgba);
        }
        Self {
            width,
            height,
            data,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn as_raw(&self) -> &[u8] {
        &self.data
    }

    pub fn as_mut_raw(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    pub fn get_pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let off = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        [
            self.data[off],
            self.data[off + 1],
            self.data[off + 2],
            self.data[off + 3],
        ]
    }

    pub fn put_pixel(&mut self, x: u32, y: u32, pixel: [u8; 4]) {
        let off = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        self.data[off..off + 4].copy_from_slice(&pixel);
    }

    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<(), ImageError> {
        let bytes = encode_png_rgba(self.width, self.height, &self.data, false)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

pub struct AnimFrame {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub delay: Duration,
}

pub fn detect_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

pub fn decode_file(path: impl AsRef<Path>) -> Result<RgbaImage, ImageError> {
    let bytes = std::fs::read(path)?;
    decode_memory(&bytes)
}

pub fn decode_memory(bytes: &[u8]) -> Result<RgbaImage, ImageError> {
    decode_memory_bounded(bytes, DEFAULT_MAX_EDGE, DEFAULT_MAX_ALLOC)
}

pub fn decode_memory_bounded(
    bytes: &[u8],
    max_edge: u32,
    max_alloc: u64,
) -> Result<RgbaImage, ImageError> {
    let format = detect_format(bytes).ok_or(ImageError::UnsupportedFormat)?;
    decode_with_format(bytes, format, max_edge, max_alloc)
}

pub fn decode_with_format(
    bytes: &[u8],
    format: ImageFormat,
    max_edge: u32,
    max_alloc: u64,
) -> Result<RgbaImage, ImageError> {
    match format {
        ImageFormat::Png => decode_png_rgba(bytes, max_edge, max_alloc),
        ImageFormat::Jpeg => decode_jpeg_rgba(bytes, max_edge, max_alloc),
        ImageFormat::Webp => {
            let (w, h, mut frames) = decode_webp_frames_bounded(bytes, max_edge, max_alloc)?;
            let first = frames
                .drain(..)
                .next()
                .ok_or_else(|| ImageError::Decode("WebP contains no frames".into()))?;
            RgbaImage::from_raw(w, h, first.rgba).ok_or(ImageError::Dimensions(w, h))
        }
        ImageFormat::Gif => {
            let (w, h, mut frames) = gif::decode_gif_frames(bytes, max_edge, max_alloc)?;
            let first = frames
                .drain(..)
                .next()
                .ok_or_else(|| ImageError::Decode("GIF contains no frames".into()))?;
            if first.left == 0 && first.top == 0 && first.width == w && first.height == h {
                RgbaImage::from_raw(w, h, first.rgba).ok_or(ImageError::Dimensions(w, h))
            } else {
                let mut canvas = RgbaImage::new(w, h);
                for y in 0..first.height {
                    for x in 0..first.width {
                        let cx = first.left + x;
                        let cy = first.top + y;
                        if cx < w && cy < h {
                            let off = ((y * first.width + x) * 4) as usize;
                            canvas.put_pixel(
                                cx,
                                cy,
                                [
                                    first.rgba[off],
                                    first.rgba[off + 1],
                                    first.rgba[off + 2],
                                    first.rgba[off + 3],
                                ],
                            );
                        }
                    }
                }
                Ok(canvas)
            }
        }
    }
}

pub fn decode_gif_frames(bytes: &[u8]) -> Result<(u32, u32, Vec<AnimFrame>), ImageError> {
    gif::decode_gif_frames(bytes, DEFAULT_MAX_EDGE, DEFAULT_MAX_ALLOC)
}

pub fn decode_webp_frames(bytes: &[u8]) -> Result<(u32, u32, Vec<AnimFrame>), ImageError> {
    decode_webp_frames_bounded(bytes, DEFAULT_MAX_EDGE, DEFAULT_MAX_ALLOC)
}

pub fn encode_png_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
    fast: bool,
) -> Result<Vec<u8>, ImageError> {
    encode_png_inner(width, height, rgba, PNG_FORMAT_RGBA, 4, fast)
}

pub fn encode_png_rgb(
    width: u32,
    height: u32,
    rgb: &[u8],
    fast: bool,
) -> Result<Vec<u8>, ImageError> {
    encode_png_inner(width, height, rgb, PNG_FORMAT_RGB, 3, fast)
}

fn encode_png_inner(
    width: u32,
    height: u32,
    pixels: &[u8],
    format: u32,
    channels: usize,
    fast: bool,
) -> Result<Vec<u8>, ImageError> {
    let expected = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|px| px.checked_mul(channels))
        .ok_or(ImageError::Dimensions(width, height))?;
    if width == 0 || height == 0 || pixels.len() != expected {
        return Err(ImageError::Encode(format!(
            "PNG buffer length mismatch for {width}x{height}x{channels}"
        )));
    }

    let mut img = PngImage {
        version: PNG_IMAGE_VERSION,
        width,
        height,
        format,
        flags: if fast { PNG_IMAGE_FLAG_FAST } else { 0 },
        ..PngImage::default()
    };

    let mut size: usize = 0;
    let ok = unsafe {
        png_image_write_to_memory(
            &mut img,
            ptr::null_mut(),
            &mut size,
            0,
            pixels.as_ptr().cast(),
            0,
            ptr::null(),
        )
    };
    if size == 0 || (img.warning_or_error & 0x03) > 1 {
        let _ = ok;
        return Err(ImageError::Encode(png_error(&img)));
    }

    let mut out = vec![0u8; size];
    let ok = unsafe {
        png_image_write_to_memory(
            &mut img,
            out.as_mut_ptr().cast(),
            &mut size,
            0,
            pixels.as_ptr().cast(),
            0,
            ptr::null(),
        )
    };
    if ok == 0 || (img.warning_or_error & 0x03) > 1 {
        return Err(ImageError::Encode(png_error(&img)));
    }
    out.truncate(size);
    Ok(out)
}

pub fn resize(src: &RgbaImage, dst_w: u32, dst_h: u32, filter: Filter) -> RgbaImage {
    if src.width == dst_w && src.height == dst_h {
        return src.clone();
    }
    if dst_w == 0 || dst_h == 0 || src.width == 0 || src.height == 0 {
        return RgbaImage::new(dst_w, dst_h);
    }
    let support = match filter {
        Filter::Nearest => {
            let mut out = RgbaImage::new(dst_w, dst_h);
            for y in 0..dst_h {
                let sy = ((y as u64 * src.height as u64) / dst_h as u64) as u32;
                for x in 0..dst_w {
                    let sx = ((x as u64 * src.width as u64) / dst_w as u64) as u32;
                    out.put_pixel(x, y, src.get_pixel(sx, sy));
                }
            }
            return out;
        }
        Filter::Triangle => 1.0f32,
        Filter::Lanczos3 => 3.0f32,
    };

    // Horizontal pass: src (src_w x src_h) -> tmp (dst_w x src_h) as f32 RGBA.
    let mut tmp = vec![0.0f32; (dst_w as usize) * (src.height as usize) * 4];
    let h_weights = precompute_weights(src.width, dst_w, support, filter);
    for y in 0..src.height as usize {
        let src_row = &src.data[y * (src.width as usize) * 4..(y + 1) * (src.width as usize) * 4];
        let dst_row = &mut tmp[y * (dst_w as usize) * 4..(y + 1) * (dst_w as usize) * 4];
        for (dx, taps) in h_weights.iter().enumerate() {
            let mut acc = [0.0f32; 4];
            for &(sx, w) in taps {
                let off = sx * 4;
                acc[0] += src_row[off] as f32 * w;
                acc[1] += src_row[off + 1] as f32 * w;
                acc[2] += src_row[off + 2] as f32 * w;
                acc[3] += src_row[off + 3] as f32 * w;
            }
            let doff = dx * 4;
            dst_row[doff..doff + 4].copy_from_slice(&acc);
        }
    }

    // Vertical pass: tmp (dst_w x src_h) -> out (dst_w x dst_h) as u8 RGBA.
    let mut out = RgbaImage::new(dst_w, dst_h);
    let v_weights = precompute_weights(src.height, dst_h, support, filter);
    for (dy, taps) in v_weights.iter().enumerate() {
        for dx in 0..dst_w as usize {
            let mut acc = [0.0f32; 4];
            for &(sy, w) in taps {
                let off = (sy * (dst_w as usize) + dx) * 4;
                acc[0] += tmp[off] * w;
                acc[1] += tmp[off + 1] * w;
                acc[2] += tmp[off + 2] * w;
                acc[3] += tmp[off + 3] * w;
            }
            out.put_pixel(
                dx as u32,
                dy as u32,
                [
                    acc[0].round().clamp(0.0, 255.0) as u8,
                    acc[1].round().clamp(0.0, 255.0) as u8,
                    acc[2].round().clamp(0.0, 255.0) as u8,
                    acc[3].round().clamp(0.0, 255.0) as u8,
                ],
            );
        }
    }
    out
}

pub fn resize_to_fill(src: &RgbaImage, dst_w: u32, dst_h: u32, filter: Filter) -> RgbaImage {
    if src.width == 0 || src.height == 0 || dst_w == 0 || dst_h == 0 {
        return RgbaImage::new(dst_w, dst_h);
    }
    let src_ratio = src.width as f64 / src.height as f64;
    let dst_ratio = dst_w as f64 / dst_h as f64;

    let (crop_x, crop_y, crop_w, crop_h) = if src_ratio > dst_ratio {
        let cw = ((src.height as f64 * dst_ratio).round() as u32).clamp(1, src.width);
        ((src.width - cw) / 2, 0, cw, src.height)
    } else {
        let ch = ((src.width as f64 / dst_ratio).round() as u32).clamp(1, src.height);
        (0, (src.height - ch) / 2, src.width, ch)
    };

    if crop_x == 0 && crop_y == 0 && crop_w == src.width && crop_h == src.height {
        return resize(src, dst_w, dst_h, filter);
    }

    let mut cropped = RgbaImage::new(crop_w, crop_h);
    for y in 0..crop_h {
        let src_start = (((crop_y + y) * src.width + crop_x) * 4) as usize;
        let dst_start = (y * crop_w * 4) as usize;
        let row_bytes = (crop_w * 4) as usize;
        cropped.data[dst_start..dst_start + row_bytes]
            .copy_from_slice(&src.data[src_start..src_start + row_bytes]);
    }
    resize(&cropped, dst_w, dst_h, filter)
}

fn precompute_weights(
    src_len: u32,
    dst_len: u32,
    support: f32,
    filter: Filter,
) -> Vec<Vec<(usize, f32)>> {
    let ratio = src_len as f32 / dst_len as f32;
    let scale = ratio.max(1.0);
    let radius = support * scale;
    let mut table = Vec::with_capacity(dst_len as usize);
    for dst in 0..dst_len {
        let center = (dst as f32 + 0.5) * ratio - 0.5;
        let start = (center - radius).floor().max(0.0) as usize;
        let end = (center + radius).ceil().min((src_len - 1) as f32) as usize;
        let mut taps = Vec::with_capacity(end.saturating_sub(start) + 1);
        let mut sum = 0.0f32;
        for src in start..=end {
            let x = (src as f32 - center) / scale;
            let w = match filter {
                Filter::Triangle => (1.0 - x.abs()).max(0.0),
                Filter::Lanczos3 => lanczos3(x),
                Filter::Nearest => 1.0,
            };
            if w != 0.0 {
                taps.push((src, w));
                sum += w;
            }
        }
        if sum > 0.0 {
            for (_, w) in &mut taps {
                *w /= sum;
            }
        }
        table.push(taps);
    }
    table
}

fn lanczos3(x: f32) -> f32 {
    let ax = x.abs();
    if ax < 1e-6 {
        1.0
    } else if ax >= 3.0 {
        0.0
    } else {
        let pi_x = std::f32::consts::PI * ax;
        (pi_x.sin() / pi_x) * ((pi_x / 3.0).sin() / (pi_x / 3.0))
    }
}

// --- FFI: libpng16, libturbojpeg, libwebp + libwebpdemux ---

const PNG_IMAGE_VERSION: u32 = 1;
const PNG_FORMAT_RGB: u32 = 0x02;
const PNG_FORMAT_RGBA: u32 = 0x02 | 0x01;
const PNG_IMAGE_FLAG_FAST: u32 = 0x02;
const TJPF_RGBA: c_int = 7;
const WEBP_DEMUX_ABI_VERSION: c_int = 0x0107;
const MODE_RGBA: c_int = 1;

#[repr(C)]
struct PngImage {
    opaque: *mut c_void,
    version: u32,
    width: u32,
    height: u32,
    format: u32,
    flags: u32,
    colormap_entries: u32,
    warning_or_error: u32,
    message: [c_char; 64],
}

impl Default for PngImage {
    fn default() -> Self {
        Self {
            opaque: ptr::null_mut(),
            version: PNG_IMAGE_VERSION,
            width: 0,
            height: 0,
            format: 0,
            flags: 0,
            colormap_entries: 0,
            warning_or_error: 0,
            message: [0; 64],
        }
    }
}

impl Drop for PngImage {
    fn drop(&mut self) {
        unsafe { png_image_free(self) };
    }
}

#[repr(C)]
struct WebPData {
    bytes: *const u8,
    size: usize,
}

#[repr(C)]
struct WebPAnimDecoderOptions {
    color_mode: c_int,
    use_threads: c_int,
    padding: [u32; 7],
}

#[repr(C)]
#[derive(Default)]
struct WebPAnimInfo {
    canvas_width: u32,
    canvas_height: u32,
    loop_count: u32,
    bgcolor: u32,
    frame_count: u32,
    pad: [u32; 4],
}

unsafe extern "C" {
    fn png_image_begin_read_from_memory(
        image: *mut PngImage,
        memory: *const c_void,
        size: usize,
    ) -> c_int;
    fn png_image_finish_read(
        image: *mut PngImage,
        background: *const c_void,
        buffer: *mut c_void,
        row_stride: i32,
        colormap: *mut c_void,
    ) -> c_int;
    fn png_image_write_to_memory(
        image: *mut PngImage,
        memory: *mut c_void,
        memory_bytes: *mut usize,
        convert_to_8_bit: c_int,
        buffer: *const c_void,
        row_stride: i32,
        colormap: *const c_void,
    ) -> c_int;
    fn png_image_free(image: *mut PngImage);

    fn tjInitDecompress() -> *mut c_void;
    fn tjDestroy(handle: *mut c_void) -> c_int;
    fn tjDecompressHeader3(
        handle: *mut c_void,
        jpeg_buf: *const c_uchar,
        jpeg_size: std::ffi::c_ulong,
        width: *mut c_int,
        height: *mut c_int,
        jpeg_subsamp: *mut c_int,
        jpeg_colorspace: *mut c_int,
    ) -> c_int;
    fn tjDecompress2(
        handle: *mut c_void,
        jpeg_buf: *const c_uchar,
        jpeg_size: std::ffi::c_ulong,
        dst_buf: *mut c_uchar,
        width: c_int,
        pitch: c_int,
        height: c_int,
        pixel_format: c_int,
        flags: c_int,
    ) -> c_int;
    fn tjGetErrorStr2(handle: *mut c_void) -> *const c_char;

    fn WebPAnimDecoderOptionsInitInternal(
        dec_options: *mut WebPAnimDecoderOptions,
        abi_version: c_int,
    ) -> c_int;
    fn WebPAnimDecoderNewInternal(
        webp_data: *const WebPData,
        dec_options: *const WebPAnimDecoderOptions,
        abi_version: c_int,
    ) -> *mut c_void;
    fn WebPAnimDecoderGetInfo(dec: *const c_void, info: *mut WebPAnimInfo) -> c_int;
    fn WebPAnimDecoderHasMoreFrames(dec: *const c_void) -> c_int;
    fn WebPAnimDecoderGetNext(dec: *mut c_void, buf: *mut *mut u8, timestamp: *mut c_int) -> c_int;
    fn WebPAnimDecoderDelete(dec: *mut c_void);
}

fn decode_png_rgba(bytes: &[u8], max_edge: u32, max_alloc: u64) -> Result<RgbaImage, ImageError> {
    let mut img = PngImage::default();
    let ok =
        unsafe { png_image_begin_read_from_memory(&mut img, bytes.as_ptr().cast(), bytes.len()) };
    if ok == 0 || (img.warning_or_error & 0x03) > 1 {
        return Err(ImageError::Decode(png_error(&img)));
    }
    if img.width == 0 || img.height == 0 || img.width > max_edge || img.height > max_edge {
        return Err(ImageError::Dimensions(img.width, img.height));
    }
    let byte_len = u64::from(img.width)
        .checked_mul(u64::from(img.height))
        .and_then(|px| px.checked_mul(4))
        .filter(|len| *len <= max_alloc)
        .ok_or(ImageError::Dimensions(img.width, img.height))?;

    img.format = PNG_FORMAT_RGBA;
    let mut rgba = vec![0u8; byte_len as usize];
    let ok = unsafe {
        png_image_finish_read(
            &mut img,
            ptr::null(),
            rgba.as_mut_ptr().cast(),
            0,
            ptr::null_mut(),
        )
    };
    if ok == 0 || (img.warning_or_error & 0x03) > 1 {
        return Err(ImageError::Decode(png_error(&img)));
    }
    RgbaImage::from_raw(img.width, img.height, rgba)
        .ok_or(ImageError::Dimensions(img.width, img.height))
}

fn decode_jpeg_rgba(bytes: &[u8], max_edge: u32, max_alloc: u64) -> Result<RgbaImage, ImageError> {
    struct TjHandle(*mut c_void);
    impl Drop for TjHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { tjDestroy(self.0) };
            }
        }
    }

    let handle = TjHandle(unsafe { tjInitDecompress() });
    if handle.0.is_null() {
        return Err(ImageError::Decode("tjInitDecompress failed".into()));
    }
    let mut width = 0 as c_int;
    let mut height = 0 as c_int;
    let mut subsamp = 0 as c_int;
    let mut colorspace = 0 as c_int;
    let rc = unsafe {
        tjDecompressHeader3(
            handle.0,
            bytes.as_ptr(),
            bytes.len() as std::ffi::c_ulong,
            &mut width,
            &mut height,
            &mut subsamp,
            &mut colorspace,
        )
    };
    if rc != 0 {
        return Err(ImageError::Decode(tj_error(handle.0)));
    }
    if width <= 0 || height <= 0 || width as u32 > max_edge || height as u32 > max_edge {
        return Err(ImageError::Dimensions(
            width.max(0) as u32,
            height.max(0) as u32,
        ));
    }
    let w = width as u32;
    let h = height as u32;
    let byte_len = u64::from(w)
        .checked_mul(u64::from(h))
        .and_then(|px| px.checked_mul(4))
        .filter(|len| *len <= max_alloc)
        .ok_or(ImageError::Dimensions(w, h))?;

    let mut rgba = vec![0u8; byte_len as usize];
    let rc = unsafe {
        tjDecompress2(
            handle.0,
            bytes.as_ptr(),
            bytes.len() as std::ffi::c_ulong,
            rgba.as_mut_ptr(),
            width,
            0,
            height,
            TJPF_RGBA,
            0,
        )
    };
    if rc != 0 {
        return Err(ImageError::Decode(tj_error(handle.0)));
    }
    RgbaImage::from_raw(w, h, rgba).ok_or(ImageError::Dimensions(w, h))
}

fn decode_webp_frames_bounded(
    bytes: &[u8],
    max_edge: u32,
    max_alloc: u64,
) -> Result<(u32, u32, Vec<AnimFrame>), ImageError> {
    struct WebpDec(*mut c_void);
    impl Drop for WebpDec {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WebPAnimDecoderDelete(self.0) };
            }
        }
    }

    let mut opts = WebPAnimDecoderOptions {
        color_mode: MODE_RGBA,
        use_threads: 0,
        padding: [0; 7],
    };
    if unsafe { WebPAnimDecoderOptionsInitInternal(&mut opts, WEBP_DEMUX_ABI_VERSION) } == 0 {
        return Err(ImageError::Decode(
            "WebPAnimDecoderOptionsInit failed".into(),
        ));
    }
    opts.color_mode = MODE_RGBA;

    let data = WebPData {
        bytes: bytes.as_ptr(),
        size: bytes.len(),
    };
    let dec = WebpDec(unsafe { WebPAnimDecoderNewInternal(&data, &opts, WEBP_DEMUX_ABI_VERSION) });
    if dec.0.is_null() {
        return Err(ImageError::Decode("WebPAnimDecoderNew failed".into()));
    }

    let mut info = WebPAnimInfo::default();
    if unsafe { WebPAnimDecoderGetInfo(dec.0, &mut info) } == 0 {
        return Err(ImageError::Decode("WebPAnimDecoderGetInfo failed".into()));
    }
    let w = info.canvas_width;
    let h = info.canvas_height;
    if w == 0 || h == 0 || w > max_edge || h > max_edge {
        return Err(ImageError::Dimensions(w, h));
    }
    let frame_bytes = u64::from(w)
        .checked_mul(u64::from(h))
        .and_then(|px| px.checked_mul(4))
        .filter(|len| *len <= max_alloc)
        .ok_or(ImageError::Dimensions(w, h))? as usize;

    let mut frames = Vec::new();
    let mut prev_ts_ms: i32 = 0;
    while unsafe { WebPAnimDecoderHasMoreFrames(dec.0) } != 0 {
        let mut buf: *mut u8 = ptr::null_mut();
        let mut ts_ms: c_int = 0;
        if unsafe { WebPAnimDecoderGetNext(dec.0, &mut buf, &mut ts_ms) } == 0 || buf.is_null() {
            return Err(ImageError::Decode("WebPAnimDecoderGetNext failed".into()));
        }
        let delta_ms = (ts_ms.saturating_sub(prev_ts_ms)).max(0) as u64;
        prev_ts_ms = ts_ms;
        let slice = unsafe { std::slice::from_raw_parts(buf, frame_bytes) };
        frames.push(AnimFrame {
            left: 0,
            top: 0,
            width: w,
            height: h,
            rgba: slice.to_vec(),
            delay: Duration::from_millis(delta_ms),
        });
    }

    if frames.is_empty() {
        return Err(ImageError::Decode("WebP contains no frames".into()));
    }
    Ok((w, h, frames))
}

fn png_error(img: &PngImage) -> String {
    let msg = unsafe { CStr::from_ptr(img.message.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if msg.is_empty() {
        "libpng error".into()
    } else {
        msg
    }
}

fn tj_error(handle: *mut c_void) -> String {
    let ptr = unsafe { tjGetErrorStr2(handle) };
    if ptr.is_null() {
        "libturbojpeg error".into()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}
