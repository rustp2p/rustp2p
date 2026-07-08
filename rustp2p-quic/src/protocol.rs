use crate::transport::{now_millis as transport_now_millis, PeerInfo, RawTransportPacket};
use crate::transport::{TransportLayer, TransportPayload};
use crate::PeerId;
use bytes::Bytes;
use dashmap::DashMap;
use rust_p2p_core::nat::NatInfo;
use rust_p2p_core::punch::{PunchInfo, PunchModel};
use rust_p2p_core::route_table::RouteKey;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const VERSION: u8 = 2;
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
    nat_info: parking_lot::RwLock<NatInfo>,
    peer_nat: DashMap<PeerId, NatInfo>,
}

impl ProtocolLayer {
    pub(crate) fn new(
        peer_id: PeerId,
        transport: Arc<TransportLayer>,
        max_ttl: u8,
        punch_whitelist: Vec<PeerId>,
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
            nat_info: parking_lot::RwLock::new(NatInfo::default()),
            peer_nat: DashMap::new(),
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
        let (route, metric) = self.transport.route_metric_for(peer_id)?;
        Ok((route.route_key(), metric))
    }

    pub(crate) fn try_send_quic_payload(&self, peer_id: PeerId, payload: &[u8]) -> io::Result<()> {
        let packet = Packet::build(
            ProtocolType::QuicRelay,
            self.peer_id.clone(),
            peer_id.clone(),
            self.max_ttl,
            payload,
        )?;
        self.transport.try_send_wire(peer_id, packet.as_bytes())
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
        let encoded = bincode::serialize(&payload).map_err(bin_err)?;
        self.send_protocol(peer_id.clone(), ProtocolType::PunchRequest, &encoded)
            .await?;
        if let Some(nat_info) = self.peer_nat.get(&peer_id).map(|v| v.clone()) {
            self.execute_punch(peer_id, nat_info).await?;
        }
        Ok(())
    }

