use crate::{Config, PeerId};
use bytes::Bytes;
use dashmap::DashMap;
use rust_p2p_core::endpoint::{EndPoint, Sender as CoreSender, Transport};
use rust_p2p_core::nat::NatInfo;
use rust_p2p_core::punch::PunchInfo;
use rust_p2p_core::route_table::{Route, RouteKey, RouteTable};
use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Read-only peer information discovered by the protocol layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Application-level peer id.
    pub peer_id: PeerId,
    /// Direct socket addresses observed or advertised for this peer.
    pub addrs: Vec<SocketAddr>,
    /// Peer id of a relay that may currently reach this peer.
    pub relay_hint: Option<PeerId>,
    /// Last update time in milliseconds since the Unix epoch.
    pub last_seen: u64,
    /// Whether the peer currently has confirmed direct reachability.
    pub is_direct: bool,
    /// Latest NAT information learned for this peer, if any.
    pub nat_info: Option<NatInfo>,
}

impl PeerInfo {
    /// Creates peer information without any confirmed address.
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            addrs: Vec::new(),
            relay_hint: None,
            last_seen: now_millis(),
            is_direct: false,
            nat_info: None,
        }
    }

    /// Adds a direct address and marks the peer as direct.
    pub fn with_addr(mut self, addr: SocketAddr) -> Self {
        self.addrs.push(addr);
        self.is_direct = true;
        self
    }
}

/// Low-level protocol/control message metadata.
///
/// User-facing encrypted datagrams are delivered by `Endpoint::recv()`, not this
/// type. This remains for low-level diagnostics and control-plane escape hatches.
#[derive(Clone, Debug)]
pub struct TransportMessage {
    /// Raw protocol/control payload bytes.
    pub payload: Bytes,
    /// Source peer id encoded by the protocol packet.
    pub src: PeerId,
    /// Destination peer id encoded by the protocol packet.
    pub dest: PeerId,
    /// Core route used by this packet.
    pub route: RouteKey,
    /// Remaining packet TTL after forwarding.
    pub ttl: u8,
    /// Initial packet TTL.
    pub max_ttl: u8,
    /// Whether this packet has traversed at least one relay hop.
    pub is_relay: bool,
}

/// Current best confirmed link mode for a peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkMode {
    /// The best confirmed route is a direct transport route.
    Direct,
    /// The best confirmed route reaches the peer through another node.
    Relay,
}

/// Snapshot of the current best confirmed route for a peer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkInfo {
    /// Peer described by this route snapshot.
    pub peer_id: PeerId,
    /// Direct or relayed mode for the selected route.
    pub mode: LinkMode,
    /// Route hop metric. `0` means direct; larger values mean relayed.
    pub metric: u8,
}

#[derive(Clone, Debug)]
pub(crate) struct TransportPayload {
    pub src: PeerId,
    pub payload: Bytes,
}

#[derive(Clone, Debug)]
pub(crate) struct RawTransportPacket {
    pub data: Bytes,
    pub route_key: RouteKey,
}

enum CoreOutboundPacket {
    PeerPacket { dest: PeerId, data: Bytes },
    RoutePacket { route_key: RouteKey, data: Bytes },
}

enum CoreControl {
    ApplyNatModel {
        nat_type: rust_p2p_core::nat::NatType,
        reply: oneshot::Sender<io::Result<()>>,
    },
}

struct CoreTransportLayer {
    sender: CoreSender,
    puncher: rust_p2p_core::punch::Puncher,
    local_addr: SocketAddr,
    local_tcp_port: u16,
    raw_tx: flume::Sender<RawTransportPacket>,
    routes: parking_lot::RwLock<Option<RouteTable<PeerId>>>,
    // Cache of live core transport handles keyed by RouteKey.
    //
    // This is deliberately not the route table. A packet received from an
    // address proves only that the remote side reached us once; protocol
    // control messages decide whether the route is bidirectional and confirmed.
    transports: DashMap<RouteKey, Transport>,
    outbound_tx: mpsc::UnboundedSender<CoreOutboundPacket>,
    control_tx: mpsc::UnboundedSender<CoreControl>,
}

