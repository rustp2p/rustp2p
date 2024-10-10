use bytes::BytesMut;
use std::hash::Hash;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use tcp_pipe::TcpPipeWriterRef;

use crate::idle::IdleRouteManager;
use crate::pipe::config::PipeConfig;
use crate::pipe::extensible_pipe::{
    ExtensiblePipe, ExtensiblePipeLine, ExtensiblePipeWriter, ExtensiblePipeWriterRef,
};
use crate::pipe::tcp_pipe::{TcpPipe, TcpPipeLine, TcpPipeWriter};
use crate::pipe::udp_pipe::{UdpPipe, UdpPipeLine, UdpPipeWriter, UdpPipeWriterRef};
use crate::punch::Puncher;
use crate::route::route_table::RouteTable;
use crate::route::{ConnectProtocol, RouteKey};

pub mod config;
pub mod extensible_pipe;
pub mod recycle;
pub mod tcp_pipe;
pub mod udp_pipe;
pub const DEFAULT_ADDRESS_V4: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
pub const DEFAULT_ADDRESS_V6: SocketAddr =
    SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0));

pub type PipeComponent<PeerID> = (Pipe<PeerID>, Puncher<PeerID>, IdleRouteManager<PeerID>);
/// Construct the needed components for p2p communication with the given pipe configuration
pub fn pipe<PeerID: Hash + Eq + Clone>(
    config: PipeConfig,
) -> anyhow::Result<PipeComponent<PeerID>> {
    let route_table = RouteTable::new(config.first_latency, config.multi_pipeline);
    let udp_pipe = if let Some(mut udp_pipe_config) = config.udp_pipe_config {
        udp_pipe_config.main_pipeline_num = config.multi_pipeline;
        Some(UdpPipe::new(udp_pipe_config)?)
    } else {
        None
    };
    let tcp_pipe = if let Some(mut tcp_pipe_config) = config.tcp_pipe_config {
        tcp_pipe_config.tcp_multiplexing_limit = config.multi_pipeline;
        Some(TcpPipe::new(tcp_pipe_config)?)
    } else {
        None
    };
    let extensible_pipe = if config.enable_extend {
        Some(ExtensiblePipe::new())
    } else {
        None
    };
    let pipe = Pipe {
        route_table: route_table.clone(),
        udp_pipe,
        tcp_pipe,
        extensible_pipe,
    };
    let puncher = Puncher::from(&pipe);
    Ok((
        pipe,
        puncher,
        IdleRouteManager::new(config.route_idle_time, route_table),
    ))
}

pub struct Pipe<PeerID> {
    route_table: RouteTable<PeerID>,
    udp_pipe: Option<UdpPipe>,
    tcp_pipe: Option<TcpPipe>,
    extensible_pipe: Option<ExtensiblePipe>,
}

pub enum PipeLine {
    Udp(UdpPipeLine),
    Tcp(TcpPipeLine),
    Extend(ExtensiblePipeLine),
}

#[derive(Clone)]
pub struct PipeWriter<PeerID> {
    route_table: RouteTable<PeerID>,
    udp_pipe_writer: Option<UdpPipeWriter>,
    tcp_pipe_writer: Option<TcpPipeWriter>,
    extensible_pipe_writer: Option<ExtensiblePipeWriter>,
}

pub struct PipeWriterRef<'a, PeerID> {
    route_table: &'a RouteTable<PeerID>,
    udp_pipe_writer: Option<UdpPipeWriterRef<'a>>,
    tcp_pipe_writer: Option<TcpPipeWriterRef<'a>>,
    extensible_pipe_writer: Option<ExtensiblePipeWriterRef<'a>>,
}

impl<PeerID> Pipe<PeerID> {
    /// Accept pipelines from a given `pipe`
    pub async fn accept(&mut self) -> anyhow::Result<PipeLine> {
        tokio::select! {
            rs=accept_udp(self.udp_pipe.as_mut())=>{
                rs
            }
            rs=accept_tcp(self.tcp_pipe.as_mut())=>{
                rs
            }
            rs=accept_extend(self.extensible_pipe.as_mut())=>{
                rs
            }
        }
    }
}
async fn accept_tcp(tcp: Option<&mut TcpPipe>) -> anyhow::Result<PipeLine> {
    if let Some(tcp_pipe) = tcp {
        Ok(PipeLine::Tcp(tcp_pipe.accept().await?))
    } else {
        futures::future::pending().await
    }
}
async fn accept_udp(udp: Option<&mut UdpPipe>) -> anyhow::Result<PipeLine> {
    if let Some(udp_pipe) = udp {
        Ok(PipeLine::Udp(udp_pipe.accept().await?))
    } else {
        futures::future::pending().await
    }
}
async fn accept_extend(extend: Option<&mut ExtensiblePipe>) -> anyhow::Result<PipeLine> {
    if let Some(extend) = extend {
        Ok(PipeLine::Extend(extend.accept().await?))
    } else {
        futures::future::pending().await
    }
}

impl<PeerID> Pipe<PeerID> {
    pub fn udp_pipe_ref(&mut self) -> Option<&mut UdpPipe> {
        self.udp_pipe.as_mut()
    }
    pub fn tcp_pipe_ref(&mut self) -> Option<&mut TcpPipe> {
        self.tcp_pipe.as_mut()
    }
    /// Acquire the `route_table` associated with the `pipe`
    pub fn route_table(&self) -> &RouteTable<PeerID> {
        &self.route_table
    }
    /// Acquire a shared reference for writing to the `pipe`
    pub fn writer_ref(&self) -> PipeWriterRef<PeerID> {
        PipeWriterRef {
            route_table: &self.route_table,
            udp_pipe_writer: self.udp_pipe.as_ref().map(|v| v.writer_ref()),
            tcp_pipe_writer: self.tcp_pipe.as_ref().map(|v| v.writer_ref()),
            extensible_pipe_writer: self.extensible_pipe.as_ref().map(|v| v.writer_ref()),
        }
    }
}

