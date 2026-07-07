use crate::cert::{RustlsCertificateVerifier, RustlsClientCertificateVerifier};
use crate::config::Config;
use crate::connection::Connection;
use crate::demux::{ReceivedPacket, SharedUdpSocket};
use crate::protocol::{
    now_millis, DatagramFrame, HelloPayload, Packet, ProtocolType, RouteEntry, RouteReplyPayload,
    StreamFrame, StreamHeader,
};
use crate::reliable::{ReliableRecvStream, ReliableSendStream};
use crate::{Identity, NatInfo, PeerId};
use bytes::Bytes;
use dashmap::DashMap;
use rust_p2p_core::route_table::{Protocol, Route, RouteKey, RouteTable};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};

/// Read-only peer information discovered by the high-level P2P layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub addrs: Vec<SocketAddr>,
    pub relay_hint: Option<PeerId>,
    pub last_seen: u64,
    pub is_direct: bool,
}

impl PeerInfo {
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            addrs: Vec::new(),
            relay_hint: None,
            last_seen: now_millis(),
            is_direct: false,
        }
    }

    pub fn with_addr(mut self, addr: SocketAddr) -> Self {
        self.addrs.push(addr);
        self.is_direct = true;
        self
    }
}

/// Backwards-compatible low-level node address.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeAddr {
    pub node_id: PeerId,
    pub direct_addresses: Vec<SocketAddr>,
}

impl NodeAddr {
    pub fn new(node_id: impl Into<PeerId>, direct_addresses: Vec<SocketAddr>) -> Self {
        Self {
            node_id: node_id.into(),
            direct_addresses,
        }
    }

