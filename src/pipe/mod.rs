use std::collections::HashMap;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::time::UNIX_EPOCH;

use async_shutdown::ShutdownManager;
use rust_p2p_core::route::route_table::RouteTable;
use rust_p2p_core::route::{Route, RouteKey};
use tokio::sync::mpsc::Sender;

pub use pipe_context::NodeAddress;
pub use pipe_context::PeerNodeAddress;
use rust_p2p_core::punch::PunchConsultInfo;
pub use send_packet::SendPacket;

use crate::config::PipeConfig;
use crate::error::{Error, Result};
use crate::pipe::pipe_context::PipeContext;
use crate::protocol::broadcast::RangeBroadcastPacket;
use crate::protocol::id_route::IDRouteReplyPacket;
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::{broadcast, NetPacket, HEAD_LEN};

mod maintain;
mod pipe_context;

mod send_packet;

pub struct Pipe {
    send_buffer_size: usize,
    pipe_context: PipeContext,
    pipe: rust_p2p_core::pipe::Pipe<NodeID>,
    shutdown_manager: ShutdownManager<()>,
    active_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    passive_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
}

impl Pipe {
    pub async fn new(mut config: PipeConfig) -> Result<Pipe> {
        let send_buffer_size = config.send_buffer_size;
        let query_id_interval = config.query_id_interval;
        let query_id_max_num = config.query_id_max_num;
        let heartbeat_interval = config.heartbeat_interval;
        let self_id = config.self_id.take();
        let direct_addrs = config.direct_addrs.take();
        let mapping_addrs = config.mapping_addrs.take();
        let dns = config.dns.take();
        let default_interface = if let Some(v) = &config.udp_pipe_config {
            v.default_interface.clone()
        } else {
            None
        };
        let mut tcp_stun_servers = config.tcp_stun_servers.take().unwrap_or_default();
        for x in tcp_stun_servers.iter_mut() {
            if !x.contains(":") {
                x.push_str(":3478");
            }
        }
        let mut udp_stun_servers = config.udp_stun_servers.take().unwrap_or_default();
        for x in udp_stun_servers.iter_mut() {
            if !x.contains(":") {
                x.push_str(":3478");
            }
        }
        let (pipe, puncher, idle_route_manager) =
            rust_p2p_core::pipe::pipe::<NodeID>(config.into())?;
        let writer_ref = pipe.writer_ref();
        let local_tcp_port = if let Some(v) = writer_ref.tcp_pipe_writer_ref() {
            v.local_addr().port()
        } else {
            0
        };
        let local_udp_ports = if let Some(v) = writer_ref.udp_pipe_writer_ref() {
            v.local_ports()?
        } else {
            vec![]
        };
        let pipe_context = PipeContext::new(
            local_udp_ports,
            local_tcp_port,
            default_interface.clone(),
            dns,
        );
        if let Some(node_id) = self_id {
            pipe_context.store_self_id(node_id)?;
        }
        if let Some(addrs) = direct_addrs {
            pipe_context.set_direct_nodes(addrs);
        }
        pipe_context.update_direct_nodes().await?;
        if let Some(addrs) = mapping_addrs {
            pipe_context.set_mapping_addrs(addrs);
        }
        let shutdown_manager = ShutdownManager::<()>::new();
        let pipe_writer = PipeWriter {
            send_buffer_size,
            pipe_context: pipe_context.clone(),
            pipe_writer: pipe.writer_ref().to_owned(),
            shutdown_manager: shutdown_manager.clone(),
        };
        let (active_punch_sender, active_punch_receiver) = tokio::sync::mpsc::channel(3);
        let (passive_punch_sender, passive_punch_receiver) = tokio::sync::mpsc::channel(3);
        let mut join_set = maintain::start_task(
            &pipe_writer,
            idle_route_manager,
            puncher,
            query_id_interval,
            query_id_max_num,
            heartbeat_interval,
            tcp_stun_servers,
            udp_stun_servers,
            default_interface,
            active_punch_receiver,
            passive_punch_receiver,
        );
        let fut = shutdown_manager
            .wrap_cancel(async move { while join_set.join_next().await.is_some() {} });
        tokio::spawn(async move {
            if fut.await.is_err() {
                log::info!("maintain tasks are shutdown");
            }
        });
        Ok(Self {
            send_buffer_size,
            pipe_context,
            pipe,
            shutdown_manager,
            active_punch_sender,
            passive_punch_sender,
        })
    }
    pub fn writer(&self) -> PipeWriter {
        PipeWriter {
            send_buffer_size: self.send_buffer_size,
            pipe_context: self.pipe_context.clone(),
            pipe_writer: self.pipe.writer_ref().to_owned(),
            shutdown_manager: self.shutdown_manager.clone(),
        }
    }
}

