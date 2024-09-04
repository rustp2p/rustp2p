use crate::config::PipeConfig;
use crate::error::{Error, Result};
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::NetPacket;
use rust_p2p_core::route::RouteKey;

pub struct Pipe {
    pipe: rust_p2p_core::pipe::Pipe<NodeID>,
    puncher: rust_p2p_core::punch::Puncher<NodeID>,
    idle_route_manager: rust_p2p_core::idle::IdleRouteManager<NodeID>,
}
impl Pipe {
    pub fn new(config: PipeConfig) -> anyhow::Result<Pipe> {
        let (pipe, puncher, idle_route_manager) =
            rust_p2p_core::pipe::pipe::<NodeID>(config.into())?;
        Ok(Self {
            pipe,
            puncher,
            idle_route_manager,
        })
    }
}

impl Pipe {
    pub async fn accept(&mut self) -> anyhow::Result<PipeLine> {
        let pipe_line = self.pipe.accept().await?;
        Ok(PipeLine { pipe_line })
    }
}

pub struct PipeLine {
    pipe_line: rust_p2p_core::pipe::PipeLine,
}
impl PipeLine {
    pub async fn recv_from<'a>(&mut self, buf: &'a mut [u8]) -> Option<Result<RecvResult<'a>>> {
        let (len, route_key) = match self.pipe_line.recv_from(buf).await? {
            Ok((len, route_key)) => (len, route_key),
            Err(e) => return Some(Err(Error::Io(e))),
        };
        let packet = match NetPacket::new(&mut buf[..len]) {
            Ok(packet) => packet,
            Err(e) => return Some(Err(e)),
        };
        Some(Ok(RecvResult { packet, route_key }))
    }
}

pub struct RecvResult<'a> {
    packet: NetPacket<&'a mut [u8]>,
    route_key: RouteKey,
}
impl<'a> RecvResult<'a> {
    #[inline]
    pub fn is_user_data(&self) -> bool {
        self.packet.protocol() == ProtocolType::UserData
    }
    pub fn user_payload_mut(&mut self) -> Option<&mut [u8]> {
        if self.is_user_data() {
            Some(self.packet.payload_mut())
        } else {
            None
        }
    }
}