    pub fn from_id(node_id: impl Into<PeerId>) -> Self {
        Self {
            node_id: node_id.into(),
            direct_addresses: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReceivedMessage {
    pub payload: Bytes,
    pub src: PeerId,
    pub dest: PeerId,
    pub route: RouteKey,
    pub ttl: u8,
    pub max_ttl: u8,
    pub is_relay: bool,
    pub is_broadcast: bool,
}

pub struct IncomingBiStream {
    pub peer_id: PeerId,
    pub remote_addr: SocketAddr,
    pub is_relay: bool,
    pub send: ReliableSendStream,
    pub recv: ReliableRecvStream,
}

pub struct Builder {
    config: Config,
}

impl Builder {
    pub fn new() -> Self {
        let config = Config {
            high_level: true,
            ..Default::default()
        };
        Self { config }
    }

    pub fn identity(mut self, identity: Identity) -> Self {
        self.config.identity = Some(identity);
        self
    }

    pub fn bind(mut self, addr: SocketAddr) -> Self {
        self.config.bind_addr = addr;
        self
    }

    pub fn bootstrap(mut self, peers: Vec<SocketAddr>) -> Self {
        self.config.bootstrap = peers;
        self
    }

    pub fn certificate_verifier(mut self, verifier: Arc<dyn crate::CertificateVerifier>) -> Self {
        self.config.certificate_verifier = verifier;
        self
    }

    pub fn max_ttl(mut self, max_ttl: u8) -> Self {
        self.config.max_ttl = max_ttl.max(1);
        self
    }

    pub fn stun_servers(mut self, servers: Vec<String>) -> Self {
        self.config.stun_servers = servers;
        self
    }

    pub async fn build(self) -> crate::Result<Endpoint> {
        Endpoint::bind_with_config(self.config).await
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

struct RuntimeState {
    peer_id: PeerId,
    max_ttl: u8,
    closed: AtomicBool,
    peers: DashMap<PeerId, PeerInfo>,
    routes: RouteTable<PeerId>,
    connections: DashMap<PeerId, Connection>,
    connection_tasks: DashMap<usize, ()>,
    seen_datagrams: DashMap<(PeerId, u64), ()>,
    inbox_tx: flume::Sender<ReceivedMessage>,
    inbox_rx: flume::Receiver<ReceivedMessage>,
    stream_tx: flume::Sender<IncomingBiStream>,
    stream_rx: flume::Receiver<IncomingBiStream>,
}

/// The main QUIC endpoint for low-level QUIC and high-level P2P APIs.
pub struct Endpoint {
    quinn_endpoint: quinn::Endpoint,
    node_id: PeerId,
    stun_servers: Vec<String>,
    nat_info: Arc<parking_lot::RwLock<NatInfo>>,
    shared_socket: Arc<SharedUdpSocket>,
    direct_rx: Arc<Mutex<mpsc::Receiver<ReceivedPacket>>>,
    runtime: Option<Arc<RuntimeState>>,
}

impl Endpoint {
    pub fn builder() -> Builder {
        Builder::new()
    }

    /// Binds a low-level endpoint on the given address.
    pub async fn bind(bind_addr: SocketAddr) -> crate::Result<Self> {
        Self::bind_with_config(Config {
            bind_addr,
            identity: Some(Identity::new(
                format!("node-{bind_addr}"),
                rand::random::<[u8; 32]>(),
            )?),
            ..Default::default()
        })
        .await
    }

    /// Binds a new endpoint with custom configuration.
    pub async fn bind_with_config(mut config: Config) -> crate::Result<Self> {
        let identity = match config.identity.take() {
            Some(identity) => identity,
            None => Identity::new(
                format!("node-{}", rand::random::<u64>()),
                rand::random::<[u8; 32]>(),
            )?,
        };
        let node_id = identity.peer_id();

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

        let std_socket = std::net::UdpSocket::bind(config.bind_addr)?;
        std_socket.set_nonblocking(true)?;
        let tokio_socket = Arc::new(UdpSocket::from_std(std_socket)?);

        let (non_quic_tx, non_quic_rx) = mpsc::channel(512);
        let shared_socket = SharedUdpSocket::new(tokio_socket, non_quic_tx);

        let endpoint_config = quinn::EndpointConfig::default();
        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_server_config));
        server_config.transport_config(transport_config);
        let mut quinn_endpoint = quinn::Endpoint::new_with_abstract_socket(
            endpoint_config,
            Some(server_config),
            shared_socket.clone(),
            Arc::new(quinn::TokioRuntime),
        )?;
        quinn_endpoint.set_default_client_config(client_config);

        let (inbox_tx, inbox_rx) = flume::bounded(512);
        let (stream_tx, stream_rx) = flume::bounded(128);
        let runtime = if config.high_level {
            Some(Arc::new(RuntimeState {
                peer_id: identity.peer_id(),
                max_ttl: config.max_ttl.max(1),
                closed: AtomicBool::new(false),
                peers: DashMap::new(),
                routes: RouteTable::new(Default::default()),
                connections: DashMap::new(),
                connection_tasks: DashMap::new(),
                seen_datagrams: DashMap::new(),
                inbox_tx,
                inbox_rx,
                stream_tx,
                stream_rx,
            }))
        } else {
            None
        };

        let endpoint = Self {
            quinn_endpoint,
            node_id,
            stun_servers: config.stun_servers.clone(),
            nat_info: Arc::new(parking_lot::RwLock::new(NatInfo::default())),
            shared_socket,
            direct_rx: Arc::new(Mutex::new(non_quic_rx)),
            runtime,
        };

        endpoint.start_nat_detection(config.stun_timeout);
        if endpoint.runtime.is_some() {
            endpoint.start_high_level_tasks();
            for addr in config.bootstrap {
                endpoint.add_bootstrap(addr).await?;
            }
        }

        Ok(endpoint)
    }

    pub fn node_id(&self) -> PeerId {
        self.node_id.clone()
    }

    pub fn peer_id(&self) -> PeerId {
        self.runtime
            .as_ref()
            .map(|rt| rt.peer_id.clone())
            .unwrap_or_else(|| self.node_id.clone())
    }

    pub fn addr(&self) -> Option<SocketAddr> {
        self.quinn_endpoint.local_addr().ok()
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.addr()
    }

    pub fn nat_info(&self) -> NatInfo {
        self.nat_info.read().clone()
    }

    pub fn known_peers(&self) -> Vec<PeerInfo> {
        self.runtime
            .as_ref()
            .map(|rt| rt.peers.iter().map(|v| v.value().clone()).collect())
            .unwrap_or_default()
    }

    pub fn routes(&self, peer_id: PeerId) -> Vec<Route> {
        self.runtime
            .as_ref()
            .and_then(|rt| rt.routes.route(&peer_id))
            .unwrap_or_default()
    }

    pub async fn add_bootstrap(&self, addr: SocketAddr) -> crate::Result<PeerId> {
        let rt = self.runtime()?;
        if let Some(peer) = rt
            .peers
            .iter()
            .find(|entry| entry.value().addrs.contains(&addr))
            .map(|entry| entry.value().peer_id.clone())
        {
            return Ok(peer);
        }

        let mut last_err = None;
        for _ in 0..2 {
            match self.add_bootstrap_once(addr).await {
                Ok(peer_id) => return Ok(peer_id),
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("bootstrap failed")))
    }

    async fn add_bootstrap_once(&self, addr: SocketAddr) -> crate::Result<PeerId> {
        let rt = self.runtime()?;
        let conn = tokio::time::timeout(Duration::from_secs(30), self.connect_addr(addr))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "bootstrap connect timed out")
            })??;
        let reply = match self
            .control_round_trip_on_conn(
                &conn,
                StreamFrame::HelloRequest {
                    src: rt.peer_id.clone(),
                },
            )
            .await
        {
            Ok(reply) => reply,
            Err(e) => {
                conn.quinn().close(0u32.into(), b"bootstrap failed");
                return Err(e);
            }
        };
        let StreamFrame::HelloReply(hello) = reply else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bootstrap peer returned unexpected control frame",
            ));
        };

        let peer_id = hello.peer.peer_id.clone();
        self.upsert_peer(
            PeerInfo {
                is_direct: true,
                addrs: vec![addr],
                ..hello.peer
            },
            RouteKey::new(Protocol::UDP, addr),
            0,
        );
        rt.connections.insert(peer_id.clone(), conn.clone());
        self.start_connection_tasks(conn);
        self.ingest_route_entries(hello.peers, RouteKey::new(Protocol::UDP, addr))
            .await?;
        Ok(peer_id)
    }

    pub async fn send_to(&self, peer_id: PeerId, payload: &[u8]) -> crate::Result<()> {
        let rt = self.runtime()?;
        let conn = self.connection_to(peer_id.clone()).await?;
        let frame = DatagramFrame::User {
            id: rand::random(),
            src: rt.peer_id.clone(),
            dest: peer_id,
            payload: payload.to_vec(),
        };
        let data = Bytes::from(bincode::serialize(&frame).map_err(bin_err)?);
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

    pub fn try_send_to(&self, peer_id: PeerId, payload: &[u8]) -> crate::Result<()> {
        let rt = self.runtime()?;
        let conn = rt
            .connections
            .get(&peer_id)
            .filter(|conn| !conn.is_closed())
            .ok_or_else(|| io::Error::from(io::ErrorKind::WouldBlock))?;
        let frame = DatagramFrame::User {
            id: rand::random(),
            src: rt.peer_id.clone(),
            dest: peer_id,
            payload: payload.to_vec(),
        };
        conn.quinn()
            .send_datagram(Bytes::from(bincode::serialize(&frame).map_err(bin_err)?))
            .map_err(|e| io::Error::other(format!("send QUIC datagram: {e}")))
    }

    pub async fn broadcast(&self, payload: &[u8]) -> crate::Result<()> {
        for peer in self.known_peers() {
            if peer.peer_id != self.peer_id() {
                let _ = self.send_to(peer.peer_id, payload).await;
            }
        }
        Ok(())
    }

    pub async fn recv(&self) -> crate::Result<ReceivedMessage> {
        let rt = self.runtime()?;
        rt.inbox_rx
            .recv_async()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "endpoint closed"))
    }

    pub fn try_recv(&self) -> crate::Result<ReceivedMessage> {
        let rt = self.runtime()?;
        rt.inbox_rx.try_recv().map_err(|e| match e {
            flume::TryRecvError::Empty => io::Error::from(io::ErrorKind::WouldBlock),
            flume::TryRecvError::Disconnected => {
                io::Error::new(io::ErrorKind::UnexpectedEof, "endpoint closed")
            }
        })
    }

    pub async fn open_bi(
        &self,
        peer_id: PeerId,
    ) -> crate::Result<(ReliableSendStream, ReliableRecvStream)> {
        let mut last_err = None;
        for _ in 0..2 {
            match self.open_bi_once(peer_id.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    last_err = Some(e);
                    if let Ok(rt) = self.runtime() {
                        rt.connections.remove(&peer_id);
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("open reliable stream failed")))
    }

    async fn open_bi_once(
        &self,
        peer_id: PeerId,
    ) -> crate::Result<(ReliableSendStream, ReliableRecvStream)> {
        let rt = self.runtime()?;
        let route = self.route_for(peer_id.clone())?;
        let addr = self.shared_socket.register_virtual_peer(
            rt.peer_id.clone(),
            peer_id.clone(),
            rt.max_ttl,
            route.route_key().addr(),
        );
        let conn = self.connection_to_at(peer_id.clone(), addr).await?;
        let (mut send, recv) = conn.quinn().open_bi().await?;
        write_stream_frame(
            &mut send,
            &StreamFrame::User(StreamHeader {
                src: rt.peer_id.clone(),
                dest: peer_id,
            }),
        )
        .await?;
        Ok((ReliableSendStream::new(send), ReliableRecvStream::new(recv)))
    }

    pub async fn accept_bi(&self) -> crate::Result<IncomingBiStream> {
        let rt = self.runtime()?;
        rt.stream_rx
            .recv_async()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "endpoint closed"))
    }

    /// Connects to a remote peer using the low-level QUIC API.
    pub async fn connect(&self, node_addr: NodeAddr) -> crate::Result<Connection> {
        let mut last_err = None;
        for addr in &node_addr.direct_addresses {
            match self.quinn_endpoint.connect(*addr, "localhost") {
                Ok(connecting) => match connecting.await {
                    Ok(conn) => return Ok(Connection::new(conn)),
                    Err(e) => {
                        log::debug!("Handshake failed with {addr}: {e}");
                        last_err = Some(io::Error::other(format!("handshake: {e}")));
                    }
                },
                Err(e) => {
                    log::debug!("Connect failed to {addr}: {e}");
                    last_err = Some(io::Error::other(format!("connect: {e}")));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("no addresses to connect to")))
    }

    /// Accepts an incoming low-level QUIC connection.
    pub async fn accept(&self) -> Option<Connection> {
        let incoming = self.quinn_endpoint.accept().await?;
        match incoming.await {
            Ok(conn) => Some(Connection::new(conn)),
            Err(e) => {
                log::warn!("Failed to accept connection: {e}");
                None
            }
        }
    }

    pub fn accept_channel(&self) -> mpsc::Receiver<Connection> {
        let (tx, rx) = mpsc::channel(64);
        let endpoint = self.clone();
        tokio::spawn(async move {
            while let Some(conn) = endpoint.accept().await {
                if tx.send(conn).await.is_err() {
                    break;
                }
            }
        });
        rx
    }

    pub async fn send_raw(&self, buf: &[u8], addr: SocketAddr) -> crate::Result<()> {
        self.shared_socket.send_raw(buf, addr).await
    }

    pub async fn send_direct_datagram(
        &self,
        data: impl AsRef<[u8]>,
        addr: SocketAddr,
    ) -> crate::Result<()> {
        self.shared_socket.send_raw(data.as_ref(), addr).await
    }

    pub async fn recv_direct_datagram(&self) -> Option<ReceivedPacket> {
        self.direct_rx.lock().await.recv().await
    }

    pub fn try_send_raw(&self, buf: &[u8], addr: SocketAddr) -> crate::Result<()> {
        self.shared_socket.try_send_raw(buf, addr)
    }

    pub fn try_send_direct_datagram(
        &self,
        data: impl AsRef<[u8]>,
        addr: SocketAddr,
    ) -> crate::Result<()> {
        self.shared_socket.try_send_raw(data.as_ref(), addr)
    }

    pub async fn close(&self) {
        if let Some(rt) = &self.runtime {
            rt.closed.store(true, Ordering::Relaxed);
        }
        self.quinn_endpoint.close(0u32.into(), b"shutdown");
    }

    pub fn quinn(&self) -> &quinn::Endpoint {
        &self.quinn_endpoint
    }

    fn runtime(&self) -> crate::Result<Arc<RuntimeState>> {
        self.runtime
            .clone()
            .ok_or_else(|| io::Error::other("high-level P2P runtime is not enabled"))
    }

    fn route_for(&self, peer_id: PeerId) -> crate::Result<Route> {
        let rt = self.runtime()?;
        if let Ok(route) = rt.routes.get_route_by_id(&peer_id) {
            return Ok(route);
        }
        if let Some(peer) = rt.peers.get(&peer_id) {
            if peer.is_direct {
                if let Some(addr) = peer.addrs.first().copied() {
                    let route = Route::from_default_rt(RouteKey::new(Protocol::UDP, addr), 0);
                    rt.routes.add_route(peer_id, route);
                    return Ok(route);
                }
            }
            if let Some(relay) = &peer.relay_hint {
                if let Ok(route) = rt.routes.get_route_by_id(relay) {
                    rt.routes.add_route(
                        peer_id.clone(),
                        Route::from_default_rt(route.route_key(), route.metric().saturating_add(1)),
                    );
                    return rt.routes.get_route_by_id(&peer_id);
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "peer route not found",
        ))
    }

    async fn connection_to(&self, peer_id: PeerId) -> crate::Result<Connection> {
        let rt = self.runtime()?;
        let route = self.route_for(peer_id.clone())?;
        let addr = self.shared_socket.register_virtual_peer(
            rt.peer_id.clone(),
            peer_id.clone(),
            rt.max_ttl,
            route.route_key().addr(),
        );
        self.connection_to_at(peer_id, addr).await
    }

    async fn connection_to_at(
        &self,
        peer_id: PeerId,
        addr: SocketAddr,
    ) -> crate::Result<Connection> {
        let rt = self.runtime()?;
        if let Some(conn) = rt.connections.get(&peer_id) {
            if !conn.is_closed() {
                return Ok(conn.clone());
            }
        }
        let conn = self
            .connect(NodeAddr::new(peer_id.clone(), vec![addr]))
            .await?;
        rt.connections.insert(peer_id, conn.clone());
        self.start_connection_tasks(conn.clone());
        Ok(conn)
    }

    async fn connect_addr(&self, addr: SocketAddr) -> crate::Result<Connection> {
        match self.quinn_endpoint.connect(addr, "localhost") {
            Ok(connecting) => connecting
                .await
                .map(Connection::new)
                .map_err(|e| io::Error::other(format!("handshake: {e}"))),
            Err(e) => Err(io::Error::other(format!("connect: {e}"))),
        }
    }

    async fn control_round_trip_on_conn(
        &self,
        conn: &Connection,
        frame: StreamFrame,
    ) -> crate::Result<StreamFrame> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut send, mut recv) = conn.quinn().open_bi().await?;
            write_stream_frame(&mut send, &frame).await?;
            read_stream_frame(&mut recv).await
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "control stream timed out"))?
    }

    async fn control_round_trip(
        &self,
        peer_id: PeerId,
        frame: StreamFrame,
    ) -> crate::Result<StreamFrame> {
        let conn = self.connection_to(peer_id).await?;
        self.control_round_trip_on_conn(&conn, frame).await
    }

    fn start_high_level_tasks(&self) {
        self.start_packet_dispatcher();
        self.start_accept_loop();
        self.start_maintenance_loop();
    }

    fn start_packet_dispatcher(&self) {
        let endpoint = self.clone();
        tokio::spawn(async move {
            loop {
                if endpoint
                    .runtime
                    .as_ref()
                    .is_some_and(|rt| rt.closed.load(Ordering::Relaxed))
                {
                    break;
                }
                let packet = {
                    let mut rx = endpoint.direct_rx.lock().await;
                    rx.recv().await
                };
                let Some(packet) = packet else { break };
                if let Err(e) = endpoint.handle_direct_packet(packet).await {
                    log::debug!("high-level packet handling failed: {e}");
                }
            }
        });
    }

    async fn handle_direct_packet(&self, received: ReceivedPacket) -> crate::Result<()> {
        let rt = self.runtime()?;
        let packet = Packet::parse(received.data.to_vec())?;
        if packet.protocol()? != ProtocolType::QuicRelay {
            return Ok(());
        }
        let src = packet.src();
        if src == rt.peer_id || src.is_unspecified() {
            return Ok(());
        }

        let route_key = RouteKey::new(Protocol::UDP, received.addr);
        let metric = packet.max_ttl().saturating_sub(packet.ttl());
        let relay_hint = if metric > 0 {
            rt.routes.route_to_id(&route_key)
        } else {
            None
        };
        self.upsert_peer(
            PeerInfo {
                peer_id: src.clone(),
                addrs: if metric == 0 {
                    vec![received.addr]
                } else {
                    Vec::new()
                },
                relay_hint,
                last_seen: now_millis(),
                is_direct: metric == 0,
            },
            route_key,
            metric,
        );

        let dest = packet.dest();
        if dest != rt.peer_id && !dest.is_broadcast() {
            self.forward_packet(packet).await?;
            return Ok(());
        }

        let next_hop = self
            .route_for(src.clone())
            .map(|route| route.route_key().addr())
            .unwrap_or(received.addr);
        let virtual_addr =
            self.shared_socket
                .register_virtual_peer(rt.peer_id.clone(), src, rt.max_ttl, next_hop);
        self.shared_socket
            .inject_routed_quic(Bytes::copy_from_slice(packet.payload()), virtual_addr);
        Ok(())
    }

    async fn ingest_route_entries(
        &self,
        entries: Vec<RouteEntry>,
        via: RouteKey,
    ) -> crate::Result<()> {
        let rt = self.runtime()?;
        for item in entries {
            if item.peer.peer_id == rt.peer_id || item.peer.peer_id.is_unspecified() {
                continue;
            }
            let metric = item.metric.saturating_add(1);
            self.upsert_peer(item.peer.clone(), via, metric);
        }
        Ok(())
    }

    async fn discovery_entries(&self, requester: &PeerId) -> crate::Result<Vec<RouteEntry>> {
        let rt = self.runtime()?;
        let self_peer = self.self_peer_info().await;
        let peers = std::iter::once(RouteEntry {
            peer: self_peer.clone(),
            metric: 0,
        })
        .chain(rt.peers.iter().filter_map(|entry| {
            let peer = entry.value().clone();
            if &peer.peer_id == requester {
                None
            } else {
                let metric = rt
                    .routes
                    .route_one(&peer.peer_id)
                    .map(|route| route.metric().saturating_add(1))
                    .unwrap_or(1);
                Some(RouteEntry { peer, metric })
            }
        }))
        .collect();
        Ok(peers)
    }

    async fn hello_payload(&self, requester: &PeerId) -> crate::Result<HelloPayload> {
        Ok(HelloPayload {
            peer: self.self_peer_info().await,
            peers: self.discovery_entries(requester).await?,
        })
    }

    async fn query_routes(&self, peer_id: PeerId) -> crate::Result<()> {
        let rt = self.runtime()?;
        let via = self.route_for(peer_id.clone())?.route_key();
        let reply = self
            .control_round_trip(
                peer_id,
                StreamFrame::RouteQuery {
                    src: rt.peer_id.clone(),
                },
            )
            .await?;
        let StreamFrame::RouteReply(reply) = reply else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "peer returned unexpected route control frame",
            ));
        };
        self.ingest_route_entries(reply.peers, via).await
    }

    async fn forward_packet(&self, mut packet: Packet) -> crate::Result<()> {
        if !packet.decrement_ttl() {
            return Ok(());
        }
        let route = self.route_for(packet.dest())?;
        self.shared_socket
            .send_raw(packet.as_bytes(), route.route_key().addr())
            .await
    }

    fn upsert_peer(&self, peer: PeerInfo, route_key: RouteKey, metric: u8) {
        let Ok(rt) = self.runtime() else { return };
        if peer.peer_id == rt.peer_id {
            return;
        }
        let mut merged = peer.clone();
        merged.last_seen = now_millis();
        if let Some(existing) = rt.peers.get(&peer.peer_id) {
            for addr in &existing.addrs {
                if !merged.addrs.contains(addr) {
                    merged.addrs.push(*addr);
                }
            }
            merged.is_direct |= existing.is_direct;
            if merged.relay_hint.is_none() {
                merged.relay_hint = existing.relay_hint.clone();
            }
        }
        if metric == 0 {
            merged.is_direct = true;
            if !merged.addrs.contains(&route_key.addr()) {
                merged.addrs.push(route_key.addr());
            }
        }
        rt.routes.add_route(
            merged.peer_id.clone(),
            Route::from_default_rt(route_key, metric),
        );
        rt.peers.insert(merged.peer_id.clone(), merged);
    }

    fn start_accept_loop(&self) {
        let endpoint = self.clone();
        tokio::spawn(async move {
            while let Some(conn) = endpoint.accept().await {
                if endpoint
                    .runtime
                    .as_ref()
                    .is_some_and(|rt| rt.closed.load(Ordering::Relaxed))
                {
                    break;
                }
                endpoint.start_connection_tasks(conn);
            }
        });
    }

    fn start_connection_tasks(&self, conn: Connection) {
        let Ok(rt) = self.runtime() else { return };
        let stable_id = conn.quinn().stable_id();
        if rt.connection_tasks.insert(stable_id, ()).is_some() {
            return;
        }

        let stream_endpoint = self.clone();
        let stream_conn = conn.clone();
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
        });

        let datagram_endpoint = self.clone();
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
        });
    }

    async fn handle_incoming_stream(
        &self,
        remote_addr: SocketAddr,
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
    ) -> crate::Result<()> {
        let rt = self.runtime()?;
        let frame = read_stream_frame(&mut recv).await?;
        match frame {
            StreamFrame::User(header) => {
                if header.dest != rt.peer_id {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "stream destination peer mismatch",
                    ));
                }
                let (route, metric) = self.route_from_remote(&header.src, remote_addr)?;
                self.upsert_peer(
                    PeerInfo {
                        peer_id: header.src.clone(),
                        addrs: if metric == 0 {
                            vec![remote_addr]
                        } else {
                            Vec::new()
                        },
                        relay_hint: None,
                        last_seen: now_millis(),
                        is_direct: metric == 0,
                    },
                    route.route_key(),
                    metric,
                );
                let incoming = IncomingBiStream {
                    peer_id: header.src,
                    remote_addr,
                    is_relay: metric > 0,
                    send: ReliableSendStream::new(send),
                    recv: ReliableRecvStream::new(recv),
                };
                let _ = rt.stream_tx.send_async(incoming).await;
            }
            StreamFrame::HelloRequest { src } => {
                let (route, metric) = self.route_from_remote(&src, remote_addr)?;
                self.upsert_peer(
                    PeerInfo {
                        peer_id: src.clone(),
                        addrs: if metric == 0 {
                            vec![remote_addr]
                        } else {
                            Vec::new()
                        },
                        relay_hint: None,
                        last_seen: now_millis(),
                        is_direct: metric == 0,
                    },
                    route.route_key(),
                    metric,
                );
                let payload = self.hello_payload(&src).await?;
                write_stream_frame(&mut send, &StreamFrame::HelloReply(payload)).await?;
            }
            StreamFrame::RouteQuery { src } => {
                let (route, metric) = self.route_from_remote(&src, remote_addr)?;
                self.upsert_peer(
                    PeerInfo {
                        peer_id: src.clone(),
                        addrs: if metric == 0 {
                            vec![remote_addr]
                        } else {
                            Vec::new()
                        },
                        relay_hint: None,
                        last_seen: now_millis(),
                        is_direct: metric == 0,
                    },
                    route.route_key(),
                    metric,
                );
                let payload = RouteReplyPayload {
                    peers: self.discovery_entries(&src).await?,
                };
                write_stream_frame(&mut send, &StreamFrame::RouteReply(payload)).await?;
            }
            StreamFrame::HelloReply(_) | StreamFrame::RouteReply(_) => {}
        }
        Ok(())
    }

    async fn handle_incoming_datagram(
        &self,
        remote_addr: SocketAddr,
        data: Bytes,
    ) -> crate::Result<()> {
        let rt = self.runtime()?;
        let frame: DatagramFrame = bincode::deserialize(&data).map_err(bin_err)?;
        match frame {
            DatagramFrame::User {
                id,
                src,
                dest,
                payload,
            } => {
                if dest != rt.peer_id {
                    return Ok(());
                }
                if rt.seen_datagrams.insert((src.clone(), id), ()).is_some() {
                    return Ok(());
                }
                let (route, metric) = self.route_from_remote(&src, remote_addr)?;
                let _ = rt
                    .inbox_tx
                    .send_async(ReceivedMessage {
                        payload: Bytes::from(payload),
                        src,
                        dest,
                        route: route.route_key(),
                        ttl: rt.max_ttl.saturating_sub(metric),
                        max_ttl: rt.max_ttl,
                        is_relay: metric > 0,
                        is_broadcast: false,
                    })
                    .await;
            }
        }
        Ok(())
    }

    fn route_from_remote(
        &self,
        peer_id: &PeerId,
        remote_addr: SocketAddr,
    ) -> crate::Result<(Route, u8)> {
        if is_virtual_addr(remote_addr) {
            let route = self.route_for(peer_id.clone())?;
            let metric = route.metric();
            Ok((route, metric))
        } else {
            Ok((
                Route::from_default_rt(RouteKey::new(Protocol::UDP, remote_addr), 0),
                0,
            ))
        }
    }

    async fn self_peer_info(&self) -> PeerInfo {
        let rt = self.runtime().expect("runtime exists");
        let addrs = self.addr().into_iter().collect();
        PeerInfo {
            peer_id: rt.peer_id.clone(),
            addrs,
            relay_hint: None,
            last_seen: now_millis(),
            is_direct: true,
        }
    }

    fn start_maintenance_loop(&self) {
        let endpoint = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                let Ok(rt) = endpoint.runtime() else { break };
                if rt.closed.load(Ordering::Relaxed) {
                    break;
                }
                for peer in endpoint.known_peers() {
                    if peer.peer_id != rt.peer_id {
                        let _ = endpoint.query_routes(peer.peer_id).await;
                    }
                }
            }
        });
    }

    fn start_nat_detection(&self, timeout: Duration) {
        let stun_servers = self.stun_servers.clone();
        if stun_servers.is_empty() {
            return;
        }
        let nat_info = self.nat_info.clone();

        tokio::spawn(async move {
            loop {
                match rust_p2p_core::stun::stun_test_nat(stun_servers.clone(), None).await {
                    Ok(result) => {
                        let mut info = nat_info.write();
                        info.nat_type = result.nat_type;
                        info.public_ips = result.public_ipv4;
                        info.ipv6 = result.public_ipv6;
                        info.public_udp_ports = result.public_udp_ports;
                        info.public_port_range = result.port_range;
                    }
                    Err(e) => {
                        log::debug!("NAT detection failed: {e}");
                    }
                }
                tokio::time::sleep(timeout).await;
            }
        });
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
    write_frame(send, &bincode::serialize(frame).map_err(bin_err)?).await
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
    bincode::deserialize(&read_frame(recv).await?).map_err(bin_err)
}

fn is_virtual_addr(addr: SocketAddr) -> bool {
    matches!(addr.ip(), IpAddr::V4(ip) if ip.octets()[0] == 127 && ip.octets()[1] == 255)
}

fn bin_err(err: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("bincode: {err}"))
}

impl Clone for Endpoint {
    fn clone(&self) -> Self {
        Self {
            quinn_endpoint: self.quinn_endpoint.clone(),
            node_id: self.node_id.clone(),
            stun_servers: self.stun_servers.clone(),
            nat_info: self.nat_info.clone(),
            shared_socket: self.shared_socket.clone(),
            direct_rx: self.direct_rx.clone(),
            runtime: self.runtime.clone(),
        }
    }
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("peer_id", &self.peer_id())
            .field("addr", &self.addr())
            .finish()
    }
}
