use crate::transport::{now_millis as transport_now_millis, PeerInfo, RawTransportPacket};
use crate::transport::{TransportLayer, TransportPayload};
use crate::PeerId;
use bytes::Bytes;
use dashmap::{DashMap, DashSet};
use prost::Message;
use rust_p2p_core::nat::{NatInfo, NatType};
use rust_p2p_core::punch::{PunchInfo, PunchModel};
use rust_p2p_core::route_table::{Protocol, RouteKey};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub(crate) mod pb {
    include!(concat!(env!("OUT_DIR"), "/rustp2p.quic.v3.rs"));
}

/// Version of the rustp2p overlay packet header.
///
/// The fixed header is still hand-encoded for fast demux and TTL updates.
/// Protocol-specific payloads inside the header are protobuf messages.
pub const VERSION: u8 = 3;
const FIXED_HEADER_LEN: usize = 13;

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

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ProtocolType {
    MessageData = 0,
    PunchConsultRequest = 1,
    PunchConsultReply = 2,
    PunchRequest = 3,
    PunchReply = 4,
    EchoRequest = 5,
    EchoReply = 6,
    TimestampRequest = 7,
    TimestampReply = 8,
    IDRouteQuery = 9,
    IDRouteReply = 10,
    RangeBroadcast = 11,
    QuicRelay = 12,
    HelloRequest = 13,
    HelloReply = 14,
    NatObserveRequest = 15,
    NatObserveReply = 16,
}

impl TryFrom<u8> for ProtocolType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::MessageData),
            1 => Ok(Self::PunchConsultRequest),
            2 => Ok(Self::PunchConsultReply),
            3 => Ok(Self::PunchRequest),
            4 => Ok(Self::PunchReply),
            5 => Ok(Self::EchoRequest),
            6 => Ok(Self::EchoReply),
            7 => Ok(Self::TimestampRequest),
            8 => Ok(Self::TimestampReply),
            9 => Ok(Self::IDRouteQuery),
            10 => Ok(Self::IDRouteReply),
            11 => Ok(Self::RangeBroadcast),
            12 => Ok(Self::QuicRelay),
            13 => Ok(Self::HelloRequest),
            14 => Ok(Self::HelloReply),
            15 => Ok(Self::NatObserveRequest),
            16 => Ok(Self::NatObserveReply),
            val => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid protocol type: {val}"),
            )),
        }
    }
}

impl From<ProtocolType> for u8 {
    fn from(value: ProtocolType) -> Self {
        value as u8
    }
}

#[derive(Clone, Debug)]
pub struct Packet {
    buf: Vec<u8>,
    src: PeerId,
    dest: PeerId,
    payload_offset: usize,
}

