use crate::config::Config;
use crate::protocol::ProtocolLayer;
use crate::quic::QuicEndpoint;
use crate::transport::{LinkInfo, LinkMode, PeerInfo, TransportHandle, TransportLayer};
use crate::{Identity, NatInfo, PeerId};
use rustp2p_core::endpoint::LoadBalance;
use rustp2p_core::route_table::Route;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::Arc;

/// Builder for the high-level PeerId-based QUIC endpoint.
///
/// Bootstrap configuration accepts real socket addresses only for initial
/// reachability. After discovery, application data APIs use `PeerId`.
pub struct Builder {
    config: Config,
}

impl Builder {
    /// Creates a builder with high-level P2P tasks enabled.
    pub fn new() -> Self {
        let config = Config {
            high_level: true,
            ..Default::default()
        };
        Self { config }
    }

    /// Sets the local identity.
    ///
    /// The identity supplies the public `PeerId` and the deterministic QUIC
    /// certificate material.
    pub fn identity(mut self, identity: Identity) -> Self {
        self.config.identity = Some(identity);
        self
    }

    /// Sets the local core transport bind address.
    pub fn bind(mut self, addr: SocketAddr) -> Self {
        self.config.bind_addr = addr;
        self
    }

    /// Sets initial directly reachable bootstrap addresses.
    ///
    /// These addresses are only entry points. The endpoint learns remote
    /// `PeerId`s through the protocol hello exchange.
    pub fn bootstrap(mut self, peers: Vec<SocketAddr>) -> Self {
        self.config.bootstrap = peers;
        self
    }

    /// Installs the QUIC certificate verifier used for server and client certs.
    pub fn certificate_verifier(mut self, verifier: Arc<dyn crate::CertificateVerifier>) -> Self {
        self.config.certificate_verifier = verifier;
        self
    }

    /// Sets the maximum overlay forwarding TTL.
    pub fn max_ttl(mut self, max_ttl: u8) -> Self {
        self.config.max_ttl = max_ttl.max(1);
        self
    }

    /// Sets STUN servers used only for NAT type and port-range maintenance.
    pub fn stun_servers(mut self, servers: Vec<String>) -> Self {
        self.config.stun_servers = servers;
        self
    }

    /// Enables or disables the underlying core TCP transport.
    pub fn enable_tcp(mut self, enabled: bool) -> Self {
        self.config.enable_tcp = enabled;
        self
    }

    /// Sets the local TCP port used by the underlying core transport.
    pub fn tcp_port(mut self, port: u16) -> Self {
        self.config.tcp_port = Some(port);
        self
    }

    /// Sets externally mapped UDP addresses known by configuration.
    pub fn mapping_udp_addrs(mut self, addrs: Vec<SocketAddr>) -> Self {
        self.config.mapping_udp_addrs = addrs;
        self
    }

    /// Sets externally mapped TCP addresses known by configuration.
    pub fn mapping_tcp_addrs(mut self, addrs: Vec<SocketAddr>) -> Self {
        self.config.mapping_tcp_addrs = addrs;
        self
    }

    /// Sets route selection policy when multiple confirmed routes exist.
    pub fn load_balance(mut self, load_balance: LoadBalance) -> Self {
        self.config.load_balance = load_balance;
        self
    }

    /// Sets the initial peers allowed to participate in hole punching.
    pub fn punch_whitelist(mut self, peers: Vec<PeerId>) -> Self {
        self.config.punch_whitelist = peers;
        self
    }

    /// Restricts NAT observation to selected direct peers.
    ///
    /// An empty list means any currently direct known peer may act as observer.
    pub fn nat_observers(mut self, peers: Vec<PeerId>) -> Self {
        self.config.nat_observers = peers;
        self
    }

    /// Sets the maximum assistant UDP sockets used after symmetric NAT detection.
    ///
    /// The protocol layer detects the local `NatType`; transport then applies
    /// that external result to core. `0` disables assistant sockets.
    pub fn max_assistant_sockets(mut self, max: usize) -> Self {
        self.config.max_assistant_sockets = max;
        self
    }

