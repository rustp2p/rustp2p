use crate::config::PipeConfig;
use crate::error::{Error, Result};
use crate::extend::byte_pool::{Block, BufferPool};
use crate::pipe::pipe_context::PipeContext;
use crate::protocol::broadcast::RangeBroadcastPacket;
use crate::protocol::id_route::IDRouteReplyPacket;
use crate::protocol::node_id::{GroupCode, NodeID};
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::{broadcast, NetPacket, HEAD_LEN};
use async_shutdown::ShutdownManager;
#[cfg(feature = "use-async-std")]
use async_std::channel::Sender;
use bytes::BytesMut;
use dashmap::DashMap;
pub use pipe_context::NodeAddress;
pub use pipe_context::PeerNodeAddress;
use rust_p2p_core::nat::NatType;
use rust_p2p_core::pipe::recycle::RecycleBuf;
use rust_p2p_core::punch::PunchConsultInfo;
use rust_p2p_core::route::route_table::RouteTable;
use rust_p2p_core::route::{ConnectProtocol, Route, RouteKey};
pub use send_packet::SendPacket;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
#[cfg(feature = "use-tokio")]
use tokio::sync::mpsc::Sender;

mod maintain;
mod pipe_context;

mod send_packet;

pub struct Pipe {
    send_buffer_size: usize,
    recv_buffer_size: usize,
    pipe_context: PipeContext,
    pipe: rust_p2p_core::pipe::Pipe<NodeID>,
    shutdown_manager: ShutdownManager<()>,
    active_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    passive_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    buffer_pool: Option<BufferPool<BytesMut>>,
    recycle_buf: Option<RecycleBuf>,
}

impl Pipe {
    pub async fn new(mut config: PipeConfig) -> Result<Pipe> {
        let multi_pipeline = config.multi_pipeline;
        let send_buffer_size = config.send_buffer_size;
        let recv_buffer_size = config.recv_buffer_size;
        let query_id_interval = config.query_id_interval;
        let query_id_max_num = config.query_id_max_num;
        let heartbeat_interval = config.heartbeat_interval;
        let route_idle_time = config.route_idle_time;
        let group_code = config.group_code.take();
        let self_id = config.self_id.take();
        let direct_addrs = config.direct_addrs.take();
        let mapping_addrs = config.mapping_addrs.take();
        let dns = config.dns.take();
        let default_interface = config.default_interface.clone();
        let buffer_pool = if config.recycle_buf_cap > 0 {
            Some(BufferPool::new(
                config.recycle_buf_cap,
                config.recv_buffer_size,
            ))
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
        #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
        let cipher = config.encryption.clone().map(crate::cipher::Cipher::from);

        let config: rust_p2p_core::pipe::config::PipeConfig = config.into();
        let mut recycle_buf: Option<RecycleBuf> = None;
        if let Some(v) = config.tcp_pipe_config.as_ref() {
            recycle_buf.clone_from(&v.recycle_buf);
        } else if let Some(v) = config.udp_pipe_config.as_ref() {
            recycle_buf.clone_from(&v.recycle_buf);
        };
        let (pipe, puncher, idle_route_manager) = rust_p2p_core::pipe::pipe::<NodeID>(config)?;
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
            multi_pipeline,
            local_udp_ports,
            local_tcp_port,
            default_interface.clone(),
            dns,
            #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
            cipher,
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
            recycle_buf: recycle_buf.clone(),
        };
        let (active_punch_sender, active_punch_receiver) =
            rust_p2p_core::async_compat::mpsc::channel(3);
        let (passive_punch_sender, passive_punch_receiver) =
            rust_p2p_core::async_compat::mpsc::channel(3);
        let join_set = maintain::start_task(
            &pipe_writer,
            idle_route_manager,
            puncher,
            query_id_interval,
            query_id_max_num,
            heartbeat_interval,
            route_idle_time,
            tcp_stun_servers,
            udp_stun_servers,
            default_interface,
            active_punch_receiver,
            passive_punch_receiver,
        );
        #[cfg(feature = "use-tokio")]
        let mut join_set = join_set;
        #[cfg(feature = "use-tokio")]
        let fut = shutdown_manager
            .wrap_cancel(async move { while join_set.join_next().await.is_some() {} });
        #[cfg(feature = "use-async-std")]
        let fut = shutdown_manager.wrap_cancel(async move {
            join_set.await;
        });
        rust_p2p_core::async_compat::spawn(async move {
            if fut.await.is_err() {
                log::debug!("recv shutdown signal: built-in maintain tasks are shutdown");
            }
        });
        Ok(Self {
            send_buffer_size,
            recv_buffer_size,
            pipe_context,
            pipe,
            shutdown_manager,
            active_punch_sender,
            passive_punch_sender,
            buffer_pool,
            recycle_buf,
        })
    }
    pub fn writer(&self) -> PipeWriter {
        PipeWriter {
            send_buffer_size: self.send_buffer_size,
            pipe_context: self.pipe_context.clone(),
            pipe_writer: self.pipe.writer_ref().to_owned(),
            shutdown_manager: self.shutdown_manager.clone(),
            recycle_buf: self.recycle_buf.clone(),
        }
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        _ = self.shutdown_manager.trigger_shutdown(());
    }
}