impl Pipe {
    pub async fn accept(&mut self) -> Result<PipeLine> {
        let Ok(pipe_line) = self.shutdown_manager.wrap_cancel(self.pipe.accept()).await else {
            return Err(Error::ShutDown);
        };
        let pipe_line = pipe_line?;
        Ok(PipeLine {
            pipe_context: self.pipe_context.clone(),
            pipe_line,
            pipe_writer: self.writer(),
            route_table: self.pipe.route_table().clone(),
            active_punch_sender: self.active_punch_sender.clone(),
            passive_punch_sender: self.passive_punch_sender.clone(),
        })
    }
}

#[derive(Clone)]
pub struct PipeWriter {
    send_buffer_size: usize,
    pipe_context: PipeContext,
    pipe_writer: rust_p2p_core::pipe::PipeWriter<NodeID>,
    shutdown_manager: ShutdownManager<()>,
}

impl PipeWriter {
    pub fn pipe_context(&self) -> &PipeContext {
        &self.pipe_context
    }
    pub(crate) async fn send_to_id<B: AsRef<[u8]>>(
        &self,
        buf: &NetPacket<B>,
        id: &NodeID,
    ) -> Result<()> {
        self.pipe_writer.send_to_id(buf.buffer(), id).await?;
        Ok(())
    }
    pub(crate) async fn send_to_route(&self, buf: &[u8], route_key: &RouteKey) -> Result<()> {
        self.pipe_writer.send_to(buf, route_key).await?;
        Ok(())
    }
    async fn send_to0(&self, buf: &[u8], src_id: &NodeID, dest_id: &NodeID) -> Result<()> {
        if dest_id.is_broadcast() {
            self.send_broadcast0(buf, src_id).await;
            return Ok(());
        }

        if let Ok(route) = self.pipe_writer.route_table().get_route_by_id(dest_id) {
            self.pipe_writer.send_to(buf, &route.route_key()).await?
        } else if let Some(turn_id) = self.pipe_context().reachable_node(dest_id) {
            self.pipe_writer.send_to_id(buf, &turn_id).await?
        } else {
            Err(Error::NodeIDNotAvailable)?
        };
        Ok(())
    }
    async fn send_multiple_to0(
        &self,
        bufs: &[IoSlice<'_>],
        src_id: &NodeID,
        dest_id: &NodeID,
    ) -> Result<()> {
        if dest_id.is_broadcast() {
            for buf in bufs {
                self.send_broadcast0(buf.as_ref(), src_id).await;
            }
            return Ok(());
        }

        if let Ok(route) = self.pipe_writer.route_table().get_route_by_id(dest_id) {
            self.pipe_writer
                .send_multiple_to(bufs, &route.route_key())
                .await?
        } else if let Some(turn_id) = self.pipe_context().reachable_node(dest_id) {
            self.pipe_writer.send_multiple_to_id(bufs, &turn_id).await?
        } else {
            Err(Error::NodeIDNotAvailable)?
        };
        Ok(())
    }
    async fn send_broadcast0(&self, buf: &[u8], src_id: &NodeID) {
        let broadcast_id = NodeID::broadcast();
        let route_table = self.pipe_writer.route_table();
        let table = route_table.route_table_one();
        let mut map: HashMap<NodeID, (Vec<NodeID>, RouteKey)> = HashMap::new();
        for (id, route) in &table {
            if route.is_p2p() {
                if !map.contains_key(id) {
                    map.insert(*id, (vec![*id], route.route_key()));
                }
            } else if let Some(owner_id) = route_table.get_id_by_route_key(&route.route_key()) {
                if let Some((list, _)) = map.get_mut(&owner_id) {
                    list.push(*id);
                } else {
                    map.insert(owner_id, (vec![owner_id, *id], route.route_key()));
                }
            }
        }
        for (owner_id, (list, route_key)) in map {
            if list.is_empty() {
                if let Err(e) = self.pipe_writer.send_to(buf, &route_key).await {
                    log::debug!("send_broadcast0 {e:?} {owner_id:?}");
                }
            } else {
                match broadcast::Builder::build_range_broadcast(&list, buf) {
                    Ok(mut packet) => {
                        packet.set_src_id(src_id);
                        packet.set_dest_id(&broadcast_id);
                        if let Err(e) = self.pipe_writer.send_to(packet.buffer(), &route_key).await
                        {
                            log::debug!("send_range_broadcast {e:?} {owner_id:?}");
                        }
                    }
                    Err(e) => {
                        log::debug!("build_range_broadcast {e:?} {owner_id:?}");
                    }
                }
            }
        }
    }

    pub async fn broadcast_packet(&self, packet: &mut SendPacket) -> Result<()> {
        if let Some(src_id) = self.pipe_context.load_id() {
            packet.set_src_id(&src_id);
            packet.set_dest_id(&NodeID::broadcast());
            self.send_to0(packet.buf_mut(), &src_id, &NodeID::broadcast())
                .await?;
            Ok(())
        } else {
            Err(Error::NoIDSpecified)
        }
    }
    pub async fn send_to_packet(&self, packet: &mut SendPacket, dest_id: &NodeID) -> Result<()> {
        if let Some(src_id) = self.pipe_context.load_id() {
            packet.set_src_id(&src_id);
            packet.set_dest_id(dest_id);
            self.send_to0(packet.buf_mut(), &src_id, dest_id).await
        } else {
            Err(Error::NoIDSpecified)
        }
    }
    pub async fn send_multiple_to_packet(
        &self,
        packets: &mut [&mut SendPacket],
        dest_id: &NodeID,
    ) -> Result<()> {
        if let Some(src_id) = self.pipe_context.load_id() {
            for packet in packets.iter_mut() {
                packet.set_src_id(&src_id);
                packet.set_dest_id(dest_id);
            }
            let bufs: Vec<IoSlice> = packets.iter().map(|v| IoSlice::new(v.buf())).collect();
            self.send_multiple_to0(&bufs, &src_id, dest_id).await
        } else {
            Err(Error::NoIDSpecified)
        }
    }
    pub fn allocate_send_packet(&self) -> Result<SendPacket> {
        self.allocate_send_packet_proto(ProtocolType::UserData, self.send_buffer_size)
    }
    pub(crate) fn allocate_send_packet_proto(
        &self,
        protocol_type: ProtocolType,
        payload_size: usize,
    ) -> Result<SendPacket> {
        let src_id = if let Some(src_id) = self.pipe_context.load_id() {
            src_id
        } else {
            return Err(Error::NoIDSpecified);
        };
        let mut send_packet = SendPacket::allocate(payload_size);
        let mut packet = NetPacket::unchecked(send_packet.buf_mut());
        packet.set_high_flag();
        packet.set_protocol(protocol_type);
        packet.set_ttl(15);
        packet.set_src_id(&src_id);
        packet.set_dest_id(&NodeID::unspecified());
        packet.reset_data_len();
        Ok(send_packet)
    }
    pub fn shutdown(&self) -> Result<()> {
        self.shutdown_manager
            .trigger_shutdown(())
            .map_err(|_| Error::AlreadyShutdown)?;
        Ok(())
    }
}

pub struct PipeLine {
    pipe_context: PipeContext,
    pipe_line: rust_p2p_core::pipe::PipeLine,
    pipe_writer: PipeWriter,
    route_table: RouteTable<NodeID>,
    active_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    passive_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
}

impl PipeLine {
    pub fn store_self_id(&self, node_id: NodeID) -> Result<()> {
        self.pipe_context.store_self_id(node_id)
    }
    pub async fn recv_from(
        &mut self,
        buf: &mut [u8],
    ) -> core::result::Result<core::result::Result<HandleResult, HandleError>, RecvError> {
        loop {
            let (len, route_key) = match self.pipe_line.recv_from(buf).await {
                None => return Err(RecvError::Done),
                Some(recv_rs) => match recv_rs {
                    Ok(rs) => rs,
                    Err(e) => return Err(RecvError::Io(e)),
                },
            };
            if len == 0 {
                return Err(RecvError::Io(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )));
            }
            if rust_p2p_core::stun::is_stun_response(&buf[..len]) {
                if let Some(pub_addr) = rust_p2p_core::stun::recv_stun_response(&buf[..len]) {
                    self.pipe_context
                        .update_public_addr(route_key.index(), pub_addr);
                } else {
                    log::debug!("stun error {route_key:?}")
                }
                continue;
            }
            return match self
                .handle(RecvResult::new(&mut buf[..len], route_key))
                .await
            {
                Ok(handle_result) => {
                    if let Some(rs) = handle_result {
                        return Ok(Ok(rs));
                    } else {
                        continue;
                    }
                }
                Err(e) => Ok(Err(HandleError::new(route_key, e))),
            };
        }
    }
    pub(crate) async fn send_to<B: AsRef<[u8]>>(
        &self,
        buf: &NetPacket<B>,
        id: &NodeID,
    ) -> Result<()> {
        self.pipe_writer.send_to_id(buf, id).await
    }
    pub(crate) async fn send_to_route(&self, buf: &[u8], route_key: &RouteKey) -> Result<()> {
        self.pipe_writer.send_to_route(buf, route_key).await
    }
    async fn handle<'a>(&mut self, recv_result: RecvResult<'a>) -> Result<Option<HandleResult>> {
        let mut packet = NetPacket::new(recv_result.buf)?;
        let src_id = NodeID::try_from(packet.src_id())?;

        if src_id.is_unspecified() || src_id.is_broadcast() {
            return Err(Error::InvalidArgument("src id is unspecified".into()));
        }
        let dest_id = NodeID::try_from(packet.dest_id())?;

        if packet.max_ttl() < packet.ttl() {
            return Err(Error::InvalidArgument("ttl error".into()));
        }

        if packet.ttl() == 0 {
            return Ok(None);
        }
        let route_key = recv_result.route_key;

        let self_id = if let Some(self_id) = self.pipe_context.load_id() {
            if self_id == src_id {
                log::debug!("{packet:?}");
                return Err(Error::InvalidArgument("id loop error".into()));
            }
            if self_id != dest_id && !dest_id.is_unspecified() && !dest_id.is_broadcast() {
                if packet.incr_ttl() {
                    self.send_to(&packet, &dest_id).await?;
                }
                return Ok(None);
            }
            self_id
        } else {
            return Err(Error::InvalidArgument("self id is none".into()));
        };
        let metric = packet.max_ttl() - packet.ttl() + 1;
        self.route_table
            .add_route_if_absent(src_id, Route::from_default_rt(route_key, metric));
        match packet.protocol()? {
            ProtocolType::PunchRequest => {
                packet.set_protocol(ProtocolType::PunchReply);
                packet.set_ttl(packet.max_ttl());
                packet.set_dest_id(&src_id);
                packet.set_src_id(&self_id);
                self.send_to_route(packet.buffer(), &route_key).await?;
                log::debug!("===========PunchRequest {route_key:?} {src_id:?}")
            }
            ProtocolType::PunchReply => {
                log::debug!("===========PunchReply {route_key:?} {src_id:?}")
            }
            ProtocolType::EchoRequest => {
                packet.set_protocol(ProtocolType::EchoReply);
                packet.set_ttl(packet.max_ttl());
                packet.set_dest_id(&src_id);
                packet.set_src_id(&self_id);
                self.send_to_route(packet.buffer(), &route_key).await?;
            }
            ProtocolType::EchoReply => {}
            ProtocolType::TimestampRequest => {
                packet.set_protocol(ProtocolType::TimestampReply);
                packet.set_ttl(packet.max_ttl());
                packet.set_dest_id(&src_id);
                packet.set_src_id(&self_id);
                self.send_to_route(packet.buffer(), &route_key).await?;
            }
            ProtocolType::TimestampReply => {
                // update rtt
                let time = packet.payload();
                if time.len() != 4 {
                    return Err(Error::InvalidArgument("time error".into()));
                }
                let time = u32::from_be_bytes(time.try_into().unwrap());
                let now = std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)?
                    .as_millis() as u32;
                let rtt = now.saturating_sub(time);
                self.route_table
                    .add_route(src_id, Route::from(route_key, metric, rtt));
            }
            ProtocolType::IDRouteQuery => {
                let payload = packet.payload();
                if payload.len() != 4 {
                    return Err(Error::InvalidArgument("IDRouteQuery error".into()));
                }
                let query_id = u16::from_be_bytes(payload[2..].try_into().unwrap());
                // reply reachable node id
                let mut list = self.route_table.route_table_min_metric();
                // Not supporting too many nodes
                list.truncate(255);
                let list: Vec<_> = list
                    .into_iter()
                    .filter(|(node_id, _)| node_id != &src_id)
                    .map(|(node_id, route)| (node_id, route.metric()))
                    .collect();
                let mut packet = crate::protocol::id_route::Builder::build_reply(
                    &list,
                    query_id,
                    list.len() as _,
                )?;
                packet.set_dest_id(&src_id);
                packet.set_src_id(&self_id);
                self.send_to_route(packet.buffer(), &route_key).await?;
            }
            ProtocolType::IDRouteReply => {
                let reply_packet = IDRouteReplyPacket::new(packet.payload())?;
                let id = reply_packet.query_id();
                if id != 0 {
                    self.pipe_context.update_direct_node_id(id, src_id);
                }
                for (reachable_id, metric) in reply_packet.iter() {
                    if reachable_id == self_id {
                        continue;
                    }
                    self.pipe_context
                        .update_reachable_nodes(src_id, reachable_id, metric);
                }
            }
            ProtocolType::UserData => {
                return Ok(Some(HandleResult {
                    start: HEAD_LEN,
                    end: packet.buffer().len(),
                    src_id,
                    dest_id,
                    route_key,
                }))
            }
            ProtocolType::RangeBroadcast => {
                let end = packet.buffer().len();

                let broadcast_packet = RangeBroadcastPacket::new(packet.payload_mut())?;
                let in_packet = NetPacket::new(broadcast_packet.payload())?;
                let start = HEAD_LEN + broadcast_packet.head_len() + HEAD_LEN;
                let mut broadcast_to_self = false;

                let range_id: Vec<NodeID> = broadcast_packet.iter().collect();
                for node_id in range_id {
                    if node_id == self_id {
                        broadcast_to_self = true
                    } else if let Err(e) = self.send_to(&in_packet, &node_id).await {
                        log::debug!("RangeBroadcast {e:?}")
                    }
                }
                if broadcast_to_self {
                    return Ok(Some(HandleResult {
                        start,
                        end,
                        src_id,
                        dest_id,
                        route_key,
                    }));
                }
            }
            ProtocolType::PunchConsultRequest => {
                let punch_info = rmp_serde::from_slice::<PunchConsultInfo>(packet.payload())?;
                log::debug!("PunchConsultRequest {:?}", punch_info);
                let consult_info = self
                    .pipe_context
                    .gen_punch_info(punch_info.peer_nat_info.seq);
                let data = rmp_serde::to_vec(&consult_info)?;
                let mut send_packet = self
                    .pipe_writer
                    .allocate_send_packet_proto(ProtocolType::PunchConsultReply, data.len())?;
                send_packet.data_mut()[..data.len()].copy_from_slice(&data);
                send_packet.set_payload_len(data.len());
                if let Ok(sender) = self.passive_punch_sender.try_reserve() {
                    self.pipe_writer
                        .send_to_packet(&mut send_packet, &src_id)
                        .await?;
                    sender.send((src_id, punch_info))
                }
            }
            ProtocolType::PunchConsultReply => {
                let punch_info = rmp_serde::from_slice::<PunchConsultInfo>(packet.payload())?;
                log::debug!("PunchConsultReply {:?}", punch_info);

                if self
                    .active_punch_sender
                    .try_send((src_id, punch_info))
                    .is_err()
                {
                    log::debug!("active_punch_sender err src_id={self_id:?}");
                }
            }
        }

        Ok(None)
    }
}

pub struct RecvResult<'a> {
    buf: &'a mut [u8],
    route_key: RouteKey,
}

impl<'a> RecvResult<'a> {
    pub fn new(buf: &'a mut [u8], route_key: RouteKey) -> Self {
        Self { buf, route_key }
    }
    pub fn buf(&mut self) -> &mut [u8] {
        self.buf
    }
    pub fn remote_addr(&self) -> SocketAddr {
        self.route_key.addr()
    }
}

#[derive(Debug)]
pub struct HandleResult {
    pub start: usize,
    pub end: usize,
    pub src_id: NodeID,
    pub dest_id: NodeID,
    pub route_key: RouteKey,
}

#[derive(thiserror::Error, Debug)]
pub enum RecvError {
    #[error("done")]
    Done,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
#[error("handle error {route_key:?},err={err:?}")]
pub struct HandleError {
    route_key: RouteKey,
    err: Error,
}

impl HandleError {
    pub(crate) fn new(route_key: RouteKey, err: Error) -> Self {
        Self { route_key, err }
    }
    pub fn addr(&self) -> SocketAddr {
        self.route_key.addr()
    }
    pub fn err(&self) -> &Error {
        &self.err
    }
}
