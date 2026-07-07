use crate::config::Config;
use crate::connection::Connection;
use crate::demux::{ReceivedPacket, SharedUdpSocket};
use crate::protocol::{
    now_millis, Packet, ProtocolType, RouteEntry, RouteReplyPayload, TimestampPayload,
};
use crate::reliable::{ReliableRecvStream, ReliableSendStream, ReliableStream};
use crate::{GroupCode, Identity, NatInfo, PeerId};
use bytes::Bytes;
use dashmap::DashMap;
use rust_p2p_core::route_table::{Protocol, Route, RouteKey, RouteTable};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};

/// A peer node address containing peer identity and direct addresses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerAddr {
    pub peer_id: PeerId,
    pub public_key: Option<[u8; 32]>,
    pub addrs: Vec<SocketAddr>,
    pub relay_hint: Option<PeerId>,
}

impl PeerAddr {
    pub fn new(peer_id: PeerId, addrs: Vec<SocketAddr>) -> Self {
        Self {
            peer_id,
            public_key: None,
            addrs,
            relay_hint: None,
        }
    }

    pub fn with_public_key(mut self, public_key: [u8; 32]) -> Self {
        self.public_key = Some(public_key);
        self
    }

    pub fn with_relay_hint(mut self, relay: PeerId) -> Self {
        self.relay_hint = Some(relay);
        self
    }
}

/// Backwards-compatible low-level node address.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeAddr {
    pub node_id: [u8; 32],
    pub direct_addresses: Vec<SocketAddr>,
}

impl NodeAddr {
    pub fn new(node_id: [u8; 32], direct_addresses: Vec<SocketAddr>) -> Self {
        Self {
            node_id,
            direct_addresses,
        }
    }

    pub fn from_id(node_id: [u8; 32]) -> Self {
        Self {
            node_id,
            direct_addresses: Vec::new(),
        }
    }
}

impl From<PeerAddr> for NodeAddr {
    fn from(value: PeerAddr) -> Self {
        Self {
            node_id: value.peer_id.into(),
            direct_addresses: value.addrs,
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

    pub fn group(mut self, group_code: GroupCode) -> Self {
        self.config.group_code = group_code;
        self
    }

    pub fn bootstrap(mut self, peers: Vec<PeerAddr>) -> Self {
        self.config.bootstrap = peers;
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
    identity: Identity,
    peer_id: PeerId,
    group_code: GroupCode,
    max_ttl: u8,
    peers: DashMap<PeerId, PeerAddr>,
    routes: RouteTable<PeerId>,
    connections: DashMap<PeerId, Connection>,
    inbox_tx: mpsc::Sender<ReceivedMessage>,
    inbox_rx: Mutex<mpsc::Receiver<ReceivedMessage>>,
    stream_tx: mpsc::Sender<ReliableStream>,
    stream_rx: Mutex<mpsc::Receiver<ReliableStream>>,
}

/// The main QUIC endpoint for low-level QUIC and high-level P2P APIs.
pub struct Endpoint {
    quinn_endpoint: quinn::Endpoint,
    node_id: [u8; 32],
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
            ..Default::default()
        })
        .await
    }

    /// Binds a new endpoint with custom configuration.
    pub async fn bind_with_config(mut config: Config) -> crate::Result<Self> {
        use rcgen::KeyPair;
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

        let identity = match config.identity.take() {
            Some(identity) => identity,
            None => Identity::generate()?,
        };
        let node_id: [u8; 32] = identity.peer_id().into();

        let key_pair = KeyPair::generate().map_err(|e| io::Error::other(e.to_string()))?;
        let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .map_err(|e| io::Error::other(e.to_string()))?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| io::Error::other(e.to_string()))?;

        let cert_der = cert.der().clone();
        let key_der = PrivatePkcs8KeyDer::from(key_pair.serialize_der());

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
                .with_no_client_auth()
                .with_single_cert(vec![cert_der], PrivateKeyDer::Pkcs8(key_der.clone_key()))
                .map_err(|e| io::Error::other(e.to_string()))?;
        server_crypto.alpn_protocols = config.alpns.clone();

        let quic_server_config =
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
                .map_err(|e| io::Error::other(format!("quic server config: {e}")))?;

