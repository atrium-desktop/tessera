//! RTCP sender reports for the WFD media stream.
//!
//! The video path can render without RTCP, but the sink uses the SR's NTP/RTP
//! timestamp pair to build a shared clock for audio/video sync. A source that
//! never sends SR leaves some Android sinks with video and no audio. This module
//! sends the minimal compound packet: SR + SDES(CNAME).

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{SystemTime, UNIX_EPOCH};

/// The NTP epoch offset (1900-01-01 to 1970-01-01), in seconds.
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

/// A UDP RTCP transmitter bound to the source's advertised RTCP port.
#[derive(Debug)]
pub struct RtcpSender {
    socket: UdpSocket,
    target: SocketAddr,
    ssrc: u32,
    cname: String,
}

impl RtcpSender {
    /// Bind the source RTCP port and send to the sink's RTCP port.
    pub fn bind(local: SocketAddr, target: SocketAddr, ssrc: u32) -> io::Result<Self> {
        let socket = UdpSocket::bind(local)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            target,
            ssrc,
            cname: format!("tessera-cast@{}", local.ip()),
        })
    }

    /// Send one compound SR + SDES packet.
    pub fn send_sr(&self, rtp_timestamp: u32, packets: u32, octets: u32) -> io::Result<()> {
        let packet = sender_report(self.ssrc, rtp_timestamp, packets, octets, &self.cname);
        self.socket.send_to(&packet, self.target)?;
        Ok(())
    }

    /// Read one receiver report from the sink, if one is queued.
    pub fn poll(&self) -> Option<RtcpReport> {
        let mut buffer = [0u8; 1500];
        let (read, _) = self.socket.recv_from(&mut buffer).ok()?;
        parse_report(&buffer[..read])
    }
}

/// The sink's view of the source stream from an RTCP receiver report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcpReport {
    pub source_ssrc: u32,
    pub fraction_lost: u8,
    pub cumulative_lost: u32,
    pub highest_sequence: u32,
    pub jitter: u32,
}

fn parse_report(packet: &[u8]) -> Option<RtcpReport> {
    if packet.len() < 8 || packet[0] >> 6 != 2 {
        return None;
    }
    let report_count = usize::from(packet[0] & 0x1f);
    if report_count == 0 {
        return None;
    }
    let offset = match packet[1] {
        201 => 8,  // RR: header + sender SSRC
        200 => 28, // SR: header + sender info
        _ => return None,
    };
    let block = packet.get(offset..offset + 24)?;
    Some(RtcpReport {
        source_ssrc: u32::from_be_bytes([block[0], block[1], block[2], block[3]]),
        fraction_lost: block[4],
        cumulative_lost: u32::from_be_bytes([0, block[5], block[6], block[7]]),
        highest_sequence: u32::from_be_bytes([block[8], block[9], block[10], block[11]]),
        jitter: u32::from_be_bytes([block[16], block[17], block[18], block[19]]),
    })
}

/// Build a compound RTCP SR + SDES(CNAME) packet.
pub fn sender_report(
    ssrc: u32,
    rtp_timestamp: u32,
    packets: u32,
    octets: u32,
    cname: &str,
) -> Vec<u8> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let ntp_seconds = (now.as_secs() + NTP_UNIX_OFFSET) as u32;
    let ntp_fraction = ((u64::from(now.subsec_nanos()) << 32) / 1_000_000_000) as u32;

    let mut out = Vec::with_capacity(64);
    // Sender Report: V=2, P=0, RC=0, PT=200, length=6 (28 bytes).
    out.push(0x80);
    out.push(200);
    out.extend_from_slice(&6u16.to_be_bytes());
    out.extend_from_slice(&ssrc.to_be_bytes());
    out.extend_from_slice(&ntp_seconds.to_be_bytes());
    out.extend_from_slice(&ntp_fraction.to_be_bytes());
    out.extend_from_slice(&rtp_timestamp.to_be_bytes());
    out.extend_from_slice(&packets.to_be_bytes());
    out.extend_from_slice(&octets.to_be_bytes());

    // SDES: V=2, SC=1, PT=202, then one CNAME chunk padded to 32 bits.
    let cname = &cname.as_bytes()[..cname.len().min(255)];
    let chunk_len = 4 + 2 + cname.len() + 1; // SSRC + item header + text + NUL
    let padded = (chunk_len + 3) & !3;
    let sdes_len = ((4 + padded) / 4 - 1) as u16;
    out.push(0x81);
    out.push(202);
    out.extend_from_slice(&sdes_len.to_be_bytes());
    out.extend_from_slice(&ssrc.to_be_bytes());
    out.push(1); // CNAME
    out.push(cname.len() as u8);
    out.extend_from_slice(cname);
    out.push(0);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sender_report_has_the_rtcp_shape_and_the_sender_counts() {
        let packet = sender_report(0x1234_5678, 90_000, 42, 12_345, "source@test");
        assert_eq!(packet[0], 0x80, "V=2, RC=0");
        assert_eq!(packet[1], 200, "PT=SR");
        assert_eq!(u16::from_be_bytes([packet[2], packet[3]]), 6);
        assert_eq!(&packet[4..8], &0x1234_5678u32.to_be_bytes());
        assert_eq!(&packet[16..20], &90_000u32.to_be_bytes(), "RTP timestamp");
        assert_eq!(&packet[20..24], &42u32.to_be_bytes(), "packet count");
        assert_eq!(&packet[24..28], &12_345u32.to_be_bytes(), "octet count");
        assert_eq!(packet[28], 0x81, "SDES V=2, SC=1");
        assert_eq!(packet[29], 202, "PT=SDES");
        assert_eq!(packet.len() % 4, 0, "RTCP packets are 32-bit aligned");
    }

    #[test]
    fn the_cname_length_and_padding_are_consistent() {
        let packet = sender_report(1, 0, 0, 0, "a");
        let sdes_start = 28;
        let length = u16::from_be_bytes([packet[sdes_start + 2], packet[sdes_start + 3]]);
        assert_eq!(
            (usize::from(length) + 1) * 4,
            packet.len() - sdes_start,
            "SDES length covers the padded chunk"
        );
    }

    #[test]
    fn a_receiver_report_block_is_parsed() {
        let mut packet = vec![0u8; 32];
        packet[0] = 0x81; // V=2, RC=1
        packet[1] = 201; // RR
        packet[2..4].copy_from_slice(&7u16.to_be_bytes());
        packet[4..8].copy_from_slice(&0x1122_3344u32.to_be_bytes()); // sink SSRC
        packet[8..12].copy_from_slice(&0x1900_0000u32.to_be_bytes()); // source SSRC
        packet[12] = 5; // fraction lost
        packet[13..16].copy_from_slice(&[0, 0, 10]); // cumulative lost
        packet[16..20].copy_from_slice(&100u32.to_be_bytes()); // highest seq
        packet[24..28].copy_from_slice(&42u32.to_be_bytes()); // jitter
        let report = parse_report(&packet).expect("RR");
        assert_eq!(report.source_ssrc, 0x1900_0000);
        assert_eq!(report.fraction_lost, 5);
        assert_eq!(report.cumulative_lost, 10);
        assert_eq!(report.highest_sequence, 100);
        assert_eq!(report.jitter, 42);
    }
}
