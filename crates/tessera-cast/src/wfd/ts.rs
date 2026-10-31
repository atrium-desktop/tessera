//! MPEG2 transport stream muxing (ISO/IEC 13818-1): PAT, PMT, PES, PCR.
//!
//! A WFD source sends H.264 in a transport stream whose single program carries
//! one video elementary stream. This module owns exactly that shape — no audio,
//! no multiple programs — so the tables are generated rather than edited.
//!
//! Fixed choices, all recorded here rather than discovered by a sink:
//! transport stream id `1`, program number `1`, PMT PID `0x1000`, video PID
//! `0x1011`, `stream_type` `0x1B` (H.264), and a 90 kHz PCR that advances with
//! the presentation timestamps.

/// Transport packet size, bytes.
pub const TS_PACKET_LEN: usize = 188;
const TS_SYNC: u8 = 0x47;
const TS_HEADER_LEN: usize = 4;
const TS_PAYLOAD_LEN: usize = TS_PACKET_LEN - TS_HEADER_LEN; // 184

const PID_PAT: u16 = 0x0000;
const PID_SDT: u16 = 0x0011;
const TABLE_ID_PAT: u8 = 0x00;
const TABLE_ID_PMT: u8 = 0x02;
const TABLE_ID_SDT: u8 = 0x42;
/// H.264 video, ISO/IEC 13818-1 stream type table.
const STREAM_TYPE_H264: u8 = 0x1b;
/// AAC audio in ADTS framing, ISO/IEC 13818-1 stream type.
const STREAM_TYPE_AAC_ADTS: u8 = 0x0f;
const PES_STREAM_ID_VIDEO: u8 = 0xe0;
const PES_STREAM_ID_AUDIO: u8 = 0xc0;

/// The muxer's generated table and stream identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TsLayout {
    pub transport_stream_id: u16,
    pub program_number: u16,
    pub pmt_pid: u16,
    pub video_pid: u16,
    pub audio_pid: u16,
}

impl Default for TsLayout {
    fn default() -> Self {
        Self {
            transport_stream_id: 1,
            program_number: 1,
            pmt_pid: 0x1000,
            video_pid: 0x0100,
            audio_pid: 0x0101,
        }
    }
}

/// A MPEG2 transport stream muxer carrying one H.264 elementary stream, and
/// optionally one AAC audio stream.
#[derive(Debug)]
pub struct TsMuxer {
    layout: TsLayout,
    continuity: [u8; 0x2000],
    audio: bool,
}

impl Default for TsMuxer {
    fn default() -> Self {
        Self::new(TsLayout::default())
    }
}

impl TsMuxer {
    /// A video-only muxer.
    pub fn new(layout: TsLayout) -> Self {
        Self {
            layout,
            continuity: [0; 0x2000],
            audio: false,
        }
    }

    /// A muxer that declares and accepts the AAC audio elementary stream.
    pub fn with_audio(layout: TsLayout) -> Self {
        Self {
            layout,
            continuity: [0; 0x2000],
            audio: true,
        }
    }

    pub fn layout(&self) -> TsLayout {
        self.layout
    }

    pub fn has_audio(&self) -> bool {
        self.audio
    }

    /// The program association table: one program, pointing at the PMT PID.
    pub fn pat(&mut self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(TS_PAYLOAD_LEN);
        payload.push(0x00); // pointer_field
        payload.push(TABLE_ID_PAT);
        payload.extend_from_slice(&[0xb0, 0x0d]); // section_syntax_indicator, length 13
        payload.extend_from_slice(&self.layout.transport_stream_id.to_be_bytes());
        payload.push(0xc1); // version 0, current_next 1
        payload.push(0x00); // section_number
        payload.push(0x00); // last_section_number
        payload.extend_from_slice(&self.layout.program_number.to_be_bytes());
        payload.push(0xe0 | ((self.layout.pmt_pid >> 8) as u8 & 0x1f));
        payload.push((self.layout.pmt_pid & 0xff) as u8);
        let crc = crc32_mpeg2(&payload[1..]);
        payload.extend_from_slice(&crc.to_be_bytes());
        self.section_packet(PID_PAT, payload, 0)
    }

