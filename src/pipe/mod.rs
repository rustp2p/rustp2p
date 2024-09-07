use std::fmt::Display;
use std::net::SocketAddr;
use std::time::UNIX_EPOCH;

use rust_p2p_core::route::route_table::RouteTable;
use rust_p2p_core::route::{ConnectProtocol, Route, RouteKey};

pub use pipe_context::NodeAddress;
pub use send_packet::SendPacket;

use crate::config::PipeConfig;
use crate::error::{Error, Result};
use crate::pipe::pipe_context::PipeContext;
use crate::protocol::id_route::IDRouteReplyPacket;
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::NetPacket;

mod maintain;
mod pipe_context;

mod send_packet;

pub struct Pipe {
    send_buffer_size: usize,
    pipe_context: PipeContext,
    pipe: rust_p2p_core::pipe::Pipe<NodeID>,
    puncher: rust_p2p_core::punch::Puncher<NodeID>,
}

impl Pipe {
    pub async fn new(mut config: PipeConfig) -> Result<Pipe> {
        let pipe_context = PipeContext::default();
        let send_buffer_size = config.send_buffer_size;
        if let Some(node_id) = config.self_id.take() {
            pipe_context.store_self_id(node_id)?;
        }
        if let Some(addrs) = config.direct_addrs.take() {
            let x = addrs.into_iter().map(|v| (v, None)).collect();
            pipe_context.set_direct_nodes(x);
        }
        let (pipe, puncher, idle_route_manager) =
            rust_p2p_core::pipe::pipe::<NodeID>(config.into())?;
        let pipe_writer = PipeWriter {
            send_buffer_size,
            pipe_context: pipe_context.clone(),
            pipe_writer: pipe.writer_ref().to_owned(),
        };
        maintain::start_task(&pipe_writer, idle_route_manager);
        Ok(Self {
            send_buffer_size,
            pipe_context,
            pipe,
            puncher,
        })
    }
    pub fn writer(&self) -> PipeWriter {
        PipeWriter {
            send_buffer_size: self.send_buffer_size,
            pipe_context: self.pipe_context.clone(),
            pipe_writer: self.pipe.writer_ref().to_owned(),
        }
    }
}

impl Pipe {
    pub async fn accept(&mut self) -> anyhow::Result<PipeLine> {
        let pipe_line = self.pipe.accept().await?;
        Ok(PipeLine {
            pipe_context: self.pipe_context.clone(),
            pipe_line,
            pipe_writer: self.pipe.writer_ref().to_owned(),
            route_table: self.pipe.route_table().clone(),
        })
    }
}

#[derive(Clone)]
pub struct PipeWriter {
    send_buffer_size: usize,
    pipe_context: PipeContext,
    pipe_writer: rust_p2p_core::pipe::PipeWriter<NodeID>,
}