impl CoreTransportLayer {
    async fn bind(
        config: &Config,
        raw_tx: flume::Sender<RawTransportPacket>,
    ) -> io::Result<Arc<Self>> {
        let udp_port = config.bind_addr.port();
        let mut core_config = if config.enable_tcp {
            rust_p2p_core::endpoint::Config::new()
                .udp_port(udp_port)
                .tcp_port(config.tcp_port.unwrap_or(udp_port))
        } else {
            rust_p2p_core::endpoint::Config::udp(udp_port)
        };
        core_config = core_config
            .stun_servers(config.stun_servers.clone())
            .mapping_udp_addr(config.mapping_udp_addrs.clone())
            .mapping_tcp_addr(config.mapping_tcp_addrs.clone())
            .load_balance(config.load_balance)
            .max_assistant_sockets(config.max_assistant_sockets);

        let endpoint = EndPoint::bind(core_config).await?;
        let sender = endpoint.sender();
        let puncher = endpoint.puncher();
        let local_tcp_port = endpoint.local_tcp_port();
        let local_addr = normalize_local_addr(endpoint.local_addr().await?, config.bind_addr);
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let layer = Arc::new(Self {
            sender,
            puncher,
            local_addr,
            local_tcp_port,
            raw_tx,
            routes: parking_lot::RwLock::new(None),
            transports: DashMap::new(),
            outbound_tx,
            control_tx,
        });
        layer.start(endpoint, outbound_rx, control_rx);
        Ok(layer)
    }

    fn start(
        self: &Arc<Self>,
        endpoint: EndPoint,
        outbound_rx: mpsc::UnboundedReceiver<CoreOutboundPacket>,
        control_rx: mpsc::UnboundedReceiver<CoreControl>,
    ) {
        self.start_core_receiver(endpoint, control_rx);
        self.start_core_sender(outbound_rx);
    }

    fn set_route_table(&self, routes: RouteTable<PeerId>) {
        *self.routes.write() = Some(routes);
    }

    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn local_tcp_addr(&self) -> Option<SocketAddr> {
        (self.local_tcp_port != 0)
            .then(|| SocketAddr::new(self.local_addr.ip(), self.local_tcp_port))
    }

    async fn send_raw_to_addr(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        self.sender.send_to(buf, addr).await
    }

