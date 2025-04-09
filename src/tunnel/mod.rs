use crate::config::TunnelManagerConfig;
use crate::extend::byte_pool::{Block, BufferPool};
use crate::protocol::broadcast::RangeBroadcastPacket;
use crate::protocol::id_route::IDRouteReplyPacket;
use crate::protocol::node_id::{GroupCode, NodeID};
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::{broadcast, NetPacket, HEAD_LEN};
use crate::tunnel::node_context::NodeContext;
use async_shutdown::ShutdownManager;

use bytes::BytesMut;
use dashmap::DashMap;
pub use node_context::NodeAddress;
pub use node_context::PeerNodeAddress;
use rust_p2p_core::nat::NatType;
use rust_p2p_core::punch::PunchConsultInfo;
use rust_p2p_core::route::route_table::RouteTable;
use rust_p2p_core::route::{ConnectProtocol, Route, RouteKey};
use rust_p2p_core::tunnel::config::LoadBalance;
use rust_p2p_core::tunnel::recycle::RecycleBuf;
pub use send_packet::SendPacket;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tokio::sync::mpsc::Sender;

mod maintain;
mod node_context;

mod send_packet;

pub struct TunnelManager {
    send_buffer_size: usize,
    recv_buffer_size: usize,
    node_context: NodeContext,
    tunnel_factory: rust_p2p_core::tunnel::UnifiedTunnelFactory<NodeID>,
    shutdown_manager: ShutdownManager<()>,
    active_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    passive_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    buffer_pool: Option<BufferPool<BytesMut>>,
    recycle_buf: Option<RecycleBuf>,
}

