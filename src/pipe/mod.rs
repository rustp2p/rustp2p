use std::collections::HashMap;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use async_shutdown::ShutdownManager;
use dashmap::DashMap;
use rust_p2p_core::nat::NatType;
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
use crate::protocol::node_id::{GroupCode, NodeID};
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
        let group_code = config.group_code.take();
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
            if !x.contains(':') {
                x.push_str(":3478");
            }
        }
        let mut udp_stun_servers = config.udp_stun_servers.take().unwrap_or_default();
        for x in udp_stun_servers.iter_mut() {
            if !x.contains(':') {
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
        if let Some(group_code) = group_code {
            pipe_context.store_group_code(group_code)?;
        }
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
                log::debug!("recv shutdown signal: built-in maintain tasks are shutdown");
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
    pub fn switch_model(&self, nat_type: NatType) -> Result<()> {
        use rust_p2p_core::pipe::udp_pipe::Model;
        match nat_type {
            NatType::Cone => {
                if let Some(writer) = self.pipe_writer.udp_pipe_writer() {
                    writer.switch_model(Model::Low)?;
                }
            }
            NatType::Symmetric => {
                if let Some(writer) = self.pipe_writer.udp_pipe_writer() {
                    writer.switch_model(Model::High)?;
                }
            }
        }
        Ok(())
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
    async fn send_to0(
        &self,
        buf: &[u8],
        group_code: &GroupCode,
        src_id: &NodeID,
        dest_id: &NodeID,
    ) -> Result<()> {
        if dest_id.is_broadcast() {
            self.send_broadcast0(buf, src_id).await;
            return Ok(());
        }

        if let Ok(route) = self.pipe_writer.route_table().get_route_by_id(dest_id) {
            self.pipe_writer.send_to(buf, &route.route_key()).await?
        } else if let Some((relay_group_code, relay_node_id)) =
            self.pipe_context().reachable_node(group_code, dest_id)
        {
            if &relay_group_code != group_code {
                self.pipe_writer.send_to_id(buf, &relay_node_id).await?
            } else {
                let route;
                if let Some(v) = self.pipe_context().other_route_table.get(&relay_group_code) {
                    route = v.get_route_by_id(&relay_node_id)?;
                } else {
                    return Err(Error::NodeIDNotAvailable);
                }
                self.pipe_writer.send_to(buf, &route.route_key()).await?
            }
        } else {
            Err(Error::NodeIDNotAvailable)?
        };
        Ok(())
    }
    async fn send_multiple_to0(
        &self,
        bufs: &[IoSlice<'_>],
        group_code: &GroupCode,
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
        } else if let Some((relay_group_code, relay_node_id)) =
            self.pipe_context().reachable_node(group_code, dest_id)
        {
            if &relay_group_code != group_code {
                self.pipe_writer
                    .send_multiple_to_id(bufs, &relay_node_id)
                    .await?
            } else {
                let route;
                if let Some(v) = self.pipe_context().other_route_table.get(&relay_group_code) {
                    route = v.get_route_by_id(&relay_node_id)?;
                } else {
                    return Err(Error::NodeIDNotAvailable);
                }
                self.pipe_writer
                    .send_multiple_to(bufs, &route.route_key())
                    .await?
            }
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
        self.send_packet_to(packet, &NodeID::broadcast()).await
    }
    pub async fn send_packet_to(&self, packet: &mut SendPacket, dest_id: &NodeID) -> Result<()> {
        let group_code = self.pipe_context.load_group_code();
        if let Some(src_id) = self.pipe_context.load_id() {
            packet.set_group_code(&group_code);
            packet.set_src_id(&src_id);
            packet.set_dest_id(dest_id);
            self.send_to0(packet.buf_mut(), &group_code, &src_id, dest_id)
                .await
        } else {
            Err(Error::NoIDSpecified)
        }
    }
    pub async fn send_multi_packet_to(
        &self,
        packets: &mut [&mut SendPacket],
        dest_id: &NodeID,
    ) -> Result<()> {
        let group_code = self.pipe_context.load_group_code();
        if let Some(src_id) = self.pipe_context.load_id() {
            for packet in packets.iter_mut() {
                packet.set_group_code(&group_code);
                packet.set_src_id(&src_id);
                packet.set_dest_id(dest_id);
            }
            let bufs: Vec<IoSlice> = packets.iter().map(|v| IoSlice::new(v.buf())).collect();
            self.send_multiple_to0(&bufs, &group_code, &src_id, dest_id)
                .await
        } else {
            Err(Error::NoIDSpecified)
        }
    }
    pub fn allocate_send_packet(&self) -> Result<SendPacket> {
        let mut packet =
            self.allocate_send_packet_proto(ProtocolType::UserData, self.send_buffer_size)?;
        unsafe {
            packet.set_payload_len(HEAD_LEN);
        }
        Ok(packet)
    }
    pub(crate) fn allocate_send_packet_proto(
        &self,
        protocol_type: ProtocolType,
        payload_size: usize,
    ) -> Result<SendPacket> {
        let group_code = self.pipe_context.load_group_code();
        let src_id = if let Some(src_id) = self.pipe_context.load_id() {
            src_id
        } else {
            return Err(Error::NoIDSpecified);
        };
        let mut send_packet = SendPacket::with_capacity(payload_size);
        unsafe {
            send_packet.set_payload_len(payload_size);
        }
        let mut packet = NetPacket::unchecked(send_packet.buf_mut());
        packet.set_high_flag();
        packet.set_protocol(protocol_type);
        packet.set_ttl(15);
        packet.set_group_code(&group_code);
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
    pub async fn recv_from<'a>(
        &mut self,
        buf: &'a mut [u8],
    ) -> core::result::Result<core::result::Result<HandleResult<'a>, HandleError>, RecvError> {
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
                        return Ok(Ok(HandleResult {
                            _src_id: rs.src_id,
                            _dest_id: rs.dest_id,
                            _route_key: rs.route_key,
                            _payload: &buf[rs.start..rs.end],
                            _ttl: rs.ttl,
                            _max_ttl: rs.max_ttl,
                        }));
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
    async fn other_group_handle(
        &mut self,
        mut packet: NetPacket<&mut [u8]>,
        route_key: RouteKey,
        self_group_code: GroupCode,
        self_id: NodeID,
    ) -> Result<()> {
        let metric = packet.max_ttl() - packet.ttl() + 1;
        let src_group_code = GroupCode::try_from(packet.group_code())?;
        let src_id = NodeID::try_from(packet.src_id())?;
        {
            let ref_mut = self
                .pipe_context
                .other_route_table
                .entry(src_group_code)
                .or_insert_with(|| RouteTable::new(false, 1));
            ref_mut.add_route_if_absent(src_id, Route::from_default_rt(route_key, metric));
        }
        match packet.protocol()? {
            ProtocolType::IDRouteQuery => {
                self.id_route_query_handle(
                    packet,
                    route_key,
                    self_group_code,
                    self_id,
                    src_group_code,
                    src_id,
                )
                .await?;
                return Ok(());
            }
            ProtocolType::IDRouteReply => {
                self.id_route_reply_handle(
                    packet,
                    self_group_code,
                    self_id,
                    src_group_code,
                    src_id,
                )
                .await?;
                return Ok(());
            }
            _ => {}
        }

        if !packet.incr_ttl() {
            return Ok(());
        }
        if src_id.is_unspecified() || src_id.is_broadcast() {
            return Err(Error::InvalidArgument("src id is unspecified".into()));
        }
        let dest_id = NodeID::try_from(packet.dest_id())?;
        if dest_id.is_unspecified() {
            return Ok(());
        }

        if let Some(route_table) = self.pipe_context.other_route_table.get(&src_group_code) {
            if dest_id.is_broadcast() {
                // broadcast
            } else if let Ok(v) = route_table.get_route_by_id(&dest_id) {
                self.send_to_route(packet.buffer(), &v.route_key()).await?;
            } else if let Some((relay_group_code, relay_node_id)) =
                self.pipe_context.reachable_node(&src_group_code, &dest_id)
            {
                drop(route_table);
                if let Some(route_table) =
                    self.pipe_context.other_route_table.get(&relay_group_code)
                {
                    if let Ok(v) = route_table.get_route_by_id(&relay_node_id) {
                        self.send_to_route(packet.buffer(), &v.route_key()).await?;
                    } else {
                        // broadcast query?
                    }
                }
            } else {
                // broadcast query?
            }
        }
        Ok(())
    }
    async fn handle<'a>(
        &mut self,
        recv_result: RecvResult<'a>,
    ) -> Result<Option<HandleResultInner>> {
        let mut packet = NetPacket::new(recv_result.buf)?;
        if packet.max_ttl() < packet.ttl() {
            return Err(Error::InvalidArgument("ttl error".into()));
        }
        if packet.ttl() == 0 {
            return Ok(None);
        }
        let self_id = if let Some(self_id) = self.pipe_context.load_id() {
            self_id
        } else {
            return Err(Error::InvalidArgument("self id is none".into()));
        };
        let my_group_code = self.pipe_context.load_group_code();
        let route_key = recv_result.route_key;
        let group_code = if my_group_code.as_ref() == packet.group_code() {
            my_group_code
        } else {
            self.other_group_handle(packet, route_key, my_group_code, self_id)
                .await?;
            return Ok(None);
        };
        let src_id = NodeID::try_from(packet.src_id())?;

        if src_id.is_unspecified() || src_id.is_broadcast() {
            return Err(Error::InvalidArgument("src id is unspecified".into()));
        }
        let dest_id = NodeID::try_from(packet.dest_id())?;

        let metric = packet.max_ttl() - packet.ttl() + 1;

        if self_id == src_id {
            log::debug!("{packet:?}");
            return Err(Error::InvalidArgument("id loop error".into()));
        }
        if self_id != dest_id && !dest_id.is_unspecified() && !dest_id.is_broadcast() {
            if packet.incr_ttl() {
                self.route_table
                    .add_route_if_absent(src_id, Route::from_default_rt(route_key, metric));
                self.send_to(&packet, &dest_id).await?;
            }
            return Ok(None);
        }
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
                self.id_route_query_handle(
                    packet, route_key, group_code, self_id, group_code, src_id,
                )
                .await?
            }
            ProtocolType::IDRouteReply => {
                self.id_route_reply_handle(packet, group_code, self_id, group_code, src_id)
                    .await?
            }
            ProtocolType::UserData => {
                return Ok(Some(HandleResultInner {
                    start: HEAD_LEN,
                    end: packet.buffer().len(),
                    src_id,
                    dest_id,
                    route_key,
                    ttl: packet.ttl(),
                    max_ttl: packet.max_ttl(),
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
                    return Ok(Some(HandleResultInner {
                        start,
                        end,
                        src_id,
                        dest_id,
                        route_key,
                        ttl: packet.ttl(),
                        max_ttl: packet.max_ttl(),
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
                send_packet.set_payload(&data);
                if let Ok(sender) = self.passive_punch_sender.try_reserve() {
                    self.pipe_writer
                        .send_packet_to(&mut send_packet, &src_id)
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
            ProtocolType::IDQuery => {}
            ProtocolType::IDReply => {}
        }

        Ok(None)
    }
    async fn id_route_query_handle(
        &mut self,
        packet: NetPacket<&mut [u8]>,
        route_key: RouteKey,
        self_group_code: GroupCode,
        self_id: NodeID,
        src_group_code: GroupCode,
        src_id: NodeID,
    ) -> Result<()> {
        let payload = packet.payload();
        if payload.len() != 4 {
            return Err(Error::InvalidArgument("IDRouteQuery error".into()));
        }
        let query_id = u16::from_be_bytes(payload[2..].try_into().unwrap());
        // reply reachable node id
        let mut list = self.route_table.route_table_min_metric();
        // Not supporting too many nodes
        list.truncate(255);
        let list: Vec<_> = if self_group_code == src_group_code {
            list.into_iter()
                .filter(|(node_id, _)| node_id != &src_id)
                .map(|(node_id, route)| (node_id, route.metric()))
                .collect()
        } else {
            list.into_iter()
                .map(|(node_id, route)| (node_id, route.metric()))
                .collect()
        };
        if !list.is_empty() {
            let mut packet = crate::protocol::id_route::Builder::build_reply(
                &self_group_code,
                &list,
                query_id,
                list.len() as _,
            )?;
            packet.set_dest_id(&src_id);
            packet.set_src_id(&self_id);
            packet.set_group_code(&self_group_code);
            self.send_to_route(packet.buffer(), &route_key).await?;
        }

        if !self.pipe_context.other_route_table.is_empty() {
            // Reply to reachable nodes of other groups
            let pipe_writer = self.pipe_writer.clone();
            let other_route_table = self.pipe_context.other_route_table.clone();
            tokio::spawn(async move {
                if let Err(e) = id_route_reply(
                    pipe_writer,
                    other_route_table,
                    route_key,
                    self_group_code,
                    self_id,
                    src_group_code,
                    src_id,
                )
                .await
                {
                    log::debug!("id_route_reply {e:?}");
                }
            });
        }
        Ok(())
    }
    async fn id_route_reply_handle(
        &mut self,
        packet: NetPacket<&mut [u8]>,
        self_group_code: GroupCode,
        self_id: NodeID,
        src_group_code: GroupCode,
        src_id: NodeID,
    ) -> Result<()> {
        let reply_packet = IDRouteReplyPacket::new(packet.payload())?;
        let reachable_group_code = GroupCode::try_from(reply_packet.group_code())?;

        let id = reply_packet.query_id();
        if reachable_group_code == self_group_code && id != 0 {
            self.pipe_context.update_direct_node_id(id, src_id);
        }
        for (reachable_id, metric) in reply_packet.iter() {
            if self_group_code == reachable_group_code && reachable_id == self_id {
                continue;
            }
            self.pipe_context.update_reachable_nodes(
                reachable_group_code,
                reachable_id,
                src_group_code,
                src_id,
                metric,
            );
        }
        Ok(())
    }
}
async fn id_route_reply(
    pipe_writer: PipeWriter,
    other_route_table: Arc<DashMap<GroupCode, RouteTable<NodeID>>>,
    route_key: RouteKey,
    self_group_code: GroupCode,
    self_id: NodeID,
    src_group_code: GroupCode,
    src_id: NodeID,
) -> Result<()> {
    let mut list = Vec::with_capacity(other_route_table.len());
    for x in other_route_table.iter() {
        let mut table: Vec<_> = if &src_group_code == x.key() {
            x.route_table_min_metric()
                .into_iter()
                .map(|(node_id, route)| (node_id, route.metric()))
                .collect()
        } else {
            x.route_table_min_metric()
                .into_iter()
                .filter(|(node_id, _)| node_id != &src_id)
                .map(|(node_id, route)| (node_id, route.metric()))
                .collect()
        };
        // Not supporting too many nodes
        table.truncate(255);
        if table.is_empty() {
            continue;
        }
        list.push((*x.key(), table))
    }
    for (group_code, table) in list {
        let mut packet = crate::protocol::id_route::Builder::build_reply(
            &group_code,
            &table,
            0,
            table.len() as _,
        )?;
        packet.set_dest_id(&src_id);
        packet.set_src_id(&self_id);
        packet.set_group_code(&self_group_code);
        pipe_writer
            .send_to_route(packet.buffer(), &route_key)
            .await?;
    }
    Ok(())
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
struct HandleResultInner {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) src_id: NodeID,
    pub(crate) dest_id: NodeID,
    pub(crate) route_key: RouteKey,
    pub(crate) ttl: u8,
    pub(crate) max_ttl: u8,
}

#[derive(Debug)]
pub struct HandleResult<'a> {
    _payload: &'a [u8],
    _ttl: u8,
    _max_ttl: u8,
    _src_id: NodeID,
    _dest_id: NodeID,
    _route_key: RouteKey,
}

impl<'a> HandleResult<'a> {
    pub fn ttl(&self) -> u8 {
        self._ttl
    }
    pub fn max_ttl(&self) -> u8 {
        self._max_ttl
    }
    pub fn payload(&self) -> &[u8] {
        self._payload
    }
    pub fn src_id(&self) -> NodeID {
        self._src_id
    }
    pub fn dest_id(&self) -> NodeID {
        self._dest_id
    }
    pub fn route_key(&self) -> RouteKey {
        self._route_key
    }
    pub fn is_relay(&self) -> bool {
        (self._max_ttl - self._ttl) != 0
    }
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
