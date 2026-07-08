use crate::protocol::{Packet, ProtocolType};
use crate::PeerId;
use bytes::Bytes;
use dashmap::DashMap;
use quinn::udp::{RecvMeta, Transmit};
use quinn::AsyncUdpSocket;
use rust_p2p_core::route_table::RouteTable;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Packet types identified by the first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Stun,
    Quic,
    Punch,
    RustP2p(u8),
    Other(u8),
}

/// Identify packet type from the first byte.
pub fn classify_packet(first_byte: u8) -> PacketType {
    if first_byte & 0x40 != 0 {
        PacketType::Quic
    } else {
        match first_byte {
            0x01 => PacketType::Stun,
            0x02..=0x03 => PacketType::Punch,
            b if b & 0x80 != 0 => PacketType::RustP2p(b & 0x7f),
            b => PacketType::Other(b),
        }
    }
}

/// A received non-QUIC packet.
#[derive(Debug)]
pub struct ReceivedPacket {
    pub data: Bytes,
    pub addr: SocketAddr,
    pub packet_type: PacketType,
}

#[derive(Clone, Debug)]
struct VirtualPeer {
    peer_id: PeerId,
    src: PeerId,
    max_ttl: u8,
}

#[derive(Clone, Debug)]
struct RoutedQuicPacket {
    data: Bytes,
    addr: SocketAddr,
}

/// Shared UDP socket with packet demuxing for QUIC + other protocols.
pub struct SharedUdpSocket {
    socket: Arc<UdpSocket>,
    non_quic_tx: mpsc::Sender<ReceivedPacket>,
    routed_quic_tx: mpsc::UnboundedSender<RoutedQuicPacket>,
    routed_quic_rx: parking_lot::Mutex<mpsc::UnboundedReceiver<RoutedQuicPacket>>,
    virtual_by_addr: DashMap<SocketAddr, VirtualPeer>,
    virtual_by_peer: DashMap<PeerId, SocketAddr>,
    routes: parking_lot::RwLock<Option<RouteTable<PeerId>>>,
}