impl Packet {
    pub fn build(
        protocol: ProtocolType,
        src: PeerId,
        dest: PeerId,
        max_ttl: u8,
        payload: &[u8],
    ) -> io::Result<Self> {
        let src_bytes = src.as_str().as_bytes();
        let dest_bytes = dest.as_str().as_bytes();
        if src_bytes.len() > u16::MAX as usize || dest_bytes.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "peer id exceeds 65535 bytes",
            ));
        }
        let len = FIXED_HEADER_LEN
            .checked_add(src_bytes.len())
            .and_then(|n| n.checked_add(dest_bytes.len()))
            .and_then(|n| n.checked_add(payload.len()))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "packet too large"))?;
        if len > u32::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "packet too large",
            ));
        }

        let mut buf = Vec::with_capacity(len);
        buf.push(0x80 | u8::from(protocol));
        buf.push(VERSION);
        buf.push(0);
        buf.push(max_ttl);
        buf.push(max_ttl);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&(src_bytes.len() as u16).to_be_bytes());
        buf.extend_from_slice(&(dest_bytes.len() as u16).to_be_bytes());
        buf.extend_from_slice(src_bytes);
        buf.extend_from_slice(dest_bytes);
        let payload_offset = buf.len();
        buf.extend_from_slice(payload);

        Ok(Self {
            buf,
            src,
            dest,
            payload_offset,
        })
    }

    pub fn parse(buf: Vec<u8>) -> io::Result<Self> {
        if buf.len() < FIXED_HEADER_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "short packet"));
        }
        if buf[0] & 0x80 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a rustp2p packet",
            ));
        }
        if buf[1] != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported packet version: {}", buf[1]),
            ));
        }
        let len = u32::from_be_bytes(buf[5..9].try_into().unwrap()) as usize;
        if len != buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "packet length mismatch",
            ));
        }
        let src_len = u16::from_be_bytes(buf[9..11].try_into().unwrap()) as usize;
        let dest_len = u16::from_be_bytes(buf[11..13].try_into().unwrap()) as usize;
        let src_start = FIXED_HEADER_LEN;
        let dest_start = src_start
            .checked_add(src_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid src length"))?;
        let payload_offset = dest_start
            .checked_add(dest_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid dest length"))?;
        if payload_offset > buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "peer id length exceeds packet",
            ));
        }
        let src = std::str::from_utf8(&buf[src_start..dest_start])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("src peer id: {e}")))?
            .to_string();
        let dest = std::str::from_utf8(&buf[dest_start..payload_offset])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("dest peer id: {e}")))?
            .to_string();
        Ok(Self {
            buf,
            src: PeerId::from(src),
            dest: PeerId::from(dest),
            payload_offset,
        })
    }

    pub fn protocol(&self) -> io::Result<ProtocolType> {
        (self.buf[0] & 0x7f).try_into()
    }

    pub fn src(&self) -> PeerId {
        self.src.clone()
    }

    pub fn dest(&self) -> PeerId {
        self.dest.clone()
    }

    pub fn ttl(&self) -> u8 {
        self.buf[4]
    }

    pub fn max_ttl(&self) -> u8 {
        self.buf[3]
    }

    pub fn decrement_ttl(&mut self) -> bool {
        if self.buf[4] <= 1 {
            return false;
        }
        self.buf[4] -= 1;
        true
    }

    pub fn payload(&self) -> &[u8] {
        &self.buf[self.payload_offset..]
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    #[cfg(test)]
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelloPayload {
    pub peer: PeerInfo,
    pub peers: Vec<RouteEntry>,
    pub observed_addr: Option<NatObservation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteReplyPayload {
    pub peers: Vec<RouteEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteEntry {
    pub peer: PeerInfo,
    pub metric: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PunchPayload {
    pub request_id: u64,
    pub src: PeerId,
    pub dest: PeerId,
    pub nat_info: NatInfo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ObservedProtocol {
    Udp,
    Tcp,
}

impl From<Protocol> for ObservedProtocol {
    fn from(value: Protocol) -> Self {
        match value {
            Protocol::UDP => Self::Udp,
            Protocol::TCP => Self::Tcp,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NatObservation {
    pub observed_protocol: ObservedProtocol,
    pub observed_addr: SocketAddr,
    pub observer_peer_id: PeerId,
    pub observed_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NatObserveReplyPayload {
    pub observation: NatObservation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamHeader {
    pub src: PeerId,
    pub dest: PeerId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamFrame {
    User(StreamHeader),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DatagramFrame {
    User {
        id: u64,
        src: PeerId,
        dest: PeerId,
        payload: Vec<u8>,
    },
}

pub(crate) struct ProtocolLayer {
    transport: Arc<TransportLayer>,
    peer_id: PeerId,
    max_ttl: u8,
    closed: AtomicBool,
    hello_tx: flume::Sender<(SocketAddr, PeerId)>,
    hello_rx: flume::Receiver<(SocketAddr, PeerId)>,
    quic_tx: flume::Sender<TransportPayload>,
    quic_rx: flume::Receiver<TransportPayload>,
    punch_whitelist: parking_lot::RwLock<HashSet<PeerId>>,
    nat_observers: parking_lot::RwLock<HashSet<PeerId>>,
    nat_info: parking_lot::RwLock<NatInfo>,
    peer_nat: DashMap<PeerId, NatInfo>,
    // Bootstrap hello requests sent to raw socket addresses. A matching
    // HelloReply confirms that the direct route is bidirectional.
    pending_hello_routes: DashSet<RouteKey>,
    // Direct NAT observation requests waiting for a reply. Relayed observations
    // are ignored because they would describe the relay, not the requester.
    pending_nat_observe: DashMap<PeerId, RouteKey>,
    // Hole-punch request ids waiting for PunchReply. Only a matching reply can
    // promote a punch candidate into a confirmed direct route.
    pending_punch: DashMap<u64, PeerId>,
    // One-hop candidates observed from inbound UDP/control traffic. Candidates
    // can be used to answer handshakes but are hidden from public route queries
    // until a protocol-specific confirmation arrives.
    route_candidates: DashMap<PeerId, RouteKey>,
}

impl ProtocolLayer {
    pub(crate) fn new(
        peer_id: PeerId,
        transport: Arc<TransportLayer>,
        max_ttl: u8,
        punch_whitelist: Vec<PeerId>,
        nat_observers: Vec<PeerId>,
        initial_nat_info: NatInfo,
    ) -> Arc<Self> {
        let (hello_tx, hello_rx) = flume::bounded(128);
        let (quic_tx, quic_rx) = flume::bounded(512);
        let layer = Arc::new(Self {
            transport,
            peer_id,
            max_ttl: max_ttl.max(1),
            closed: AtomicBool::new(false),
            hello_tx,
            hello_rx,
            quic_tx,
            quic_rx,
            punch_whitelist: parking_lot::RwLock::new(punch_whitelist.into_iter().collect()),
            nat_observers: parking_lot::RwLock::new(nat_observers.into_iter().collect()),
            nat_info: parking_lot::RwLock::new(initial_nat_info),
            peer_nat: DashMap::new(),
            pending_hello_routes: DashSet::new(),
            pending_nat_observe: DashMap::new(),
            pending_punch: DashMap::new(),
            route_candidates: DashMap::new(),
        });
        layer.start_packet_dispatcher();
        layer.start_maintenance_loop();
        layer
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    pub(crate) fn max_ttl(&self) -> u8 {
        self.max_ttl
    }

    pub(crate) fn nat_info(&self) -> NatInfo {
        self.nat_info.read().clone()
    }

    pub(crate) fn ensure_peer_reachable(&self, peer_id: &PeerId) -> io::Result<()> {
        self.transport.route_for(peer_id).map(|_| ())
    }

    pub(crate) fn route_metric_for(&self, peer_id: &PeerId) -> io::Result<(RouteKey, u8)> {
        // QUIC may need metadata for packets received while the route is still
        // a direct candidate. This does not expose the candidate through
        // Endpoint::routes or link_mode.
        match self.transport.route_metric_for(peer_id) {
            Ok((route, metric)) => Ok((route.route_key(), metric)),
            Err(e) => self
                .route_candidates
                .get(peer_id)
                .map(|route| (*route, 0))
                .ok_or(e),
        }
    }

    pub(crate) fn try_send_quic_payload(&self, peer_id: PeerId, payload: &[u8]) -> io::Result<()> {
        // QUIC ciphertext is always wrapped as QuicRelay before entering the
        // transport layer. User datagrams never become raw rustp2p payloads.
        let packet = Packet::build(
            ProtocolType::QuicRelay,
            self.peer_id.clone(),
            peer_id.clone(),
            self.max_ttl,
            payload,
        )?;
        if self.transport.route_for(&peer_id).is_ok() {
            return self.transport.try_send_wire(peer_id, packet.as_bytes());
        }
        if let Some(route) = self.route_candidates.get(&peer_id).map(|route| *route) {
            self.transport
                .try_send_wire_to_route(route, packet.as_bytes())
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "peer route not found",
            ))
        }
    }

    pub(crate) async fn recv_quic_payload(&self) -> io::Result<TransportPayload> {
        self.quic_rx
            .recv_async()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "protocol closed"))
    }

    pub(crate) async fn add_bootstrap(&self, addr: SocketAddr) -> io::Result<PeerId> {
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

    async fn add_bootstrap_once(&self, addr: SocketAddr) -> io::Result<PeerId> {
        let packet = Packet::build(
            ProtocolType::HelloRequest,
            self.peer_id.clone(),
            PeerId::broadcast(),
            self.max_ttl,
            &[],
        )?;
        self.transport
            .send_raw_to_addr(packet.as_bytes(), addr)
            .await?;
        self.pending_hello_routes
            .insert(RouteKey::new(Protocol::UDP, addr));

        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let (seen_addr, peer_id) =
                    self.hello_rx.recv_async().await.map_err(|_| {
                        io::Error::new(io::ErrorKind::UnexpectedEof, "protocol closed")
                    })?;
                if seen_addr == addr {
                    return Ok(peer_id);
                }
            }
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "bootstrap hello timed out"))?
    }

    pub(crate) fn allow_punch(&self, peer_id: PeerId) {
        self.punch_whitelist.write().insert(peer_id);
    }

    pub(crate) fn deny_punch(&self, peer_id: &PeerId) {
        self.punch_whitelist.write().remove(peer_id);
    }

    pub(crate) fn set_punch_whitelist(&self, peers: Vec<PeerId>) {
        *self.punch_whitelist.write() = peers.into_iter().collect();
    }

    pub(crate) fn punch_whitelist(&self) -> Vec<PeerId> {
        self.punch_whitelist.read().iter().cloned().collect()
    }

    pub(crate) async fn punch(&self, peer_id: PeerId) -> io::Result<()> {
        if !self.punch_allowed(&peer_id) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "peer is not in punch whitelist",
            ));
        }
        let payload = self.punch_payload(peer_id.clone());
        self.pending_punch
            .insert(payload.request_id, peer_id.clone());
        let encoded = encode_punch_payload(&payload);
        self.send_protocol(peer_id.clone(), ProtocolType::PunchRequest, &encoded)
            .await?;
        if let Some(nat_info) = self.peer_nat.get(&peer_id).map(|v| v.clone()) {
            self.execute_punch(peer_id, nat_info).await?;
        }
        Ok(())
    }

    pub(crate) fn start_nat_maintenance(
        self: &Arc<Self>,
        timeout: Duration,
        stun_servers: Vec<String>,
    ) {
        let protocol = self.clone();
        if !stun_servers.is_empty() {
            let protocol = protocol.clone();
            tokio::spawn(async move {
                loop {
                    match rust_p2p_core::stun::stun_test_nat(stun_servers.clone(), None).await {
                        Ok(result) => {
                            let nat_type = result.nat_type;
                            {
                                let mut info = protocol.nat_info.write();
                                apply_stun_result_to_nat_info(&mut info, &result);
                            }
                            if let Err(e) = protocol.transport.apply_nat_model(nat_type).await {
                                log::debug!("failed to apply NAT model: {e}");
                            }
                        }
                        Err(e) => {
                            log::debug!("NAT detection failed: {e}");
                        }
                    }
                    tokio::time::sleep(timeout).await;
                }
            });
        }

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(timeout.max(Duration::from_secs(1)));
            loop {
                interval.tick().await;
                if protocol.closed.load(Ordering::Relaxed) {
                    break;
                }
                for peer in protocol.transport.known_peers() {
                    if protocol.should_observe_with(&peer.peer_id)
                        && protocol.has_direct_route(&peer.peer_id)
                    {
                        if let Ok(route) = protocol.transport.route_for(&peer.peer_id) {
                            protocol
                                .pending_nat_observe
                                .insert(peer.peer_id.clone(), route.route_key());
                        }
                        let _ = protocol
                            .send_protocol(peer.peer_id, ProtocolType::NatObserveRequest, &[])
                            .await;
                    }
                }
            }
        });
    }

    fn start_packet_dispatcher(self: &Arc<Self>) {
        let protocol = self.clone();
        tokio::spawn(async move {
            loop {
                if protocol.closed.load(Ordering::Relaxed) {
                    break;
                }
                let packet = match protocol.transport.recv_wire().await {
                    Ok(packet) => packet,
                    Err(e) => {
                        log::debug!("protocol packet loop ended: {e}");
                        break;
                    }
                };
                if let Err(e) = protocol.handle_transport_packet(packet).await {
                    log::debug!("protocol packet handling failed: {e}");
                }
            }
        });
    }

    async fn handle_transport_packet(&self, received: RawTransportPacket) -> io::Result<()> {
        let Some(first) = received.data.first().copied() else {
            return Ok(());
        };
        if !matches!(classify_packet(first), PacketType::RustP2p(_)) {
            return Ok(());
        }

        let packet = Packet::parse(received.data.to_vec())?;
        let src = packet.src();
        if src == self.peer_id || src.is_unspecified() {
            return Ok(());
        }

        let route_key = received.route_key;
        let metric = packet.max_ttl().saturating_sub(packet.ttl());
        self.transport.update_route_read_time(&src, &route_key);
        let peer = self.peer_info_from_packet(&src, route_key, metric);
        // Peer store updates are cheap discovery metadata. They are not route
        // confirmation; confirmed routes are written only in protocol branches
        // that prove bidirectional reachability or advertised relay reachability.
        self.transport.upsert_peer_info(peer.clone());
        if metric == 0 {
            self.route_candidates.insert(src.clone(), route_key);
        }

        let dest = packet.dest();
        if dest != self.peer_id && !dest.is_broadcast() {
            self.forward_packet(packet).await?;
            return Ok(());
        }

        match packet.protocol()? {
            ProtocolType::HelloRequest => {
                self.confirm_tcp_route(&peer, route_key, metric);
                let payload = encode_hello_payload(
                    &self
                        .hello_payload(&src, Some(self.observation_from_route(route_key)))
                        .await?,
                );
                self.send_protocol_to_route(src, route_key, ProtocolType::HelloReply, &payload)
                    .await?;
            }
            ProtocolType::HelloReply => {
                let hello = decode_hello_payload(packet.payload())?;
                let peer_id = hello.peer.peer_id.clone();
                if let Some(nat_info) = &hello.peer.nat_info {
                    self.peer_nat.insert(peer_id.clone(), nat_info.clone());
                }
                if metric == 0 {
                    if let Some(observation) = hello.observed_addr {
                        self.apply_observation(observation);
                    }
                }
                let pending = self.pending_hello_routes.remove(&route_key).is_some();
                if metric == 0 && pending {
                    // A reply to our raw-address hello proves we can send to the
                    // peer and receive from it over the same direct route.
                    self.transport.confirm_peer_route(hello.peer, route_key, 0);
                } else {
                    self.transport.upsert_peer_info(hello.peer);
                }
                let _ = self.hello_tx.send_async((route_key.addr(), peer_id)).await;
                self.ingest_route_entries(hello.peers, route_key).await?;
            }
            ProtocolType::IDRouteQuery => {
                self.confirm_relay_route_from_control(&peer, route_key, metric);
                let payload = encode_route_reply_payload(&RouteReplyPayload {
                    peers: self.discovery_entries(&src).await?,
                });
                self.send_protocol_to_route(src, route_key, ProtocolType::IDRouteReply, &payload)
                    .await?;
            }
            ProtocolType::IDRouteReply => {
                let reply = decode_route_reply_payload(packet.payload())?;
                self.confirm_tcp_route(&peer, route_key, metric);
                self.confirm_relay_route_from_control(&peer, route_key, metric);
                self.ingest_route_entries(reply.peers, route_key).await?;
            }
            ProtocolType::QuicRelay => {
                // The relay payload is opaque QUIC ciphertext. The protocol
                // layer only forwards or injects it into quinn; it never parses
                // user data from this payload.
                let _ = self
                    .quic_tx
                    .send_async(TransportPayload {
                        payload: Bytes::copy_from_slice(packet.payload()),
                        src,
                    })
                    .await;
            }
            ProtocolType::PunchConsultRequest => {
                self.handle_punch_consult_request(packet, route_key).await?;
            }
            ProtocolType::PunchConsultReply => {
                self.handle_punch_payload(packet.payload()).await?;
            }
            ProtocolType::PunchRequest => {
                let payload = self.handle_punch_payload(packet.payload()).await?;
                if metric == 0 {
                    // A punch request proves only that the peer reached us. It
                    // stays a candidate until a matching PunchReply completes
                    // the request/response exchange.
                    self.route_candidates.insert(payload.src.clone(), route_key);
                }
                if self.punch_allowed(&payload.src) {
                    self.execute_punch(payload.src.clone(), payload.nat_info)
                        .await?;
                    let reply = encode_punch_payload(
                        &self
                            .punch_payload_with_request_id(payload.src.clone(), payload.request_id),
                    );
                    self.send_protocol_to_route(
                        payload.src,
                        route_key,
                        ProtocolType::PunchReply,
                        &reply,
                    )
                    .await?;
                }
            }
            ProtocolType::PunchReply => {
                let payload = self.handle_punch_payload(packet.payload()).await?;
                if let Some((_, peer_id)) = self.pending_punch.remove(&payload.request_id) {
                    if peer_id == payload.src && metric == 0 {
                        // Matching request id prevents unrelated punch traffic
                        // from promoting a route to direct reachability.
                        self.transport.confirm_peer_route(
                            self.peer_info_from_packet(&payload.src, route_key, metric),
                            route_key,
                            0,
                        );
                    }
                }
                if self.punch_allowed(&payload.src) {
                    self.execute_punch(payload.src, payload.nat_info).await?;
                }
            }
            ProtocolType::EchoRequest => {
                self.confirm_tcp_route(&peer, route_key, metric);
                self.send_protocol_to_route(
                    src,
                    route_key,
                    ProtocolType::EchoReply,
                    packet.payload(),
                )
                .await?;
            }
            ProtocolType::TimestampRequest => {
                self.confirm_tcp_route(&peer, route_key, metric);
                self.send_protocol_to_route(
                    src,
                    route_key,
                    ProtocolType::TimestampReply,
                    packet.payload(),
                )
                .await?;
            }
            ProtocolType::NatObserveRequest => {
                if self.is_confirmed_or_candidate(&src, route_key, metric) {
                    let payload = encode_nat_observe_reply_payload(&NatObserveReplyPayload {
                        observation: self.observation_from_route(route_key),
                    });
                    self.send_protocol_to_route(
                        src,
                        route_key,
                        ProtocolType::NatObserveReply,
                        &payload,
                    )
                    .await?;
                }
            }
            ProtocolType::NatObserveReply => {
                if self.is_confirmed_or_pending_nat_observe(&src, route_key, metric) {
                    let reply = decode_nat_observe_reply_payload(packet.payload())?;
                    if reply.observation.observer_peer_id == src && self.should_observe_with(&src) {
                        self.apply_observation(reply.observation);
                        if metric == 0 {
                            self.transport.confirm_peer_route(peer, route_key, 0);
                        }
                    }
                }
            }
            ProtocolType::MessageData
            | ProtocolType::RangeBroadcast
            | ProtocolType::EchoReply
            | ProtocolType::TimestampReply => {}
        }
        Ok(())
    }

    async fn handle_punch_consult_request(
        &self,
        packet: Packet,
        route_key: RouteKey,
    ) -> io::Result<()> {
        let payload = self.handle_punch_payload(packet.payload()).await?;
        if !self.punch_allowed(&payload.src) {
            return Ok(());
        }
        let reply = encode_punch_payload(&self.punch_payload(payload.src.clone()));
        self.send_protocol_to_route(
            payload.src,
            route_key,
            ProtocolType::PunchConsultReply,
            &reply,
        )
        .await
    }

    async fn handle_punch_payload(&self, payload: &[u8]) -> io::Result<PunchPayload> {
        let payload = decode_punch_payload(payload)?;
        self.peer_nat
            .insert(payload.src.clone(), payload.nat_info.clone());
        Ok(payload)
    }

    async fn execute_punch(&self, peer_id: PeerId, peer_nat_info: NatInfo) -> io::Result<()> {
        let payload = encode_punch_payload(&self.punch_payload(peer_id.clone()));
        let packet = Packet::build(
            ProtocolType::PunchRequest,
            self.peer_id.clone(),
            peer_id,
            self.max_ttl,
            &payload,
        )?;
        let bytes = packet.as_bytes().to_vec();
        self.transport
            .punch(
                Some(bytes.as_slice()),
                bytes.as_slice(),
                PunchInfo::new(PunchModel::all(), peer_nat_info),
            )
            .await
    }

    fn punch_payload(&self, dest: PeerId) -> PunchPayload {
        self.punch_payload_with_request_id(dest, rand::random())
    }

    fn punch_payload_with_request_id(&self, dest: PeerId, request_id: u64) -> PunchPayload {
        PunchPayload {
            request_id,
            src: self.peer_id.clone(),
            dest,
            nat_info: self.nat_info(),
        }
    }

    fn punch_allowed(&self, peer_id: &PeerId) -> bool {
        self.punch_whitelist.read().contains(peer_id)
    }

    async fn ingest_route_entries(
        &self,
        entries: Vec<RouteEntry>,
        via: RouteKey,
    ) -> io::Result<()> {
        for item in entries {
            if item.peer.peer_id == self.peer_id || item.peer.peer_id.is_unspecified() {
                continue;
            }
            if let Some(nat_info) = &item.peer.nat_info {
                self.peer_nat
                    .insert(item.peer.peer_id.clone(), nat_info.clone());
            }
            let metric = item.metric.saturating_add(1);
            // Route replies advertise transport reachability. The next hop is
            // the route used to receive this reply, while metric records the
            // additional overlay hop distance to the advertised peer.
            self.transport
                .confirm_peer_route(item.peer.clone(), via, metric);
        }
        Ok(())
    }

    async fn discovery_entries(&self, requester: &PeerId) -> io::Result<Vec<RouteEntry>> {
        let self_peer = self.self_peer_info();
        let peers = std::iter::once(RouteEntry {
            peer: self_peer.clone(),
            metric: 0,
        })
        .chain(self.transport.known_peers().into_iter().filter_map(|peer| {
            if &peer.peer_id == requester {
                None
            } else {
                let metric = self
                    .transport
                    .routes(peer.peer_id.clone())
                    .into_iter()
                    .next()
                    .map(|route| route.metric().saturating_add(1))
                    .unwrap_or(1);
                Some(RouteEntry { peer, metric })
            }
        }))
        .collect();
        Ok(peers)
    }

    async fn hello_payload(
        &self,
        requester: &PeerId,
        observed_addr: Option<NatObservation>,
    ) -> io::Result<HelloPayload> {
        Ok(HelloPayload {
            peer: self.self_peer_info(),
            peers: self.discovery_entries(requester).await?,
            observed_addr,
        })
    }

    async fn query_routes(&self, peer_id: PeerId) -> io::Result<()> {
        self.send_protocol(peer_id, ProtocolType::IDRouteQuery, &[])
            .await
    }

    async fn forward_packet(&self, mut packet: Packet) -> io::Result<()> {
        if !packet.decrement_ttl() {
            return Ok(());
        }
        // Forwarding is packet-level. Relays only inspect the rustp2p header and
        // never terminate QUIC connections or decrypt QuicRelay payloads.
        self.transport
            .send_wire(packet.dest(), packet.as_bytes())
            .await
    }

    async fn send_protocol(
        &self,
        dest: PeerId,
        protocol: ProtocolType,
        payload: &[u8],
    ) -> io::Result<()> {
        let packet = Packet::build(
            protocol,
            self.peer_id.clone(),
            dest.clone(),
            self.max_ttl,
            payload,
        )?;
        self.transport.send_wire(dest, packet.as_bytes()).await
    }

    async fn send_protocol_to_route(
        &self,
        dest: PeerId,
        route_key: RouteKey,
        protocol: ProtocolType,
        payload: &[u8],
    ) -> io::Result<()> {
        let packet = Packet::build(protocol, self.peer_id.clone(), dest, self.max_ttl, payload)?;
        self.transport
            .send_wire_to_route(route_key, packet.as_bytes())
            .await
    }

    fn self_peer_info(&self) -> PeerInfo {
        let mut addrs = vec![self.transport.local_addr()];
        if let Some(addr) = self.transport.local_tcp_addr() {
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
        }
        PeerInfo {
            peer_id: self.peer_id.clone(),
            addrs,
            relay_hint: None,
            last_seen: transport_now_millis(),
            is_direct: true,
            nat_info: Some(self.nat_info()),
        }
    }

    fn observation_from_route(&self, route_key: RouteKey) -> NatObservation {
        NatObservation {
            observed_protocol: route_key.protocol().into(),
            observed_addr: route_key.addr(),
            observer_peer_id: self.peer_id.clone(),
            observed_at: now_millis(),
        }
    }

    fn apply_observation(&self, observation: NatObservation) {
        let mut info = self.nat_info.write();
        apply_observation_to_nat_info(&mut info, &observation);
    }

    fn should_observe_with(&self, peer_id: &PeerId) -> bool {
        let observers = self.nat_observers.read();
        observers.is_empty() || observers.contains(peer_id)
    }

    fn has_direct_route(&self, peer_id: &PeerId) -> bool {
        self.transport
            .routes(peer_id.clone())
            .into_iter()
            .any(|route| route.is_direct())
    }

    fn peer_info_from_packet(&self, src: &PeerId, route_key: RouteKey, metric: u8) -> PeerInfo {
        let relay_hint = if metric > 0 {
            self.transport.route_to_id(&route_key)
        } else {
            None
        };
        PeerInfo {
            peer_id: src.clone(),
            addrs: if metric == 0 {
                vec![route_key.addr()]
            } else {
                Vec::new()
            },
            relay_hint,
            last_seen: now_millis(),
            is_direct: false,
            nat_info: self.peer_nat.get(src).map(|entry| entry.clone()),
        }
    }

    fn confirm_tcp_route(&self, peer: &PeerInfo, route_key: RouteKey, metric: u8) {
        if route_key.protocol().is_tcp() && metric == 0 {
            // TCP transports are connection-oriented, so a valid direct control
            // packet on TCP is enough to confirm bidirectional reachability.
            self.transport
                .confirm_peer_route(peer.clone(), route_key, metric);
        }
    }

    fn confirm_relay_route_from_control(&self, peer: &PeerInfo, route_key: RouteKey, metric: u8) {
        if metric > 0 {
            // Relayed control replies confirm that this next hop can carry
            // packets toward the source peer, but they do not make the peer
            // direct.
            self.transport
                .confirm_peer_route(peer.clone(), route_key, metric);
        }
    }

    fn is_confirmed_or_candidate(&self, peer_id: &PeerId, route_key: RouteKey, metric: u8) -> bool {
        if metric != 0 {
            return false;
        }
        self.transport
            .routes(peer_id.clone())
            .into_iter()
            .any(|route| route.route_key() == route_key && route.is_direct())
            || self
                .route_candidates
                .get(peer_id)
                .is_some_and(|route| *route == route_key)
    }

    fn is_confirmed_or_pending_nat_observe(
        &self,
        peer_id: &PeerId,
        route_key: RouteKey,
        metric: u8,
    ) -> bool {
        self.is_confirmed_or_candidate(peer_id, route_key, metric)
            || (metric == 0
                && self
                    .pending_nat_observe
                    .remove(peer_id)
                    .is_some_and(|(_, route)| route == route_key))
    }

    fn start_maintenance_loop(self: &Arc<Self>) {
        let protocol = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                if protocol.closed.load(Ordering::Relaxed) {
                    break;
                }
                for peer in protocol.transport.known_peers() {
                    if peer.peer_id != protocol.peer_id {
                        let _ = protocol.query_routes(peer.peer_id).await;
                    }
                }
            }
        });
    }
}