impl PipeWriter {
    pub fn pipe_context(&self) -> &PipeContext {
        &self.pipe_context
    }
    pub(crate) async fn send_to_route(&self, buf: &[u8], route_key: &RouteKey) -> Result<usize> {
        let len = self.pipe_writer.send_to(buf, route_key).await?;
        Ok(len)
    }
    /// user data is `buf[start..end]`,
    ///
    ///  # Panics
    /// Panics if dst buffer is too small.
    /// Need to ensure that start>=head_reserve .
    ///
    /// [`PipeWriter::head_reserve()`]
    pub async fn send_to(
        &self,
        buf: &mut [u8],
        start: usize,
        end: usize,
        dest_id: &NodeID,
    ) -> Result<usize> {
        let src_id = if let Some(src_id) = self.pipe_context.load_id() {
            src_id
        } else {
            return Err(Error::NoIDSpecified);
        };
        let head_reserve = 4 + src_id.len() * 2;
        self.send_to0(&mut buf[start - head_reserve..end], &src_id, dest_id)
            .await
    }
    async fn send_to0(&self, buf: &mut [u8], src_id: &NodeID, dest_id: &NodeID) -> Result<usize> {
        let mut packet = NetPacket::unchecked(buf);
        if packet.ttl() == 0 || packet.ttl() != packet.first_ttl() {
            packet.set_ttl(15);
        }
        packet.set_id_length(src_id.len() as _);
        packet.set_src_id(src_id)?;
        packet.set_dest_id(dest_id)?;
        let len = if let Ok(route) = self.pipe_writer.route_table().get_route_by_id(dest_id) {
            self.pipe_writer
                .send_to(packet.buffer(), &route.route_key())
                .await?
        } else {
            if let Some(turn_id) = self.pipe_context().reachable_node(dest_id) {
                self.pipe_writer
                    .send_to_id(packet.buffer(), &turn_id)
                    .await?
            } else if let Some(addr) = self.pipe_context().default_route() {
                // default route
                match addr {
                    NodeAddress::Tcp(addr) => {
                        self.pipe_writer
                            .send_to_addr(ConnectProtocol::TCP, packet.buffer(), addr)
                            .await?
                    }
                    NodeAddress::Udp(addr) => {
                        self.pipe_writer
                            .send_to_addr(ConnectProtocol::TCP, packet.buffer(), addr)
                            .await?
                    }
                }
            } else {
                Err(Error::NodeIDNotAvailable)?
            }
        };

        Ok(len)
    }
    /// use [`PipeWriter::send_to()`] head reserve
    pub fn head_reserve(&self) -> Result<usize> {
        if let Some(id) = self.pipe_context.load_id() {
            Ok(4 + id.len() * 2)
        } else {
            Err(Error::NoIDSpecified)
        }
    }
    pub async fn send_to_packet(&self, packet: &mut SendPacket, dest_id: &NodeID) -> Result<usize> {
        if let Some(src_id) = self.pipe_context.load_id() {
            self.send_to0(packet.buf_mut(), &src_id, dest_id).await
        } else {
            Err(Error::NoIDSpecified)
        }
    }
    pub fn allocate_send_packet(&self) -> Result<SendPacket> {
        let head_reserve = self.head_reserve()?;
        let send_packet = SendPacket::new_capacity(head_reserve, self.send_buffer_size);
        Ok(send_packet)
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
        let head_reserve = 4 + src_id.len() * 2;
        let mut send_packet = SendPacket::new_capacity(head_reserve, head_reserve + payload_size);
        let mut packet = NetPacket::unchecked(send_packet.buf_mut());
        packet.set_id_length(src_id.len() as _);
        packet.set_ttl(15);
        packet.set_src_id(&src_id)?;
        packet.set_protocol(protocol_type);
        Ok(send_packet)
    }
}

pub struct PipeLine {
    pipe_context: PipeContext,
    pipe_line: rust_p2p_core::pipe::PipeLine,
    pipe_writer: rust_p2p_core::pipe::PipeWriter<NodeID>,
    route_table: RouteTable<NodeID>,
}

