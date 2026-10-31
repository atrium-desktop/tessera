//! Media streaming driver: MPEG-2 TS muxing, RTP packetization, and UDP transmission
//! with simultaneous RTSP session maintenance.

use std::fs::File;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::audio::AudioFrameSource;
use super::pattern::FrameSource;
use super::rtcp::RtcpSender;
use super::udp::UdpMediaSender;
use crate::wfd::rtp::{DEFAULT_TS_RTP_PAYLOAD, H264_CLOCK_RATE, RtpPacketizer};
use crate::wfd::session::{Effect, WfdSourceSession};
use crate::wfd::transport::{RtspTransport, TransportError};
use crate::wfd::ts::{TsLayout, TsMuxer};

/// Start the media timeline with a lead, matching ffmpeg's mpegtsmux output.
/// Some Android audio decoders stutter for the first second when the first PTS
/// is zero; a 1.5 s lead gives them a buffer before presentation starts.
const MEDIA_START_PTS: u64 = 135_000;

/// Configuration for a streaming run.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Target frame rate in frames per second (default 30).
    pub fps: u32,
    /// Stop streaming after this many frames, if set.
    pub max_frames: Option<u64>,
    /// Stop streaming after this duration, if set.
    pub max_duration: Option<Duration>,
    /// RTP Synchronization Source identifier.
    pub ssrc: u32,
    /// Set by the session owner to stop the stream at the next frame boundary.
    /// A cast service needs this for an operator-initiated stop that is not a
    /// process kill.
    pub stop: Option<Arc<AtomicBool>>,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            max_frames: None,
            max_duration: None,
            ssrc: 0x1234_5678,
            stop: None,
        }
    }
}

/// Why a media stream stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamTermination {
    /// Reached configured maximum duration.
    DurationLimit,
    /// Reached configured maximum frame count.
    FrameLimit,
    /// The pre-muxed TS file ended.
    SourceExhausted,
    /// The sink sent TEARDOWN.
    SinkTeardown,
    /// The peer closed the RTSP control connection.
    PeerClosed,
    /// Operator cancelled / interrupted the stream.
    UserInterrupt,
    /// An I/O or transport error occurred.
    Error(String),
}

impl std::fmt::Display for StreamTermination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DurationLimit => write!(f, "reached duration limit"),
            Self::FrameLimit => write!(f, "reached frame limit"),
            Self::SourceExhausted => write!(f, "media file ended"),
            Self::SinkTeardown => write!(f, "sink requested teardown"),
            Self::PeerClosed => write!(f, "peer closed RTSP connection"),
            Self::UserInterrupt => write!(f, "interrupted by operator"),
            Self::Error(reason) => write!(f, "streaming error: {reason}"),
        }
    }
}

/// Statistics collected during a media streaming session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamStats {
    pub frames_sent: u64,
    pub audio_frames_sent: u64,
    pub rtp_packets_sent: u64,
    pub bytes_sent: u64,
    pub duration: Duration,
    pub termination: StreamTermination,
    /// A terminal audio capture/encode error, if audio stopped mid-session.
    pub audio_failure: Option<String>,
    /// An RTCP sender-report failure, if one occurred.
    pub rtcp_failure: Option<String>,
}

impl StreamStats {
    /// Calculate average video bitrate in bits per second.
    pub fn average_bitrate_bps(&self) -> u64 {
        let secs = self.duration.as_secs_f64();
        if secs > 0.001 {
            ((self.bytes_sent * 8) as f64 / secs) as u64
        } else {
            0
        }
    }
}