    /// The program map table: one H.264 video stream and, when enabled, one AAC
    /// audio stream.
    pub fn pmt(&mut self) -> Vec<u8> {
        let mut section = Vec::with_capacity(TS_PAYLOAD_LEN);
        section.push(TABLE_ID_PMT);
        // 9 bytes of fixed fields + 5 per elementary stream + 4 CRC.
        let streams: u16 = if self.audio { 2 } else { 1 };
        let section_len: u16 = 9 + 5 * streams + 4;
        section.push(0xb0 | ((section_len >> 8) as u8 & 0x0f));
        section.push((section_len & 0xff) as u8);
        section.extend_from_slice(&self.layout.program_number.to_be_bytes());
        section.push(0xc1); // version 0, current_next 1
        section.push(0x00); // section_number
        section.push(0x00); // last_section_number
        // PCR is carried on the video PID.
        section.push(0xe0 | ((self.layout.video_pid >> 8) as u8 & 0x1f));
        section.push((self.layout.video_pid & 0xff) as u8);
        section.push(0xf0); // program_info_length high nibble
        section.push(0x00);
        // One elementary stream.
        section.push(STREAM_TYPE_H264);
        section.push(0xe0 | ((self.layout.video_pid >> 8) as u8 & 0x1f));
        section.push((self.layout.video_pid & 0xff) as u8);
        section.push(0xf0); // ES_info_length
        section.push(0x00);
        if self.audio {
            section.push(STREAM_TYPE_AAC_ADTS);
            section.push(0xe0 | ((self.layout.audio_pid >> 8) as u8 & 0x1f));
            section.push((self.layout.audio_pid & 0xff) as u8);
            section.push(0xf0); // ES_info_length
            section.push(0x00);
        }
        let crc = crc32_mpeg2(&section);
        section.extend_from_slice(&crc.to_be_bytes());

        let mut payload = Vec::with_capacity(TS_PAYLOAD_LEN);
        payload.push(0x00); // pointer_field
        payload.extend_from_slice(&section);
        self.section_packet(self.layout.pmt_pid, payload, 0)
    }

    /// The service description table. ffmpeg's mpegtsmux emits one every
    /// 500 ms; some Android demuxers bind the audio track to the service from
    /// the SDT before they will play it, even when the PMT is otherwise valid.
    pub fn sdt(&mut self) -> Vec<u8> {
        let mut body = Vec::with_capacity(TS_PAYLOAD_LEN);
        body.extend_from_slice(&self.layout.transport_stream_id.to_be_bytes());
        body.push(0xc1); // reserved, version 0, current_next 1
        body.push(0x00); // section_number
        body.push(0x00); // last_section_number
        body.extend_from_slice(&0x0001u16.to_be_bytes()); // original_network_id
        body.push(0x00); // reserved_future_use

        // One service.
        body.extend_from_slice(&self.layout.program_number.to_be_bytes());
        body.push(0xfc); // reserved, EIT flags 0, running, free_CA 0
        let provider = b"Tessera";
        let service = b"Tessera";
        let mut descriptors = Vec::with_capacity(32);
        descriptors.push(0x48); // service_descriptor
        descriptors.push(0x00); // descriptor length, filled below
        descriptors.push(0x01); // digital television service
        descriptors.push(provider.len() as u8);
        descriptors.extend_from_slice(provider);
        descriptors.push(service.len() as u8);
        descriptors.extend_from_slice(service);
        descriptors[1] = (descriptors.len() - 2) as u8;
        body.extend_from_slice(&(descriptors.len() as u16).to_be_bytes());
        body.extend_from_slice(&descriptors);

        let section_len = (body.len() + 4) as u16; // CRC32
        let mut section = Vec::with_capacity(TS_PAYLOAD_LEN);
        section.push(TABLE_ID_SDT);
        section.push(0xb0 | ((section_len >> 8) as u8 & 0x0f));
        section.push((section_len & 0xff) as u8);
        section.extend_from_slice(&body);
        let crc = crc32_mpeg2(&section);
        section.extend_from_slice(&crc.to_be_bytes());

        let mut payload = Vec::with_capacity(TS_PAYLOAD_LEN);
        payload.push(0x00); // pointer_field
        payload.extend_from_slice(&section);
        self.section_packet(PID_SDT, payload, 0)
    }