fn apply_stun_result_to_nat_info(info: &mut NatInfo, result: &rust_p2p_core::stun::StunResult) {
    info.nat_type = result.nat_type;
    info.public_port_range = result.port_range;
}

fn apply_observation_to_nat_info(info: &mut NatInfo, observation: &NatObservation) {
    match observation.observed_addr {
        SocketAddr::V4(addr) => {
            let ip = *addr.ip();
            if !info.public_ips.contains(&ip) {
                info.public_ips.push(ip);
            }
            match observation.observed_protocol {
                ObservedProtocol::Udp => {
                    if !info.public_udp_ports.contains(&addr.port()) {
                        info.public_udp_ports.push(addr.port());
                    }
                }
                ObservedProtocol::Tcp => {
                    info.public_tcp_port = addr.port();
                }
            }
        }
        SocketAddr::V6(addr) => {
            info.ipv6 = Some(*addr.ip());
            match observation.observed_protocol {
                ObservedProtocol::Udp => {
                    if !info.public_udp_ports.contains(&addr.port()) {
                        info.public_udp_ports.push(addr.port());
                    }
                }
                ObservedProtocol::Tcp => {
                    info.public_tcp_port = addr.port();
                }
            }
        }
    }
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn encode_msg<T: Message>(msg: &T) -> Vec<u8> {
    msg.encode_to_vec()
}

pub(crate) fn decode_msg<T: Message + Default>(payload: &[u8]) -> io::Result<T> {
    // Only protocol payloads use protobuf. The outer packet header remains the
    // compact rustp2p header so relays can update TTL without decoding a full
    // protobuf envelope.
    T::decode(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("protobuf: {e}")))
}