    pub(crate) fn start_nat_detection(
        self: &Arc<Self>,
        timeout: Duration,
        stun_servers: Vec<String>,
    ) {
        if stun_servers.is_empty() {
            return;
        }
        let protocol = self.clone();
        tokio::spawn(async move {
            loop {
                match rust_p2p_core::stun::stun_test_nat(stun_servers.clone(), None).await {
                    Ok(result) => {
                        let mut info = protocol.nat_info.write();
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
        let relay_hint = if metric > 0 {
            self.transport.route_to_id(&route_key)
        } else {
            None
        };
        self.transport.update_route_read_time(&src, &route_key);
        let known_nat = self.peer_nat.get(&src).map(|entry| entry.clone());
        self.transport.upsert_peer(
            PeerInfo {
                peer_id: src.clone(),
                addrs: if metric == 0 {
                    vec![route_key.addr()]
                } else {
                    Vec::new()
                },
                relay_hint,
                last_seen: now_millis(),
                is_direct: metric == 0,
                nat_info: known_nat,
            },
            route_key,
            metric,
        );

        let dest = packet.dest();
        if dest != self.peer_id && !dest.is_broadcast() {
            self.forward_packet(packet).await?;
            return Ok(());
        }

        match packet.protocol()? {
            ProtocolType::HelloRequest => {
                let payload =
                    bincode::serialize(&self.hello_payload(&src).await?).map_err(bin_err)?;
                self.send_protocol(src, ProtocolType::HelloReply, &payload)
                    .await?;
            }
            ProtocolType::HelloReply => {
                let hello: HelloPayload =
                    bincode::deserialize(packet.payload()).map_err(bin_err)?;
                let peer_id = hello.peer.peer_id.clone();
                if let Some(nat_info) = &hello.peer.nat_info {
                    self.peer_nat.insert(peer_id.clone(), nat_info.clone());
                }
                self.transport.upsert_peer(hello.peer, route_key, metric);
                let _ = self.hello_tx.send_async((route_key.addr(), peer_id)).await;
                self.ingest_route_entries(hello.peers, route_key).await?;
            }
            ProtocolType::IDRouteQuery => {
                let payload = bincode::serialize(&RouteReplyPayload {
                    peers: self.discovery_entries(&src).await?,
                })
                .map_err(bin_err)?;
                self.send_protocol(src, ProtocolType::IDRouteReply, &payload)
                    .await?;
            }
            ProtocolType::IDRouteReply => {
                let reply: RouteReplyPayload =
                    bincode::deserialize(packet.payload()).map_err(bin_err)?;
                self.ingest_route_entries(reply.peers, route_key).await?;
            }
            ProtocolType::QuicRelay => {
                let _ = self
                    .quic_tx
                    .send_async(TransportPayload {
                        payload: Bytes::copy_from_slice(packet.payload()),
                        src,
                    })
                    .await;
            }
            ProtocolType::PunchConsultRequest => {
                self.handle_punch_consult_request(packet).await?;
            }
            ProtocolType::PunchConsultReply => {
                self.handle_punch_payload(packet.payload()).await?;
            }
            ProtocolType::PunchRequest => {
                let payload = self.handle_punch_payload(packet.payload()).await?;
                if self.punch_allowed(&payload.src) {
                    self.execute_punch(payload.src.clone(), payload.nat_info)
                        .await?;
                    let reply = bincode::serialize(&self.punch_payload(payload.src.clone()))
                        .map_err(bin_err)?;
                    self.send_protocol(payload.src, ProtocolType::PunchReply, &reply)
                        .await?;
                }
            }
            ProtocolType::PunchReply => {
                let payload = self.handle_punch_payload(packet.payload()).await?;
                if self.punch_allowed(&payload.src) {
                    self.execute_punch(payload.src, payload.nat_info).await?;
                }
            }
            ProtocolType::EchoRequest => {
                self.send_protocol(src, ProtocolType::EchoReply, packet.payload())
                    .await?;
            }
            ProtocolType::TimestampRequest => {
                self.send_protocol(src, ProtocolType::TimestampReply, packet.payload())
                    .await?;
            }
            ProtocolType::MessageData
            | ProtocolType::RangeBroadcast
            | ProtocolType::EchoReply
            | ProtocolType::TimestampReply => {}
        }
        Ok(())
    }

    async fn handle_punch_consult_request(&self, packet: Packet) -> io::Result<()> {
        let payload = self.handle_punch_payload(packet.payload()).await?;
        if !self.punch_allowed(&payload.src) {
            return Ok(());
        }
        let reply =
            bincode::serialize(&self.punch_payload(payload.src.clone())).map_err(bin_err)?;
        self.send_protocol(payload.src, ProtocolType::PunchConsultReply, &reply)
            .await
    }

    async fn handle_punch_payload(&self, payload: &[u8]) -> io::Result<PunchPayload> {
        let payload: PunchPayload = bincode::deserialize(payload).map_err(bin_err)?;
        self.peer_nat
            .insert(payload.src.clone(), payload.nat_info.clone());
        Ok(payload)
    }

    async fn execute_punch(&self, peer_id: PeerId, peer_nat_info: NatInfo) -> io::Result<()> {
        let payload = bincode::serialize(&self.punch_payload(peer_id.clone())).map_err(bin_err)?;
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
        PunchPayload {
            request_id: rand::random(),
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
            self.transport.upsert_peer(item.peer.clone(), via, metric);
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

    async fn hello_payload(&self, requester: &PeerId) -> io::Result<HelloPayload> {
        Ok(HelloPayload {
            peer: self.self_peer_info(),
            peers: self.discovery_entries(requester).await?,
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

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn bin_err(err: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("bincode: {err}"))
}

#[cfg(test)]
mod tests {
    use super::{classify_packet, Packet, PacketType, ProtocolType};
    use crate::PeerId;

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