    /// Wrap one access unit in a PES packet and packetize it into transport
    /// packets. `pts_90k` is the presentation timestamp in 90 kHz units; it is
    /// also the PCR, because this stream has no reordering delay to express.
    pub fn video_access_unit(&mut self, access_unit: &[u8], pts_90k: u64) -> Vec<u8> {
        self.video_access_unit_with_pcr(access_unit, pts_90k, Some(pts_90k))
    }

    /// Like [`Self::video_access_unit`], but lets the caller choose when the
    /// transition stream carries a PCR. ffmpeg's mpegtsmux uses 100 ms.
    pub fn video_access_unit_with_pcr(
        &mut self,
        access_unit: &[u8],
        pts_90k: u64,
        pcr_90k: Option<u64>,
    ) -> Vec<u8> {
        let pes = self.pes_with_pts(PES_STREAM_ID_VIDEO, access_unit, pts_90k, false);
        let pid = self.layout.video_pid;
        self.access_unit_packets(pid, &pes, pcr_90k)
    }

    /// Wrap one AAC ADTS frame in a bounded PES packet and packetize it on the
    /// audio PID. Audio carries no PCR; the video PID owns the program clock.
    pub fn audio_access_unit(&mut self, adts_frame: &[u8], pts_90k: u64) -> Vec<u8> {
        let pes = self.pes_with_pts(PES_STREAM_ID_AUDIO, adts_frame, pts_90k, true);
        let pid = self.layout.audio_pid;
        self.access_unit_packets(pid, &pes, None)
    }

    fn access_unit_packets(&mut self, pid: u16, pes: &[u8], pcr_90k: Option<u64>) -> Vec<u8> {
        let mut out = Vec::with_capacity(pes.len().div_ceil(TS_PAYLOAD_LEN) * TS_PACKET_LEN);
        let mut offset = 0;
        let mut first = true;
        while offset < pes.len() {
            let remaining = pes.len() - offset;
            let base_capacity = if first {
                if pcr_90k.is_some() {
                    TS_PAYLOAD_LEN - 8
                } else {
                    TS_PAYLOAD_LEN
                }
            } else {
                TS_PAYLOAD_LEN
            };
            // A packet that does not fill the base capacity needs an adaptation
            // field (length byte + flags, plus the PCR on the first packet). That
            // field consumes payload space; reserve it before choosing the chunk.
            let tail = remaining < base_capacity;
            let capacity = if tail {
                base_capacity - 2
            } else {
                base_capacity
            };
            let take = remaining.min(capacity);
            let chunk = &pes[offset..offset + take];
            let pcr = first.then_some(pcr_90k).flatten();
            let packet = self.payload_packet(pid, chunk, first, pcr, tail);
            out.extend_from_slice(&packet);
            offset += take;
            first = false;
        }
        out
    }

    fn pes_with_pts(&self, stream_id: u8, payload: &[u8], pts_90k: u64, bounded: bool) -> Vec<u8> {
        let header_data_len = 5u8;
        // A PES length of zero is only legal for video; audio carries its exact
        // length so a demuxer can bound the frame.
        let pes_len: u16 = if bounded {
            u16::try_from(3 + usize::from(header_data_len) + payload.len())
                .expect("audio frame fits in a PES packet")
        } else {
            0
        };
        let mut pes = Vec::with_capacity(payload.len() + 14);
        pes.extend_from_slice(&[0x00, 0x00, 0x01, stream_id]);
        pes.extend_from_slice(&pes_len.to_be_bytes());
        pes.push(0x80); // marker bits, no scrambling
        pes.push(0x80); // PTS present
        pes.push(header_data_len);
        pes.extend_from_slice(&encode_pts(pts_90k, 0b0010));
        pes.extend_from_slice(payload);
        pes
    }