impl TunnelManager {
    pub async fn new(config: TunnelManagerConfig) -> io::Result<TunnelManager> {
        Box::pin(Self::new_impl(config)).await
    }
    pub(crate) async fn new_impl(mut config: TunnelManagerConfig) -> io::Result<TunnelManager> {
        let major_socket_count = config.major_socket_count;
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

        let config: rust_p2p_core::tunnel::config::TunnelConfig = config.into();
        let mut recycle_buf: Option<RecycleBuf> = None;
        if let Some(v) = config.tcp_tunnel_config.as_ref() {
            recycle_buf.clone_from(&v.recycle_buf);
        } else if let Some(v) = config.udp_tunnel_config.as_ref() {
            recycle_buf.clone_from(&v.recycle_buf);
        };
        let (tunnel_factory, puncher, idle_route_manager) =
            rust_p2p_core::tunnel::new_tunnel_component::<NodeID>(config)?;
        let local_tcp_port = if let Some(v) = tunnel_factory.tcp_socket_manager_as_ref() {
            v.local_addr().port()
        } else {
            0
        };
        let local_udp_ports = if let Some(v) = tunnel_factory.udp_socket_manager_as_ref() {
            v.local_ports()?
        } else {
            vec![]
        };
        let node_context = NodeContext::new(
            major_socket_count,
            local_udp_ports,
            local_tcp_port,
            default_interface.clone(),
            dns,
            #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
            cipher,
        );
        if let Some(group_code) = group_code {
            node_context.store_group_code(group_code)?;
        }
        if let Some(node_id) = self_id {
            node_context.store_self_id(node_id)?;
        }
        if let Some(addrs) = direct_addrs {
            node_context.update_direct_nodes(addrs).await?;
        }
        if let Some(addrs) = mapping_addrs {
            node_context.set_mapping_addrs(addrs);
        }
        let shutdown_manager = ShutdownManager::<()>::new();
        let tunnel_tx = TunnelTransmitHub {
            send_buffer_size,
            node_context: node_context.clone(),
            socket_manager: tunnel_factory.socket_manager(),
            shutdown_manager: shutdown_manager.clone(),
            recycle_buf: recycle_buf.clone(),
        };
        let (active_punch_sender, active_punch_receiver) = tokio::sync::mpsc::channel(3);
        let (passive_punch_sender, passive_punch_receiver) = tokio::sync::mpsc::channel(3);
        let join_set = maintain::start_task(
            &tunnel_tx,
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
        let mut join_set = join_set;
        let fut = shutdown_manager
            .wrap_cancel(async move { while join_set.join_next().await.is_some() {} });
        tokio::spawn(async move {
            if fut.await.is_err() {
                log::debug!("recv shutdown signal: built-in maintain tasks are shutdown");
            }
        });
        Ok(Self {
            send_buffer_size,
            recv_buffer_size,
            node_context,
            tunnel_factory,
            shutdown_manager,
            active_punch_sender,
            passive_punch_sender,
            buffer_pool,
            recycle_buf,
        })
    }
    pub(crate) fn tunnel_send_hub(&self) -> TunnelTransmitHub {
        TunnelTransmitHub {
            send_buffer_size: self.send_buffer_size,
            node_context: self.node_context.clone(),
            socket_manager: self.tunnel_factory.socket_manager(),
            shutdown_manager: self.shutdown_manager.clone(),
            recycle_buf: self.recycle_buf.clone(),
        }
    }
}

impl Drop for TunnelManager {
    fn drop(&mut self) {
        _ = self.shutdown_manager.trigger_shutdown(());
    }
}

impl TunnelManager {
    pub(crate) async fn dispatch(&mut self) -> io::Result<Tunnel> {
        if self.shutdown_manager.is_shutdown_triggered() {
            return Err(io::Error::new(io::ErrorKind::Other, "shutdown"));
        }
        let Ok(tunnel) = self
            .shutdown_manager
            .wrap_cancel(self.tunnel_factory.dispatch())
            .await
        else {
            return Err(io::Error::new(io::ErrorKind::Other, "shutdown"));
        };
        let tunnel = tunnel?;
        let recv_buff = Vec::with_capacity(BUF_SIZE);
        let recv_sizes = Vec::with_capacity(BUF_SIZE);
        let recv_addrs = Vec::with_capacity(BUF_SIZE);
        Ok(Tunnel {
            shutdown_manager: self.shutdown_manager.clone(),
            node_context: self.node_context.clone(),
            tunnel,
            tunnel_transmit: self.tunnel_send_hub(),
            route_table: self.tunnel_factory.route_table().clone(),
            active_punch_sender: self.active_punch_sender.clone(),
            passive_punch_sender: self.passive_punch_sender.clone(),
            buffer_pool: self.buffer_pool.clone(),
            recv_buffer_size: self.recv_buffer_size,
            recv_buff,
            recv_sizes,
            recv_addrs,
        })
    }
}

#[derive(Clone)]
pub struct TunnelTransmitHub {
    send_buffer_size: usize,
    node_context: NodeContext,
    socket_manager: rust_p2p_core::tunnel::UnifiedSocketManager<NodeID>,
    shutdown_manager: ShutdownManager<()>,
    recycle_buf: Option<RecycleBuf>,
}

impl TunnelTransmitHub {
    pub fn node_context(&self) -> &NodeContext {
        &self.node_context
    }
    pub fn switch_model(&self, nat_type: NatType) -> io::Result<()> {
        if self.shutdown_manager.is_shutdown_triggered() {
            return Err(io::Error::new(io::ErrorKind::Other, "shutdown"));
        }
        use rust_p2p_core::tunnel::udp::Model;
        match nat_type {
            NatType::Cone => {
                if let Some(socket_manager) = self.socket_manager.udp_socket_manager_as_ref() {
                    socket_manager.switch_model(Model::Low)?;
                }
            }
            NatType::Symmetric => {
                if let Some(socket_manager) = self.socket_manager.udp_socket_manager_as_ref() {
                    socket_manager.switch_model(Model::High)?;
                }
            }
        }
        Ok(())
    }
    pub fn lookup_route(&self, node_id: &NodeID) -> Option<Vec<Route>> {
        self.socket_manager.route_table().route(node_id)
    }
    pub fn nodes(&self) -> Vec<NodeID> {
        self.socket_manager.route_table().route_table_ids()
    }
    pub fn other_group_codes(&self) -> Vec<GroupCode> {
        self.node_context
            .other_route_table
            .iter()
            .map(|v| *v.key())
            .collect()
    }
    pub fn other_group_nodes(&self, group_code: &GroupCode) -> Option<Vec<NodeID>> {
        self.node_context
            .other_route_table
            .get(group_code)
            .map(|v| v.route_table_ids())
    }
    pub fn other_group_route(
        &self,
        group_code: &GroupCode,
        node_id: &NodeID,
    ) -> Option<Vec<Route>> {
        self.node_context
            .other_route_table
            .get(group_code)
            .map(|v| v.route(node_id))?
    }
    pub fn current_group_code(&self) -> GroupCode {
        self.node_context.load_group_code()
    }
    pub fn route_to_node_id(&self, route: &RouteKey) -> Option<NodeID> {
        self.socket_manager.route_table().route_to_id(route)
    }
    pub fn other_route_to_node_id(
        &self,
        group_code: &GroupCode,
        route: &RouteKey,
    ) -> Option<NodeID> {
        self.node_context
            .other_route_table
            .get(group_code)
            .map(|v| v.route_to_id(route))?
    }
    pub(crate) async fn send_to_id_by_code<B: AsRef<[u8]>>(
        &self,
        buf: &NetPacket<B>,
        group_code: &GroupCode,
        id: &NodeID,
    ) -> io::Result<()> {
        let code = self.node_context.load_group_code();
        if &code == group_code {
            self.send_to_id(buf, id).await
        } else {
            let route = if let Some(v) = self.node_context.other_route_table.get(group_code) {
                v.get_route_by_id(id)?
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "group_code not found",
                ));
            };
            self.send_to_route(buf.buffer(), &route.route_key()).await
        }
    }
    pub(crate) async fn send_to_id<B: AsRef<[u8]>>(
        &self,
        buf: &NetPacket<B>,
        id: &NodeID,
    ) -> io::Result<()> {
        self.socket_manager
            .send_to_id(buf.buffer().into(), id)
            .await?;
        Ok(())
    }
    pub(crate) async fn send_to_route(&self, buf: &[u8], route_key: &RouteKey) -> io::Result<()> {
        self.socket_manager.send_to(buf.into(), route_key).await?;
        Ok(())
    }
    async fn send_to_impl(
        &self,
        buf: BytesMut,
        group_code: &GroupCode,
        src_id: &NodeID,
        dest_id: &NodeID,
    ) -> io::Result<()> {
        if dest_id.is_broadcast() {
            self.send_broadcast_impl(&buf, group_code, src_id).await;
            return Ok(());
        }

        if let Ok(route) = self.socket_manager.route_table().get_route_by_id(dest_id) {
            self.socket_manager.send_to(buf, &route.route_key()).await?
        } else if let Some((relay_group_code, relay_node_id)) =
            self.node_context().reachable_node(group_code, dest_id)
        {
            if &relay_group_code == group_code {
                self.socket_manager.send_to_id(buf, &relay_node_id).await?
            } else {
                let route;
                if let Some(v) = self.node_context().other_route_table.get(&relay_group_code) {
                    route = v.get_route_by_id(&relay_node_id)?;
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrNotAvailable,
                        "group route not found",
                    ));
                }
                self.socket_manager.send_to(buf, &route.route_key()).await?
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "node route not found",
            ));
        };
        Ok(())
    }

    async fn send_broadcast_impl(&self, buf: &[u8], group_code: &GroupCode, src_id: &NodeID) {
        let route_table = self.socket_manager.route_table();
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
                    .socket_manager
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
                            .socket_manager
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

    pub(crate) async fn broadcast_packet(&self, packet: SendPacket) -> io::Result<()> {
        self.send_packet_to(packet, &NodeID::broadcast()).await
    }
    pub(crate) async fn send_packet_to(
        &self,
        mut packet: SendPacket,
        dest_id: &NodeID,
    ) -> io::Result<()> {
        let group_code = self.node_context.load_group_code();
        if let Some(src_id) = self.node_context.load_id() {
            packet.set_group_code(&group_code);
            packet.set_src_id(&src_id);
            packet.set_dest_id(dest_id);
            #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
            if packet.is_user_data() {
                if let Some(cipher) = self.node_context.cipher.as_ref() {
                    let data_len = packet.len();
                    packet.resize(data_len + cipher.reserved_len(), 0);

                    cipher.encrypt(tag(&src_id, dest_id), &mut packet)?;
                    packet.set_encrypt_flag(true);
                }
            }
            self.send_to_impl(packet.into_buf(), &group_code, &src_id, dest_id)
                .await
        } else {
            Err(io::Error::new(io::ErrorKind::Other, "no id specified"))
        }
    }

    pub(crate) fn allocate_send_packet(&self) -> SendPacket {
        let buf = if let Some(recycle_buf) = self.recycle_buf.as_ref() {
            recycle_buf.alloc(self.send_buffer_size)
        } else {
            BytesMut::with_capacity(self.send_buffer_size)
        };
        SendPacket::with_bytes_mut(buf)
    }
    #[allow(dead_code)]
    pub(crate) fn release_send_packet(&self, send_packet: SendPacket) {
        if let Some(recycle_buf) = self.recycle_buf.as_ref() {
            recycle_buf.push(send_packet.into_buf());
        }
    }
    pub(crate) fn allocate_send_packet_proto(
        &self,
        protocol_type: ProtocolType,
        payload_size: usize,
    ) -> io::Result<SendPacket> {
        let group_code = self.node_context.load_group_code();
        let src_id = if let Some(src_id) = self.node_context.load_id() {
            src_id
        } else {
            return Err(io::Error::new(io::ErrorKind::Other, "no id specified"));
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
    pub fn shutdown(&self) -> io::Result<()> {
        self.shutdown_manager
            .trigger_shutdown(())
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "already shutdown"))?;
        Ok(())
    }
}

