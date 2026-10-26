//! RTP (RFC 3550) headers and H.264 payloads (RFC 6184).
//!
//! A WFD source sends H.264 in MPEG2-TS over RTP, so this module owns the
//! packetization from NAL units to network-order RTP packets: one packet for a
//! NAL that fits the MTU, and Fragmentation Unit type A (`FU-A`) for one that
//! does not. The `marker` bit is set on the last packet of the last NAL of a
//! frame, which is what a sink uses to assemble an access unit.

use std::collections::BTreeMap;

/// RTP fixed header size, bytes.
pub const RTP_HEADER_LEN: usize = 12;

/// Dynamic payload type for H.264 (RFC 6184 §RTP Payload Format Parameters).
pub const H264_PAYLOAD_TYPE: u8 = 96;

/// Payload type for MPEG-2 Transport Stream (RFC 2250 / RFC 3551 §5).
pub const MP2T_PAYLOAD_TYPE: u8 = 33;

/// MPEG-2 Transport Stream packet length in bytes.
pub const TS_PACKET_LEN: usize = 188;

/// Default number of TS packets packed into one RTP datagram (7 * 188 = 1316 bytes).
pub const TS_PACKETS_PER_RTP: usize = 7;

/// Default payload size for MPEG-2 TS over RTP (1316 bytes).
pub const DEFAULT_TS_RTP_PAYLOAD: usize = TS_PACKETS_PER_RTP * TS_PACKET_LEN;

/// Clock rate for a 90 kHz video RTP timestamp. A WFD stream advances the
/// timestamp by `90_000 / fps` per frame.
pub const H264_CLOCK_RATE: u32 = 90_000;

/// Default payload budget per packet. 1400 keeps the whole datagram inside a
/// 1500-byte MTU after IPv4 (20) and UDP (8) headers with margin for a
/// transport with a smaller MTU than Ethernet.
pub const DEFAULT_MAX_PAYLOAD: usize = 1400;

/// FU-A indicator/header pair, 2 bytes, so a fragment carries
/// `DEFAULT_MAX_PAYLOAD - 2` NAL bytes.
const FU_A_HEADER_LEN: usize = 2;

/// RTP fixed header fields this module sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
    pub payload_type: u8,
    pub marker: bool,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl RtpHeader {
    fn write(&self, out: &mut Vec<u8>) {
        out.push(0x80); // V=2, P=0, X=0, CC=0
        let marker = u8::from(self.marker) << 7;
        out.push(marker | (self.payload_type & 0x7f));
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.ssrc.to_be_bytes());
    }
}

/// A stream of RTP packets for one SSRC, with monotonic sequencing.
#[derive(Debug, Clone)]
pub struct RtpPacketizer {
    ssrc: u32,
    sequence: u16,
    max_payload: usize,
}

impl RtpPacketizer {
    pub fn new(ssrc: u32) -> Self {
        Self {
            ssrc,
            sequence: 0,
            max_payload: DEFAULT_MAX_PAYLOAD,
        }
    }

    /// Override the payload budget (tests use a small one to force
    /// fragmentation without megabyte fixtures).
    pub fn with_max_payload(mut self, max_payload: usize) -> Self {
        // The header pair is not optional, so a budget below it cannot carry
        // any NAL byte.
        self.max_payload = max_payload.max(FU_A_HEADER_LEN + 1);
        self
    }

    pub fn sequence(&self) -> u16 {
        self.sequence
    }

