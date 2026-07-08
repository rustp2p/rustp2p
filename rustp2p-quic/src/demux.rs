use crate::protocol::{Packet, ProtocolType};
use crate::PeerId;
use bytes::Bytes;
use dashmap::DashMap;
use quinn::udp::{RecvMeta, Transmit};
use quinn::AsyncUdpSocket;
use rust_p2p_core::endpoint::{Config as CoreConfig, EndPoint, Sender as CoreSender, Transport};
use rust_p2p_core::route_table::{RouteKey, RouteTable};
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
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

#[derive(Debug)]
pub(crate) struct ReceivedPacket {
    pub(crate) data: Bytes,
    pub(crate) addr: SocketAddr,
    pub(crate) packet_type: PacketType,
}

enum CoreOutboundPacket {
    RawAddr {
        data: Bytes,
        addr: SocketAddr,
    },
    QuicPeer {
        src: PeerId,
        dest: PeerId,
        max_ttl: u8,
        data: Bytes,
    },
}

/// Core-backed transport layer. This owns real transport I/O and route-based sending.
pub(crate) struct CoreTransportLayer {
    sender: CoreSender,
    local_addr: SocketAddr,
    overlay_tx: mpsc::Sender<ReceivedPacket>,
    routes: parking_lot::RwLock<Option<RouteTable<PeerId>>>,
    transports: DashMap<RouteKey, Transport>,
    outbound_tx: mpsc::UnboundedSender<CoreOutboundPacket>,
}

impl CoreTransportLayer {
    pub(crate) async fn bind(
        bind_addr: SocketAddr,
        stun_servers: Vec<String>,
        overlay_tx: mpsc::Sender<ReceivedPacket>,
    ) -> io::Result<Arc<Self>> {
        let core_config = CoreConfig::udp(bind_addr.port()).stun_servers(stun_servers);
        let endpoint = EndPoint::bind(core_config).await?;
        let sender = endpoint.sender();
        let local_addr = normalize_local_addr(endpoint.local_addr().await?, bind_addr);
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let layer = Arc::new(Self {
            sender,
            local_addr,
            overlay_tx,
            routes: parking_lot::RwLock::new(None),
            transports: DashMap::new(),
            outbound_tx,
        });
        layer.start(endpoint, outbound_rx);
        Ok(layer)
    }

    fn start(
        self: &Arc<Self>,
        endpoint: EndPoint,
        outbound_rx: mpsc::UnboundedReceiver<CoreOutboundPacket>,
    ) {
        self.start_core_receiver(endpoint);
        self.start_core_sender(outbound_rx);
    }