impl SharedUdpSocket {
    pub fn new(socket: Arc<UdpSocket>, non_quic_tx: mpsc::Sender<ReceivedPacket>) -> Arc<Self> {
        let (routed_quic_tx, routed_quic_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            socket,
            non_quic_tx,
            routed_quic_tx,
            routed_quic_rx: parking_lot::Mutex::new(routed_quic_rx),
            virtual_by_addr: DashMap::new(),
            virtual_by_peer: DashMap::new(),
            routes: parking_lot::RwLock::new(None),
        })
    }

    pub(crate) fn set_route_table(&self, routes: RouteTable<PeerId>) {
        *self.routes.write() = Some(routes);
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

    pub(crate) fn register_virtual_peer(
        &self,
        src: PeerId,
        peer_id: PeerId,
        max_ttl: u8,
    ) -> SocketAddr {
        if let Some(addr) = self.virtual_by_peer.get(&peer_id).map(|entry| *entry) {
            self.virtual_by_addr.insert(
                addr,
                VirtualPeer {
                    peer_id,
                    src,
                    max_ttl,
                },
            );
            return addr;
        }

        let addr = self.allocate_virtual_addr();
        self.virtual_by_peer.insert(peer_id.clone(), addr);
        self.virtual_by_addr.insert(
            addr,
            VirtualPeer {
                peer_id,
                src,
                max_ttl,
            },
        );
        addr
    }

    pub(crate) fn peer_for_virtual_addr(&self, addr: SocketAddr) -> Option<PeerId> {
        self.virtual_by_addr
            .get(&addr)
            .map(|entry| entry.peer_id.clone())
    }

    pub(crate) fn release_virtual_peer(&self, peer_id: &PeerId) {
        if let Some((_, addr)) = self.virtual_by_peer.remove(peer_id) {
            self.virtual_by_addr.remove(&addr);
        }
    }

    fn allocate_virtual_addr(&self) -> SocketAddr {
        for _ in 0..128 {
            let suffix = rand::random::<u16>();
            let port = 1024 + (rand::random::<u16>() % (u16::MAX - 1024));
            let addr = SocketAddr::from(([127, 255, (suffix >> 8) as u8, suffix as u8], port));
            if !self.virtual_by_addr.contains_key(&addr) {
                return addr;
            }
        }

        loop {
            let suffix = rand::random::<u16>();
            let addr = SocketAddr::from(([127, 255, (suffix >> 8) as u8, suffix as u8], 4433));
            if !self.virtual_by_addr.contains_key(&addr) {
                return addr;
            }
        }
    }

    pub(crate) fn inject_routed_quic(&self, data: Bytes, addr: SocketAddr) {
        if self
            .routed_quic_tx
            .send(RoutedQuicPacket { data, addr })
            .is_err()
        {
            log::warn!("routed QUIC packet channel closed, dropping");
        }
    }

    fn forward_non_quic(&self, packet: ReceivedPacket) {
        if self.non_quic_tx.try_send(packet).is_err() {
            log::warn!("non-quic packet channel full, dropping");
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
        if let Some(peer) = self.virtual_by_addr.get(&transmit.destination) {
            let routes = self.routes.read();
            let route = routes
                .as_ref()
                .and_then(|routes| routes.route_one(&peer.peer_id))
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "peer route not found"))?;
            let packet = Packet::build(
                ProtocolType::QuicRelay,
                peer.src.clone(),
                peer.peer_id.clone(),
                peer.max_ttl,
                transmit.contents,
            )?;
            return self
                .socket
                .try_send_to(packet.as_bytes(), route.route_key().addr())
                .map(|_| ());
        }
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
        if let Poll::Ready(Some(packet)) = Pin::new(&mut *self.routed_quic_rx.lock()).poll_recv(cx)
        {
            let copy_len = packet.data.len().min(bufs[0].len());
            bufs[0][..copy_len].copy_from_slice(&packet.data[..copy_len]);
            meta[0] = RecvMeta {
                addr: packet.addr,
                len: copy_len,
                stride: copy_len,
                ecn: None,
                dst_ip: None,
            };
            return Poll::Ready(Ok(1));
        }

        // Read a single datagram from the socket
        let mut buf = vec![0u8; 65536];
        let mut read_buf = tokio::io::ReadBuf::new(&mut buf);

        let addr = match self.socket.poll_recv_from(cx, &mut read_buf) {
            Poll::Ready(Ok(addr)) => addr,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        };

        let n = read_buf.filled().len();
        if n == 0 {
            return Poll::Ready(Ok(0));
        }

        let first_byte = read_buf.filled()[0];
        let packet_type = classify_packet(first_byte);

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
                // Non-QUIC packet - forward it to the direct datagram receiver.
                let data = Bytes::copy_from_slice(read_buf.filled());
                let packet = ReceivedPacket {
                    data,
                    addr,
                    packet_type,
                };
                self.forward_non_quic(packet);
                cx.waker().wake_by_ref();
                Poll::Pending
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

#[cfg(test)]
mod tests {
    use super::{classify_packet, PacketType, SharedUdpSocket};
    use crate::PeerId;
    use quinn::udp::Transmit;
    use quinn::AsyncUdpSocket;
    use rust_p2p_core::route_table::{Protocol, Route, RouteKey, RouteTable};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::net::UdpSocket;
    use tokio::sync::mpsc;

    #[test]
    fn classifies_quic_by_fixed_bit() {
        assert_eq!(classify_packet(0xc0), PacketType::Quic);
        assert_eq!(classify_packet(0x40), PacketType::Quic);
    }

    #[test]
    fn keeps_rustp2p_packets_out_of_quic_path() {
        assert_eq!(classify_packet(0x80), PacketType::RustP2p(0));
        assert_eq!(classify_packet(0x8e), PacketType::RustP2p(14));
    }

    #[tokio::test]
    async fn virtual_peer_mapping_is_stable_unique_and_released() {
        let shared = shared_socket().await;
        let src = PeerId::from("node-a");
        let peer_a = PeerId::from("node-b");
        let peer_b = PeerId::from("node-c");

        let addr_a = shared.register_virtual_peer(src.clone(), peer_a.clone(), 8);
        let addr_a_again = shared.register_virtual_peer(src.clone(), peer_a.clone(), 8);
        let addr_b = shared.register_virtual_peer(src, peer_b.clone(), 8);

        assert_eq!(addr_a, addr_a_again);
        assert_ne!(addr_a, addr_b);
        assert!(is_virtual_addr(addr_a));
        assert!(is_virtual_addr(addr_b));
        assert_eq!(shared.peer_for_virtual_addr(addr_a), Some(peer_a.clone()));

        shared.release_virtual_peer(&peer_a);
        assert_eq!(shared.peer_for_virtual_addr(addr_a), None);
    }

    #[tokio::test]
    async fn virtual_send_uses_current_route_for_peer() {
        let shared = shared_socket().await;
        let routes = RouteTable::new(Default::default());
        shared.set_route_table(routes.clone());

        let src = PeerId::from("node-a");
        let peer = PeerId::from("node-b");
        let virtual_addr = shared.register_virtual_peer(src, peer.clone(), 8);

        let recv_one = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let recv_two = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let route_one = RouteKey::new(Protocol::UDP, recv_one.local_addr().unwrap());
        let route_two = RouteKey::new(Protocol::UDP, recv_two.local_addr().unwrap());

        routes.add_route(peer.clone(), Route::from_default_rt(route_one, 0));
        send_with_retry(&shared, virtual_addr, b"one").await;
        let mut buf = [0u8; 256];
        let (n, _) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            recv_one.recv_from(&mut buf),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(n > 0);

        routes.remove_route(&peer, &route_one);
        routes.add_route(peer, Route::from_default_rt(route_two, 0));
        send_with_retry(&shared, virtual_addr, b"two").await;
        let (n, _) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            recv_two.recv_from(&mut buf),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(n > 0);
    }

    async fn shared_socket() -> Arc<SharedUdpSocket> {
        let std_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        std_socket.set_nonblocking(true).unwrap();
        let socket = Arc::new(UdpSocket::from_std(std_socket).unwrap());
        let (tx, _rx) = mpsc::channel(8);
        SharedUdpSocket::new(socket, tx)
    }

    fn transmit(destination: SocketAddr, contents: &[u8]) -> Transmit<'_> {
        Transmit {
            destination,
            ecn: None,
            contents,
            segment_size: None,
            src_ip: None,
        }
    }

    async fn send_with_retry(shared: &Arc<SharedUdpSocket>, destination: SocketAddr, data: &[u8]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            match shared.try_send(&transmit(destination, data)) {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "timed out waiting for UDP socket to become writable"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Err(e) => panic!("send failed: {e}"),
            }
        }
    }

    fn is_virtual_addr(addr: SocketAddr) -> bool {
        matches!(addr.ip(), std::net::IpAddr::V4(ip) if ip.octets()[0] == 127 && ip.octets()[1] == 255)
    }
}