    /// Split one access unit into RTP packets.
    ///
    /// `nals` are H.264 NAL units **without** Annex-B start codes and without
    /// emulation-prevention bytes added (a WFD source sends the encoder's
    /// output as-is). `timestamp` is the 90 kHz presentation timestamp for the
    /// whole access unit; every packet of the unit carries it, and the last
    /// packet carries `marker`.
    ///
    /// An empty access unit yields no packets — an encoder that produced
    /// nothing must not emit a stray marker packet, which a sink would treat as
    /// an end-of-frame with no data.
    pub fn packetize(&mut self, nals: &[Vec<u8>], timestamp: u32) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        if nals.is_empty() {
            return packets;
        }
        let last_nal = nals.len() - 1;
        for (index, nal) in nals.iter().enumerate() {
            if nal.is_empty() {
                continue;
            }
            let is_last_nal = index == last_nal;
            self.packetize_nal(nal, timestamp, is_last_nal, &mut packets);
        }
        // If the only NALs were empty, the output is empty and no marker was
        // emitted; the caller still gets a consistent (empty) result.
        packets
    }

    fn packetize_nal(
        &mut self,
        nal: &[u8],
        timestamp: u32,
        is_last_nal: bool,
        out: &mut Vec<Vec<u8>>,
    ) {
        let header = |marker: bool, sequence: u16| RtpHeader {
            payload_type: H264_PAYLOAD_TYPE,
            marker,
            sequence,
            timestamp,
            ssrc: self.ssrc,
        };
        if nal.len() <= self.max_payload {
            let mut packet = Vec::with_capacity(RTP_HEADER_LEN + nal.len());
            header(is_last_nal, self.sequence).write(&mut packet);
            packet.extend_from_slice(nal);
            self.sequence = self.sequence.wrapping_add(1);
            out.push(packet);
            return;
        }
        // FU-A: indicator (F|NRI from the NAL header, type 28), then a FU
        // header carrying S/E and the original type in its low five bits.
        let nal_header = nal[0];
        let indicator = (nal_header & 0xe0) | 28;
        let original_type = nal_header & 0x1f;
        let body = &nal[1..];
        let chunk = self.max_payload - FU_A_HEADER_LEN;
        let mut offset = 0;
        while offset < body.len() {
            let end = (offset + chunk).min(body.len());
            let start_bit = offset == 0;
            let end_bit = end == body.len();
            let mut packet = Vec::with_capacity(RTP_HEADER_LEN + FU_A_HEADER_LEN + (end - offset));
            header(is_last_nal && end_bit, self.sequence).write(&mut packet);
            packet.push(indicator);
            let se = (u8::from(start_bit) << 7) | (u8::from(end_bit) << 6);
            packet.push(se | original_type);
            packet.extend_from_slice(&body[offset..end]);
            self.sequence = self.sequence.wrapping_add(1);
            out.push(packet);
            offset = end;
        }
    }

    /// Packetize MPEG-2 Transport Stream packets into RTP packets (RFC 2250).
    ///
    /// `ts_stream` contains one or more 188-byte TS packets.
    /// Packets are grouped into chunks of up to `TS_PACKETS_PER_RTP` (7 * 188 = 1316 bytes).
    /// Each chunk is wrapped with an RTP header with payload type `MP2T_PAYLOAD_TYPE` (33)
    /// and the access unit's 90 kHz `timestamp`.
    /// `marker` is set on the last RTP packet of the access unit.
    pub fn packetize_ts(&mut self, ts_stream: &[u8], timestamp: u32) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        if ts_stream.is_empty() {
            return packets;
        }
        let chunk_size = self.max_payload.min(DEFAULT_TS_RTP_PAYLOAD);
        // Align chunk size to 188-byte TS packet boundaries
        let aligned_chunk = (chunk_size / TS_PACKET_LEN).max(1) * TS_PACKET_LEN;

        let mut offset = 0;
        while offset < ts_stream.len() {
            let end = (offset + aligned_chunk).min(ts_stream.len());
            let is_last = end == ts_stream.len();
            let mut packet = Vec::with_capacity(RTP_HEADER_LEN + (end - offset));
            let header = RtpHeader {
                payload_type: MP2T_PAYLOAD_TYPE,
                marker: is_last,
                sequence: self.sequence,
                timestamp,
                ssrc: self.ssrc,
            };
            header.write(&mut packet);
            packet.extend_from_slice(&ts_stream[offset..end]);
            self.sequence = self.sequence.wrapping_add(1);
            packets.push(packet);
            offset = end;
        }
        packets
    }
}

/// One parsed RTP packet, for traces and for reassembly in a test sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPacket {
    pub marker: bool,
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtpError {
    TooShort(usize),
    NotVersion2(u8),
    HeaderExtensionUnsupported,
    CsrcListUnsupported,
    UnexpectedPayloadType(u8),
    MisalignedTsPayload(usize),
}

impl std::fmt::Display for RtpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort(len) => write!(formatter, "RTP packet of {len} bytes is too short"),
            Self::NotVersion2(version) => write!(formatter, "RTP version {version} is not 2"),
            Self::HeaderExtensionUnsupported => {
                formatter.write_str("RTP header extension is not supported")
            }
            Self::CsrcListUnsupported => formatter.write_str("RTP CSRC list is not supported"),
            Self::UnexpectedPayloadType(pt) => {
                write!(formatter, "expected MP2T payload type 33, got {pt}")
            }
            Self::MisalignedTsPayload(len) => {
                write!(
                    formatter,
                    "TS payload length {len} is not a multiple of 188"
                )
            }
        }
    }
}