    fn try_send_packet(&self, dest: PeerId, data: Bytes) -> io::Result<()> {
        self.outbound_tx
            .send(CoreOutboundPacket::PeerPacket { dest, data })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "transport sender closed"))
    }

    fn try_send_route(&self, route_key: RouteKey, data: Bytes) -> io::Result<()> {
        self.outbound_tx
            .send(CoreOutboundPacket::RoutePacket { route_key, data })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "transport sender closed"))
    }

    async fn punch(
        &self,
        tcp_buf: Option<&[u8]>,
        udp_buf: &[u8],
        punch_info: PunchInfo,
    ) -> io::Result<()> {
        self.puncher.punch_now(tcp_buf, udp_buf, punch_info).await
    }

    async fn apply_nat_model(&self, nat_type: rust_p2p_core::nat::NatType) -> io::Result<()> {
        let (reply, result) = oneshot::channel();
        self.control_tx
            .send(CoreControl::ApplyNatModel { nat_type, reply })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "core control task closed"))?;
        result
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "core control reply closed"))?
    }

    fn start_core_receiver(
        self: &Arc<Self>,
        mut endpoint: EndPoint,
        mut control_rx: mpsc::UnboundedReceiver<CoreControl>,
    ) {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    received = endpoint.recv() => {
                        let Some(received) = received else {
                            break;
                        };
                        let route_key = RouteKey::from_transport(&received.transport);
                        // Store the send handle for this core transport, but do not add
                        // a PeerId route here. Only the protocol layer can confirm that
                        // a route is usable for outbound peer traffic.
                        this.transports
                            .insert(route_key, received.transport.clone());
                        if received.data.is_empty() {
                            continue;
                        }
                        if this
                            .raw_tx
                            .send_async(RawTransportPacket {
                                data: received.data,
                                route_key,
                            })
                            .await
                            .is_err()
                        {
                            log::debug!("raw transport receiver closed");
                            break;
                        }
                    }
                    Some(command) = control_rx.recv() => {
                        match command {
                            CoreControl::ApplyNatModel { nat_type, reply } => {
                                // The core EndPoint owns the socket pool. Apply the local
                                // NAT model here instead of routing it through Puncher, whose
                                // job is remote-peer punching strategy.
                                let _ = reply.send(endpoint.apply_nat_model(nat_type).await);
                            }
                        }
                    }
                }
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
                    CoreOutboundPacket::PeerPacket { dest, data } => {
                        this.send_packet_to_peer(dest, &data).await
                    }
                    CoreOutboundPacket::RoutePacket { route_key, data } => {
                        this.send_to_route(route_key, &data).await
                    }
                };
                if let Err(e) = result {
                    log::debug!("core transport send failed: {e}");
                }
            }
        });
    }

    async fn send_packet_to_peer(&self, peer_id: PeerId, data: &[u8]) -> io::Result<()> {
        let route = self
            .routes
            .read()
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "route table not attached"))?
            .get_route_by_id(&peer_id)?;
        self.send_to_route(route.route_key(), data).await
    }

    async fn send_to_route(&self, route_key: RouteKey, data: &[u8]) -> io::Result<()> {
        if let Some(transport) = self.transports.get(&route_key) {
            return transport.send(data).await;
        }
        if route_key.protocol().is_tcp() {
            self.sender.write_to(data, route_key.addr()).await
        } else {
            self.sender.send_to(data, route_key.addr()).await
        }
    }
}

struct TransportState {
    peer_id: PeerId,
    closed: AtomicBool,
    peers: DashMap<PeerId, PeerInfo>,
    routes: RouteTable<PeerId>,
    raw_rx: flume::Receiver<RawTransportPacket>,
}

pub(crate) struct TransportLayer {
    core: Arc<CoreTransportLayer>,
    state: Arc<TransportState>,
}

impl TransportLayer {
    pub(crate) async fn bind(peer_id: PeerId, config: &Config) -> io::Result<Arc<Self>> {
        let (raw_tx, raw_rx) = flume::bounded(512);
        let core = CoreTransportLayer::bind(config, raw_tx).await?;
        let routes = RouteTable::new(config.load_balance);
        core.set_route_table(routes.clone());
        Ok(Arc::new(Self {
            core,
            state: Arc::new(TransportState {
                peer_id,
                closed: AtomicBool::new(false),
                peers: DashMap::new(),
                routes,
                raw_rx,
            }),
        }))
    }

    pub(crate) fn local_addr(&self) -> SocketAddr {
        self.core.local_addr()
    }

    pub(crate) fn local_tcp_addr(&self) -> Option<SocketAddr> {
        self.core.local_tcp_addr()
    }

    pub(crate) fn close(&self) {
        self.state.closed.store(true, Ordering::Relaxed);
    }

    pub(crate) fn known_peers(&self) -> Vec<PeerInfo> {
        self.state.peers.iter().map(|v| v.value().clone()).collect()
    }

    pub(crate) fn routes(&self, peer_id: PeerId) -> Vec<Route> {
        self.state.routes.route(&peer_id).unwrap_or_default()
    }

    pub(crate) fn link_mode(&self, peer_id: PeerId) -> Option<LinkMode> {
        self.link_info(peer_id).map(|info| info.mode)
    }

    pub(crate) fn link_info(&self, peer_id: PeerId) -> Option<LinkInfo> {
        let route = self.state.routes.route_one(&peer_id)?;
        Some(LinkInfo {
            peer_id,
            mode: if route.metric() == 0 {
                LinkMode::Direct
            } else {
                LinkMode::Relay
            },
            metric: route.metric(),
        })
    }

    pub(crate) fn route_for(&self, peer_id: &PeerId) -> io::Result<Route> {
        self.state.routes.get_route_by_id(peer_id)
    }

