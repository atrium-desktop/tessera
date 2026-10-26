//! H.264 Annex-B stream parser: splitting a byte stream into complete Access Units.
//!
//! An H.264 stream transmitted over MPEG-2 TS consists of Annex-B formatted NAL units
//! preceded by 3-byte (`0x00 0x00 0x01`) or 4-byte (`0x00 0x00 0x00 0x01`) start codes.
//! This reader accumulates bytes from any [`std::io::Read`] and yields complete
//! Access Units (SPS, PPS, SEI, and slice data for a single frame) suitable for
//! [`crate::wfd::ts::TsMuxer::video_access_unit`].

use std::io::{self, Read};

/// An access unit parser over any byte reader.
#[derive(Debug)]
pub struct H264AccessUnitReader<R> {
    reader: R,
    buffer: Vec<u8>,
    eof: bool,
}

impl<R: Read> H264AccessUnitReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::with_capacity(32 * 1024),
            eof: false,
        }
    }

    /// Read from the underlying reader and return the next complete Access Unit,
    /// or `None` if EOF has been reached with no remaining data.
    pub fn next_access_unit(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut chunk = [0u8; 4096];

        loop {
            // Check if our accumulated buffer contains a complete access unit boundary
            if let Some(au_end) = self.find_access_unit_boundary() {
                let au = self.buffer[..au_end].to_vec();
                self.buffer.drain(..au_end);
                return Ok(Some(au));
            }

            if self.eof {
                if !self.buffer.is_empty() {
                    let au = std::mem::take(&mut self.buffer);
                    return Ok(Some(au));
                }
                return Ok(None);
            }

            match self.reader.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                }
                Ok(n) => {
                    self.buffer.extend_from_slice(&chunk[..n]);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    }

    /// Scan `self.buffer` for start codes and locate the end of the current access unit.
    fn find_access_unit_boundary(&self) -> Option<usize> {
        let mut idx = 0;
        let buf_len = self.buffer.len();
        let mut has_vcl = false;

        while idx + 3 < buf_len {
            let (is_start_code, prefix_len) =
                if idx + 4 <= buf_len && self.buffer[idx..idx + 4] == [0x00, 0x00, 0x00, 0x01] {
                    (true, 4)
                } else if self.buffer[idx..idx + 3] == [0x00, 0x00, 0x01] {
                    (true, 3)
                } else {
                    (false, 0)
                };

            if is_start_code {
                let nal_header_idx = idx + prefix_len;
                if nal_header_idx < buf_len {
                    let nal_type = self.buffer[nal_header_idx] & 0x1f;
                    let is_vcl = (1..=5).contains(&nal_type);

                    if has_vcl && (nal_type == 7 || nal_type == 9 || is_vcl) {
                        return Some(idx);
                    }

                    if is_vcl {
                        has_vcl = true;
                    }
                }
                idx += prefix_len;
            } else {
                idx += 1;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reader_splits_annex_b_stream_into_access_units() {
        // Frame 0: SPS (7) + PPS (8) + IDR (5)
        // Frame 1: Slice (1)
        // Frame 2: Slice (1)
        let mut stream = Vec::new();

        // Frame 0
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00]); // SPS
        stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xce]); // PPS
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x99]); // IDR slice

        // Frame 1
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x61, 0xaa, 0xbb]); // P slice

        // Frame 2
        stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x61, 0xcc, 0xdd]); // P slice

        let mut reader = H264AccessUnitReader::new(Cursor::new(stream));

        let au0 = reader.next_access_unit().expect("read au0").expect("au0");
        assert!(au0.len() >= 16);
        assert_eq!(&au0[..4], &[0x00, 0x00, 0x00, 0x01]);
        assert_eq!(au0[4] & 0x1f, 7); // Starts with SPS

        let au1 = reader.next_access_unit().expect("read au1").expect("au1");
        assert_eq!(&au1[..4], &[0x00, 0x00, 0x00, 0x01]);
        assert_eq!(au1[4] & 0x1f, 1); // P slice

        let au2 = reader.next_access_unit().expect("read au2").expect("au2");
        assert_eq!(&au2[..3], &[0x00, 0x00, 0x01]);
        assert_eq!(au2[3] & 0x1f, 1); // P slice

        let au3 = reader.next_access_unit().expect("read au3");
        assert!(au3.is_none(), "EOF reached");
    }
}