impl std::error::Error for RtpError {}

impl RtpPacket {
    /// Parse a fixed-header RTP packet. Extensions and CSRC lists are refused
    /// rather than skipped: a WFD source never sends them, so their presence
    /// means the trace is not the stream we think it is.
    pub fn parse(bytes: &[u8]) -> Result<Self, RtpError> {
        if bytes.len() < RTP_HEADER_LEN {
            return Err(RtpError::TooShort(bytes.len()));
        }
        let version = bytes[0] >> 6;
        if version != 2 {
            return Err(RtpError::NotVersion2(version));
        }
        if bytes[0] & 0x10 != 0 {
            return Err(RtpError::HeaderExtensionUnsupported);
        }
        if bytes[0] & 0x0f != 0 {
            return Err(RtpError::CsrcListUnsupported);
        }
        let marker = bytes[1] & 0x80 != 0;
        let payload_type = bytes[1] & 0x7f;
        let sequence = u16::from_be_bytes([bytes[2], bytes[3]]);
        let timestamp = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let ssrc = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        Ok(Self {
            marker,
            payload_type,
            sequence,
            timestamp,
            ssrc,
            payload: bytes[RTP_HEADER_LEN..].to_vec(),
        })
    }
}

/// Reassemble NAL units from RTP packets, for traces and tests. Not used by the
/// sender; it exists so a captured stream can be proven to carry what was sent.
#[derive(Debug, Default)]
pub struct RtpDepacketizer {
    fragment: Option<(u8, Vec<u8>)>,
    last_timestamp: Option<u32>,
    completed: BTreeMap<u32, Vec<Vec<u8>>>,
}

impl RtpDepacketizer {
    /// Feed one packet; returns a completed access unit's NALs on the packet
    /// that carried `marker`.
    pub fn push(&mut self, packet: &RtpPacket) -> Option<Vec<Vec<u8>>> {
        if self.last_timestamp != Some(packet.timestamp) {
            // A new timestamp abandons an incomplete fragment: a partial NAL
            // must never be handed on as if it were whole.
            self.fragment = None;
            self.last_timestamp = Some(packet.timestamp);
        }
        let mut nals: Option<Vec<Vec<u8>>> = None;
        if packet.payload.len() >= 2 {
            let indicator = packet.payload[0];
            if indicator & 0x1f == 28 {
                let fu_header = packet.payload[1];
                let start = fu_header & 0x80 != 0;
                let end = fu_header & 0x40 != 0;
                let original_type = fu_header & 0x1f;
                let reconstructed = (indicator & 0xe0) | original_type;
                if start {
                    self.fragment = Some((reconstructed, Vec::new()));
                }
                if let Some((header, buffer)) = self.fragment.as_mut()
                    && *header == reconstructed
                {
                    buffer.extend_from_slice(&packet.payload[2..]);
                }
                if end && let Some((header, buffer)) = self.fragment.take() {
                    let mut nal = Vec::with_capacity(buffer.len() + 1);
                    nal.push(header);
                    nal.extend_from_slice(&buffer);
                    self.completed
                        .entry(packet.timestamp)
                        .or_default()
                        .push(nal);
                }
            } else {
                self.completed
                    .entry(packet.timestamp)
                    .or_default()
                    .push(packet.payload.clone());
            }
        }
        if packet.marker {
            nals = self.completed.remove(&packet.timestamp);
        }
        nals
    }

