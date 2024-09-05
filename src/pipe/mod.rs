use std::net::SocketAddr;
use std::time::UNIX_EPOCH;

use rust_p2p_core::route::route_table::RouteTable;
use rust_p2p_core::route::{Route, RouteKey};

use crate::config::PipeConfig;
use crate::error::{Error, Result};
use crate::pipe::pipe_context::PipeContext;
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::NetPacket;

mod pipe_context;
mod pipe_manager;

pub struct Pipe {
    pipe_context: PipeContext,
    pipe: rust_p2p_core::pipe::Pipe<NodeID>,
    puncher: rust_p2p_core::punch::Puncher<NodeID>,
    idle_route_manager: rust_p2p_core::idle::IdleRouteManager<NodeID>,
}

impl Pipe {
    pub fn new(config: PipeConfig) -> Result<Pipe> {
        let (pipe, puncher, idle_route_manager) =
            rust_p2p_core::pipe::pipe::<NodeID>(config.into())?;
        Ok(Self {
            pipe_context: PipeContext::default(),
            pipe,
            puncher,
            idle_route_manager,
        })
    }
    pub fn store_self_id(&self, node_id: NodeID) -> Result<()> {
        self.pipe_context.store_self_id(node_id)
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
    pipe_context: PipeContext,
    pipe_writer: rust_p2p_core::pipe::PipeWriter<NodeID>,
}
impl PipeWriter {
    pub fn pipe_context(&self) -> &PipeContext {
        &self.pipe_context
    }
    pub async fn send_to(&self, buf: &[u8], route_key: &RouteKey) -> Result<usize> {
        let len = self.pipe_writer.send_to(buf, route_key).await?;
        Ok(len)
    }
    pub async fn send_to_id(&self, buf: &[u8], peer_id: &NodeID) -> Result<usize> {
        let len = self.pipe_writer.send_to_id(buf, peer_id).await?;
        Ok(len)
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
    pub async fn recv_from<'a>(&mut self, buf: &'a mut [u8]) -> Option<Result<RecvResult<'a>>> {
        let (len, route_key) = match self.pipe_line.recv_from(buf).await? {
            Ok((len, route_key)) => (len, route_key),
            Err(e) => return Some(Err(Error::Io(e))),
        };
        Some(Ok(RecvResult::new(&mut buf[..len], route_key)))
    }
    pub async fn send_to(&self, buf: &[u8], id: &NodeID) -> Result<()> {
        self.pipe_writer.send_to_id(buf, id).await?;
        Ok(())
    }
    pub async fn send_to_route(&self, buf: &[u8], route_key: &RouteKey) -> Result<()> {
        self.pipe_writer.send_to(buf, route_key).await?;
        Ok(())
    }
    pub async fn handle<'a>(&mut self, recv_result: RecvResult<'a>) -> Result<HandleResult<'a>> {
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
            return Ok(HandleResult::Done);
        }
        let _self_id = if let Some(self_id) = self.pipe_context.load_id() {
            if self_id.len() != dest_id.len() {
                return Err(Error::InvalidArgument("id len error".into()));
            }
            if self_id != dest_id && !dest_id.is_broadcast() {
                return if packet.incr_ttl() {
                    Ok(HandleResult::Turn(packet, dest_id))
                } else {
                    Ok(HandleResult::Done)
                };
            }
            self_id
        } else {
            return Err(Error::InvalidArgument("self id is none".into()));
        };

        let route_key = recv_result.route_key;
        let metric = packet.first_ttl() - packet.ttl() + 1;
        self.route_table
            .add_route_if_absent(src_id, Route::from_default_rt(route_key, metric));
        match packet.protocol()? {
            ProtocolType::PunchConsult => {}
            ProtocolType::PunchRequest => {
                packet.set_protocol(ProtocolType::PunchReply);
                packet.set_ttl(packet.first_ttl());
                packet.exchange_id();
                return Ok(HandleResult::Reply(packet, route_key));
            }
            ProtocolType::PunchReply => {}
            ProtocolType::EchoRequest => {
                packet.set_protocol(ProtocolType::EchoReply);
                packet.set_ttl(packet.first_ttl());
                packet.exchange_id();
                return Ok(HandleResult::Reply(packet, route_key));
            }
            ProtocolType::EchoReply => {}
            ProtocolType::TimestampRequest => {
                packet.set_protocol(ProtocolType::TimestampRequest);
                packet.set_ttl(packet.first_ttl());
                packet.exchange_id();
                return Ok(HandleResult::Reply(packet, route_key));
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
                // Not supporting too many nodes
                list.truncate(255);
                let list: Vec<_> = list
                    .into_iter()
                    .map(|(node_id, route)| (node_id, route.metric()))
                    .collect();
                let packet =
                    crate::protocol::id_route::Builder::build_reply(&list, list.len() as _)?;
                return Ok(HandleResult::ReplyVec(packet, route_key));
            }
            ProtocolType::IDRouteReply => {}
            ProtocolType::UserData => return Ok(HandleResult::UserData(packet, src_id, route_key)),
        }

        return Ok(HandleResult::Done);
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
    Done,
    Reply(NetPacket<&'a mut [u8]>, RouteKey),
    ReplyVec(NetPacket<Vec<u8>>, RouteKey),
    Turn(NetPacket<&'a mut [u8]>, NodeID),
    UserData(NetPacket<&'a mut [u8]>, NodeID, RouteKey),
}