        let mut client_crypto = rustls::ClientConfig::builder_with_provider(Arc::new(key_provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| io::Error::other(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();
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

        let (inbox_tx, inbox_rx) = mpsc::channel(512);
        let (stream_tx, stream_rx) = mpsc::channel(128);
        let runtime = if config.high_level {
            Some(Arc::new(RuntimeState {
                identity: identity.clone(),
                peer_id: identity.peer_id(),
                group_code: config.group_code,
                max_ttl: config.max_ttl.max(1),
                peers: DashMap::new(),
                routes: RouteTable::new(Default::default()),
                connections: DashMap::new(),
                inbox_tx,
                inbox_rx: Mutex::new(inbox_rx),
                stream_tx,
                stream_rx: Mutex::new(stream_rx),
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
            for peer in config.bootstrap {
                endpoint.add_peer(peer).await?;
            }
        }

        Ok(endpoint)
    }

    pub fn node_id(&self) -> [u8; 32] {
        self.node_id
    }

    pub fn peer_id(&self) -> PeerId {
        self.runtime
            .as_ref()
            .map(|rt| rt.peer_id)
            .unwrap_or(PeerId(self.node_id))
    }

    pub fn group_code(&self) -> GroupCode {
        self.runtime
            .as_ref()
            .map(|rt| rt.group_code)
            .unwrap_or_default()
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

    pub fn known_peers(&self) -> Vec<PeerAddr> {
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

    pub async fn add_peer(&self, mut peer: PeerAddr) -> crate::Result<()> {
        let rt = self.runtime()?;
        if peer.public_key.is_none() && peer.peer_id == rt.peer_id {
            peer.public_key = Some(rt.identity.public_key());
        }
        self.store_peer_routes(peer.clone(), 0);
        self.send_control(peer.peer_id, ProtocolType::IDRouteQuery, &[])
            .await?;
        let _ = self
            .send_control(peer.peer_id, ProtocolType::PunchConsultRequest, &[])
            .await;
        Ok(())
    }

    pub async fn send_to(&self, peer_id: PeerId, payload: &[u8]) -> crate::Result<()> {
        self.send_control(peer_id, ProtocolType::MessageData, payload)
            .await
    }

    pub fn try_send_to(&self, peer_id: PeerId, payload: &[u8]) -> crate::Result<()> {
        let packet = self.build_packet(ProtocolType::MessageData, peer_id, payload)?;
        let route = self.route_for(peer_id)?;
        self.shared_socket
            .try_send_raw(packet.as_bytes(), route.route_key().addr())
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
        let mut rx = rt.inbox_rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "endpoint closed"))
    }

    pub fn try_recv(&self) -> crate::Result<ReceivedMessage> {
        let rt = self.runtime()?;
        let mut rx = rt
            .inbox_rx
            .try_lock()
            .map_err(|_| io::Error::from(io::ErrorKind::WouldBlock))?;
        rx.try_recv().map_err(|e| match e {
            mpsc::error::TryRecvError::Empty => io::Error::from(io::ErrorKind::WouldBlock),
            mpsc::error::TryRecvError::Disconnected => {
                io::Error::new(io::ErrorKind::UnexpectedEof, "endpoint closed")
            }
        })
    }

    pub async fn open_stream_to(&self, peer_id: PeerId) -> crate::Result<ReliableStream> {
        let mut last_err = None;
        for _ in 0..2 {
            match self.open_stream_to_once(peer_id).await {
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

    pub async fn open_bi(
        &self,
        peer_id: PeerId,
    ) -> crate::Result<(ReliableSendStream, ReliableRecvStream)> {
        Ok(self.open_stream_to(peer_id).await?.into_split())
    }

    async fn open_stream_to_once(&self, peer_id: PeerId) -> crate::Result<ReliableStream> {
        let rt = self.runtime()?;
        let route = self.route_for(peer_id)?;
        let addr = if route.metric() > 0 {
            self.shared_socket.register_virtual_peer(
                rt.peer_id,
                peer_id,
                rt.group_code,
                rt.max_ttl,
                route.route_key().addr(),
            )
        } else {
            route.route_key().addr()
        };
        let conn = self.connection_to(peer_id, addr).await?;
        let (send, recv) = conn.quinn().open_bi().await?;
        Ok(ReliableStream::new(send, recv))
    }

    pub async fn accept_stream(&self) -> crate::Result<ReliableStream> {
        let rt = self.runtime()?;
        let mut rx = rt.stream_rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "endpoint closed"))
    }

    pub async fn accept_bi(&self) -> crate::Result<(ReliableSendStream, ReliableRecvStream)> {
        Ok(self.accept_stream().await?.into_split())
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
            if let Some(addr) = peer.addrs.first().copied() {
                let route = Route::from_default_rt(RouteKey::new(Protocol::UDP, addr), 0);
                rt.routes.add_route(peer_id, route);
                return Ok(route);
            }
            if let Some(relay) = peer.relay_hint {
                if let Ok(route) = rt.routes.get_route_by_id(&relay) {
                    rt.routes.add_route(
                        peer_id,
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

    fn build_packet(
        &self,
        protocol: ProtocolType,
        dest: PeerId,
        payload: &[u8],
    ) -> crate::Result<Packet> {
        let rt = self.runtime()?;
        Packet::build(
            protocol,
            rt.group_code,
            rt.peer_id,
            dest,
            rt.max_ttl,
            payload,
        )
    }

    async fn send_control(
        &self,
        dest: PeerId,
        protocol: ProtocolType,
        payload: &[u8],
    ) -> crate::Result<()> {
        let packet = self.build_packet(protocol, dest, payload)?;
        let route = self.route_for(dest)?;
        self.shared_socket
            .send_raw(packet.as_bytes(), route.route_key().addr())
            .await
    }

    async fn connection_to(&self, peer_id: PeerId, addr: SocketAddr) -> crate::Result<Connection> {
        let rt = self.runtime()?;
        if let Some(conn) = rt.connections.get(&peer_id) {
            if !conn.is_closed() {
                return Ok(conn.clone());
            }
        }
        let conn = self
            .connect(NodeAddr::new(peer_id.into(), vec![addr]))
            .await?;
        rt.connections.insert(peer_id, conn.clone());
        Ok(conn)
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
        let src = packet.src();
        if src == rt.peer_id || src.is_unspecified() {
            return Ok(());
        }
        let route_key = RouteKey::new(Protocol::UDP, received.addr);
        let metric = packet.max_ttl().saturating_sub(packet.ttl());
        rt.routes
            .add_route(src, Route::from_default_rt(route_key, metric));
        rt.peers
            .entry(src)
            .or_insert_with(|| PeerAddr::new(src, vec![received.addr]));

        let dest = packet.dest();
        if dest != rt.peer_id && !dest.is_broadcast() {
            self.forward_packet(packet).await?;
            return Ok(());
        }

        let protocol = packet.protocol()?;
        if packet.group() != rt.group_code {
            return Ok(());
        }

        match protocol {
            ProtocolType::QuicRelay => {
                let next_hop = self
                    .route_for(src)
                    .map(|route| route.route_key().addr())
                    .unwrap_or(received.addr);
                let virtual_addr = self.shared_socket.register_virtual_peer(
                    rt.peer_id,
                    src,
                    packet.group(),
                    rt.max_ttl,
                    next_hop,
                );
                self.shared_socket
                    .inject_routed_quic(Bytes::copy_from_slice(packet.payload()), virtual_addr);
            }
            ProtocolType::MessageData | ProtocolType::RangeBroadcast => {
                let _ = rt
                    .inbox_tx
                    .send(ReceivedMessage {
                        payload: Bytes::copy_from_slice(packet.payload()),
                        src,
                        dest,
                        route: route_key,
                        ttl: packet.ttl(),
                        max_ttl: packet.max_ttl(),
                        is_relay: metric > 0,
                        is_broadcast: dest.is_broadcast(),
                    })
                    .await;
            }
            ProtocolType::IDRouteQuery => {
                let self_peer = self.self_peer_addr().await;
                let peers = std::iter::once(RouteEntry {
                    peer: self_peer,
                    metric: 0,
                })
                .chain(rt.peers.iter().filter_map(|entry| {
                    let peer = entry.value().clone();
                    if peer.peer_id == src {
                        None
                    } else {
                        let metric = rt
                            .routes
                            .route_one(&peer.peer_id)
                            .map(|route| route.metric() + 1)
                            .unwrap_or(1);
                        Some(RouteEntry { peer, metric })
                    }
                }))
                .collect();
                let payload = bincode::serialize(&RouteReplyPayload { peers }).map_err(bin_err)?;
                self.send_control(src, ProtocolType::IDRouteReply, &payload)
                    .await?;
            }
            ProtocolType::IDRouteReply => {
                let reply: RouteReplyPayload =
                    bincode::deserialize(packet.payload()).map_err(bin_err)?;
                for item in reply.peers {
                    if item.peer.peer_id == rt.peer_id {
                        continue;
                    }
                    rt.peers.insert(item.peer.peer_id, item.peer.clone());
                    rt.routes.add_route(
                        item.peer.peer_id,
                        Route::from_default_rt(route_key, item.metric.saturating_add(1)),
                    );
                }
            }
            ProtocolType::EchoRequest => {
                self.send_control(src, ProtocolType::EchoReply, packet.payload())
                    .await?;
            }
            ProtocolType::TimestampRequest => {
                self.send_control(src, ProtocolType::TimestampReply, packet.payload())
                    .await?;
            }
            ProtocolType::TimestampReply => {
                if let Ok(ts) = bincode::deserialize::<TimestampPayload>(packet.payload()) {
                    let rtt = now_millis().saturating_sub(ts.millis).min(u32::MAX as u64) as u32;
                    rt.routes
                        .add_route(src, Route::from(route_key, metric, rtt));
                }
            }
            ProtocolType::PunchConsultRequest => {
                let payload = bincode::serialize(&crate::protocol::PunchPayload {
                    peer: self.self_peer_addr().await,
                    nat_info: Some(self.nat_info()),
                })
                .map_err(bin_err)?;
                self.send_control(src, ProtocolType::PunchConsultReply, &payload)
                    .await?;
            }
            ProtocolType::PunchConsultReply => {
                if let Ok(payload) =
                    bincode::deserialize::<crate::protocol::PunchPayload>(packet.payload())
                {
                    self.store_peer_routes(payload.peer, 0);
                    self.punch_candidates(src, payload.nat_info).await?;
                }
            }
            ProtocolType::PunchRequest => {
                self.send_control(src, ProtocolType::PunchReply, &[])
                    .await?;
            }
            ProtocolType::EchoReply
            | ProtocolType::PunchReply
            | ProtocolType::ReliableRelayOpen
            | ProtocolType::ReliableRelayClose => {}
        }
        Ok(())
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

    fn store_peer_routes(&self, peer: PeerAddr, metric: u8) {
        let Ok(rt) = self.runtime() else { return };
        for addr in &peer.addrs {
            rt.routes.add_route(
                peer.peer_id,
                Route::from_default_rt(RouteKey::new(Protocol::UDP, *addr), metric),
            );
        }
        rt.peers.insert(peer.peer_id, peer);
    }

    async fn punch_candidates(
        &self,
        peer_id: PeerId,
        nat_info: Option<NatInfo>,
    ) -> crate::Result<()> {
        let Some(nat_info) = nat_info else {
            return Ok(());
        };
        let packet = self.build_packet(ProtocolType::PunchRequest, peer_id, &[])?;
        let mut candidates = nat_info.mapping_udp_addr.clone();
        candidates.extend(nat_info.local_ipv4_addrs());
        candidates.extend(nat_info.public_ipv4_addr());
        candidates.extend(nat_info.ipv6_udp_addr());
        candidates.sort();
        candidates.dedup();
        for addr in candidates {
            let _ = self.send_direct_datagram(packet.as_bytes(), addr).await;
        }
        Ok(())
    }

    fn start_accept_loop(&self) {
        let endpoint = self.clone();
        tokio::spawn(async move {
            while let Some(conn) = endpoint.accept().await {
                let endpoint = endpoint.clone();
                tokio::spawn(async move {
                    while let Ok((send, recv)) = conn.quinn().accept_bi().await {
                        let endpoint = endpoint.clone();
                        tokio::spawn(async move {
                            if let Err(e) = endpoint.handle_incoming_stream(send, recv).await {
                                log::debug!("stream handling failed: {e}");
                            }
                        });
                    }
                });
            }
        });
    }

    async fn handle_incoming_stream(
        &self,
        send: quinn::SendStream,
        recv: quinn::RecvStream,
    ) -> crate::Result<()> {
        let rt = self.runtime()?;
        let _ = rt.stream_tx.send(ReliableStream::new(send, recv)).await;
        Ok(())
    }

    async fn self_peer_addr(&self) -> PeerAddr {
        let rt = self.runtime().expect("runtime exists");
        let addrs = self.addr().into_iter().collect();
        PeerAddr::new(rt.peer_id, addrs).with_public_key(rt.identity.public_key())
    }

    fn start_maintenance_loop(&self) {
        let endpoint = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                let Ok(rt) = endpoint.runtime() else { break };
                let payload = bincode::serialize(&TimestampPayload {
                    millis: now_millis(),
                })
                .unwrap_or_default();
                for peer in endpoint.known_peers() {
                    if peer.peer_id != rt.peer_id {
                        let _ = endpoint
                            .send_control(peer.peer_id, ProtocolType::TimestampRequest, &payload)
                            .await;
                    }
                }
            }
        });
    }

    fn start_nat_detection(&self, timeout: Duration) {
        let stun_servers = self.stun_servers.clone();
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

fn bin_err(err: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("bincode: {err}"))
}

impl Clone for Endpoint {
    fn clone(&self) -> Self {
        Self {
            quinn_endpoint: self.quinn_endpoint.clone(),
            node_id: self.node_id,
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

#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