    pub(crate) fn route_metric_for(&self, peer_id: &PeerId) -> io::Result<(Route, u8)> {
        let route = self.route_for(peer_id)?;
        let metric = route.metric();
        Ok((route, metric))
    }

    pub(crate) async fn recv_wire(&self) -> io::Result<RawTransportPacket> {
        self.state
            .raw_rx
            .recv_async()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "transport closed"))
    }

    pub(crate) async fn send_wire(&self, peer_id: PeerId, wire_bytes: &[u8]) -> io::Result<()> {
        self.core.send_packet_to_peer(peer_id, wire_bytes).await
    }

    pub(crate) fn try_send_wire(&self, peer_id: PeerId, wire_bytes: &[u8]) -> io::Result<()> {
        self.core
            .try_send_packet(peer_id, Bytes::copy_from_slice(wire_bytes))
    }

    pub(crate) fn try_send_wire_to_route(
        &self,
        route_key: RouteKey,
        wire_bytes: &[u8],
    ) -> io::Result<()> {
        self.core
            .try_send_route(route_key, Bytes::copy_from_slice(wire_bytes))
    }

    pub(crate) async fn send_raw_to_addr(
        &self,
        wire_bytes: &[u8],
        addr: SocketAddr,
    ) -> io::Result<()> {
        self.core.send_raw_to_addr(wire_bytes, addr).await
    }

    pub(crate) async fn send_wire_to_route(
        &self,
        route_key: RouteKey,
        wire_bytes: &[u8],
    ) -> io::Result<()> {
        self.core.send_to_route(route_key, wire_bytes).await
    }

    pub(crate) fn confirm_route(&self, peer_id: PeerId, route_key: RouteKey, metric: u8) {
        // Protocol is the only caller for this primitive in normal operation.
        // Keeping route mutation behind this method prevents receive-side packet
        // observation from becoming confirmed reachability by accident.
        self.state
            .routes
            .add_route(peer_id, Route::from_default_rt(route_key, metric));
    }

    pub(crate) fn update_route_read_time(&self, peer_id: &PeerId, route_key: &RouteKey) {
        self.state.routes.update_read_time(peer_id, route_key);
    }

    pub(crate) fn route_to_id(&self, route_key: &RouteKey) -> Option<PeerId> {
        self.state.routes.route_to_id(route_key)
    }

    pub(crate) fn upsert_peer_info(&self, peer: PeerInfo) {
        if peer.peer_id == self.state.peer_id {
            return;
        }
        let mut merged = peer.clone();
        merged.last_seen = now_millis();
        if let Some(existing) = self.state.peers.get(&peer.peer_id) {
            for addr in &existing.addrs {
                if !merged.addrs.contains(addr) {
                    merged.addrs.push(*addr);
                }
            }
            merged.is_direct |= existing.is_direct;
            if merged.relay_hint.is_none() {
                merged.relay_hint = existing.relay_hint.clone();
            }
            if merged.nat_info.is_none() {
                merged.nat_info = existing.nat_info.clone();
            }
        }
        self.state.peers.insert(merged.peer_id.clone(), merged);
    }

    pub(crate) fn confirm_peer_route(&self, mut peer: PeerInfo, route_key: RouteKey, metric: u8) {
        // Confirmed direct routes keep their socket address. Confirmed relay
        // routes point at the next hop route and intentionally do not expose the
        // final peer's socket address as directly reachable.
        if metric == 0 {
            peer.is_direct = true;
            if !peer.addrs.contains(&route_key.addr()) {
                peer.addrs.push(route_key.addr());
            }
        } else {
            peer.is_direct = false;
            peer.addrs.clear();
            if peer.relay_hint.is_none() {
                peer.relay_hint = self.route_to_id(&route_key);
            }
        }
        self.confirm_route(peer.peer_id.clone(), route_key, metric);
        self.upsert_peer_info(peer);
    }

    pub(crate) async fn punch(
        &self,
        tcp_buf: Option<&[u8]>,
        udp_buf: &[u8],
        punch_info: PunchInfo,
    ) -> io::Result<()> {
        self.core.punch(tcp_buf, udp_buf, punch_info).await
    }

    pub(crate) async fn apply_nat_model(
        &self,
        nat_type: rust_p2p_core::nat::NatType,
    ) -> io::Result<()> {
        self.core.apply_nat_model(nat_type).await
    }
}