    /// A PSI packet: one 188-byte transport packet whose payload is a complete
    /// section followed by `0xff` stuffing.
    fn section_packet(&mut self, pid: u16, payload: Vec<u8>, _unused: u8) -> Vec<u8> {
        let mut packet = Vec::with_capacity(TS_PACKET_LEN);
        let cc = self.next_continuity(pid);
        packet.push(TS_SYNC);
        packet.push(0x40 | ((pid >> 8) as u8 & 0x1f)); // payload_unit_start_indicator
        packet.push((pid & 0xff) as u8);
        packet.push(0x10 | cc); // payload only
        debug_assert!(payload.len() <= TS_PAYLOAD_LEN);
        packet.extend_from_slice(&payload);
        packet.resize(TS_PACKET_LEN, 0xff);
        packet
    }

    /// One transport packet carrying `chunk` of an elementary stream, with an
    /// adaptation field when a PCR or stuffing is required.
    fn payload_packet(
        &mut self,
        pid: u16,
        chunk: &[u8],
        unit_start: bool,
        pcr_90k: Option<u64>,
        stuff: bool,
    ) -> Vec<u8> {
        let cc = self.next_continuity(pid);
        // Adaptation field content: flags byte, then optional PCR (6 bytes).
        let pcr_len = if pcr_90k.is_some() { 6 } else { 0 };
        let need_adaptation = pcr_90k.is_some() || stuff;
        let adaptation_content = if need_adaptation { 1 + pcr_len } else { 0 };
        let mut packet = Vec::with_capacity(TS_PACKET_LEN);
        packet.push(TS_SYNC);
        let pusi = u8::from(unit_start) << 6;
        packet.push(pusi | ((pid >> 8) as u8 & 0x1f));
        packet.push((pid & 0xff) as u8);
        if need_adaptation {
            packet.push(0x30 | cc); // adaptation field then payload
            // `adaptation_field_length` counts the bytes after itself; add
            // stuffing so the payload lands at the end of the packet.
            let stuffing = TS_PAYLOAD_LEN - 1 - adaptation_content - chunk.len();
            packet.push(u8::try_from(adaptation_content + stuffing).expect("fits in a packet"));
            let mut flags = 0u8;
            if pcr_90k.is_some() {
                flags |= 0x10; // PCR_flag
            }
            packet.push(flags);
            if let Some(pcr) = pcr_90k {
                packet.extend_from_slice(&encode_pcr(pcr));
            }
            packet.resize(packet.len() + stuffing, 0xff);
        } else {
            packet.push(0x10 | cc); // payload only
        }
        packet.extend_from_slice(chunk);
        debug_assert_eq!(
            packet.len(),
            TS_PACKET_LEN,
            "packet must be exactly 188 bytes"
        );
        packet
    }

    fn next_continuity(&mut self, pid: u16) -> u8 {
        let slot = &mut self.continuity[usize::from(pid & 0x1fff)];
        let value = *slot & 0x0f;
        *slot = value.wrapping_add(1) & 0x0f;
        value
    }
}

/// A 33-bit PTS encoded in the 5-byte PES form.
fn encode_pts(pts: u64, prefix: u8) -> [u8; 5] {
    let pts = pts & 0x1_ffff_ffff; // 33 bits
    [
        (prefix << 4) | (((pts >> 30) as u8 & 0x07) << 1) | 1,
        (pts >> 22) as u8,
        (((pts >> 15) as u8 & 0x7f) << 1) | 1,
        (pts >> 7) as u8,
        ((pts as u8 & 0x7f) << 1) | 1,
    ]
}

