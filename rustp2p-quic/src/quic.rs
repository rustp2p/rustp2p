use crate::cert::{RustlsCertificateVerifier, RustlsClientCertificateVerifier};
use crate::config::Config;
use crate::connection::Connection;
use crate::protocol::{
    decode_datagram_frame, decode_stream_frame, encode_datagram_frame, encode_stream_frame,
    DatagramFrame, ProtocolLayer, StreamFrame, StreamHeader,
};
use crate::reliable::{ReliableRecvStream, ReliableSendStream};
use crate::{Identity, PeerId};
use bytes::Bytes;
use dashmap::DashMap;
use quinn::udp::{RecvMeta, Transmit};
use quinn::AsyncUdpSocket;
use rust_p2p_core::route_table::RouteKey;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
/// Decrypted QUIC DATAGRAM user message delivered by [`Endpoint::recv`](crate::Endpoint::recv).
pub struct ReceivedMessage {
    /// User payload after QUIC decryption and frame decoding.
    pub payload: Bytes,
    /// Source peer id declared inside the QUIC application frame.
    pub src: PeerId,
    /// Destination peer id declared inside the QUIC application frame.
    pub dest: PeerId,
    /// Current route snapshot used to describe the source path.
    pub route: RouteKey,
    /// Remaining overlay TTL inferred from the route metric.
    pub ttl: u8,
    /// Initial overlay TTL configured for this endpoint.
    pub max_ttl: u8,
    /// Whether the current route snapshot is relayed.
    pub is_relay: bool,
    /// Whether this message was produced by a broadcast send.
    pub is_broadcast: bool,
}

/// Inbound end-to-end bidirectional QUIC stream.
///
/// Relay nodes do not receive this value for forwarded QUIC traffic; only the
/// actual destination endpoint accepts the stream.
pub struct IncomingBiStream {
    /// Peer id of the remote QUIC endpoint.
    pub peer_id: PeerId,
    /// Send half of the accepted bidirectional stream.
    pub send: ReliableSendStream,
    /// Receive half of the accepted bidirectional stream.
    pub recv: ReliableRecvStream,
}

#[derive(Clone, Debug)]
struct VirtualPeer {
    peer_id: PeerId,
}

#[derive(Clone, Debug)]
struct RoutedQuicPacket {
    data: Bytes,
    addr: SocketAddr,
}

/// Quinn socket adapter.
///
/// Quinn requires `SocketAddr`s, but the public transport model is `PeerId`.
/// This adapter owns the synthetic address table and delegates real delivery to
/// the protocol layer, which wraps QUIC packets and sends them by peer id.
pub(crate) struct QuicPeerSocket {
    protocol: Arc<ProtocolLayer>,
    routed_quic_tx: mpsc::UnboundedSender<RoutedQuicPacket>,
    routed_quic_rx: parking_lot::Mutex<mpsc::UnboundedReceiver<RoutedQuicPacket>>,
    virtual_by_addr: DashMap<SocketAddr, VirtualPeer>,
    virtual_by_peer: DashMap<PeerId, SocketAddr>,
}

impl QuicPeerSocket {
    pub(crate) fn new(protocol: Arc<ProtocolLayer>) -> Arc<Self> {
        let (routed_quic_tx, routed_quic_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            protocol,
            routed_quic_tx,
            routed_quic_rx: parking_lot::Mutex::new(routed_quic_rx),
            virtual_by_addr: DashMap::new(),
            virtual_by_peer: DashMap::new(),
        })
    }

    pub(crate) fn register_virtual_peer(&self, peer_id: PeerId) -> SocketAddr {
        if let Some(addr) = self.virtual_by_peer.get(&peer_id).map(|entry| *entry) {
            self.virtual_by_addr.insert(addr, VirtualPeer { peer_id });
            return addr;
        }

        let addr = self.allocate_virtual_addr();
        self.virtual_by_peer.insert(peer_id.clone(), addr);
        self.virtual_by_addr.insert(addr, VirtualPeer { peer_id });
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
            return self
                .protocol
                .try_send_quic_payload(peer.peer_id.clone(), transmit.contents);
        }
        // Unknown destinations are internal state errors. Treating the address
        // as a real network address would bypass the PeerId overlay and route
        // confirmation rules.
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "unknown synthetic QUIC destination: {}",
                transmit.destination
            ),
        ))
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
        Ok(SocketAddr::from(([127, 255, 0, 1], 0)))
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
            .field("local_addr", &SocketAddr::from(([127, 255, 0, 1], 0)))
            .finish()
    }
}