impl PipeLine {
    pub fn store_self_id(&self, node_id: NodeID) -> Result<()> {
        self.pipe_context.store_self_id(node_id)
    }
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
            match self
                .handle(RecvResult::new(&mut buf[..len], route_key))
                .await
            {
                Ok(handle_result) => {
                    if let Some(handle_result) = handle_result {
                        //return Ok(Ok(handle_result));
                        // workaround borrowing checker
                        return match handle_result {
                            HandleResult::Turn(_, arg_1, arg_2) => Ok(Ok(HandleResult::Turn(
                                NetPacket::new(&mut buf[..len]).unwrap(),
                                arg_1,
                                arg_2,
                            ))),
                            HandleResult::UserData(_, arg_1, arg_2) => {
                                Ok(Ok(HandleResult::UserData(
                                    NetPacket::new(&mut buf[..len]).unwrap(),
                                    arg_1,
                                    arg_2,
                                )))
                            }
                        };
                    }
                }
                Err(e) => return Ok(Err(HandleError::new(route_key, e))),
            };
        }
    }
    pub async fn send_to(&self, buf: &[u8], id: &NodeID) -> Result<()> {
        self.pipe_writer.send_to_id(buf, id).await?;
        Ok(())
    }
    pub(crate) async fn send_to_route(&self, buf: &[u8], route_key: &RouteKey) -> Result<()> {
        self.pipe_writer.send_to(buf, route_key).await?;
        Ok(())
    }
    pub async fn handle<'a>(
        &mut self,
        recv_result: RecvResult<'a>,
    ) -> Result<Option<HandleResult<'a>>> {
        let mut packet = NetPacket::new(recv_result.buf)?;
        let src_id = NodeID::new(packet.src_id())?;

        if src_id.is_unspecified() || src_id.is_broadcast() {
            return Err(Error::InvalidArgument("src id is unspecified".into()));
        }
        let dest_id = NodeID::new(packet.dest_id())?;
        if src_id.is_unspecified() {
            return Err(Error::InvalidArgument("src id is unspecified".into()));
        }

        if packet.first_ttl() < packet.ttl() {
            return Err(Error::InvalidArgument("ttl error".into()));
        }

        if packet.ttl() == 0 {
            return Ok(None);
        }
        let route_key = recv_result.route_key;

        let self_id = if let Some(self_id) = self.pipe_context.load_id() {
            if self_id.len() != dest_id.len() {
                return Err(Error::InvalidArgument("id len error".into()));
            }
            if self_id == src_id {
                return Err(Error::InvalidArgument("id loop error".into()));
            }
            if self_id != dest_id && !dest_id.is_unspecified() {
                return if packet.incr_ttl() {
                    Ok(Some(HandleResult::Turn(packet, dest_id, route_key)))
                } else {
                    Ok(None)
                };
            }
            self_id
        } else {
            return Err(Error::InvalidArgument("self id is none".into()));
        };
        let id_length = self_id.len();
        let metric = packet.first_ttl() - packet.ttl() + 1;
        self.route_table
            .add_route_if_absent(src_id, Route::from_default_rt(route_key, metric));
        match packet.protocol()? {
            ProtocolType::PunchConsult => {}
            ProtocolType::PunchRequest => {
                packet.set_protocol(ProtocolType::PunchReply);
                packet.set_ttl(packet.first_ttl());
                packet.exchange_id();
                packet.set_src_id(&self_id)?;
                self.send_to_route(packet.buffer(), &route_key).await?;
            }
            ProtocolType::PunchReply => {}
            ProtocolType::EchoRequest => {
                packet.set_protocol(ProtocolType::EchoReply);
                packet.set_ttl(packet.first_ttl());
                packet.exchange_id();
                packet.set_src_id(&self_id)?;
                self.send_to_route(packet.buffer(), &route_key).await?;
            }
            ProtocolType::EchoReply => {}
            ProtocolType::TimestampRequest => {
                packet.set_protocol(ProtocolType::TimestampReply);
                packet.set_ttl(packet.first_ttl());
                packet.exchange_id();
                packet.set_src_id(&self_id)?;
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
                let rtt = now.checked_sub(time).unwrap_or(0) as _;
                self.route_table
                    .add_route(src_id, Route::from(route_key, metric, rtt));
            }
            ProtocolType::IDRouteQuery => {
                // reply reachable node id
                let mut list = self.route_table.route_table_min_metric();
                if !list.is_empty() {
                    // Not supporting too many nodes
                    list.truncate(255);
                    let list: Vec<_> = list
                        .into_iter()
                        .filter(|(node_id, _)| node_id != &src_id)
                        .map(|(node_id, route)| (node_id, route.metric()))
                        .collect();
                    if !list.is_empty() {
                        let packet = crate::protocol::id_route::Builder::build_reply(
                            &list,
                            list.len() as _,
                        )?;
                        self.send_to_route(packet.buffer(), &route_key).await?;
                    }
                }
            }
            ProtocolType::IDRouteReply => {
                let reply_packet = IDRouteReplyPacket::new(packet.payload(), id_length as _)?;
                for (reachable_id, metric) in reply_packet.iter() {
                    if reachable_id == self_id {
                        continue;
                    }
                    self.pipe_context
                        .update_reachable_nodes(src_id, reachable_id, metric);
                }
            }
            ProtocolType::UserData => {
                return Ok(Some(HandleResult::UserData(packet, src_id, route_key)))
            }
        }

        return Ok(None);
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

pub enum HandleResult<'a> {
    Turn(NetPacket<&'a mut [u8]>, NodeID, RouteKey),
    UserData(NetPacket<&'a mut [u8]>, NodeID, RouteKey),
}

#[derive(Debug)]
pub enum RecvError {
    Done,
    Io(std::io::Error),
}

#[derive(Debug)]
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