/// A 42-bit PCR (base 33 bits, extension 9 bits) in its 6-byte form. The
/// extension is left at zero: this stream has no system clock jitter to express.
fn encode_pcr(pcr_90k: u64) -> [u8; 6] {
    let base = pcr_90k & 0x1_ffff_ffff;
    [
        (base >> 25) as u8,
        (base >> 17) as u8,
        (base >> 9) as u8,
        (base >> 1) as u8,
        ((base as u8 & 0x01) << 7) | 0x7e, // 6 reserved bits set, extension high bit 0
        0x00,
    ]
}

/// CRC-32/MPEG-2: polynomial 0x04C11DB7, init 0xFFFFFFFF, no reflection, no
/// final XOR. Used by every PSI section.
pub fn crc32_mpeg2(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &byte in bytes {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet_pid(packet: &[u8]) -> u16 {
        (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2])
    }

    fn continuity(packet: &[u8]) -> u8 {
        packet[3] & 0x0f
    }

    /// Collect the payload bytes of the packets on `pid`, honouring the
    /// adaptation field, so a test can prove what a demuxer would read.
    fn payloads(stream: &[u8], pid: u16) -> Vec<u8> {
        let mut out = Vec::new();
        for packet in stream.chunks_exact(TS_PACKET_LEN) {
            if packet_pid(packet) != pid {
                continue;
            }
            let adaptation = packet[3] & 0x20 != 0;
            let mut offset = TS_HEADER_LEN;
            if adaptation {
                let length = packet[offset] as usize;
                offset += 1 + length;
            }
            out.extend_from_slice(&packet[offset..]);
        }
        out
    }

    #[test]
    fn every_packet_is_188_bytes_and_starts_with_the_sync_byte() {
        let mut muxer = TsMuxer::default();
        let mut stream = muxer.pat();
        stream.extend_from_slice(&muxer.pmt());
        stream.extend_from_slice(&muxer.video_access_unit(&[0u8; 500], 9000));
        assert_eq!(stream.len() % TS_PACKET_LEN, 0);
        for packet in stream.chunks_exact(TS_PACKET_LEN) {
            assert_eq!(packet[0], TS_SYNC);
        }
    }

    #[test]
    fn the_pat_points_at_the_pmt_and_its_crc_is_correct() {
        let mut muxer = TsMuxer::default();
        let packet = muxer.pat();
        assert_eq!(packet_pid(&packet), PID_PAT);
        let payload = payloads(&packet, PID_PAT);
        assert_eq!(payload[0], 0x00, "pointer_field");
        assert_eq!(payload[1], TABLE_ID_PAT);
        let section_len = (u16::from(payload[2] & 0x0f) << 8) | u16::from(payload[3]);
        let section = &payload[1..1 + 3 + usize::from(section_len)];
        let (body, crc) = section.split_at(section.len() - 4);
        assert_eq!(crc32_mpeg2(body).to_be_bytes(), crc, "PAT CRC must verify");
        // program_number then the PMT PID.
        let program = u16::from_be_bytes([body[8], body[9]]);
        assert_eq!(program, muxer.layout().program_number);
        let pmt_pid = (u16::from(body[10] & 0x1f) << 8) | u16::from(body[11]);
        assert_eq!(pmt_pid, muxer.layout().pmt_pid);
    }

    #[test]
    fn the_pmt_declares_one_h264_stream_on_the_video_pid() {
        let mut muxer = TsMuxer::default();
        let packet = muxer.pmt();
        assert_eq!(packet_pid(&packet), muxer.layout().pmt_pid);
        let payload = payloads(&packet, muxer.layout().pmt_pid);
        let section = &payload[1..];
        let section_len = (u16::from(section[1] & 0x0f) << 8) | u16::from(section[2]);
        let whole = &section[..3 + usize::from(section_len)];
        let (body, crc) = whole.split_at(whole.len() - 4);
        assert_eq!(crc32_mpeg2(body).to_be_bytes(), crc, "PMT CRC must verify");
        // Last five bytes of the body are the elementary stream entry.
        let es = &body[body.len() - 5..];
        assert_eq!(es[0], STREAM_TYPE_H264);
        let es_pid = (u16::from(es[1] & 0x1f) << 8) | u16::from(es[2]);
        assert_eq!(es_pid, muxer.layout().video_pid);
    }

    #[test]
    fn an_access_unit_survives_pes_and_transport_packetization() {
        let mut muxer = TsMuxer::default();
        let access_unit: Vec<u8> = (0..1000u32).map(|index| (index % 251) as u8).collect();
        let stream = muxer.video_access_unit(&access_unit, 45_000);
        let pid = muxer.layout().video_pid;

        // Reassemble the PES payload exactly as a demuxer would.
        let mut pes = Vec::new();
        for packet in stream.chunks_exact(TS_PACKET_LEN) {
            assert_eq!(packet_pid(packet), pid);
            let adaptation = packet[3] & 0x20 != 0;
            let mut offset = TS_HEADER_LEN;
            if adaptation {
                let length = packet[offset] as usize;
                offset += 1 + length;
            }
            pes.extend_from_slice(&packet[offset..]);
        }
        assert_eq!(&pes[..4], &[0x00, 0x00, 0x01, PES_STREAM_ID_VIDEO]);
        let header_data_len = pes[8] as usize;
        let payload_start = 9 + header_data_len;
        assert_eq!(&pes[payload_start..], &access_unit[..]);
    }

    #[test]
    fn the_pes_pts_encodes_and_reads_back() {
        for pts in [0u64, 1, 90_000, 0x1_ffff_ffff] {
            let encoded = encode_pts(pts, 0b0010);
            // Decode the 33-bit field back out of its 5-byte form.
            let decoded = (u64::from((encoded[0] >> 1) & 0x07) << 30)
                | (u64::from(encoded[1]) << 22)
                | (u64::from(encoded[2] >> 1) << 15)
                | (u64::from(encoded[3]) << 7)
                | u64::from(encoded[4] >> 1);
            assert_eq!(decoded, pts & 0x1_ffff_ffff, "PTS {pts}");
            // ISO/IEC 13818-1 puts a marker bit in the bytes that carry a
            // prefix nibble or a split field — octets 0, 2 and 4. The two whole
            // octets (1 and 3) carry eight unscathed bits, so they must NOT be
            // forced to 1.
            for octet in [0usize, 2, 4] {
                assert_eq!(encoded[octet] & 0x01, 1, "marker bit in PTS octet {octet}");
            }
        }
    }

    #[test]
    fn a_whole_octet_of_the_pts_is_not_destroyed_by_a_marker_bit() {
        // An all-ones 33-bit timestamp makes octets 1 and 3 all ones. Forcing a
        // marker bit into them would silently corrupt the timestamp, so this
        // pins that they carry eight unscathed bits.
        let pts = 0x1_ffff_ffff;
        let encoded = encode_pts(pts, 0b0010);
        assert_eq!(encoded[1], 0xff, "PTS octet 1 is a whole octet");
        assert_eq!(encoded[3], 0xff, "PTS octet 3 is a whole octet");
        let decoded = (u64::from((encoded[0] >> 1) & 0x07) << 30)
            | (u64::from(encoded[1]) << 22)
            | (u64::from(encoded[2] >> 1) << 15)
            | (u64::from(encoded[3]) << 7)
            | u64::from(encoded[4] >> 1);
        assert_eq!(decoded, pts);
    }

    #[test]
    fn the_first_packet_of_an_access_unit_carries_the_pcr() {
        let mut muxer = TsMuxer::default();
        let stream = muxer.video_access_unit(&[0u8; 300], 12_345);
        let first = &stream[..TS_PACKET_LEN];
        assert_ne!(
            first[1] & 0x40,
            0,
            "payload_unit_start_indicator on the first packet"
        );
        assert_ne!(first[3] & 0x20, 0, "adaptation field present for the PCR");
        let adaptation_length = first[TS_HEADER_LEN] as usize;
        assert!(adaptation_length >= 7, "flags + 6-byte PCR");
        assert_ne!(first[5] & 0x10, 0, "PCR_flag");
    }

    #[test]
    fn continuity_counters_advance_per_pid_and_wrap_at_sixteen() {
        let mut muxer = TsMuxer::default();
        let pid = muxer.layout().video_pid;
        let mut seen = Vec::new();
        for _ in 0..20 {
            let stream = muxer.video_access_unit(&[0u8; 100], 0);
            seen.push(continuity(&stream[..TS_PACKET_LEN]));
            assert!(seen.last().unwrap() < &16);
        }
        assert_eq!(seen[0], 0);
        assert_eq!(seen[15], 15);
        assert_eq!(seen[16], 0, "continuity wraps at 16");
        let _ = pid;
    }

    #[test]
    fn crc32_mpeg2_matches_a_known_vector() {
        // The 0xFFFFFFFF-initialised, non-reflected MPEG-2 CRC of "123456789".
        assert_eq!(crc32_mpeg2(b"123456789"), 0x0376_e6e7);
    }

    #[test]
    fn a_pmt_with_audio_declares_aac_on_the_audio_pid() {
        let mut muxer = TsMuxer::with_audio(TsLayout::default());
        let packet = muxer.pmt();
        let payload = payloads(&packet, muxer.layout().pmt_pid);
        let section = &payload[1..];
        let section_len = (u16::from(section[1] & 0x0f) << 8) | u16::from(section[2]);
        let whole = &section[..3 + usize::from(section_len)];
        let (body, crc) = whole.split_at(whole.len() - 4);
        assert_eq!(crc32_mpeg2(body).to_be_bytes(), crc, "PMT CRC must verify");
        let audio = &body[body.len() - 5..];
        assert_eq!(audio[0], STREAM_TYPE_AAC_ADTS);
        let audio_pid = (u16::from(audio[1] & 0x1f) << 8) | u16::from(audio[2]);
        assert_eq!(audio_pid, muxer.layout().audio_pid);
    }

    #[test]
    fn an_audio_access_unit_has_a_bounded_pes_with_a_pts() {
        let mut muxer = TsMuxer::with_audio(TsLayout::default());
        let frame = vec![0xaa; 300];
        let stream = muxer.audio_access_unit(&frame, 45_000);
        let pid = muxer.layout().audio_pid;
        for packet in stream.chunks_exact(TS_PACKET_LEN) {
            assert_eq!(packet_pid(packet), pid);
        }
        let pes = payloads(&stream, pid);
        assert_eq!(&pes[..4], &[0x00, 0x00, 0x01, PES_STREAM_ID_AUDIO]);
        let pes_len = u16::from_be_bytes([pes[4], pes[5]]);
        assert_eq!(usize::from(pes_len), pes.len() - 6);
        let payload_start = 9 + 5;
        assert_eq!(&pes[payload_start..], &frame[..]);
    }

    #[test]
    fn every_packet_of_an_audio_stream_is_188_bytes() {
        let mut muxer = TsMuxer::with_audio(TsLayout::default());
        let mut stream = muxer.pat();
        stream.extend_from_slice(&muxer.pmt());
        stream.extend_from_slice(&muxer.video_access_unit(&[0u8; 500], 0));
        stream.extend_from_slice(&muxer.audio_access_unit(&[0xaa; 500], 0));
        assert_eq!(stream.len() % TS_PACKET_LEN, 0);
        for packet in stream.chunks_exact(TS_PACKET_LEN) {
            assert_eq!(packet[0], TS_SYNC);
        }
    }
}