fn encode_hello_payload(payload: &HelloPayload) -> Vec<u8> {
    encode_msg(&hello_payload_to_pb(payload))
}

fn decode_hello_payload(payload: &[u8]) -> io::Result<HelloPayload> {
    hello_payload_from_pb(decode_msg(payload)?)
}

fn encode_route_reply_payload(payload: &RouteReplyPayload) -> Vec<u8> {
    encode_msg(&route_reply_payload_to_pb(payload))
}

fn decode_route_reply_payload(payload: &[u8]) -> io::Result<RouteReplyPayload> {
    route_reply_payload_from_pb(decode_msg(payload)?)
}

fn encode_punch_payload(payload: &PunchPayload) -> Vec<u8> {
    encode_msg(&punch_payload_to_pb(payload))
}

fn decode_punch_payload(payload: &[u8]) -> io::Result<PunchPayload> {
    punch_payload_from_pb(decode_msg(payload)?)
}

fn encode_nat_observe_reply_payload(payload: &NatObserveReplyPayload) -> Vec<u8> {
    encode_msg(&nat_observe_reply_payload_to_pb(payload))
}

fn decode_nat_observe_reply_payload(payload: &[u8]) -> io::Result<NatObserveReplyPayload> {
    nat_observe_reply_payload_from_pb(decode_msg(payload)?)
}

