//! Video frame sources and test patterns.
//!
//! A WFD source transmits H.264 elementary streams muxed into MPEG-2 Transport
//! Stream packets. This module defines the [`FrameSource`] abstraction and
//! provides a self-contained, spec-compliant [`TestPatternSource`] (Stage 0
//! deliverable in ADR-0192 §5: "video source replaced by a static test pattern").

/// Static 1280x720 SMPTE color bars test pattern encoded as H.264 Constrained
/// Baseline Profile (Level 3.1) at 30 fps.
///
/// Contains Annex-B start codes prefixing SPS (type 7), PPS (type 8), and an
/// IDR keyframe slice (type 5).
pub const TEST_PATTERN_720P: &[u8] = include_bytes!("../../assets/pattern_720p.h264");

/// A source of H.264 video access units for streaming.
pub trait FrameSource: Send {
    /// Returns the next access unit (Annex-B H.264 byte stream) to present.
    fn next_access_unit(&mut self) -> Option<&[u8]>;

    /// Video width and height in pixels.
    fn resolution(&self) -> (u32, u32);

    /// Nominal frame rate (frames per second).
    fn fps(&self) -> u32;
}

/// A static H.264 test pattern source producing 1280x720 video at 30 fps.
#[derive(Debug, Clone)]
pub struct TestPatternSource {
    frame_data: &'static [u8],
    fps: u32,
    frames_produced: u64,
}

impl Default for TestPatternSource {
    fn default() -> Self {
        Self::new(30)
    }
}

impl TestPatternSource {
    pub fn new(fps: u32) -> Self {
        Self {
            frame_data: TEST_PATTERN_720P,
            fps: fps.max(1),
            frames_produced: 0,
        }
    }

    pub fn frames_produced(&self) -> u64 {
        self.frames_produced
    }
}

impl FrameSource for TestPatternSource {
    fn next_access_unit(&mut self) -> Option<&[u8]> {
        self.frames_produced += 1;
        Some(self.frame_data)
    }

    fn resolution(&self) -> (u32, u32) {
        (1280, 720)
    }

    fn fps(&self) -> u32 {
        self.fps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_starts_with_annex_b_start_code() {
        assert!(TEST_PATTERN_720P.len() > 100);
        assert_eq!(&TEST_PATTERN_720P[..4], &[0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn test_pattern_contains_sps_pps_and_idr() {
        let mut found_sps = false;
        let mut found_pps = false;
        let mut found_idr = false;

        let mut offset = 0;
        while offset + 3 < TEST_PATTERN_720P.len() {
            let nal_byte = if offset + 4 < TEST_PATTERN_720P.len()
                && TEST_PATTERN_720P[offset..offset + 4] == [0x00, 0x00, 0x00, 0x01]
            {
                Some(TEST_PATTERN_720P[offset + 4])
            } else if TEST_PATTERN_720P[offset..offset + 3] == [0x00, 0x00, 0x01] {
                Some(TEST_PATTERN_720P[offset + 3])
            } else {
                None
            };
            if let Some(byte) = nal_byte {
                let nal_type = byte & 0x1f;
                match nal_type {
                    7 => found_sps = true,
                    8 => found_pps = true,
                    5 => found_idr = true,
                    _ => {}
                }
            }
            offset += 1;
        }

        assert!(found_sps, "SPS NAL (type 7) must be present");
        assert!(found_pps, "PPS NAL (type 8) must be present");
        assert!(found_idr, "IDR NAL (type 5) must be present");
    }

    #[test]
    fn test_pattern_source_advances_frame_counter() {
        let mut source = TestPatternSource::default();
        assert_eq!(source.frames_produced(), 0);
        assert_eq!(source.resolution(), (1280, 720));
        assert_eq!(source.fps(), 30);

        let f1 = source.next_access_unit();
        assert!(f1.is_some());
        assert_eq!(source.frames_produced(), 1);

        let f2 = source.next_access_unit();
        assert!(f2.is_some());
        assert_eq!(source.frames_produced(), 2);
    }
}