pub(crate) struct QuicEndpoint {
    endpoint: quinn::Endpoint,
    peer_id: PeerId,
    protocol: Arc<ProtocolLayer>,
    socket: Arc<QuicPeerSocket>,
    closed: AtomicBool,
    connections: DashMap<PeerId, Connection>,
    connection_tasks: DashMap<usize, ()>,
    seen_datagrams: DashMap<(PeerId, u64), ()>,
    inbox_tx: flume::Sender<ReceivedMessage>,
    inbox_rx: flume::Receiver<ReceivedMessage>,
    stream_tx: flume::Sender<IncomingBiStream>,
    stream_rx: flume::Receiver<IncomingBiStream>,
}

impl QuicEndpoint {
    pub(crate) async fn bind(
        identity: Identity,
        config: &Config,
        protocol: Arc<ProtocolLayer>,
    ) -> io::Result<Arc<Self>> {
        let cert_der = identity.certificate_der().to_vec();
        let key_der = identity.private_key_der().to_vec();

        let transport_config = Arc::new({
            let mut transport = quinn::TransportConfig::default();
            transport.datagram_receive_buffer_size(config.datagram_receive_buffer_size);
            transport.datagram_send_buffer_size(config.datagram_send_buffer_size);
            transport
        });

        let key_provider = rustls::crypto::ring::default_provider();
        let mut server_crypto =
            rustls::ServerConfig::builder_with_provider(Arc::new(key_provider.clone()))
                .with_safe_default_protocol_versions()
                .map_err(|e| io::Error::other(e.to_string()))?
                .with_client_cert_verifier(RustlsClientCertificateVerifier::new(
                    config.certificate_verifier.clone(),
                ))
                .with_single_cert(
                    vec![CertificateDer::from(cert_der.clone())],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der.clone())),
                )
                .map_err(|e| io::Error::other(e.to_string()))?;
        server_crypto.alpn_protocols = config.alpns.clone();

        let quic_server_config =
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
                .map_err(|e| io::Error::other(format!("quic server config: {e}")))?;

        let mut client_crypto = rustls::ClientConfig::builder_with_provider(Arc::new(key_provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| io::Error::other(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(RustlsCertificateVerifier::new(
                config.certificate_verifier.clone(),
            ))
            .with_client_auth_cert(
                vec![CertificateDer::from(cert_der)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
        client_crypto.alpn_protocols = config.alpns.clone();

        let quic_client_config =
            quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)
                .map_err(|e| io::Error::other(format!("quic client config: {e}")))?;
        let mut client_config = quinn::ClientConfig::new(Arc::new(quic_client_config));
        client_config.transport_config(transport_config.clone());

        let socket = QuicPeerSocket::new(protocol.clone());
        let endpoint_config = quinn::EndpointConfig::default();
        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_server_config));
        server_config.transport_config(transport_config);
        let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
            endpoint_config,
            Some(server_config),
            socket.clone(),
            Arc::new(quinn::TokioRuntime),
        )?;
        endpoint.set_default_client_config(client_config);

        let (inbox_tx, inbox_rx) = flume::bounded(512);
        let (stream_tx, stream_rx) = flume::bounded(128);
        let quic = Arc::new(Self {
            endpoint,
            peer_id: identity.peer_id(),
            protocol,
            socket,
            closed: AtomicBool::new(false),
            connections: DashMap::new(),
            connection_tasks: DashMap::new(),
            seen_datagrams: DashMap::new(),
            inbox_tx,
            inbox_rx,
            stream_tx,
            stream_rx,
        });
        quic.start_accept_loop();
        quic.start_transport_loop();
        Ok(quic)
    }

    pub(crate) async fn send_to(
        self: &Arc<Self>,
        peer_id: PeerId,
        payload: &[u8],
    ) -> io::Result<()> {
        let conn = self.connection_to(peer_id.clone()).await?;
        let frame = DatagramFrame::User {
            id: rand::random(),
            src: self.peer_id.clone(),
            dest: peer_id,
            payload: payload.to_vec(),
        };
        let data = Bytes::from(encode_datagram_frame(&frame));
        for attempt in 0..5 {
            conn.quinn()
                .send_datagram_wait(data.clone())
                .await
                .map_err(|e| io::Error::other(format!("send QUIC datagram: {e}")))?;
            if attempt < 4 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        Ok(())
    }

    pub(crate) fn try_send_to(&self, peer_id: PeerId, payload: &[u8]) -> io::Result<()> {
        let conn = self
            .connections
            .get(&peer_id)
            .filter(|conn| !conn.is_closed())
            .ok_or_else(|| io::Error::from(io::ErrorKind::WouldBlock))?;
        let frame = DatagramFrame::User {
            id: rand::random(),
            src: self.peer_id.clone(),
            dest: peer_id,
            payload: payload.to_vec(),
        };
        conn.quinn()
            .send_datagram(Bytes::from(encode_datagram_frame(&frame)))
            .map_err(|e| io::Error::other(format!("send QUIC datagram: {e}")))
    }

    pub(crate) async fn recv(&self) -> io::Result<ReceivedMessage> {
        self.inbox_rx
            .recv_async()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "endpoint closed"))
    }

    pub(crate) fn try_recv(&self) -> io::Result<ReceivedMessage> {
        self.inbox_rx.try_recv().map_err(|e| match e {
            flume::TryRecvError::Empty => io::Error::from(io::ErrorKind::WouldBlock),
            flume::TryRecvError::Disconnected => {
                io::Error::new(io::ErrorKind::UnexpectedEof, "endpoint closed")
            }
        })
    }

    pub(crate) async fn open_bi(
        self: &Arc<Self>,
        peer_id: PeerId,
    ) -> io::Result<(ReliableSendStream, ReliableRecvStream)> {
        let mut last_err = None;
        for _ in 0..2 {
            match self.open_bi_once(peer_id.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    last_err = Some(e);
                    self.connections.remove(&peer_id);
                    self.socket.release_virtual_peer(&peer_id);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("open reliable stream failed")))
    }

    async fn open_bi_once(
        self: &Arc<Self>,
        peer_id: PeerId,
    ) -> io::Result<(ReliableSendStream, ReliableRecvStream)> {
        let conn = self.connection_to(peer_id.clone()).await?;
        let (mut send, recv) = conn.quinn().open_bi().await?;
        write_stream_frame(
            &mut send,
            &StreamFrame::User(StreamHeader {
                src: self.peer_id.clone(),
                dest: peer_id,
            }),
        )
        .await?;
        Ok((ReliableSendStream::new(send), ReliableRecvStream::new(recv)))
    }

    pub(crate) async fn accept_bi(&self) -> io::Result<IncomingBiStream> {
        self.stream_rx
            .recv_async()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "endpoint closed"))
    }

    pub(crate) async fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.endpoint.close(0u32.into(), b"shutdown");
    }

    async fn connection_to(self: &Arc<Self>, peer_id: PeerId) -> io::Result<Connection> {
        self.protocol.ensure_peer_reachable(&peer_id)?;
        // The address is only a local quinn handle. It does not encode the next
        // hop; route lookup happens each time protocol sends a QUIC packet.
        let addr = self.socket.register_virtual_peer(peer_id.clone());
        self.connection_to_at(peer_id, addr).await
    }

    async fn connection_to_at(
        self: &Arc<Self>,
        peer_id: PeerId,
        addr: SocketAddr,
    ) -> io::Result<Connection> {
        if let Some(conn) = self.connections.get(&peer_id) {
            if !conn.is_closed() {
                return Ok(conn.clone());
            }
        }
        let conn = match self.connect_quic_addr(addr).await {
            Ok(conn) => conn,
            Err(e) => {
                self.socket.release_virtual_peer(&peer_id);
                return Err(e);
            }
        };
        self.connections.insert(peer_id, conn.clone());
        self.start_connection_tasks(conn.clone());
        Ok(conn)
    }

    async fn connect_quic_addr(&self, addr: SocketAddr) -> io::Result<Connection> {
        match self.endpoint.connect(addr, "localhost") {
            Ok(connecting) => {
                let conn = tokio::time::timeout(Duration::from_secs(5), connecting)
                    .await
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "QUIC handshake timed out")
                    })?
                    .map_err(|e| io::Error::other(format!("handshake: {e}")))?;
                match tokio::time::timeout(Duration::from_millis(50), conn.closed()).await {
                    Ok(reason) => Err(io::Error::other(format!("connection closed: {reason}"))),
                    Err(_) => {
                        if let Some(reason) = conn.close_reason() {
                            Err(io::Error::other(format!("connection closed: {reason}")))
                        } else {
                            Ok(Connection::new(conn))
                        }
                    }
                }
            }
            Err(e) => Err(io::Error::other(format!("connect: {e}"))),
        }
    }

    async fn accept_quic(&self) -> Option<Connection> {
        let incoming = self.endpoint.accept().await?;
        match incoming.await {
            Ok(conn) => Some(Connection::new(conn)),
            Err(e) => {
                log::warn!("Failed to accept connection: {e}");
                None
            }
        }
    }

    fn start_transport_loop(self: &Arc<Self>) {
        let quic = self.clone();
        tokio::spawn(async move {
            loop {
                if quic.closed.load(Ordering::Relaxed) {
                    break;
                }
                let payload = match quic.protocol.recv_quic_payload().await {
                    Ok(payload) => payload,
                    Err(e) => {
                        log::debug!("transport QUIC payload loop ended: {e}");
                        break;
                    }
                };
                let virtual_addr = quic.socket.register_virtual_peer(payload.src);
                quic.socket
                    .inject_routed_quic(payload.payload, virtual_addr);
            }
        });
    }

    fn start_accept_loop(self: &Arc<Self>) {
        let quic = self.clone();
        tokio::spawn(async move {
            while let Some(conn) = quic.accept_quic().await {
                if quic.closed.load(Ordering::Relaxed) {
                    break;
                }
                quic.start_connection_tasks(conn);
            }
        });
    }

    fn start_connection_tasks(self: &Arc<Self>, conn: Connection) {
        let stable_id = conn.quinn().stable_id();
        if self.connection_tasks.insert(stable_id, ()).is_some() {
            return;
        }
        let connection_peer = self.socket.peer_for_virtual_addr(conn.remote_addr());

        let stream_endpoint = self.clone();
        let stream_conn = conn.clone();
        let stream_peer = connection_peer.clone();
        tokio::spawn(async move {
            loop {
                match stream_conn.quinn().accept_bi().await {
                    Ok((send, recv)) => {
                        let endpoint = stream_endpoint.clone();
                        let remote_addr = stream_conn.remote_addr();
                        tokio::spawn(async move {
                            if let Err(e) = endpoint
                                .handle_incoming_stream(remote_addr, send, recv)
                                .await
                            {
                                log::debug!("stream handling failed: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        log::debug!("stream accept ended: {e}");
                        break;
                    }
                }
            }
            stream_endpoint.cleanup_connection(stable_id, stream_peer);
        });

        let datagram_endpoint = self.clone();
        let datagram_peer = connection_peer;
        tokio::spawn(async move {
            loop {
                match conn.quinn().read_datagram().await {
                    Ok(data) => {
                        if let Err(e) = datagram_endpoint
                            .handle_incoming_datagram(conn.remote_addr(), data)
                            .await
                        {
                            log::debug!("datagram handling failed: {e}");
                        }
                    }
                    Err(e) => {
                        log::debug!("datagram reader ended: {e}");
                        break;
                    }
                }
            }
            datagram_endpoint.cleanup_connection(stable_id, datagram_peer);
        });
    }

    fn cleanup_connection(&self, stable_id: usize, peer_id: Option<PeerId>) {
        self.connection_tasks.remove(&stable_id);
        if let Some(peer_id) = peer_id {
            self.connections.remove(&peer_id);
            self.socket.release_virtual_peer(&peer_id);
        }
    }

    async fn handle_incoming_stream(
        &self,
        remote_addr: SocketAddr,
        send: quinn::SendStream,
        mut recv: quinn::RecvStream,
    ) -> io::Result<()> {
        if self.socket.peer_for_virtual_addr(remote_addr).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "stream arrived from unknown synthetic peer",
            ));
        }
        let StreamFrame::User(header) = read_stream_frame(&mut recv).await?;
        if header.dest != self.peer_id {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "stream destination peer mismatch",
            ));
        }
        self.protocol.ensure_peer_reachable(&header.src)?;
        let incoming = IncomingBiStream {
            peer_id: header.src,
            send: ReliableSendStream::new(send),
            recv: ReliableRecvStream::new(recv),
        };
        let _ = self.stream_tx.send_async(incoming).await;
        Ok(())
    }

    async fn handle_incoming_datagram(
        &self,
        remote_addr: SocketAddr,
        data: Bytes,
    ) -> io::Result<()> {
        if self.socket.peer_for_virtual_addr(remote_addr).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "datagram arrived from unknown synthetic peer",
            ));
        }
        let frame = decode_datagram_frame(&data)?;
        match frame {
            DatagramFrame::User {
                id,
                src,
                dest,
                payload,
            } => {
                if dest != self.peer_id {
                    return Ok(());
                }
                if self.seen_datagrams.insert((src.clone(), id), ()).is_some() {
                    return Ok(());
                }
                let (route, metric) = self.protocol.route_metric_for(&src)?;
                let max_ttl = self.protocol.max_ttl();
                let _ = self
                    .inbox_tx
                    .send_async(ReceivedMessage {
                        payload: Bytes::from(payload),
                        src,
                        dest,
                        route,
                        ttl: max_ttl.saturating_sub(metric),
                        max_ttl,
                        is_relay: metric > 0,
                        is_broadcast: false,
                    })
                    .await;
            }
        }
        Ok(())
    }
}