    /// Feed one RTP packet carrying MPEG-2 Transport Stream; returns the TS bytes
    /// if the payload type matches `MP2T_PAYLOAD_TYPE` and payload length is a
    /// multiple of 188.
    pub fn push_ts(&self, packet: &RtpPacket) -> Result<Vec<u8>, RtpError> {
        if packet.payload_type != MP2T_PAYLOAD_TYPE {
            return Err(RtpError::UnexpectedPayloadType(packet.payload_type));
        }
        if !packet.payload.len().is_multiple_of(TS_PACKET_LEN) {
            return Err(RtpError::MisalignedTsPayload(packet.payload.len()));
        }
        Ok(packet.payload.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(header: u8, len: usize) -> Vec<u8> {
        let mut nal = vec![header];
        nal.extend(std::iter::repeat_n(0xa5u8, len.saturating_sub(1)));
        nal
    }

    #[test]
    fn a_small_nal_is_a_single_packet_with_the_marker() {
        let mut packetizer = RtpPacketizer::new(0x1234_5678);
        let packets = packetizer.packetize(&[nal(0x65, 100)], 90_000);
        assert_eq!(packets.len(), 1);
        let parsed = RtpPacket::parse(&packets[0]).expect("parse");
        assert_eq!(parsed.payload_type, H264_PAYLOAD_TYPE);
        assert!(
            parsed.marker,
            "single-NAL access unit must mark its only packet"
        );
        assert_eq!(parsed.timestamp, 90_000);
        assert_eq!(parsed.ssrc, 0x1234_5678);
        assert_eq!(parsed.payload, nal(0x65, 100));
    }

    #[test]
    fn an_oversized_nal_fragments_with_start_and_end_flags() {
        let mut packetizer = RtpPacketizer::new(1).with_max_payload(100);
        let source = nal(0x65, 400);
        let packets = packetizer.packetize(std::slice::from_ref(&source), 1234);
        assert!(packets.len() > 1, "must fragment");

        let mut reassembled: Vec<u8> = Vec::new();
        let total = packets.len();
        for (index, bytes) in packets.iter().enumerate() {
            let parsed = RtpPacket::parse(bytes).expect("parse");
            assert_eq!(parsed.timestamp, 1234);
            assert_eq!(parsed.sequence, index as u16, "sequence must be contiguous");
            assert_eq!(
                parsed.marker,
                index == total - 1,
                "only the final fragment carries the marker"
            );
            let indicator = parsed.payload[0];
            let fu_header = parsed.payload[1];
            assert_eq!(indicator & 0x1f, 28, "FU-A indicator");
            assert_eq!(indicator & 0xe0, source[0] & 0xe0, "F|NRI copied");
            assert_eq!(fu_header & 0x1f, source[0] & 0x1f, "original type copied");
            assert_eq!(
                fu_header & 0x80 != 0,
                index == 0,
                "start bit on the first fragment only"
            );
            assert_eq!(
                fu_header & 0x40 != 0,
                index == total - 1,
                "end bit on the last fragment only"
            );
            if index == 0 {
                reassembled.push((indicator & 0xe0) | (fu_header & 0x1f));
            }
            reassembled.extend_from_slice(&parsed.payload[2..]);
        }
        assert_eq!(reassembled, source, "FU-A fragments must rebuild the NAL");
    }

    #[test]
    fn depacketizer_rebuilds_a_multi_nal_access_unit() {
        let mut packetizer = RtpPacketizer::new(7).with_max_payload(64);
        let a = nal(0x67, 200);
        let b = nal(0x68, 20);
        let c = nal(0x65, 300);
        let packets = packetizer.packetize(&[a.clone(), b.clone(), c.clone()], 999);
        let mut depacketizer = RtpDepacketizer::default();
        let mut units = Vec::new();
        for bytes in &packets {
            let parsed = RtpPacket::parse(bytes).expect("parse");
            if let Some(unit) = depacketizer.push(&parsed) {
                units.push(unit);
            }
        }
        assert_eq!(units.len(), 1, "one marker closes one access unit");
        assert_eq!(units[0], vec![a, b, c]);
    }

    #[test]
    fn an_empty_access_unit_emits_nothing() {
        let mut packetizer = RtpPacketizer::new(3);
        assert!(packetizer.packetize(&[], 0).is_empty());
        // Empty NALs must not produce a marker-only packet either.
        assert!(packetizer.packetize(&[Vec::new()], 0).is_empty());
        assert_eq!(
            packetizer.sequence(),
            0,
            "no packet consumed a sequence number"
        );
    }

    #[test]
    fn sequence_numbers_wrap_without_panicking() {
        let mut packetizer = RtpPacketizer::new(1);
        packetizer.sequence = u16::MAX;
        let packets = packetizer.packetize(&[nal(0x65, 10)], 1);
        assert_eq!(RtpPacket::parse(&packets[0]).unwrap().sequence, u16::MAX);
        let packets = packetizer.packetize(&[nal(0x65, 10)], 2);
        assert_eq!(RtpPacket::parse(&packets[0]).unwrap().sequence, 0);
    }

    #[test]
    fn parse_rejects_headers_a_wfd_source_never_sends() {
        assert!(matches!(
            RtpPacket::parse(&[0x80; 8]),
            Err(RtpError::TooShort(8))
        ));
        let mut not_v2 = vec![0u8; RTP_HEADER_LEN];
        not_v2[0] = 0x40; // version 1
        assert!(matches!(
            RtpPacket::parse(&not_v2),
            Err(RtpError::NotVersion2(1))
        ));
        let mut extension = vec![0u8; RTP_HEADER_LEN];
        extension[0] = 0x90; // X=1
        assert!(matches!(
            RtpPacket::parse(&extension),
            Err(RtpError::HeaderExtensionUnsupported)
        ));
        let mut csrc = vec![0u8; RTP_HEADER_LEN];
        csrc[0] = 0x81; // CC=1
        assert!(matches!(
            RtpPacket::parse(&csrc),
            Err(RtpError::CsrcListUnsupported)
        ));
    }

    #[test]
    fn ts_packets_are_packed_into_rtp_datagrams_with_pt_33() {
        let mut packetizer = RtpPacketizer::new(0x1234_5678);
        let ts_dummy = vec![0x47u8; 188]; // 1 TS packet
        let packets = packetizer.packetize_ts(&ts_dummy, 90_000);
        assert_eq!(packets.len(), 1);
        let parsed = RtpPacket::parse(&packets[0]).expect("parse");
        assert_eq!(parsed.payload_type, MP2T_PAYLOAD_TYPE);
        assert_eq!(parsed.timestamp, 90_000);
        assert_eq!(parsed.ssrc, 0x1234_5678);
        assert!(parsed.marker, "last packet of AU has marker set");
        assert_eq!(parsed.payload.len(), 188);

        let depacketizer = RtpDepacketizer::default();
        let extracted = depacketizer.push_ts(&parsed).expect("depacketize ts");
        assert_eq!(extracted, ts_dummy);
    }

    #[test]
    fn ts_packets_group_up_to_seven_per_datagram() {
        let mut packetizer = RtpPacketizer::new(1);
        // 15 TS packets: 7 in packet 1, 7 in packet 2, 1 in packet 3
        let ts_stream = vec![0x47u8; 15 * TS_PACKET_LEN];
        let packets = packetizer.packetize_ts(&ts_stream, 180_000);
        assert_eq!(packets.len(), 3);

        let p1 = RtpPacket::parse(&packets[0]).unwrap();
        assert_eq!(p1.sequence, 0);
        assert!(!p1.marker);
        assert_eq!(p1.payload.len(), 7 * TS_PACKET_LEN);

        let p2 = RtpPacket::parse(&packets[1]).unwrap();
        assert_eq!(p2.sequence, 1);
        assert!(!p2.marker);
        assert_eq!(p2.payload.len(), 7 * TS_PACKET_LEN);

        let p3 = RtpPacket::parse(&packets[2]).unwrap();
        assert_eq!(p3.sequence, 2);
        assert!(p3.marker, "tail packet has marker");
        assert_eq!(p3.payload.len(), TS_PACKET_LEN);

        let depacketizer = RtpDepacketizer::default();
        let mut reassembled = Vec::new();
        for p in [&p1, &p2, &p3] {
            reassembled.extend(depacketizer.push_ts(p).unwrap());
        }
        assert_eq!(reassembled, ts_stream);
    }

    #[test]
    fn depacketize_ts_rejects_wrong_pt_or_misaligned_payload() {
        let depacketizer = RtpDepacketizer::default();
        let wrong_pt = RtpPacket {
            marker: true,
            payload_type: H264_PAYLOAD_TYPE,
            sequence: 0,
            timestamp: 0,
            ssrc: 0,
            payload: vec![0x47; 188],
        };
        assert!(matches!(
            depacketizer.push_ts(&wrong_pt),
            Err(RtpError::UnexpectedPayloadType(96))
        ));

        let misaligned = RtpPacket {
            marker: true,
            payload_type: MP2T_PAYLOAD_TYPE,
            sequence: 0,
            timestamp: 0,
            ssrc: 0,
            payload: vec![0x47; 180], // not a multiple of 188
        };
        assert!(matches!(
            depacketizer.push_ts(&misaligned),
            Err(RtpError::MisalignedTsPayload(180))
        ));
    }
}