pub(crate) fn encode_stream_frame(frame: &StreamFrame) -> Vec<u8> {
    let frame = match frame {
        StreamFrame::User(header) => pb::stream_frame::Frame::User(stream_header_to_pb(header)),
    };
    encode_msg(&pb::StreamFrame { frame: Some(frame) })
}

pub(crate) fn decode_stream_frame(payload: &[u8]) -> io::Result<StreamFrame> {
    let frame: pb::StreamFrame = decode_msg(payload)?;
    match required(frame.frame, "stream_frame.frame")? {
        pb::stream_frame::Frame::User(header) => {
            Ok(StreamFrame::User(stream_header_from_pb(header)))
        }
    }
}

pub(crate) fn encode_datagram_frame(frame: &DatagramFrame) -> Vec<u8> {
    let frame = match frame {
        DatagramFrame::User {
            id,
            src,
            dest,
            payload,
        } => pb::datagram_frame::Frame::User(pb::UserDatagram {
            id: *id,
            src: src.as_str().to_string(),
            dest: dest.as_str().to_string(),
            payload: payload.clone(),
        }),
    };
    encode_msg(&pb::DatagramFrame { frame: Some(frame) })
}

pub(crate) fn decode_datagram_frame(payload: &[u8]) -> io::Result<DatagramFrame> {
    let frame: pb::DatagramFrame = decode_msg(payload)?;
    match required(frame.frame, "datagram_frame.frame")? {
        pb::datagram_frame::Frame::User(user) => Ok(DatagramFrame::User {
            id: user.id,
            src: PeerId::from(user.src),
            dest: PeerId::from(user.dest),
            payload: user.payload,
        }),
    }
}