async fn write_frame(send: &mut quinn::SendStream, data: &[u8]) -> io::Result<()> {
    if data.len() > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stream header too large",
        ));
    }
    send.write_all(&(data.len() as u32).to_be_bytes())
        .await
        .map_err(|e| io::Error::other(format!("write stream header: {e}")))?;
    send.write_all(data)
        .await
        .map_err(|e| io::Error::other(format!("write stream header: {e}")))?;
    Ok(())
}

async fn write_stream_frame(send: &mut quinn::SendStream, frame: &StreamFrame) -> io::Result<()> {
    // Stream metadata is a small protobuf frame before the user-visible stream
    // bytes. The enclosing QUIC stream still provides end-to-end encryption.
    write_frame(send, &encode_stream_frame(frame)).await
}

async fn read_frame(recv: &mut quinn::RecvStream) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, format!("{e}")))?;
    let len = u32::from_be_bytes(len) as usize;
    if len > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stream header too large",
        ));
    }
    let mut data = vec![0u8; len];
    recv.read_exact(&mut data)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, format!("{e}")))?;
    Ok(data)
}

async fn read_stream_frame(recv: &mut quinn::RecvStream) -> io::Result<StreamFrame> {
    // Only the internal stream header is length-prefixed. User bytes remain on
    // the raw QUIC stream for `ReliableRecvStream` to consume.
    decode_stream_frame(&read_frame(recv).await?)
}