pub struct Tunnel {
    shutdown_manager: ShutdownManager<()>,
    node_context: NodeContext,
    tunnel: rust_p2p_core::tunnel::UnifiedTunnel,
    tunnel_transmit: TunnelTransmitHub,
    route_table: RouteTable<NodeID>,
    active_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    passive_punch_sender: Sender<(NodeID, PunchConsultInfo)>,
    buffer_pool: Option<BufferPool<BytesMut>>,
    recv_buffer_size: usize,
    recv_buff: Vec<Data>,
    recv_sizes: Vec<usize>,
    recv_addrs: Vec<RouteKey>,
}

const BUF_SIZE: usize = 16;

type RecvRs = Result<Result<(RecvUserData, RecvMetadata), HandleError>, RecvError>;

impl Tunnel {
    #[allow(dead_code)]
    pub(crate) async fn next(&mut self) -> RecvRs {
        self.preprocess_next::<crate::config::DefaultInterceptor>(None)
            .await
    }
    #[allow(dead_code)]
    pub(crate) async fn batch_recv(
        &mut self,
        data_vec: &mut Vec<(RecvUserData, RecvMetadata)>,
    ) -> Result<Result<(), HandleError>, RecvError> {
        self.preprocess_batch_recv::<crate::config::DefaultInterceptor>(None, data_vec)
            .await
    }
    /// Receive a batch of data and use interceptors.
    pub(crate) async fn preprocess_batch_recv<I: crate::config::DataInterceptor>(
        &mut self,
        interceptor: Option<&I>,
        data_vec: &mut Vec<(RecvUserData, RecvMetadata)>,
    ) -> Result<Result<(), HandleError>, RecvError> {
        self.recv_addrs.resize(BUF_SIZE, RouteKey::default());
        self.recv_sizes.resize(BUF_SIZE, 0);
        self.recv_buff.truncate(BUF_SIZE);
        while self.recv_buff.len() < BUF_SIZE {
            self.recv_buff.push(self.alloc());
        }
        loop {
            if self.shutdown_manager.is_shutdown_triggered() {
                self.tunnel.done();
                return Err(RecvError::Done);
            }
            let num = if let Ok(v) = self
                .shutdown_manager
                .wrap_cancel(self.tunnel.batch_recv_from(
                    &mut self.recv_buff,
                    &mut self.recv_sizes,
                    &mut self.recv_addrs,
                ))
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
                self.tunnel.done();
                return Err(RecvError::Done);
            };
            for index in 0..num {
                let len = self.recv_sizes[index];

                if len == 0 {
                    return Err(RecvError::Io(std::io::Error::from(
                        io::ErrorKind::UnexpectedEof,
                    )));
                }
                let mut block = self.recv_buff.swap_remove(index);
                let route_key = std::mem::take(&mut self.recv_addrs[index]);
                unsafe {
                    block.set_len(len);
                }
                match self.handle_data(interceptor, block, route_key).await {
                    Ok(rs) => match rs? {
                        Ok(data) => {
                            data_vec.push(data);
                            self.recv_buff.push(self.alloc());
                        }
                        Err(e) => return Ok(Err(e)),
                    },
                    Err(mut data) => {
                        unsafe {
                            let capacity = data.capacity();
                            data.set_len(capacity);
                        }
                        self.recv_buff.push(data)
                    }
                }
                if num == BUF_SIZE {
                    self.recv_buff.swap(index, BUF_SIZE - 1);
                }
            }
            if !data_vec.is_empty() {
                return Ok(Ok(()));
            }
        }
    }
    pub(crate) async fn preprocess_next<I: crate::config::DataInterceptor>(
        &mut self,
        interceptor: Option<&I>,
    ) -> RecvRs {
        let mut block = self.alloc();
        loop {
            unsafe {
                let capacity = block.capacity();
                block.set_len(capacity);
            }
            if self.shutdown_manager.is_shutdown_triggered() {
                self.tunnel.done();
                return Err(RecvError::Done);
            }
            let (len, route_key) = if let Ok(v) = self
                .shutdown_manager
                .wrap_cancel(self.tunnel.recv_from(&mut block))
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
                self.tunnel.done();
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
            match self.handle_data(interceptor, block, route_key).await {
                Ok(rs) => return rs,
                Err(data) => block = data,
            }
        }
    }
    fn alloc(&self) -> Data {
        let mut block = if let Some(buffer_pool) = self.buffer_pool.as_ref() {
            Data::Recyclable(buffer_pool.alloc())
        } else {
            Data::Temporary(BytesMut::with_capacity(self.recv_buffer_size))
        };
        unsafe {
            let capacity = block.capacity();
            block.set_len(capacity);
        }
        block
    }
    async fn handle_data<I: crate::config::DataInterceptor>(
        &mut self,
        interceptor: Option<&I>,
        mut block: Data,
        route_key: RouteKey,
    ) -> Result<RecvRs, Data> {
        if rust_p2p_core::stun::is_stun_response(&block) {
            if let Some(pub_addr) = rust_p2p_core::stun::recv_stun_response(&block) {
                self.node_context
                    .update_public_addr(route_key.index(), pub_addr);
            } else {
                log::debug!("stun error {route_key:?}")
            }
            return Err(block);
        }
        let mut recv_result = RecvResult::new(&mut block, route_key);
        if let Some(interceptor) = interceptor {
            if interceptor.pre_handle(&mut recv_result).await {
                return Err(block);
            }
        }

        match self.handle(recv_result).await {
            Ok(handle_result) => {
                if let Some(rs) = handle_result {
                    #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
                    let mut rs = rs;
                    #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
                    {
                        if rs.is_encrypt != self.node_context.cipher.is_some() {
                            return Ok(Ok(Err(HandleError::new(
                                route_key,
                                io::Error::new(
                                    io::ErrorKind::Other,
                                    format!(
                                        "Inconsistent encryption status: data tag-{} current tag-{}",
                                        rs.is_encrypt,
                                        self.node_context.cipher.is_some()
                                    ),
                                ),
                            ))));
                        }
                        if let Some(cipher) = self.node_context.cipher.as_ref() {
                            match cipher
                                .decrypt(tag(&rs.src_id, &rs.dest_id), &mut block[rs.start..rs.end])
                            {
                                Ok(_len) => rs.end -= cipher.reserved_len(),
                                Err(e) => return Ok(Ok(Err(HandleError::new(route_key, e)))),
                            }
                        }
                    }
                    block.truncate(rs.end);
                    Ok(Ok(Ok((
                        RecvUserData {
                            _offset: rs.start,

                            _data: block,
                        },
                        RecvMetadata {
                            _src_id: rs.src_id,
                            _dest_id: rs.dest_id,
                            _route_key: rs.route_key,
                            _ttl: rs.ttl,
                            _max_ttl: rs.max_ttl,
                        },
                    ))))
                } else {
                    Err(block)
                }
            }
            Err(e) => Ok(Ok(Err(HandleError::new(route_key, e)))),
        }
    }
    pub fn protocol(&self) -> ConnectProtocol {
        self.tunnel.protocol()
    }
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.tunnel.remote_addr()
    }
    pub(crate) async fn send_to<B: AsRef<[u8]>>(
        &self,
        buf: &NetPacket<B>,
        id: &NodeID,
    ) -> io::Result<()> {
        self.tunnel_transmit.send_to_id(buf, id).await
    }
    pub(crate) async fn send_to_route(&self, buf: &[u8], route_key: &RouteKey) -> io::Result<()> {
        self.tunnel_transmit.send_to_route(buf, route_key).await
    }
    async fn other_group_handle(
        &mut self,
        mut packet: NetPacket<&mut [u8]>,
        route_key: RouteKey,
        self_group_code: GroupCode,
        self_id: NodeID,
    ) -> io::Result<()> {
        let metric = packet.max_ttl() - packet.ttl();
        let src_group_code = GroupCode::try_from(packet.group_code())?;
        let src_id = NodeID::try_from(packet.src_id())?;
        {
            let ref_mut = self
                .node_context
                .other_route_table
                .entry(src_group_code)
                .or_insert_with(|| RouteTable::new(LoadBalance::MinHopLowestLatency, 1));
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
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "time error"));
                    }
                    let time = u32::from_be_bytes(time.try_into().unwrap());
                    let now = now()?;
                    let rtt = now.saturating_sub(time);
                    self.node_context
                        .other_route_table
                        .get(&src_group_code)
                        .map(|v| v.add_route(src_id, Route::from(route_key, metric, rtt)));
                }
                _ => {}
            }
            return Ok(());
        }

        if let Some(route_table) = self.node_context.other_route_table.get(&src_group_code) {
            if dest_id.is_broadcast() {
                // broadcast
            } else if let Ok(v) = route_table.get_route_by_id(&dest_id) {
                if !route_table.is_route_of_peer_id(&src_id, &v.route_key()) {
                    self.send_to_route(packet.buffer(), &v.route_key()).await?;
                }
            } else if let Some((relay_group_code, relay_node_id)) =
                self.node_context.reachable_node(&src_group_code, &dest_id)
            {
                drop(route_table);
                if let Some(route_table) =
                    self.node_context.other_route_table.get(&relay_group_code)
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
    async fn handle(
        &mut self,
        recv_result: RecvResult<'_>,
    ) -> io::Result<Option<HandleResultInner>> {
        let mut packet = NetPacket::new(recv_result.buf)?;
        if packet.max_ttl() < packet.ttl() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "ttl error"));
        }
        if packet.ttl() == 0 {
            return Ok(None);
        }
        let src_id = NodeID::try_from(packet.src_id())?;

        if src_id.is_unspecified() || src_id.is_broadcast() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "src id is unspecified",
            ));
        }
        let self_id = if let Some(self_id) = self.node_context.load_id() {
            self_id
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "self id is none",
            ));
        };
        let group_code = self.node_context.load_group_code();
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
            return Err(io::Error::new(io::ErrorKind::InvalidData, "id loop error"));
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
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "time error"));
                }
                let time = u32::from_be_bytes(time.try_into().unwrap());
                let now = now()?;
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
                let punch_info = rmp_serde::from_slice::<PunchConsultInfo>(packet.payload())
                    .map_err(|e| {
                        io::Error::new(io::ErrorKind::Other, format!("RmpDecodeError: {e:?}"))
                    })?;
                log::debug!("PunchConsultRequest {:?}", punch_info);
                let consult_info = self
                    .node_context
                    .gen_punch_info(punch_info.peer_nat_info.seq);
                let data = rmp_serde::to_vec(&consult_info).map_err(|e| {
                    io::Error::new(io::ErrorKind::Other, format!("RmpEncodeError: {e:?}"))
                })?;
                let mut send_packet = self
                    .tunnel_transmit
                    .allocate_send_packet_proto(ProtocolType::PunchConsultReply, data.len())?;
                send_packet.set_payload(&data);
                if let Ok(sender) = self.passive_punch_sender.try_reserve() {
                    self.tunnel_transmit
                        .send_packet_to(send_packet, &src_id)
                        .await?;
                    sender.send((src_id, punch_info))
                }
            }
            ProtocolType::PunchConsultReply => {
                let punch_info = rmp_serde::from_slice::<PunchConsultInfo>(packet.payload())
                    .map_err(|e| {
                        io::Error::new(io::ErrorKind::Other, format!("RmpDecodeError: {e:?}"))
                    })?;
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
    ) -> io::Result<()> {
        let payload = packet.payload();
        if payload.len() != 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IDRouteQuery packet error",
            ));
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

        if !self.node_context.other_route_table.is_empty() {
            // Reply to reachable nodes of other groups
            let tunnel_tx = self.tunnel_transmit.clone();
            let other_route_table = self.node_context.other_route_table.clone();
            tokio::spawn(async move {
                if let Err(e) = id_route_reply(
                    tunnel_tx,
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
    ) -> io::Result<()> {
        let reply_packet = IDRouteReplyPacket::new(packet.payload())?;
        let reachable_group_code = GroupCode::try_from(reply_packet.group_code())?;

        let id = reply_packet.query_id();
        if id != 0 {
            self.node_context
                .update_direct_node_id(id, reachable_group_code, src_id);
        }
        for (reachable_id, metric) in reply_packet.iter() {
            if self_group_code == reachable_group_code && reachable_id == self_id {
                continue;
            }
            self.node_context.update_reachable_nodes(
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
    tunnel_tx: TunnelTransmitHub,
    other_route_table: Arc<DashMap<GroupCode, RouteTable<NodeID>>>,
    route_key: RouteKey,
    self_group_code: GroupCode,
    self_id: NodeID,
    src_group_code: GroupCode,
    src_id: NodeID,
) -> io::Result<()> {
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
        tunnel_tx.send_to_route(packet.buffer(), &route_key).await?;
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
    pub fn net_packet(&self) -> io::Result<NetPacket<&[u8]>> {
        NetPacket::new(self.buf)
    }
    pub fn net_packet_mut(&mut self) -> io::Result<NetPacket<&mut [u8]>> {
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
#[derive(Copy, Clone, Debug)]
pub struct RecvMetadata {
    _ttl: u8,
    _max_ttl: u8,
    _src_id: NodeID,
    _dest_id: NodeID,
    _route_key: RouteKey,
}
impl RecvMetadata {
    pub fn ttl(&self) -> u8 {
        self._ttl
    }
    pub fn max_ttl(&self) -> u8 {
        self._max_ttl
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
    pub fn is_broadcast(&self) -> bool {
        self._dest_id.is_broadcast()
    }
}
pub struct RecvUserData {
    _offset: usize,
    _data: Data,
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

impl AsMut<[u8]> for Data {
    fn as_mut(&mut self) -> &mut [u8] {
        self
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
    pub fn offset(&self) -> usize {
        self._offset
    }
    pub fn original_bytes_mut(&mut self) -> &mut BytesMut {
        &mut self._data
    }
    pub fn original_bytes(&self) -> &BytesMut {
        &self._data
    }
    pub fn is_recyclable(&self) -> bool {
        match self._data {
            Data::Recyclable(_) => true,
            Data::Temporary(_) => false,
        }
    }
}

impl RecvUserData {
    pub fn payload(&self) -> &[u8] {
        &self._data[self._offset..]
    }
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self._data[self._offset..]
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
    err: io::Error,
}

impl HandleError {
    pub(crate) fn new(route_key: RouteKey, err: io::Error) -> Self {
        Self { route_key, err }
    }
    pub fn addr(&self) -> SocketAddr {
        self.route_key.addr()
    }
    pub fn err(&self) -> &io::Error {
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
fn now() -> io::Result<u32> {
    let time = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_e| io::Error::new(io::ErrorKind::Other, "system time error"))?
        .as_millis() as u32;
    Ok(time)
}