fn hello_payload_to_pb(payload: &HelloPayload) -> pb::HelloPayload {
    pb::HelloPayload {
        peer: Some(peer_info_to_pb(&payload.peer)),
        peers: payload.peers.iter().map(route_entry_to_pb).collect(),
        observed_addr: payload.observed_addr.as_ref().map(nat_observation_to_pb),
    }
}

fn hello_payload_from_pb(payload: pb::HelloPayload) -> io::Result<HelloPayload> {
    Ok(HelloPayload {
        peer: peer_info_from_pb(required(payload.peer, "hello.peer")?)?,
        peers: payload
            .peers
            .into_iter()
            .map(route_entry_from_pb)
            .collect::<io::Result<_>>()?,
        observed_addr: payload
            .observed_addr
            .map(nat_observation_from_pb)
            .transpose()?,
    })
}

fn route_reply_payload_to_pb(payload: &RouteReplyPayload) -> pb::RouteReplyPayload {
    pb::RouteReplyPayload {
        peers: payload.peers.iter().map(route_entry_to_pb).collect(),
    }
}

fn route_reply_payload_from_pb(payload: pb::RouteReplyPayload) -> io::Result<RouteReplyPayload> {
    Ok(RouteReplyPayload {
        peers: payload
            .peers
            .into_iter()
            .map(route_entry_from_pb)
            .collect::<io::Result<_>>()?,
    })
}

fn route_entry_to_pb(entry: &RouteEntry) -> pb::RouteEntry {
    pb::RouteEntry {
        peer: Some(peer_info_to_pb(&entry.peer)),
        metric: u32::from(entry.metric),
    }
}

fn route_entry_from_pb(entry: pb::RouteEntry) -> io::Result<RouteEntry> {
    Ok(RouteEntry {
        peer: peer_info_from_pb(required(entry.peer, "route_entry.peer")?)?,
        metric: checked_u8("route_entry.metric", entry.metric)?,
    })
}

fn punch_payload_to_pb(payload: &PunchPayload) -> pb::PunchPayload {
    pb::PunchPayload {
        request_id: payload.request_id,
        src: payload.src.as_str().to_string(),
        dest: payload.dest.as_str().to_string(),
        nat_info: Some(nat_info_to_pb(&payload.nat_info)),
    }
}

fn punch_payload_from_pb(payload: pb::PunchPayload) -> io::Result<PunchPayload> {
    Ok(PunchPayload {
        request_id: payload.request_id,
        src: PeerId::from(payload.src),
        dest: PeerId::from(payload.dest),
        nat_info: nat_info_from_pb(required(payload.nat_info, "punch.nat_info")?)?,
    })
}

fn nat_observe_reply_payload_to_pb(payload: &NatObserveReplyPayload) -> pb::NatObserveReplyPayload {
    pb::NatObserveReplyPayload {
        observation: Some(nat_observation_to_pb(&payload.observation)),
    }
}

fn nat_observe_reply_payload_from_pb(
    payload: pb::NatObserveReplyPayload,
) -> io::Result<NatObserveReplyPayload> {
    Ok(NatObserveReplyPayload {
        observation: nat_observation_from_pb(required(
            payload.observation,
            "nat_observe_reply.observation",
        )?)?,
    })
}

fn peer_info_to_pb(peer: &PeerInfo) -> pb::PeerInfo {
    pb::PeerInfo {
        peer_id: peer.peer_id.as_str().to_string(),
        addrs: peer.addrs.iter().map(ToString::to_string).collect(),
        relay_hint: peer
            .relay_hint
            .as_ref()
            .map(|peer_id| peer_id.as_str())
            .unwrap_or_default()
            .to_string(),
        last_seen: peer.last_seen,
        is_direct: peer.is_direct,
        nat_info: peer.nat_info.as_ref().map(nat_info_to_pb),
    }
}

fn peer_info_from_pb(peer: pb::PeerInfo) -> io::Result<PeerInfo> {
    Ok(PeerInfo {
        peer_id: PeerId::from(peer.peer_id),
        addrs: peer
            .addrs
            .iter()
            .map(|addr| parse_socket_addr("peer.addrs", addr))
            .collect::<io::Result<_>>()?,
        relay_hint: (!peer.relay_hint.is_empty()).then(|| PeerId::from(peer.relay_hint)),
        last_seen: peer.last_seen,
        is_direct: peer.is_direct,
        nat_info: peer.nat_info.map(nat_info_from_pb).transpose()?,
    })
}

fn nat_info_to_pb(info: &NatInfo) -> pb::NatInfo {
    pb::NatInfo {
        nat_type: match info.nat_type {
            NatType::Cone => pb::NatType::Cone as i32,
            NatType::Symmetric => pb::NatType::Symmetric as i32,
        },
        public_ips: info.public_ips.iter().map(ToString::to_string).collect(),
        public_udp_ports: info
            .public_udp_ports
            .iter()
            .map(|port| u32::from(*port))
            .collect(),
        mapping_tcp_addr: info
            .mapping_tcp_addr
            .iter()
            .map(ToString::to_string)
            .collect(),
        mapping_udp_addr: info
            .mapping_udp_addr
            .iter()
            .map(ToString::to_string)
            .collect(),
        public_port_range: u32::from(info.public_port_range),
        local_ipv4: info.local_ipv4.to_string(),
        local_ipv4s: info.local_ipv4s.iter().map(ToString::to_string).collect(),
        ipv6: info.ipv6.map(|ip| ip.to_string()).unwrap_or_default(),
        local_udp_ports: info
            .local_udp_ports
            .iter()
            .map(|port| u32::from(*port))
            .collect(),
        local_tcp_port: u32::from(info.local_tcp_port),
        public_tcp_port: u32::from(info.public_tcp_port),
    }
}

