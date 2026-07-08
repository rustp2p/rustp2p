use crate::config::Config;
use crate::protocol::ProtocolLayer;
use crate::quic::QuicEndpoint;
use crate::transport::{LinkInfo, LinkMode, PeerInfo, TransportHandle, TransportLayer};
use crate::{Identity, NatInfo, PeerId};
use rust_p2p_core::endpoint::LoadBalance;
use rust_p2p_core::route_table::Route;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::Arc;

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

    pub fn enable_tcp(mut self, enabled: bool) -> Self {
        self.config.enable_tcp = enabled;
        self
    }

    pub fn tcp_port(mut self, port: u16) -> Self {
        self.config.tcp_port = Some(port);
        self
    }

    pub fn mapping_udp_addrs(mut self, addrs: Vec<SocketAddr>) -> Self {
        self.config.mapping_udp_addrs = addrs;
        self
    }

    pub fn mapping_tcp_addrs(mut self, addrs: Vec<SocketAddr>) -> Self {
        self.config.mapping_tcp_addrs = addrs;
        self
    }

    pub fn load_balance(mut self, load_balance: LoadBalance) -> Self {
        self.config.load_balance = load_balance;
        self
    }

    pub fn punch_whitelist(mut self, peers: Vec<PeerId>) -> Self {
        self.config.punch_whitelist = peers;
        self
    }

    pub fn nat_observers(mut self, peers: Vec<PeerId>) -> Self {
        self.config.nat_observers = peers;
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

/// The public PeerId-based QUIC endpoint.
#[derive(Clone)]
pub struct Endpoint {
    node_id: PeerId,
    transport: Arc<TransportLayer>,
    protocol: Arc<ProtocolLayer>,
    quic: Arc<QuicEndpoint>,
}

impl Endpoint {
    pub fn builder() -> Builder {
        Builder::new()
    }

    /// Binds an endpoint on the given address.
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

    pub fn node_id(&self) -> PeerId {
        self.node_id.clone()
    }

    pub fn peer_id(&self) -> PeerId {
        self.node_id.clone()
    }

    pub fn addr(&self) -> Option<SocketAddr> {
        Some(self.transport.local_addr())
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.addr()
    }

    pub fn nat_info(&self) -> NatInfo {
        self.protocol.nat_info()
    }

    pub fn known_peers(&self) -> Vec<PeerInfo> {
        self.transport.known_peers()
    }

    pub fn routes(&self, peer_id: PeerId) -> Vec<Route> {
        self.transport.routes(peer_id)
    }

    pub fn transport(&self) -> TransportHandle {
        TransportHandle::new(self.transport.clone())
    }

    pub fn link_mode(&self, peer_id: PeerId) -> Option<LinkMode> {
        self.transport.link_mode(peer_id)
    }

    pub fn link_info(&self, peer_id: PeerId) -> Option<LinkInfo> {
        self.transport.link_info(peer_id)
    }

    pub async fn add_bootstrap(&self, addr: SocketAddr) -> crate::Result<PeerId> {
        self.protocol.add_bootstrap(addr).await
    }

    pub async fn send_to(&self, peer_id: PeerId, payload: &[u8]) -> crate::Result<()> {
        self.quic.send_to(peer_id, payload).await
    }

    pub fn try_send_to(&self, peer_id: PeerId, payload: &[u8]) -> crate::Result<()> {
        self.quic.try_send_to(peer_id, payload)
    }

    pub async fn broadcast(&self, payload: &[u8]) -> crate::Result<()> {
        for peer in self.known_peers() {
            if peer.peer_id != self.peer_id() {
                let _ = self.send_to(peer.peer_id, payload).await;
            }
        }
        Ok(())
    }

    pub async fn recv(&self) -> crate::Result<crate::ReceivedMessage> {
        self.quic.recv().await
    }

    pub fn try_recv(&self) -> crate::Result<crate::ReceivedMessage> {
        self.quic.try_recv()
    }

    pub async fn open_bi(
        &self,
        peer_id: PeerId,
    ) -> crate::Result<(crate::ReliableSendStream, crate::ReliableRecvStream)> {
        self.quic.open_bi(peer_id).await
    }

    pub async fn accept_bi(&self) -> crate::Result<crate::IncomingBiStream> {
        self.quic.accept_bi().await
    }

    pub fn allow_punch(&self, peer_id: PeerId) {
        self.protocol.allow_punch(peer_id);
    }

    pub fn deny_punch(&self, peer_id: PeerId) {
        self.protocol.deny_punch(&peer_id);
    }

    pub fn set_punch_whitelist(&self, peers: Vec<PeerId>) {
        self.protocol.set_punch_whitelist(peers);
    }

    pub fn punch_whitelist(&self) -> Vec<PeerId> {
        self.protocol.punch_whitelist()
    }

    pub async fn punch(&self, peer_id: PeerId) -> crate::Result<()> {
        self.protocol.punch(peer_id).await
    }

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