    pub(crate) fn set_route_table(&self, routes: RouteTable<PeerId>) {
        *self.routes.write() = Some(routes);
    }

    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }

    pub(crate) async fn send_raw(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        self.sender.send_to(buf, addr).await
    }

    pub(crate) fn try_send_raw(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        self.outbound_tx
            .send(CoreOutboundPacket::RawAddr {
                data: Bytes::copy_from_slice(buf),
                addr,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "transport sender closed"))
    }

    pub(crate) fn try_send_quic_by_peer_id(
        &self,
        src: PeerId,
        dest: PeerId,
        max_ttl: u8,
        data: &[u8],
    ) -> io::Result<()> {
        self.outbound_tx
            .send(CoreOutboundPacket::QuicPeer {
                src,
                dest,
                max_ttl,
                data: Bytes::copy_from_slice(data),
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "transport sender closed"))
    }

    fn start_core_receiver(self: &Arc<Self>, mut endpoint: EndPoint) {
        let this = self.clone();
        tokio::spawn(async move {
            while let Some(received) = endpoint.recv().await {
                let route_key = RouteKey::from_transport(&received.transport);
                this.transports
                    .insert(route_key, received.transport.clone());
                if received.data.is_empty() {
                    continue;
                }
                let packet = ReceivedPacket {
                    packet_type: classify_packet(received.data[0]),
                    data: received.data,
                    addr: route_key.addr(),
                };
                this.forward_overlay_packet(packet);
            }
        });
    }

    fn start_core_sender(
        self: &Arc<Self>,
        mut outbound_rx: mpsc::UnboundedReceiver<CoreOutboundPacket>,
    ) {
        let this = self.clone();
        tokio::spawn(async move {
            while let Some(packet) = outbound_rx.recv().await {
                let result = match packet {
                    CoreOutboundPacket::RawAddr { data, addr } => {
                        this.sender.send_to(&data, addr).await
                    }
                    CoreOutboundPacket::QuicPeer {
                        src,
                        dest,
                        max_ttl,
                        data,
                    } => this.send_quic_by_peer_id(src, dest, max_ttl, &data).await,
                };
                if let Err(e) = result {
                    log::debug!("core transport send failed: {e}");
                }
            }
        });
    }

    async fn send_quic_by_peer_id(
        &self,
        src: PeerId,
        dest: PeerId,
        max_ttl: u8,
        data: &[u8],
    ) -> io::Result<()> {
        let packet = Packet::build(ProtocolType::QuicRelay, src, dest.clone(), max_ttl, data)?;
        self.send_packet_to_peer(dest, packet.as_bytes()).await
    }

    async fn send_packet_to_peer(&self, peer_id: PeerId, data: &[u8]) -> io::Result<()> {
        let route = self
            .routes
            .read()
            .as_ref()
            .and_then(|routes| routes.route_one(&peer_id))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "peer route not found"))?;
        let route_key = route.route_key();
        if let Some(transport) = self.transports.get(&route_key) {
            transport.send(data).await
        } else {
            self.sender.send_to(data, route_key.addr()).await
        }
    }

    fn forward_overlay_packet(&self, packet: ReceivedPacket) {
        if self.overlay_tx.try_send(packet).is_err() {
            log::warn!("transport packet channel full, dropping");
        }
    }
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

/// Quinn socket adapter. It only maps PeerId to synthetic addresses for quinn.
pub(crate) struct QuicPeerSocket {
    transport: Arc<CoreTransportLayer>,
    routed_quic_tx: mpsc::UnboundedSender<RoutedQuicPacket>,
    routed_quic_rx: parking_lot::Mutex<mpsc::UnboundedReceiver<RoutedQuicPacket>>,
    virtual_by_addr: DashMap<SocketAddr, VirtualPeer>,
    virtual_by_peer: DashMap<PeerId, SocketAddr>,
}

impl QuicPeerSocket {
    pub(crate) fn new(transport: Arc<CoreTransportLayer>) -> Arc<Self> {
        let (routed_quic_tx, routed_quic_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            transport,
            routed_quic_tx,
            routed_quic_rx: parking_lot::Mutex::new(routed_quic_rx),
            virtual_by_addr: DashMap::new(),
            virtual_by_peer: DashMap::new(),
        })
    }

    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        self.transport.local_addr()
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

    pub(crate) fn inject_routed_quic(&self, data: Bytes, addr: SocketAddr) {
        if self
            .routed_quic_tx
            .send(RoutedQuicPacket { data, addr })
            .is_err()
        {
            log::warn!("routed QUIC packet channel closed, dropping");
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
}

impl AsyncUdpSocket for QuicPeerSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn quinn::UdpPoller>> {
        Box::pin(QuicPeerPoller)
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        if let Some(peer) = self.virtual_by_addr.get(&transmit.destination) {
            return self.transport.try_send_quic_by_peer_id(
                peer.src.clone(),
                peer.peer_id.clone(),
                peer.max_ttl,
                transmit.contents,
            );
        }
        self.transport
            .try_send_raw(transmit.contents, transmit.destination)
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

        Poll::Pending
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        QuicPeerSocket::local_addr(self)
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

#[derive(Debug)]
struct QuicPeerPoller;

impl quinn::UdpPoller for QuicPeerPoller {
    fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl std::fmt::Debug for QuicPeerSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicPeerSocket")
            .field("local_addr", &self.local_addr())
            .finish()
    }
}

fn normalize_local_addr(actual: SocketAddr, requested: SocketAddr) -> SocketAddr {
    if requested.ip().is_unspecified() || !actual.ip().is_unspecified() {
        actual
    } else {
        SocketAddr::new(requested.ip(), actual.port())
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_packet, PacketType};

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
}