/// Drive media streaming while servicing the RTSP control session.
pub fn stream<S: Read + Write, F: FrameSource>(
    mut transport: RtspTransport<S>,
    session: &mut WfdSourceSession,
    mut media_sender: UdpMediaSender,
    mut frame_source: F,
    mut audio: Option<Box<dyn AudioFrameSource>>,
    rtcp: Option<RtcpSender>,
    config: StreamConfig,
) -> (StreamStats, RtspTransport<S>) {
    let mut muxer = if audio.is_some() {
        TsMuxer::with_audio(TsLayout::default())
    } else {
        TsMuxer::default()
    };
    let mut rtp_packetizer = RtpPacketizer::new(config.ssrc);

    let fps = config.fps.max(1);
    let pat_interval = (fps / 10).max(1) as u64;
    let frame_interval = Duration::from_nanos(1_000_000_000 / fps as u64);
    let pts_step = (H264_CLOCK_RATE / fps) as u64;

    let start_time = Instant::now();
    let mut frames_sent = 0u64;
    let mut audio_frames_sent = 0u64;
    let mut audio_failure = None;
    let mut rtcp_failure = None;
    let mut audio_group = Vec::new();
    let mut audio_group_pts = 0u64;
    let mut audio_group_frames = 0usize;
    let mut next_sr = Instant::now();
    let mut pts_90k = MEDIA_START_PTS;
    let mut ts_dump =
        std::env::var_os("TESSERA_CAST_DUMP_TS").and_then(|path| std::fs::File::create(path).ok());

    let termination;

    'stream_loop: loop {
        let frame_start = Instant::now();
        let elapsed = frame_start.saturating_duration_since(start_time);

        if config
            .stop
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::Relaxed))
        {
            termination = StreamTermination::UserInterrupt;
            break 'stream_loop;
        }

        if let Some(max_duration) = config.max_duration
            && elapsed >= max_duration
        {
            termination = StreamTermination::DurationLimit;
            break 'stream_loop;
        }

        if let Some(max_frames) = config.max_frames
            && frames_sent >= max_frames
        {
            termination = StreamTermination::FrameLimit;
            break 'stream_loop;
        }

        let Some(access_unit) = frame_source.next_access_unit() else {
            termination = StreamTermination::FrameLimit;
            break 'stream_loop;
        };

        // 1. Mux TS: PAT/PMT every 100 ms (ten per second) and on the first frame.
        let mut ts_data = Vec::with_capacity(access_unit.len() + 512);
        if frames_sent.is_multiple_of(pat_interval) {
            ts_data.extend_from_slice(&muxer.pat());
            ts_data.extend_from_slice(&muxer.pmt());
        }
        let video_pcr = frames_sent.is_multiple_of(pat_interval).then_some(pts_90k);
        let video_ts = muxer.video_access_unit_with_pcr(access_unit, pts_90k, video_pcr);
        ts_data.extend_from_slice(&video_ts);

        // 1b. Interleave audio in groups of eight AAC frames, as ffmpeg's
        // mpegtsmux does. One tiny PES per 21 ms frame starves some Android
        // audio decoders; a grouped PES keeps the track continuously fed.
        if let Some(source) = audio.as_mut() {
            while let Some(frame) = source.try_next() {
                if audio_group_frames == 0 {
                    audio_group_pts = frame.pts_90k + MEDIA_START_PTS;
                }
                audio_group.extend_from_slice(&frame.data);
                audio_group_frames += 1;
                audio_frames_sent += 1;
                if audio_group_frames == 8 {
                    ts_data
                        .extend_from_slice(&muxer.audio_access_unit(&audio_group, audio_group_pts));
                    audio_group.clear();
                    audio_group_frames = 0;
                }
            }
            // Prime the sink on the first frame instead of holding a partial
            // pre-roll group until eight frames have accumulated.
            if frames_sent == 0 && audio_group_frames > 0 {
                ts_data.extend_from_slice(&muxer.audio_access_unit(&audio_group, audio_group_pts));
                audio_group.clear();
                audio_group_frames = 0;
            }
            if let Some(reason) = source.failure() {
                audio_failure = Some(reason);
                audio = None;
            }
        }

        if let Some(sender) = rtcp.as_ref()
            && Instant::now() >= next_sr
        {
            let packets = media_sender.packets_sent().min(u64::from(u32::MAX)) as u32;
            let octets = media_sender.bytes_sent().min(u64::from(u32::MAX)) as u32;
            if let Err(error) = sender.send_sr(pts_90k as u32, packets, octets) {
                rtcp_failure = Some(format!("RTCP sender report failed: {error}"));
                next_sr = Instant::now() + Duration::from_secs(1);
            } else {
                next_sr = Instant::now() + Duration::from_secs(1);
            }
        }
        if let Some(sender) = rtcp.as_ref() {
            while let Some(report) = sender.poll() {
                eprintln!(
                    "rtcp rr: ssrc={:#010x} fraction_lost={} cumulative_lost={} highest_seq={} jitter={}",
                    report.source_ssrc,
                    report.fraction_lost,
                    report.cumulative_lost,
                    report.highest_sequence,
                    report.jitter
                );
            }
        }

        // 2. Packetize TS into RTP datagrams (PT 33, 7 TS packets per datagram).
        if let Some(file) = ts_dump.as_mut() {
            let _ = file.write_all(&ts_data);
        }
        let rtp_packets = rtp_packetizer.packetize_ts(&ts_data, pts_90k as u32);
        let packet_count = rtp_packets.len();
        let packet_interval =
            Duration::from_nanos(frame_interval.as_nanos() as u64 / packet_count.max(1) as u64);
        for (index, rtp) in rtp_packets.iter().enumerate() {
            if let Err(err) = media_sender.send_rtp_packet(rtp) {
                termination = StreamTermination::Error(format!("UDP send failed: {err}"));
                break 'stream_loop;
            }
            if index + 1 < packet_count {
                std::thread::sleep(packet_interval);
            }
        }
        frames_sent += 1;
        pts_90k += pts_step;

        // 3. Service any pending RTSP control messages without blocking.
        loop {
            match transport.read_message() {
                Ok(Some(inbound)) => {
                    let (outbound, effects) = session.on_message(&inbound);
                    for out_msg in outbound {
                        if let Err(err) = transport.send(&out_msg) {
                            termination =
                                StreamTermination::Error(format!("RTSP send failed: {err}"));
                            break 'stream_loop;
                        }
                    }
                    for effect in effects {
                        if effect == Effect::Terminate {
                            termination = StreamTermination::SinkTeardown;
                            break 'stream_loop;
                        }
                    }
                }
                Ok(None) => {
                    termination = StreamTermination::PeerClosed;
                    break 'stream_loop;
                }
                Err(TransportError::Timeout) => {
                    // Normal: no RTSP message pending on socket.
                    break;
                }
                Err(TransportError::Io(err)) => {
                    termination = StreamTermination::Error(format!("RTSP read error: {err}"));
                    break 'stream_loop;
                }
                Err(other) => {
                    termination =
                        StreamTermination::Error(format!("RTSP transport error: {other}"));
                    break 'stream_loop;
                }
            }
        }

        // 4. Pacing: maintain frame rate.
        let frame_duration = frame_start.elapsed();
        if frame_duration < frame_interval {
            std::thread::sleep(frame_interval - frame_duration);
        }
    }

    // Flush the last partial audio group so a short stream still carries it.
    if audio_group_frames > 0 {
        let ts_data = muxer.audio_access_unit(&audio_group, audio_group_pts);
        for rtp in rtp_packetizer.packetize_ts(&ts_data, pts_90k as u32) {
            let _ = media_sender.send_rtp_packet(&rtp);
        }
    }

    let stats = StreamStats {
        frames_sent,
        audio_frames_sent,
        rtp_packets_sent: media_sender.packets_sent(),
        bytes_sent: media_sender.bytes_sent(),
        duration: start_time.elapsed(),
        termination,
        audio_failure,
        rtcp_failure,
    };

    (stats, transport)
}