#[cfg(test)]
mod tests {
    use super::{QuicPeerSocket, Transmit};
    use crate::protocol::ProtocolLayer;
    use crate::{Config, Identity, PeerId};
    use quinn::AsyncUdpSocket;
    use rust_p2p_core::route_table::{Protocol, RouteKey};
    use std::io;

    #[tokio::test]
    async fn unknown_quic_destination_is_not_sent_as_raw_addr() {
        let socket = quic_peer_socket().await;
        let transmit = Transmit {
            destination: "127.255.1.2:4433".parse().unwrap(),
            ecn: None,
            contents: b"quic packet",
            segment_size: None,
            src_ip: None,
        };

        let err = socket.try_send(&transmit).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err
            .to_string()
            .contains("unknown synthetic QUIC destination"));
    }

    #[tokio::test]
    async fn registered_quic_destination_is_accepted() {
        let socket = quic_peer_socket().await;
        let addr = socket.register_virtual_peer(PeerId::from("node-b"));
        let transmit = Transmit {
            destination: addr,
            ecn: None,
            contents: b"quic packet",
            segment_size: None,
            src_ip: None,
        };

        socket.try_send(&transmit).unwrap();
    }

    async fn quic_peer_socket() -> Arc<QuicPeerSocket> {
        let config = Config {
            identity: Some(Identity::new("node-a", "node-a-seed").unwrap()),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            stun_servers: Vec::new(),
            ..Default::default()
        };
        let transport = crate::transport::TransportLayer::bind(PeerId::from("node-a"), &config)
            .await
            .unwrap();
        transport.confirm_route(
            PeerId::from("node-b"),
            RouteKey::new(Protocol::UDP, "127.0.0.1:9".parse().unwrap()),
            0,
        );
        let protocol = ProtocolLayer::new(
            PeerId::from("node-a"),
            transport,
            8,
            Vec::new(),
            Vec::new(),
            rust_p2p_core::nat::NatInfo::default(),
        );
        QuicPeerSocket::new(protocol)
    }

    use std::sync::Arc;
}