fn nat_info_from_pb(info: pb::NatInfo) -> io::Result<NatInfo> {
    let nat_type = match pb::NatType::try_from(info.nat_type).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid nat type: {}", info.nat_type),
        )
    })? {
        pb::NatType::Cone => NatType::Cone,
        pb::NatType::Symmetric => NatType::Symmetric,
    };
    Ok(NatInfo {
        nat_type,
        public_ips: info
            .public_ips
            .iter()
            .map(|ip| parse_ipv4("nat.public_ips", ip))
            .collect::<io::Result<_>>()?,
        public_udp_ports: checked_u16_vec("nat.public_udp_ports", &info.public_udp_ports)?,
        mapping_tcp_addr: info
            .mapping_tcp_addr
            .iter()
            .map(|addr| parse_socket_addr("nat.mapping_tcp_addr", addr))
            .collect::<io::Result<_>>()?,
        mapping_udp_addr: info
            .mapping_udp_addr
            .iter()
            .map(|addr| parse_socket_addr("nat.mapping_udp_addr", addr))
            .collect::<io::Result<_>>()?,
        public_port_range: checked_u16("nat.public_port_range", info.public_port_range)?,
        local_ipv4: parse_ipv4_or_unspecified("nat.local_ipv4", &info.local_ipv4)?,
        local_ipv4s: info
            .local_ipv4s
            .iter()
            .map(|ip| parse_ipv4("nat.local_ipv4s", ip))
            .collect::<io::Result<_>>()?,
        ipv6: if info.ipv6.is_empty() {
            None
        } else {
            Some(parse_ipv6("nat.ipv6", &info.ipv6)?)
        },
        local_udp_ports: checked_u16_vec("nat.local_udp_ports", &info.local_udp_ports)?,
        local_tcp_port: checked_u16("nat.local_tcp_port", info.local_tcp_port)?,
        public_tcp_port: checked_u16("nat.public_tcp_port", info.public_tcp_port)?,
    })
}

fn nat_observation_to_pb(observation: &NatObservation) -> pb::NatObservation {
    pb::NatObservation {
        observed_protocol: match observation.observed_protocol {
            ObservedProtocol::Udp => pb::ObservedProtocol::Udp as i32,
            ObservedProtocol::Tcp => pb::ObservedProtocol::Tcp as i32,
        },
        observed_addr: observation.observed_addr.to_string(),
        observer_peer_id: observation.observer_peer_id.as_str().to_string(),
        observed_at: observation.observed_at,
    }
}

fn nat_observation_from_pb(observation: pb::NatObservation) -> io::Result<NatObservation> {
    let observed_protocol = match pb::ObservedProtocol::try_from(observation.observed_protocol)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid observed protocol: {}",
                    observation.observed_protocol
                ),
            )
        })? {
        pb::ObservedProtocol::Udp => ObservedProtocol::Udp,
        pb::ObservedProtocol::Tcp => ObservedProtocol::Tcp,
    };
    Ok(NatObservation {
        observed_protocol,
        observed_addr: parse_socket_addr(
            "nat_observation.observed_addr",
            &observation.observed_addr,
        )?,
        observer_peer_id: PeerId::from(observation.observer_peer_id),
        observed_at: observation.observed_at,
    })
}

fn stream_header_to_pb(header: &StreamHeader) -> pb::StreamHeader {
    pb::StreamHeader {
        src: header.src.as_str().to_string(),
        dest: header.dest.as_str().to_string(),
    }
}

fn stream_header_from_pb(header: pb::StreamHeader) -> StreamHeader {
    StreamHeader {
        src: PeerId::from(header.src),
        dest: PeerId::from(header.dest),
    }
}

fn required<T>(value: Option<T>, field: &str) -> io::Result<T> {
    value.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("missing protobuf field: {field}"),
        )
    })
}

fn parse_socket_addr(field: &str, value: &str) -> io::Result<SocketAddr> {
    value.parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {field}: {value}: {e}"),
        )
    })
}

fn parse_ipv4(field: &str, value: &str) -> io::Result<Ipv4Addr> {
    value.parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {field}: {value}: {e}"),
        )
    })
}

fn parse_ipv4_or_unspecified(field: &str, value: &str) -> io::Result<Ipv4Addr> {
    if value.is_empty() {
        Ok(Ipv4Addr::UNSPECIFIED)
    } else {
        parse_ipv4(field, value)
    }
}

fn parse_ipv6(field: &str, value: &str) -> io::Result<Ipv6Addr> {
    value.parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {field}: {value}: {e}"),
        )
    })
}

fn checked_u16(field: &str, value: u32) -> io::Result<u16> {
    u16::try_from(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{field} exceeds u16: {value}"),
        )
    })
}

fn checked_u8(field: &str, value: u32) -> io::Result<u8> {
    u8::try_from(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{field} exceeds u8: {value}"),
        )
    })
}

