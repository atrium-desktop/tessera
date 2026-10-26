//! Media streaming driver: MPEG-2 TS muxing, RTP packetization, and UDP transmission
//! with simultaneous RTSP session maintenance.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use super::pattern::FrameSource;
use super::udp::UdpMediaSender;
use crate::wfd::rtp::{H264_CLOCK_RATE, RtpPacketizer};
use crate::wfd::session::{Effect, WfdSourceSession};
use crate::wfd::transport::{RtspTransport, TransportError};
use crate::wfd::ts::TsMuxer;

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
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            max_frames: None,
            max_duration: None,
            ssrc: 0x1234_5678,
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
    pub rtp_packets_sent: u64,
    pub bytes_sent: u64,
    pub duration: Duration,
    pub termination: StreamTermination,
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
    config: StreamConfig,
) -> (StreamStats, RtspTransport<S>) {
    let mut muxer = TsMuxer::default();
    let mut rtp_packetizer = RtpPacketizer::new(config.ssrc);

    let fps = config.fps.max(1);
    let frame_interval = Duration::from_nanos(1_000_000_000 / fps as u64);
    let pts_step = (H264_CLOCK_RATE / fps) as u64;

    let start_time = Instant::now();
    let mut frames_sent = 0u64;
    let mut pts_90k = 0u64;

    let termination;

    'stream_loop: loop {
        let frame_start = Instant::now();
        let elapsed = frame_start.saturating_duration_since(start_time);

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

        // 1. Mux TS: send PAT and PMT every second or on the very first frame.
        let mut ts_data = Vec::with_capacity(access_unit.len() + 512);
        if frames_sent.is_multiple_of(fps as u64) {
            ts_data.extend_from_slice(&muxer.pat());
            ts_data.extend_from_slice(&muxer.pmt());
        }
        let video_ts = muxer.video_access_unit(access_unit, pts_90k);
        ts_data.extend_from_slice(&video_ts);

        // 2. Packetize TS into RTP datagrams (PT 33, 7 TS packets per datagram).
        let rtp_packets = rtp_packetizer.packetize_ts(&ts_data, pts_90k as u32);
        for rtp in rtp_packets {
            if let Err(err) = media_sender.send_rtp_packet(&rtp) {
                termination = StreamTermination::Error(format!("UDP send failed: {err}"));
                break 'stream_loop;
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

    let stats = StreamStats {
        frames_sent,
        rtp_packets_sent: media_sender.packets_sent(),
        bytes_sent: media_sender.bytes_sent(),
        duration: start_time.elapsed(),
        termination,
    };

    (stats, transport)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        };

        let (stats, _) = stream(
            transport,
            &mut session,
            sender,
            TestPatternSource::new(60),
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
}
