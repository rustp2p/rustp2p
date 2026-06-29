use bytes::Bytes;
use quinn::udp::{RecvMeta, Transmit};
use quinn::AsyncUdpSocket;
use std::collections::VecDeque;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Packet types identified by the first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Stun,
    Quic,
    Punch,
    Other(u8),
}

/// Identify packet type from the first byte.
pub fn classify_packet(first_byte: u8) -> PacketType {
    match first_byte {
        0x01 => PacketType::Stun,
        0x02..=0x03 => PacketType::Punch,
        b if b & 0x80 != 0 => PacketType::Quic,
        b => PacketType::Other(b),
    }
}

/// A received non-QUIC packet.
#[derive(Debug)]
pub struct ReceivedPacket {
    pub data: Bytes,
    pub addr: SocketAddr,
    pub packet_type: PacketType,
}

/// Shared UDP socket with packet demuxing for QUIC + other protocols.
pub struct SharedUdpSocket {
    socket: Arc<UdpSocket>,
    non_quic_tx: mpsc::Sender<ReceivedPacket>,
    buffered: std::sync::Mutex<VecDeque<ReceivedPacket>>,
    non_quic_waker: std::sync::Mutex<Option<Waker>>,
}

impl SharedUdpSocket {
    pub fn new(socket: Arc<UdpSocket>, non_quic_tx: mpsc::Sender<ReceivedPacket>) -> Arc<Self> {
        Arc::new(Self {
            socket,
            non_quic_tx,
            buffered: std::sync::Mutex::new(VecDeque::new()),
            non_quic_waker: std::sync::Mutex::new(None),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub async fn send_raw(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        self.socket.send_to(buf, addr).await.map(|_| ())
    }

    pub fn try_send_raw(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        self.socket.try_send_to(buf, addr).map(|_| ())
    }

    fn forward_non_quic(&self, packet: ReceivedPacket) {
        if self.non_quic_tx.try_send(packet).is_err() {
            log::warn!("non-quic packet channel full, dropping");
        }
        if let Some(waker) = self.non_quic_waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

impl AsyncUdpSocket for SharedUdpSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn quinn::UdpPoller>> {
        Box::pin(SharedUdpPoller {
            socket: self.socket.clone(),
        })
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        self.socket
            .try_send_to(transmit.contents, transmit.destination)
            .map(|_| ())
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        // Forward any previously buffered non-QUIC packets
        {
            let mut buffered = self.buffered.lock().unwrap();
            while let Some(packet) = buffered.pop_front() {
                self.forward_non_quic(packet);
            }
        }

        // Read a single datagram from the socket
        let mut buf = vec![0u8; 65536];
        let mut read_buf = tokio::io::ReadBuf::new(&mut buf);

        match self.socket.poll_recv_from(cx, &mut read_buf) {
            Poll::Ready(Ok(_addr)) => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        }

        let n = read_buf.filled().len();
        if n == 0 {
            return Poll::Ready(Ok(0));
        }

        let first_byte = read_buf.filled()[0];
        let packet_type = classify_packet(first_byte);

        // Get peer address from the socket
        let addr = self
            .socket
            .peer_addr()
            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());

        match packet_type {
            PacketType::Quic => {
                // QUIC packet - copy into the provided buffers
                let copy_len = n.min(bufs[0].len());
                bufs[0][..copy_len].copy_from_slice(&read_buf.filled()[..copy_len]);
                meta[0] = RecvMeta {
                    addr,
                    len: copy_len,
                    stride: copy_len,
                    ecn: None,
                    dst_ip: None,
                };
                Poll::Ready(Ok(1))
            }
            _ => {
                // Non-QUIC packet - buffer it and return 0 to quinn
                let data = Bytes::copy_from_slice(read_buf.filled());
                let packet = ReceivedPacket {
                    data,
                    addr,
                    packet_type,
                };
                self.buffered.lock().unwrap().push_back(packet);
                // Wake consumer
                if let Some(waker) = self.non_quic_waker.lock().unwrap().take() {
                    waker.wake();
                }
                // Return 0 to indicate no QUIC data available
                Poll::Ready(Ok(0))
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

#[derive(Debug)]
struct SharedUdpPoller {
    socket: Arc<UdpSocket>,
}

impl quinn::UdpPoller for SharedUdpPoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.socket.poll_send_ready(cx)
    }
}

impl std::fmt::Debug for SharedUdpSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedUdpSocket")
            .field("local_addr", &self.local_addr())
            .finish()
    }
}