fn checked_u16_vec(field: &str, values: &[u32]) -> io::Result<Vec<u16>> {
    values
        .iter()
        .map(|value| checked_u16(field, *value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        apply_observation_to_nat_info, apply_stun_result_to_nat_info, classify_packet,
        decode_datagram_frame, decode_hello_payload, decode_nat_observe_reply_payload,
        decode_punch_payload, decode_route_reply_payload, decode_stream_frame,
        encode_datagram_frame, encode_hello_payload, encode_nat_observe_reply_payload,
        encode_punch_payload, encode_route_reply_payload, encode_stream_frame, DatagramFrame,
        HelloPayload, NatObservation, NatObserveReplyPayload, ObservedProtocol, Packet, PacketType,
        ProtocolType, PunchPayload, RouteEntry, RouteReplyPayload, StreamFrame, StreamHeader,
        VERSION,
    };
    use crate::transport::PeerInfo;
    use crate::PeerId;
    use rust_p2p_core::nat::{NatInfo, NatType};
    use rust_p2p_core::stun::StunResult;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn packet_round_trip_for_quic_relay() {
        let src = PeerId::from("node-a");
        let dest = PeerId::from("node-b");
        let packet = Packet::build(
            ProtocolType::QuicRelay,
            src.clone(),
            dest.clone(),
            8,
            b"encrypted-quic-packet",
        )
        .unwrap();
        assert_eq!(packet.protocol().unwrap(), ProtocolType::QuicRelay);
        assert_eq!(packet.src(), src);
        assert_eq!(packet.dest(), dest);
        assert_eq!(packet.payload(), b"encrypted-quic-packet");
        let parsed = Packet::parse(packet.into_bytes()).unwrap();
        assert_eq!(parsed.payload(), b"encrypted-quic-packet");
    }

    #[test]
    fn packet_rejects_v2_header() {
        let mut bytes = Packet::build(
            ProtocolType::QuicRelay,
            PeerId::from("node-a"),
            PeerId::from("node-b"),
            8,
            b"payload",
        )
        .unwrap()
        .into_bytes();
        bytes[1] = 2;

        let err = Packet::parse(bytes).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(VERSION, 3);
    }

    #[test]
    fn protobuf_control_payloads_round_trip() {
        let peer = sample_peer();
        let observed_udp = observation(ObservedProtocol::Udp, "203.0.113.1:4567");
        let hello = HelloPayload {
            peer: peer.clone(),
            peers: vec![RouteEntry {
                peer: peer.clone(),
                metric: 2,
            }],
            observed_addr: Some(observed_udp.clone()),
        };
        let decoded_hello = decode_hello_payload(&encode_hello_payload(&hello)).unwrap();
        assert_eq!(decoded_hello.peer.peer_id, peer.peer_id);
        assert_eq!(decoded_hello.peers[0].metric, 2);
        assert_eq!(
            decoded_hello.observed_addr.unwrap().observed_addr,
            observed_udp.observed_addr
        );
        assert_nat_eq(
            decoded_hello.peer.nat_info.as_ref().unwrap(),
            peer.nat_info.as_ref().unwrap(),
        );

        let route_reply = RouteReplyPayload {
            peers: vec![RouteEntry {
                peer: sample_peer(),
                metric: 3,
            }],
        };
        let decoded_route =
            decode_route_reply_payload(&encode_route_reply_payload(&route_reply)).unwrap();
        assert_eq!(decoded_route.peers[0].metric, 3);

        let punch = PunchPayload {
            request_id: 99,
            src: PeerId::from("node-a"),
            dest: PeerId::from("node-b"),
            nat_info: sample_nat_info(),
        };
        let decoded_punch = decode_punch_payload(&encode_punch_payload(&punch)).unwrap();
        assert_eq!(decoded_punch.request_id, 99);
        assert_eq!(decoded_punch.src, punch.src);
        assert_eq!(decoded_punch.dest, punch.dest);
        assert_nat_eq(&decoded_punch.nat_info, &punch.nat_info);

        let nat_observe = NatObserveReplyPayload {
            observation: observation(ObservedProtocol::Tcp, "203.0.113.2:7788"),
        };
        let decoded_observe =
            decode_nat_observe_reply_payload(&encode_nat_observe_reply_payload(&nat_observe))
                .unwrap();
        assert_eq!(
            decoded_observe.observation.observed_protocol,
            ObservedProtocol::Tcp
        );
    }

    #[test]
    fn protobuf_quic_frames_round_trip() {
        let stream = StreamFrame::User(StreamHeader {
            src: PeerId::from("node-a"),
            dest: PeerId::from("node-b"),
        });
        let StreamFrame::User(decoded_stream) =
            decode_stream_frame(&encode_stream_frame(&stream)).unwrap();
        assert_eq!(decoded_stream.src, PeerId::from("node-a"));
        assert_eq!(decoded_stream.dest, PeerId::from("node-b"));

        let datagram = DatagramFrame::User {
            id: 42,
            src: PeerId::from("node-a"),
            dest: PeerId::from("node-b"),
            payload: b"hello".to_vec(),
        };
        let DatagramFrame::User {
            id,
            src,
            dest,
            payload,
        } = decode_datagram_frame(&encode_datagram_frame(&datagram)).unwrap();
        assert_eq!(id, 42);
        assert_eq!(src, PeerId::from("node-a"));
        assert_eq!(dest, PeerId::from("node-b"));
        assert_eq!(payload, b"hello");
    }

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

    #[test]
    fn stun_update_does_not_fill_public_ports_or_ipv6() {
        let mut info = NatInfo::default();
        let result = StunResult {
            nat_type: NatType::Symmetric,
            public_ipv4: vec![Ipv4Addr::new(203, 0, 113, 10)],
            public_ipv6: Some(Ipv6Addr::LOCALHOST),
            public_udp_ports: vec![34567],
            port_range: 42,
        };

        apply_stun_result_to_nat_info(&mut info, &result);

        assert_eq!(info.nat_type, NatType::Symmetric);
        assert_eq!(info.public_port_range, 42);
        assert!(info.public_ips.is_empty());
        assert!(info.public_udp_ports.is_empty());
        assert!(info.ipv6.is_none());
    }

    #[test]
    fn udp_ipv4_observation_updates_public_udp_addr() {
        let mut info = NatInfo::default();
        apply_observation_to_nat_info(
            &mut info,
            &observation(ObservedProtocol::Udp, "203.0.113.1:4567"),
        );

        assert_eq!(info.public_ips, vec![Ipv4Addr::new(203, 0, 113, 1)]);
        assert_eq!(info.public_udp_ports, vec![4567]);
        assert_eq!(info.public_tcp_port, 0);
    }

    #[test]
    fn tcp_ipv4_observation_updates_public_tcp_port() {
        let mut info = NatInfo::default();
        apply_observation_to_nat_info(
            &mut info,
            &observation(ObservedProtocol::Tcp, "203.0.113.2:7788"),
        );

        assert_eq!(info.public_ips, vec![Ipv4Addr::new(203, 0, 113, 2)]);
        assert_eq!(info.public_tcp_port, 7788);
        assert!(info.public_udp_ports.is_empty());
    }

    #[test]
    fn ipv6_observation_updates_ipv6_and_protocol_port() {
        let mut info = NatInfo::default();
        apply_observation_to_nat_info(
            &mut info,
            &observation(ObservedProtocol::Udp, "[2001:db8::1]:9000"),
        );

        assert_eq!(info.ipv6, Some("2001:db8::1".parse().unwrap()));
        assert_eq!(info.public_udp_ports, vec![9000]);
    }

    fn observation(protocol: ObservedProtocol, addr: &str) -> NatObservation {
        NatObservation {
            observed_protocol: protocol,
            observed_addr: addr.parse::<SocketAddr>().unwrap(),
            observer_peer_id: PeerId::from("observer"),
            observed_at: 1,
        }
    }

    fn sample_peer() -> PeerInfo {
        PeerInfo {
            peer_id: PeerId::from("node-a"),
            addrs: vec!["127.0.0.1:7001".parse().unwrap()],
            relay_hint: Some(PeerId::from("relay")),
            last_seen: 123,
            is_direct: true,
            nat_info: Some(sample_nat_info()),
        }
    }

    fn sample_nat_info() -> NatInfo {
        NatInfo {
            nat_type: NatType::Symmetric,
            public_ips: vec![
                Ipv4Addr::new(203, 0, 113, 1),
                Ipv4Addr::new(198, 51, 100, 2),
            ],
            public_udp_ports: vec![4567, 4568],
            mapping_tcp_addr: vec!["192.0.2.10:6000".parse().unwrap()],
            mapping_udp_addr: vec!["192.0.2.10:6001".parse().unwrap()],
            public_port_range: 32,
            local_ipv4: Ipv4Addr::new(10, 0, 0, 2),
            local_ipv4s: vec![Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(10, 0, 0, 3)],
            ipv6: Some("2001:db8::1".parse().unwrap()),
            local_udp_ports: vec![7001, 7002],
            local_tcp_port: 7003,
            public_tcp_port: 9443,
        }
    }

    fn assert_nat_eq(left: &NatInfo, right: &NatInfo) {
        assert_eq!(left.nat_type, right.nat_type);
        assert_eq!(left.public_ips, right.public_ips);
        assert_eq!(left.public_udp_ports, right.public_udp_ports);
        assert_eq!(left.mapping_tcp_addr, right.mapping_tcp_addr);
        assert_eq!(left.mapping_udp_addr, right.mapping_udp_addr);
        assert_eq!(left.public_port_range, right.public_port_range);
        assert_eq!(left.local_ipv4, right.local_ipv4);
        assert_eq!(left.local_ipv4s, right.local_ipv4s);
        assert_eq!(left.ipv6, right.ipv6);
        assert_eq!(left.local_udp_ports, right.local_udp_ports);
        assert_eq!(left.local_tcp_port, right.local_tcp_port);
        assert_eq!(left.public_tcp_port, right.public_tcp_port);
    }
}
