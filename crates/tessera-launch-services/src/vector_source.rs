//! Reading vector icon and cursor sources off disk.
//!
//! Two formats carry the same document: SVG text, and **SVGZ**, which is SVG
//! compressed with gzip (RFC 1952). Both appear in the icon and cursor
//! specifications' extension lists, so both have to be readable wherever a theme
//! asset is loaded — and the decompression has to happen exactly once, in one
//! place, rather than in every consumer that happens to read a file.
//!
//! This module owns the *bytes*, not the format: it hands back text that an SVG
//! compiler can parse and knows nothing about what is in it. Decompression is
//! bounded, because a `.svgz` from a theme directory is untrusted input and a
//! gzip bomb must not be able to turn a 4 KiB file into gigabytes.

use std::io::Read;
use std::path::Path;

/// The most text a single `.svgz` may decompress to.
///
/// The SVG compiler has its own input ceiling; this one exists so the bomb is
/// stopped *before* the whole expansion is materialised. 16 MiB is four thousand
/// times a large icon and still small enough that a hostile theme cannot
/// exhaust memory: the limit is the point, not the number.
const MAX_DECOMPRESSED_BYTES: u64 = 16 * 1024 * 1024;

/// Why a source could not be read as vector text.
#[derive(Debug)]
pub enum SourceError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file is not valid gzip, or decompressed past the ceiling.
    Compression(&'static str),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::Io(error) => write!(f, "cannot read vector source: {error}"),
            SourceError::Compression(reason) => write!(f, "cannot decompress svgz: {reason}"),
        }
    }
}

impl std::error::Error for SourceError {}

impl From<std::io::Error> for SourceError {
    fn from(error: std::io::Error) -> Self {
        SourceError::Io(error)
    }
}

/// Read a vector source file, transparently gunzipping `.svgz`.
///
/// The format is chosen by the file's extension *and* by its magic bytes: a
/// `.svg` that actually begins with the gzip header is decompressed too, because
/// themes do get their extensions wrong and failing on that is a worse answer
/// than reading the file the user meant.
pub fn read_vector_source(path: &Path) -> Result<Vec<u8>, SourceError> {
    let bytes = std::fs::read(path)?;
    decompress_if_gzip(bytes)
}

/// Gunzip `bytes` when they carry the gzip magic number, otherwise return them
/// unchanged.
///
/// Kept separate from [`read_vector_source`] so a caller that already holds the
/// bytes (an embedded asset, an IPC payload) shares the same rule.
pub fn decompress_if_gzip(bytes: Vec<u8>) -> Result<Vec<u8>, SourceError> {
    const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
    if bytes.len() < 2 || bytes[..2] != GZIP_MAGIC {
        return Ok(bytes);
    }
    let mut decoder = flate2::read::GzDecoder::new(bytes.as_slice());
    // Read one byte past the ceiling so "exactly at the limit" is accepted and
    // "over it" is detected without trusting the gzip trailer's length field
    // (which an attacker controls).
    let mut text = Vec::new();
    let mut limited = decoder.by_ref().take(MAX_DECOMPRESSED_BYTES + 1);
    limited
        .read_to_end(&mut text)
        .map_err(|_| SourceError::Compression("invalid gzip stream"))?;
    if text.len() as u64 > MAX_DECOMPRESSED_BYTES {
        return Err(SourceError::Compression(
            "decompressed size exceeds the limit",
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(payload: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(payload).expect("write gzip payload");
        encoder.finish().expect("finish gzip")
    }

    const SVG: &[u8] = b"<svg viewBox=\"0 0 8 8\"><rect width=\"8\" height=\"8\"/></svg>";

    #[test]
    fn plain_svg_passes_through_unchanged() {
        let out = decompress_if_gzip(SVG.to_vec()).expect("plain svg");
        assert_eq!(out, SVG);
    }

    #[test]
    fn gzip_compressed_svg_is_expanded() {
        // A `.svgz` is SVG under gzip. Advertising the extension while handing
        // the compiler a gzip stream would make every svgz theme asset silently
        // fail to parse, so this is the behaviour the extension list promises.
        let out = decompress_if_gzip(gzip(SVG)).expect("svgz");
        assert_eq!(out, SVG);
    }

    #[test]
    fn compressed_bytes_without_the_extension_are_still_expanded() {
        // Themes get extensions wrong; reading the file the user meant is a
        // better answer than refusing it because of a suffix.
        let out = decompress_if_gzip(gzip(SVG)).expect("svg with wrong extension");
        assert_eq!(out, SVG);
    }

    #[test]
    fn a_bomb_is_refused_rather_than_expanded() {
        // 17 MiB of zeros compresses to a few KiB. The ceiling must fire, and it
        // must fire without materialising the whole expansion.
        let payload = vec![b'A'; (MAX_DECOMPRESSED_BYTES + 1024) as usize];
        let compressed = gzip(&payload);
        assert!(
            compressed.len() < 64 * 1024,
            "the fixture must actually be a bomb ({} bytes)",
            compressed.len()
        );
        match decompress_if_gzip(compressed) {
            Err(SourceError::Compression(_)) => {}
            other => panic!("expected a compression refusal, got {other:?}"),
        }
    }

    #[test]
    fn truncated_gzip_is_an_error_not_a_partial_document() {
        let mut compressed = gzip(SVG);
        compressed.truncate(compressed.len() / 2);
        match decompress_if_gzip(compressed) {
            Err(SourceError::Compression(_)) => {}
            other => panic!("expected a compression refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_file_is_not_gzip() {
        assert_eq!(
            decompress_if_gzip(Vec::new()).expect("empty"),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn a_single_gzip_magic_byte_is_not_gzip() {
        // The magic number is two bytes; a file that only matches the first must
        // not be fed to the decoder.
        assert_eq!(
            decompress_if_gzip(vec![0x1f]).expect("one byte"),
            vec![0x1f]
        );
    }
}