    /// Builds and starts the endpoint.
    ///
    /// Bootstrap addresses are contacted before this method returns.
    pub async fn build(self) -> crate::Result<Endpoint> {
        Endpoint::bind_with_config(self.config).await
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

/// The public PeerId-based QUIC endpoint.
///
/// User payloads always travel over QUIC: [`send_to`](Self::send_to) uses QUIC
/// DATAGRAM frames, and [`open_bi`](Self::open_bi) opens a bidirectional QUIC
/// stream. The path below QUIC may be direct or relayed, but callers address
/// peers only by `PeerId`.
#[derive(Clone)]
pub struct Endpoint {
    node_id: PeerId,
    transport: Arc<TransportLayer>,
    protocol: Arc<ProtocolLayer>,
    quic: Arc<QuicEndpoint>,
}

impl Endpoint {
    /// Creates a high-level endpoint builder.
    pub fn builder() -> Builder {
        Builder::new()
    }

    /// Binds an endpoint on the given address with a generated identity.
    ///
    /// Prefer [`builder`](Self::builder) when the peer id must be stable.
    pub async fn bind(bind_addr: SocketAddr) -> crate::Result<Self> {
        Self::bind_with_config(Config {
            bind_addr,
            high_level: true,
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
        let transport = TransportLayer::bind(node_id.clone(), &config).await?;
        let initial_nat_info = initial_nat_info(&config, &transport);
        let protocol = ProtocolLayer::new(
            node_id.clone(),
            transport.clone(),
            config.max_ttl,
            config.punch_whitelist.clone(),
            config.nat_observers.clone(),
            initial_nat_info,
        );
        protocol.start_nat_maintenance(config.stun_timeout, config.stun_servers.clone());
        let quic = QuicEndpoint::bind(identity, &config, protocol.clone()).await?;

        let endpoint = Self {
            node_id,
            transport,
            protocol,
            quic,
        };

        for addr in config.bootstrap {
            endpoint.add_bootstrap(addr).await?;
        }

        Ok(endpoint)
    }

    /// Returns the local node id.
    ///
    /// This is an alias for [`peer_id`](Self::peer_id).
    pub fn node_id(&self) -> PeerId {
        self.node_id.clone()
    }

    /// Returns the local application-level peer id.
    pub fn peer_id(&self) -> PeerId {
        self.node_id.clone()
    }

    /// Returns the local core transport address.
    ///
    /// This address is useful for bootstrap but is not used by high-level send
    /// APIs, which take `PeerId`.
    pub fn addr(&self) -> Option<SocketAddr> {
        Some(self.transport.local_addr())
    }

    /// Returns the local core transport address.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.addr()
    }

    /// Returns the latest local NAT information maintained by the protocol layer.
    pub fn nat_info(&self) -> NatInfo {
        self.protocol.nat_info()
    }

    /// Returns peers discovered by hello, route exchange, or control traffic.
    pub fn known_peers(&self) -> Vec<PeerInfo> {
        self.transport.known_peers()
    }

    /// Returns confirmed routes for a peer.
    ///
    /// Candidate routes are intentionally hidden until protocol control traffic
    /// confirms reachability.
    pub fn routes(&self, peer_id: PeerId) -> Vec<Route> {
        self.transport.routes(peer_id)
    }

    /// Returns a low-level transport handle for diagnostics or control tooling.
    ///
    /// Application data should normally use [`send_to`](Self::send_to) or
    /// [`open_bi`](Self::open_bi) so it stays inside QUIC encryption.
    pub fn transport(&self) -> TransportHandle {
        TransportHandle::new(self.transport.clone())
    }

    /// Returns whether the current best confirmed route is direct or relayed.
    pub fn link_mode(&self, peer_id: PeerId) -> Option<LinkMode> {
        self.transport.link_mode(peer_id)
    }

    /// Returns a snapshot of current confirmed route metadata for a peer.
    pub fn link_info(&self, peer_id: PeerId) -> Option<LinkInfo> {
        self.transport.link_info(peer_id)
    }

    /// Adds a bootstrap address and returns the discovered peer id.
    ///
    /// The real address is used only for the initial hello exchange. Subsequent
    /// communication should target the returned `PeerId`.
    pub async fn add_bootstrap(&self, addr: SocketAddr) -> crate::Result<PeerId> {
        self.protocol.add_bootstrap(addr).await
    }

    /// Sends an encrypted unreliable user datagram to a peer.
    ///
    /// The message is encoded as a QUIC DATAGRAM on the end-to-end QUIC
    /// connection. It is not a raw rustp2p control packet.
    pub async fn send_to(&self, peer_id: PeerId, payload: &[u8]) -> crate::Result<()> {
        self.quic.send_to(peer_id, payload).await
    }

    /// Attempts to send an encrypted QUIC DATAGRAM without waiting.
    ///
    /// Returns `WouldBlock` when no live QUIC connection is currently cached.
    pub fn try_send_to(&self, peer_id: PeerId, payload: &[u8]) -> crate::Result<()> {
        self.quic.try_send_to(peer_id, payload)
    }

    /// Sends an encrypted QUIC DATAGRAM to every currently known peer.
    ///
    /// Individual peer send failures are ignored so one bad route does not stop
    /// the rest of the broadcast.
    pub async fn broadcast(&self, payload: &[u8]) -> crate::Result<()> {
        for peer in self.known_peers() {
            if peer.peer_id != self.peer_id() {
                let _ = self.send_to(peer.peer_id, payload).await;
            }
        }
        Ok(())
    }

    /// Receives the next decrypted QUIC DATAGRAM user message.
    pub async fn recv(&self) -> crate::Result<crate::ReceivedMessage> {
        self.quic.recv().await
    }

    /// Attempts to receive a decrypted QUIC DATAGRAM user message without waiting.
    pub fn try_recv(&self) -> crate::Result<crate::ReceivedMessage> {
        self.quic.try_recv()
    }

    /// Opens an end-to-end bidirectional QUIC stream to a peer.
    pub async fn open_bi(
        &self,
        peer_id: PeerId,
    ) -> crate::Result<(crate::ReliableSendStream, crate::ReliableRecvStream)> {
        self.quic.open_bi(peer_id).await
    }

    /// Accepts the next inbound end-to-end bidirectional QUIC stream.
    ///
    /// Relay nodes that only forward encrypted QUIC packets do not receive an
    /// `IncomingBiStream` for that forwarded traffic.
    pub async fn accept_bi(&self) -> crate::Result<crate::IncomingBiStream> {
        self.quic.accept_bi().await
    }

    /// Allows the peer to participate in direct hole punching.
    pub fn allow_punch(&self, peer_id: PeerId) {
        self.protocol.allow_punch(peer_id);
    }

    /// Removes the peer from the hole punching allow list.
    pub fn deny_punch(&self, peer_id: PeerId) {
        self.protocol.deny_punch(&peer_id);
    }

    /// Replaces the hole punching allow list.
    pub fn set_punch_whitelist(&self, peers: Vec<PeerId>) {
        self.protocol.set_punch_whitelist(peers);
    }

    /// Returns the current hole punching allow list.
    pub fn punch_whitelist(&self) -> Vec<PeerId> {
        self.protocol.punch_whitelist()
    }

    /// Starts a protocol-level hole punching attempt with an allowed peer.
    pub async fn punch(&self, peer_id: PeerId) -> crate::Result<()> {
        self.protocol.punch(peer_id).await
    }

    /// Stops protocol, transport, and QUIC runtime tasks.
    pub async fn close(&self) {
        self.protocol.close();
        self.transport.close();
        self.quic.close().await;
    }
}

fn initial_nat_info(config: &Config, transport: &TransportLayer) -> NatInfo {
    let local_addr = transport.local_addr();
    let mut info = NatInfo {
        mapping_udp_addr: config.mapping_udp_addrs.clone(),
        mapping_tcp_addr: config.mapping_tcp_addrs.clone(),
        local_udp_ports: vec![local_addr.port()],
        local_tcp_port: transport
            .local_tcp_addr()
            .map(|addr| addr.port())
            .unwrap_or(0),
        ..Default::default()
    };
    if let IpAddr::V4(ip) = local_addr.ip() {
        info.local_ipv4 = ip;
    }
    info
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("peer_id", &self.peer_id())
            .field("addr", &self.addr())
            .finish()
    }
}