impl Pipe {
    pub async fn accept(&mut self) -> Result<PipeLine> {
        let Ok(pipe_line) = self.shutdown_manager.wrap_cancel(self.pipe.accept()).await else {
            return Err(Error::ShutDown);
        };
        let pipe_line = pipe_line?;
        Ok(PipeLine {
            shutdown_manager: self.shutdown_manager.clone(),
            pipe_context: self.pipe_context.clone(),
            pipe_line,
            pipe_writer: self.writer(),
            route_table: self.pipe.route_table().clone(),
            active_punch_sender: self.active_punch_sender.clone(),
            passive_punch_sender: self.passive_punch_sender.clone(),
            buffer_pool: self.buffer_pool.clone(),
            recv_buffer_size: self.recv_buffer_size,
        })
    }
}

#[derive(Clone)]
pub struct PipeWriter {
    send_buffer_size: usize,
    pipe_context: PipeContext,
    pipe_writer: rust_p2p_core::pipe::PipeWriter<NodeID>,
    shutdown_manager: ShutdownManager<()>,
    recycle_buf: Option<RecycleBuf>,
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
    pub fn lookup_route(&self, node_id: &NodeID) -> Option<Vec<Route>> {
        self.pipe_writer.route_table().route(node_id)
    }
    pub fn nodes(&self) -> Vec<NodeID> {
        self.pipe_writer.route_table().route_table_ids()
    }
    pub fn other_group_codes(&self) -> Vec<GroupCode> {
        self.pipe_context
            .other_route_table
            .iter()
            .map(|v| *v.key())
            .collect()
    }
    pub fn other_group_nodes(&self, group_code: &GroupCode) -> Option<Vec<NodeID>> {
        self.pipe_context
            .other_route_table
            .get(group_code)
            .map(|v| v.route_table_ids())
    }
    pub fn other_group_route(
        &self,
        group_code: &GroupCode,
        node_id: &NodeID,
    ) -> Option<Vec<Route>> {
        self.pipe_context
            .other_route_table
            .get(group_code)
            .map(|v| v.route(node_id))?
    }
    pub fn current_group_code(&self) -> GroupCode {
        self.pipe_context.load_group_code()
    }
    pub fn route_to_node_id(&self, route: &RouteKey) -> Option<NodeID> {
        self.pipe_writer.route_table().route_to_id(route)
    }
    pub fn other_route_to_node_id(
        &self,
        group_code: &GroupCode,
        route: &RouteKey,
    ) -> Option<NodeID> {
        self.pipe_context
            .other_route_table
            .get(group_code)
            .map(|v| v.route_to_id(route))?
    }
    pub(crate) async fn send_to_id_by_code<B: AsRef<[u8]>>(
        &self,
        buf: &NetPacket<B>,
        group_code: &GroupCode,
        id: &NodeID,
    ) -> Result<()> {
        let code = self.pipe_context.load_group_code();
        if &code == group_code {
            self.send_to_id(buf, id).await
        } else {
            let route = if let Some(v) = self.pipe_context.other_route_table.get(group_code) {
                v.get_route_by_id(id)?
            } else {
                return Err(Error::NodeIDNotAvailable);
            };
            self.send_to_route(buf.buffer(), &route.route_key()).await
        }
    }
    pub(crate) async fn send_to_id<B: AsRef<[u8]>>(
        &self,
        buf: &NetPacket<B>,
        id: &NodeID,
    ) -> Result<()> {
        self.pipe_writer.send_to_id(buf.buffer().into(), id).await?;
        Ok(())
    }
    pub(crate) async fn send_to_route(&self, buf: &[u8], route_key: &RouteKey) -> Result<()> {
        self.pipe_writer.send_to(buf.into(), route_key).await?;
        Ok(())
    }
    async fn send_to0(
        &self,
        buf: BytesMut,
        group_code: &GroupCode,
        src_id: &NodeID,
        dest_id: &NodeID,
    ) -> Result<()> {
        if dest_id.is_broadcast() {
            self.send_broadcast0(&buf, group_code, src_id).await;
            return Ok(());
        }

        if let Ok(route) = self.pipe_writer.route_table().get_route_by_id(dest_id) {
            self.pipe_writer.send_to(buf, &route.route_key()).await?
        } else if let Some((relay_group_code, relay_node_id)) =
            self.pipe_context().reachable_node(group_code, dest_id)
        {
            if &relay_group_code == group_code {
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

    async fn send_broadcast0(&self, buf: &[u8], group_code: &GroupCode, src_id: &NodeID) {
        let route_table = self.pipe_writer.route_table();
        let table = route_table.route_table_one();
        let mut map: HashMap<NodeID, (Vec<NodeID>, Route)> = HashMap::new();
        for (id, route) in &table {
            if route.is_direct() {
                let list = if let Some((list, _)) = map.remove(id) {
                    list
                } else {
                    vec![*id]
                };
                map.insert(*id, (list, *route));
            } else if let Some(owner_id) = route_table.get_id_by_route_key(&route.route_key()) {
                if let Some((list, _)) = map.get_mut(&owner_id) {
                    list.push(*id);
                } else {
                    map.insert(owner_id, (vec![owner_id, *id], *route));
                }
            } else {
                // 通过其他组的节点转发的，正常来说应该也要找到这个转发节点，这里为了简化操作，写为直接发送
                if !map.contains_key(id) {
                    map.insert(*id, (vec![*id], *route));
                }
            }
        }
        for (owner_id, (list, route)) in map {
            if list.len() <= 1 && route.is_direct() {
                if let Err(e) = self
                    .pipe_writer
                    .send_to(buf.into(), &route.route_key())
                    .await
                {
                    log::debug!("send_broadcast0 {e:?} {owner_id:?}");
                }
            } else {
                match broadcast::Builder::build_range_broadcast(&list, buf) {
                    Ok(mut packet) => {
                        packet.set_src_id(src_id);
                        packet.set_dest_id(&owner_id);
                        packet.set_group_code(group_code);
                        if let Err(e) = self
                            .pipe_writer
                            .send_to(packet.buffer().into(), &route.route_key())
                            .await
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

    pub async fn broadcast_packet(&self, packet: SendPacket) -> Result<()> {
        self.send_packet_to(packet, &NodeID::broadcast()).await
    }
    pub async fn send_packet_to(&self, mut packet: SendPacket, dest_id: &NodeID) -> Result<()> {
        let group_code = self.pipe_context.load_group_code();
        if let Some(src_id) = self.pipe_context.load_id() {
            packet.set_group_code(&group_code);
            packet.set_src_id(&src_id);
            packet.set_dest_id(dest_id);
            #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
            if packet.is_user_data() {
                if let Some(cipher) = self.pipe_context.cipher.as_ref() {
                    let data_len = packet.len();
                    packet.resize(data_len + cipher.reserved_len(), 0);

                    cipher.encrypt(tag(&src_id, dest_id), &mut packet)?;
                    packet.set_encrypt_flag(true);
                }
            }
            self.send_to0(packet.into_buf(), &group_code, &src_id, dest_id)
                .await
        } else {
            Err(Error::NoIDSpecified)
        }
    }

    pub fn allocate_send_packet(&self) -> SendPacket {
        let buf = if let Some(recycle_buf) = self.recycle_buf.as_ref() {
            recycle_buf.alloc(self.send_buffer_size)
        } else {
            BytesMut::with_capacity(self.send_buffer_size)
        };
        SendPacket::with_bytes_mut(buf)
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
    shutdown_manager: ShutdownManager<()>,
    pipe_context: PipeContext,
    pipe_line: rust_p2p_core::pipe::PipeLine,
    pipe_writer: PipeWriter,
    route_table: RouteTable<NodeID>,
    active_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    passive_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    buffer_pool: Option<BufferPool<BytesMut>>,
    recv_buffer_size: usize,
}

impl PipeLine {
    pub async fn next(
        &mut self,
    ) -> core::result::Result<core::result::Result<RecvUserData, HandleError>, RecvError> {
        self.next_process::<crate::config::DefaultInterceptor>(None)
            .await
    }
    pub async fn next_process<I: crate::config::DataInterceptor>(
        &mut self,
        interceptor: Option<&I>,
    ) -> core::result::Result<core::result::Result<RecvUserData, HandleError>, RecvError> {
        let mut block = if let Some(buffer_pool) = self.buffer_pool.as_ref() {
            Data::Recyclable(buffer_pool.alloc())
        } else {
            Data::Temporary(BytesMut::with_capacity(self.recv_buffer_size))
        };
        loop {
            unsafe {
                let capacity = block.capacity();
                block.set_len(capacity);
            }
            let (len, route_key) = if let Ok(v) = self
                .shutdown_manager
                .wrap_cancel(self.pipe_line.recv_from(&mut block))
                .await
            {
                match v {
                    None => return Err(RecvError::Done),
                    Some(recv_rs) => match recv_rs {
                        Ok(rs) => rs,
                        Err(e) => return Err(RecvError::Io(e)),
                    },
                }
            } else {
                self.pipe_line.done();
                return Err(RecvError::Done);
            };

            if len == 0 {
                return Err(RecvError::Io(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )));
            }
            unsafe {
                block.set_len(len);
            }
            if rust_p2p_core::stun::is_stun_response(&block) {
                if let Some(pub_addr) = rust_p2p_core::stun::recv_stun_response(&block) {
                    self.pipe_context
                        .update_public_addr(route_key.index(), pub_addr);
                } else {
                    log::debug!("stun error {route_key:?}")
                }
                continue;
            }
            let mut recv_result = RecvResult::new(&mut block, route_key);
            if let Some(interceptor) = interceptor {
                if interceptor.pre_handle(&mut recv_result).await {
                    continue;
                }
            }

            return match self.handle(recv_result).await {
                Ok(handle_result) => {
                    if let Some(rs) = handle_result {
                        #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
                        let mut rs = rs;
                        #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
                        {
                            if rs.is_encrypt != self.pipe_context.cipher.is_some() {
                                return Ok(Err(HandleError::new(
                                    route_key,
                                    Error::Any(anyhow::anyhow!(
                                        "Inconsistent encryption status: data tag-{} current tag-{}",
                                        rs.is_encrypt,
                                        self.pipe_context.cipher.is_some()
                                    )),
                                )));
                            }
                            if let Some(cipher) = self.pipe_context.cipher.as_ref() {
                                match cipher.decrypt(
                                    tag(&rs.src_id, &rs.dest_id),
                                    &mut block[rs.start..rs.end],
                                ) {
                                    Ok(_len) => rs.end -= cipher.reserved_len(),
                                    Err(e) => {
                                        return Ok(Err(HandleError::new(route_key, Error::Any(e))))
                                    }
                                }
                            }
                        }

                        return Ok(Ok(RecvUserData {
                            _start: rs.start,
                            _end: rs.end,
                            _src_id: rs.src_id,
                            _dest_id: rs.dest_id,
                            _route_key: rs.route_key,
                            _data: block,
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
    pub fn protocol(&self) -> ConnectProtocol {
        self.pipe_line.protocol()
    }
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.pipe_line.remote_addr()
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
        let metric = packet.max_ttl() - packet.ttl();
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

        let dest_id = NodeID::try_from(packet.dest_id())?;
        if dest_id.is_unspecified() {
            // 是不同组的节点发给自己的数据
            match packet.protocol()? {
                ProtocolType::EchoRequest => {
                    packet.set_protocol(ProtocolType::EchoReply);
                    packet.set_ttl(packet.max_ttl());
                    packet.set_dest_id(&NodeID::unspecified());
                    packet.set_src_id(&self_id);
                    packet.set_group_code(&self_group_code);
                    self.send_to_route(packet.buffer(), &route_key).await?;
                }
                ProtocolType::EchoReply => {}
                ProtocolType::TimestampRequest => {
                    packet.set_protocol(ProtocolType::TimestampReply);
                    packet.set_ttl(packet.max_ttl());
                    packet.set_dest_id(&NodeID::unspecified());
                    packet.set_src_id(&self_id);
                    packet.set_group_code(&self_group_code);
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
                    self.pipe_context
                        .other_route_table
                        .get(&src_group_code)
                        .map(|v| v.add_route(src_id, Route::from(route_key, metric, rtt)));
                }
                _ => {}
            }
            return Ok(());
        }

        if let Some(route_table) = self.pipe_context.other_route_table.get(&src_group_code) {
            if dest_id.is_broadcast() {
                // broadcast
            } else if let Ok(v) = route_table.get_route_by_id(&dest_id) {
                if !route_table.is_route_of_peer_id(&src_id, &v.route_key()) {
                    self.send_to_route(packet.buffer(), &v.route_key()).await?;
                }
            } else if let Some((relay_group_code, relay_node_id)) =
                self.pipe_context.reachable_node(&src_group_code, &dest_id)
            {
                drop(route_table);
                if let Some(route_table) =
                    self.pipe_context.other_route_table.get(&relay_group_code)
                {
                    if let Ok(v) = route_table.get_route_by_id(&relay_node_id) {
                        if !route_table.is_route_of_peer_id(&src_id, &v.route_key()) {
                            self.send_to_route(packet.buffer(), &v.route_key()).await?;
                        }
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
        let src_id = NodeID::try_from(packet.src_id())?;

        if src_id.is_unspecified() || src_id.is_broadcast() {
            return Err(Error::InvalidArgument("src id is unspecified".into()));
        }
        let self_id = if let Some(self_id) = self.pipe_context.load_id() {
            self_id
        } else {
            return Err(Error::InvalidArgument("self id is none".into()));
        };
        let group_code = self.pipe_context.load_group_code();
        let route_key = recv_result.route_key;
        if group_code.as_ref() != packet.group_code() {
            self.other_group_handle(packet, route_key, group_code, self_id)
                .await?;
            return Ok(None);
        }

        let dest_id = NodeID::try_from(packet.dest_id())?;

        let metric = packet.max_ttl() - packet.ttl();

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
                    #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
                    is_encrypt: packet.is_encrypt(),
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
                        #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
                        is_encrypt: packet.is_encrypt(),
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
                #[cfg(feature = "use-tokio")]
                if let Ok(sender) = self.passive_punch_sender.try_reserve() {
                    self.pipe_writer
                        .send_packet_to(send_packet, &src_id)
                        .await?;
                    sender.send((src_id, punch_info))
                }
                #[cfg(feature = "use-async-std")]
                if self
                    .passive_punch_sender
                    .try_send((src_id, punch_info))
                    .is_ok()
                {
                    self.pipe_writer
                        .send_packet_to(send_packet, &src_id)
                        .await?;
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
            rust_p2p_core::async_compat::spawn(async move {
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
        if id != 0 {
            self.pipe_context
                .update_direct_node_id(id, reachable_group_code, src_id);
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
    pub fn net_packet(&self) -> Result<NetPacket<&[u8]>> {
        NetPacket::new(self.buf)
    }
    pub fn net_packet_mut(&mut self) -> Result<NetPacket<&mut [u8]>> {
        NetPacket::new(self.buf)
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
    #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
    pub(crate) is_encrypt: bool,
}

pub struct RecvUserData {
    _start: usize,
    _end: usize,
    _data: Data,
    _ttl: u8,
    _max_ttl: u8,
    _src_id: NodeID,
    _dest_id: NodeID,
    _route_key: RouteKey,
}

pub enum Data {
    Recyclable(Block<BytesMut>),
    Temporary(BytesMut),
}

impl Deref for Data {
    type Target = BytesMut;

    fn deref(&self) -> &Self::Target {
        match self {
            Data::Recyclable(data) => data,
            Data::Temporary(data) => data,
        }
    }
}

impl DerefMut for Data {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Data::Recyclable(data) => data,
            Data::Temporary(data) => data,
        }
    }
}

impl RecvUserData {
    pub fn ttl(&self) -> u8 {
        self._ttl
    }
    pub fn max_ttl(&self) -> u8 {
        self._max_ttl
    }
    pub fn payload(&self) -> &[u8] {
        &self._data[self._start..self._end]
    }
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self._data[self._start..self._end]
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

#[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
fn tag(src_id: &NodeID, dest_id: &NodeID) -> [u8; 12] {
    let mut tmp = [0; 12];
    tmp[..4].copy_from_slice(src_id.as_ref());
    tmp[4..8].copy_from_slice(dest_id.as_ref());
    tmp
}