impl<'a, PeerID> PipeWriterRef<'a, PeerID> {
    pub fn to_owned(&self) -> PipeWriter<PeerID> {
        PipeWriter {
            route_table: self.route_table.clone(),
            udp_pipe_writer: self.udp_pipe_writer.as_ref().map(|v| v.to_owned()),
            tcp_pipe_writer: self.tcp_pipe_writer.as_ref().map(|v| v.to_owned()),
            extensible_pipe_writer: self.extensible_pipe_writer.as_ref().map(|v| v.to_owned()),
        }
    }
    /// Acquire a shared reference for writing to the pipe established by `TCP`
    pub fn tcp_pipe_writer_ref(&self) -> Option<TcpPipeWriterRef<'_>> {
        self.tcp_pipe_writer
    }
    /// Acquire a shared reference for writing to the pipe established by `UDP`
    pub fn udp_pipe_writer_ref(&self) -> Option<UdpPipeWriterRef<'_>> {
        self.udp_pipe_writer
    }
    /// Acquire a shared reference for writing to the pipe established by other extended protocols
    pub fn extensible_pipe_writer_ref(&self) -> Option<ExtensiblePipeWriterRef<'_>> {
        self.extensible_pipe_writer
    }
}

impl<PeerID> PipeWriter<PeerID> {
    /// Acquire a owned `writer` for writing to the pipe established by `TCP`
    pub fn udp_pipe_writer(&self) -> Option<&UdpPipeWriter> {
        self.udp_pipe_writer.as_ref()
    }
    /// Acquire a owned `writer` for writing to the pipe established by `UDP`
    pub fn tcp_pipe_writer(&self) -> Option<&TcpPipeWriter> {
        self.tcp_pipe_writer.as_ref()
    }
    /// Acquire a owned `writer` for writing to the pipe established by other extended protocols
    pub fn extensible_pipe_writer(&self) -> Option<&ExtensiblePipeWriter> {
        self.extensible_pipe_writer.as_ref()
    }
    pub fn route_table(&self) -> &RouteTable<PeerID> {
        &self.route_table
    }
}

impl<PeerID> PipeWriter<PeerID> {
    /// Writing `buf` to the target denoted by `route_key`
    pub async fn send_to(&self, buf: BytesMut, route_key: &RouteKey) -> crate::error::Result<()> {
        match route_key.protocol() {
            ConnectProtocol::UDP => {
                if let Some(w) = self.udp_pipe_writer.as_ref() {
                    return w.send_buf_to(buf, route_key).await;
                }
            }
            ConnectProtocol::TCP => {
                if let Some(w) = self.tcp_pipe_writer.as_ref() {
                    return w.send_to(buf, route_key).await;
                }
            }
            ConnectProtocol::Extend => {
                if let Some(w) = self.extensible_pipe_writer.as_ref() {
                    return w.send_to(buf, route_key).await;
                }
            }
        }
        Err(crate::error::Error::InvalidProtocol)
    }

    /// Writing `buf` to the target denoted by SocketAddr with the specified protocol
    pub async fn send_to_addr<A: Into<SocketAddr>>(
        &self,
        connect_protocol: ConnectProtocol,
        buf: BytesMut,
        addr: A,
    ) -> crate::error::Result<()> {
        match connect_protocol {
            ConnectProtocol::UDP => {
                if let Some(w) = self.udp_pipe_writer.as_ref() {
                    return w.send_buf_to_addr(buf, addr).await;
                }
            }
            ConnectProtocol::TCP => {
                if let Some(w) = self.tcp_pipe_writer.as_ref() {
                    return w.send_to_addr(buf, addr).await;
                }
            }
            ConnectProtocol::Extend => {}
        }
        Err(crate::error::Error::InvalidProtocol)
    }
}
impl<PeerID: Hash + Eq> PipeWriter<PeerID> {
    /// Writing `buf` to the target named by `peer_id`
    pub async fn send_to_id(&self, buf: BytesMut, peer_id: &PeerID) -> crate::error::Result<()> {
        let route = self.route_table.get_route_by_id(peer_id)?;
        self.send_to(buf, &route.route_key()).await
    }
}

impl PipeLine {
    /// Receving buf from the associated PipeLine
    /// `usize` in the `Ok` branch indicates how many bytes are received
    /// `RouteKey` in the `Ok` branch denotes the source where these bytes are received from
    pub async fn recv_from(
        &mut self,
        buf: &mut [u8],
    ) -> Option<std::io::Result<(usize, RouteKey)>> {
        match self {
            PipeLine::Udp(line) => line.recv_from(buf).await,
            PipeLine::Tcp(line) => Some(line.recv_from(buf).await),
            PipeLine::Extend(line) => Some(line.recv_from(buf).await),
        }
    }
}

impl PipeLine {
    /// The protocol this pipeline is using
    pub fn protocol(&self) -> ConnectProtocol {
        match self {
            PipeLine::Udp(_) => ConnectProtocol::UDP,
            PipeLine::Tcp(_) => ConnectProtocol::TCP,
            PipeLine::Extend(_) => ConnectProtocol::Extend,
        }
    }
}