/// Send a pre-muxed MPEG-TS file over the WFD RTP session.
///
/// This is a diagnostic path: it keeps the RTSP/P2P/SETUP/PLAY and RTP/RTCP
/// layers identical to [`stream`] but replaces the hand-written TS muxer with a
/// known-good file. If a sink accepts this file's audio and not the muxer's, the
/// defect is in the muxer, not in the handshake or transport.
pub fn stream_ts_file<S: Read + Write>(
    mut transport: RtspTransport<S>,
    session: &mut WfdSourceSession,
    mut media_sender: UdpMediaSender,
    mut file: File,
    rtcp: Option<RtcpSender>,
    config: StreamConfig,
    duration: Duration,
) -> (StreamStats, RtspTransport<S>) {
    let file_len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut rtp_packetizer = RtpPacketizer::new(config.ssrc);
    let start_time = Instant::now();
    let mut sent_bytes = 0u64;
    let mut next_sr = Instant::now();
    let mut rtcp_failure = None;
    let bytes_per_second = file_len as f64 / duration.as_secs_f64().max(0.1);
    let termination;

    'file_loop: loop {
        if config
            .stop
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::Relaxed))
        {
            termination = StreamTermination::UserInterrupt;
            break;
        }
        if config
            .max_duration
            .is_some_and(|limit| start_time.elapsed() >= limit)
        {
            termination = StreamTermination::DurationLimit;
            break;
        }

        let target = Duration::from_secs_f64(sent_bytes as f64 / bytes_per_second);
        let elapsed = start_time.elapsed();
        if target > elapsed {
            std::thread::sleep(target - elapsed);
        }

        let mut chunk = vec![0u8; DEFAULT_TS_RTP_PAYLOAD];
        let read = match file.read(&mut chunk) {
            Ok(0) => {
                termination = StreamTermination::SourceExhausted;
                break;
            }
            Ok(read) => read,
            Err(error) => {
                termination = StreamTermination::Error(format!("TS file read failed: {error}"));
                break;
            }
        };
        chunk.truncate(read);
        sent_bytes += read as u64;
        let timestamp = (start_time.elapsed().as_secs_f64() * 90_000.0) as u32;
        for rtp in rtp_packetizer.packetize_ts(&chunk, timestamp) {
            if let Err(error) = media_sender.send_rtp_packet(&rtp) {
                termination = StreamTermination::Error(format!("UDP send failed: {error}"));
                break 'file_loop;
            }
        }

        if let Some(sender) = rtcp.as_ref()
            && Instant::now() >= next_sr
        {
            let packets = media_sender.packets_sent().min(u64::from(u32::MAX)) as u32;
            let octets = media_sender.bytes_sent().min(u64::from(u32::MAX)) as u32;
            if let Err(error) = sender.send_sr(timestamp, packets, octets) {
                rtcp_failure = Some(format!("RTCP sender report failed: {error}"));
            }
            next_sr = Instant::now() + Duration::from_secs(1);
        }

        loop {
            match transport.read_message() {
                Ok(Some(inbound)) => {
                    let (outbound, effects) = session.on_message(&inbound);
                    for message in outbound {
                        if let Err(error) = transport.send(&message) {
                            termination =
                                StreamTermination::Error(format!("RTSP send failed: {error}"));
                            break 'file_loop;
                        }
                    }
                    if effects.contains(&Effect::Terminate) {
                        termination = StreamTermination::SinkTeardown;
                        break 'file_loop;
                    }
                }
                Ok(None) => {
                    termination = StreamTermination::PeerClosed;
                    break 'file_loop;
                }
                Err(TransportError::Timeout) => break,
                Err(error) => {
                    termination = StreamTermination::Error(format!("RTSP read failed: {error}"));
                    break 'file_loop;
                }
            }
        }
    }

    let stats = StreamStats {
        frames_sent: 0,
        audio_frames_sent: 0,
        rtp_packets_sent: media_sender.packets_sent(),
        bytes_sent: media_sender.bytes_sent(),
        duration: start_time.elapsed(),
        termination,
        audio_failure: None,
        rtcp_failure,
    };
    (stats, transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::audio::AudioFrame;
    use crate::media::pattern::TestPatternSource;
    use crate::wfd::parameters::SourceParameters;
    use crate::wfd::rtp::{MP2T_PAYLOAD_TYPE, RtpPacket, TS_PACKET_LEN};
    use std::net::UdpSocket;
    use std::sync::mpsc;

    struct TestDuplex {
        read_rx: mpsc::Receiver<u8>,
        write_tx: mpsc::Sender<u8>,
    }

    impl Read for TestDuplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.read_rx.try_recv() {
                Ok(byte) => {
                    buf[0] = byte;
                    let mut count = 1;
                    while count < buf.len() {
                        match self.read_rx.try_recv() {
                            Ok(b) => {
                                buf[count] = b;
                                count += 1;
                            }
                            Err(_) => break,
                        }
                    }
                    Ok(count)
                }
                Err(mpsc::TryRecvError::Empty) => Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "would block",
                )),
                Err(mpsc::TryRecvError::Disconnected) => Ok(0),
            }
        }
    }

    impl Write for TestDuplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            for b in buf {
                self.write_tx.send(*b).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, e.to_string())
                })?;
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn streamer_transmits_frames_and_terminates_on_limit() {
        let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
        let receiver_addr = receiver.local_addr().expect("receiver addr");

        let sender = UdpMediaSender::bind_ephemeral(receiver_addr).expect("bind sender");

        let (_sink_tx, caster_rx) = mpsc::channel();
        let (caster_tx, _sink_rx) = mpsc::channel();

        let duplex = TestDuplex {
            read_rx: caster_rx,
            write_tx: caster_tx,
        };
        let transport = RtspTransport::new(duplex);

        let params = SourceParameters::new("00 00 02 10");
        let mut session = WfdSourceSession::new(params);

        let config = StreamConfig {
            fps: 60,
            max_frames: Some(3),
            max_duration: None,
            ssrc: 0x9999,
            stop: None,
        };

        let (stats, _) = stream(
            transport,
            &mut session,
            sender,
            TestPatternSource::new(60),
            None,
            None,
            config,
        );

        assert_eq!(stats.frames_sent, 3);
        assert_eq!(stats.termination, StreamTermination::FrameLimit);
        assert!(stats.rtp_packets_sent > 0);
        assert!(stats.bytes_sent > 0);

        // Receive one datagram from receiver socket to verify RTP and TS sync byte
        let mut buf = [0u8; 1500];
        let (len, _) = receiver.recv_from(&mut buf).expect("recv udp packet");
        assert!(len > 12);
        let parsed = RtpPacket::parse(&buf[..len]).expect("parse rtp");
        assert_eq!(parsed.payload_type, MP2T_PAYLOAD_TYPE);
        assert_eq!(parsed.ssrc, 0x9999);
        // First byte of MPEG-2 TS packet must be sync byte 0x47
        assert_eq!(parsed.payload[0], 0x47);
        assert_eq!(parsed.payload.len() % TS_PACKET_LEN, 0);
    }

    struct TestAudio {
        frame: Option<AudioFrame>,
    }

    impl AudioFrameSource for TestAudio {
        fn try_next(&mut self) -> Option<AudioFrame> {
            self.frame.take()
        }
    }

    #[test]
    fn streamer_interleaves_audio_into_the_same_ts_and_rtp_flow() {
        let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
        let receiver_addr = receiver.local_addr().expect("receiver addr");
        let sender = UdpMediaSender::bind_ephemeral(receiver_addr).expect("bind sender");

        let (_sink_tx, caster_rx) = mpsc::channel();
        let (caster_tx, _sink_rx) = mpsc::channel();
        let transport = RtspTransport::new(TestDuplex {
            read_rx: caster_rx,
            write_tx: caster_tx,
        });

        let params = SourceParameters::new("00 00 02 10");
        let mut session = WfdSourceSession::new(params);
        let audio = TestAudio {
            frame: Some(AudioFrame {
                data: vec![0xaa; 200],
                pts_90k: 0,
            }),
        };
        let config = StreamConfig {
            fps: 60,
            max_frames: Some(3),
            max_duration: None,
            ssrc: 0x9999,
            stop: None,
        };

        let (stats, _) = stream(
            transport,
            &mut session,
            sender,
            TestPatternSource::new(60),
            Some(Box::new(audio)),
            None,
            config,
        );
        assert_eq!(stats.audio_frames_sent, 1);
        assert_eq!(stats.frames_sent, 3);

        receiver
            .set_read_timeout(Some(Duration::from_millis(100)))
            .expect("receiver timeout");
        let audio_pid = TsLayout::default().audio_pid;
        let mut buf = [0u8; 1500];
        let mut saw_audio = false;
        while let Ok((len, _)) = receiver.recv_from(&mut buf) {
            let packet = RtpPacket::parse(&buf[..len]).expect("parse rtp");
            assert_eq!(packet.payload_type, MP2T_PAYLOAD_TYPE);
            for ts in packet.payload.chunks_exact(TS_PACKET_LEN) {
                let pid = (u16::from(ts[1] & 0x1f) << 8) | u16::from(ts[2]);
                if pid == audio_pid {
                    saw_audio = true;
                }
            }
            if saw_audio {
                break;
            }
        }
        assert!(
            saw_audio,
            "the audio PID must travel in the video TS/RTP flow"
        );
    }
}