#[derive(Clone)]
/// Low-level handle for transport diagnostics and control tooling.
///
/// This handle sends already encoded protocol wire bytes. It does not provide
/// QUIC encryption for user payloads.
pub struct TransportHandle {
    transport: Arc<TransportLayer>,
}

impl TransportHandle {
    pub(crate) fn new(transport: Arc<TransportLayer>) -> Self {
        Self { transport }
    }

    /// Returns the local core transport address, if one is available.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        Some(self.transport.local_addr())
    }

    /// Sends already encoded protocol wire bytes to a peer.
    ///
    /// This is a low-level escape hatch. It does not encrypt user data by itself.
    /// Application data should use `Endpoint::send_to` or `Endpoint::open_bi`.
    pub async fn send_to_peer(&self, peer_id: PeerId, wire_bytes: &[u8]) -> io::Result<()> {
        self.transport.send_wire(peer_id, wire_bytes).await
    }
}

fn normalize_local_addr(actual: SocketAddr, requested: SocketAddr) -> SocketAddr {
    if requested.ip().is_unspecified() || !actual.ip().is_unspecified() {
        actual
    } else {
        SocketAddr::new(requested.ip(), actual.port())
    }
}

pub(crate) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{LinkMode, TransportLayer};
    use crate::{Config, Identity, PeerId};
    use std::io;

    #[tokio::test]
    async fn unknown_route_returns_not_found() {
        let transport = test_transport("route-a").await;
        let err = transport.route_for(&PeerId::from("route-b")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        transport.close();
    }

    #[tokio::test]
    async fn route_table_supports_multiple_routes_for_a_peer() {
        let transport = test_transport("route-multi-a").await;
        let peer = PeerId::from("route-multi-b");
        transport.confirm_route(
            peer.clone(),
            rust_p2p_core::route_table::RouteKey::new(
                rust_p2p_core::route_table::Protocol::UDP,
                "127.0.0.1:1111".parse().unwrap(),
            ),
            1,
        );
        transport.confirm_route(
            peer.clone(),
            rust_p2p_core::route_table::RouteKey::new(
                rust_p2p_core::route_table::Protocol::TCP,
                "127.0.0.1:2222".parse().unwrap(),
            ),
            1,
        );

        assert_eq!(transport.routes(peer).len(), 2);
        transport.close();
    }

    #[tokio::test]
    async fn direct_route_reports_direct_link_mode() {
        let transport = test_transport("transport-a").await;
        let peer = PeerId::from("transport-b");
        transport.confirm_route(
            peer.clone(),
            rust_p2p_core::route_table::RouteKey::new(
                rust_p2p_core::route_table::Protocol::UDP,
                "127.0.0.1:3333".parse().unwrap(),
            ),
            0,
        );

        assert_eq!(transport.link_mode(peer), Some(LinkMode::Direct));
        transport.close();
    }

    #[tokio::test]
    async fn peer_info_does_not_imply_confirmed_route() {
        let transport = test_transport("peer-info-a").await;
        let peer = PeerId::from("peer-info-b");
        let info = super::PeerInfo::new(peer.clone()).with_addr("127.0.0.1:4444".parse().unwrap());

        transport.upsert_peer_info(info);

        let err = transport.route_for(&peer).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(transport.link_mode(peer).is_none());
        transport.close();
    }

    async fn test_transport(id: &str) -> std::sync::Arc<TransportLayer> {
        let config = Config {
            identity: Some(Identity::new(id, format!("{id}-seed")).unwrap()),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            stun_servers: Vec::new(),
            ..Default::default()
        };
        TransportLayer::bind(PeerId::from(id), &config)
            .await
            .unwrap()
    }
}
