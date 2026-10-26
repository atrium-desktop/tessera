//! UDP RTP media transmission.
//!
//! A WFD source transmits RTP packets carrying MPEG-2 Transport Stream packets over
//! UDP to the sink's advertised RTP port.

use std::io;
use std::net::{SocketAddr, UdpSocket};

/// A UDP RTP packet transmitter.
#[derive(Debug)]
pub struct UdpMediaSender {
    socket: UdpSocket,
    target: SocketAddr,
    packets_sent: u64,
    bytes_sent: u64,
}

impl UdpMediaSender {
    /// Bind an ephemeral UDP socket on any available interface (`0.0.0.0:0` or `[::]:0`)
    /// targeting `target`.
    pub fn bind_ephemeral(target: SocketAddr) -> io::Result<Self> {
        let bind_addr = if target.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(bind_addr)?;
        Ok(Self {
            socket,
            target,
            packets_sent: 0,
            bytes_sent: 0,
        })
    }

    /// Construct a sender from an already-bound socket and target.
    pub fn new(socket: UdpSocket, target: SocketAddr) -> Self {
        Self {
            socket,
            target,
            packets_sent: 0,
            bytes_sent: 0,
        }
    }

    /// Local socket address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Target destination address.
    pub fn target_addr(&self) -> SocketAddr {
        self.target
    }

    /// Total RTP packets sent through this transmitter.
    pub fn packets_sent(&self) -> u64 {
        self.packets_sent
    }

    /// Total bytes sent through this transmitter.
    pub fn bytes_sent(&self) -> u64 {
        self.bytes_sent
    }

    /// Transmit one RTP datagram.
    pub fn send_rtp_packet(&mut self, packet: &[u8]) -> io::Result<()> {
        let sent = self.socket.send_to(packet, self.target)?;
        self.packets_sent += 1;
        self.bytes_sent += sent as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_sender_transmits_datagrams_and_tracks_stats() {
        let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
        let receiver_addr = receiver.local_addr().expect("receiver addr");

        let mut sender =
            UdpMediaSender::bind_ephemeral(receiver_addr).expect("bind ephemeral sender");
        assert_eq!(sender.packets_sent(), 0);
        assert_eq!(sender.bytes_sent(), 0);

        let payload = b"test rtp payload";
        sender.send_rtp_packet(payload).expect("send packet");

        assert_eq!(sender.packets_sent(), 1);
        assert_eq!(sender.bytes_sent(), payload.len() as u64);

        let mut buf = [0u8; 64];
        let (len, src) = receiver.recv_from(&mut buf).expect("recv");
        assert_eq!(&buf[..len], payload);
        assert_eq!(
            src.port(),
            sender.local_addr().expect("sender local addr").port()
        );
    }
}
