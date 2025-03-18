use bytes::BytesMut;
use std::hash::Hash;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use crate::idle::IdleRouteManager;
use crate::tunnel::config::PipeConfig;
use crate::tunnel::extensible_pipe::{
    ExtensiblePipe, ExtensiblePipeLine, ExtensiblePipeWriter, ExtensiblePipeWriterRef,
};

use crate::punch::Puncher;
use crate::route::route_table::RouteTable;
use crate::route::{ConnectProtocol, RouteKey};
use std::sync::Arc;
use tokio::net::tcp as TokioTcp;

pub mod config;
pub mod extensible_pipe;
pub mod recycle;
pub mod tcp;
pub mod udp;
pub const DEFAULT_ADDRESS_V4: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
pub const DEFAULT_ADDRESS_V6: SocketAddr =
    SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0));

pub type PipeComponent<PeerID> = (
    TunnelManager<PeerID>,
    Puncher<PeerID>,
    IdleRouteManager<PeerID>,
);
/// Construct the needed components for p2p communication with the given tunnel configuration
pub fn pipe<PeerID: Hash + Eq + Clone>(config: PipeConfig) -> io::Result<PipeComponent<PeerID>> {
    let route_table = RouteTable::new(config.load_balance, config.multi_pipeline);
    let udp_tunnel_manager = if let Some(mut udp_pipe_config) = config.udp_pipe_config {
        udp_pipe_config.main_pipeline_num = config.multi_pipeline;
        Some(udp::TunnelManager::new(udp_pipe_config)?)
    } else {
        None
    };
    let tcp_tunnel_manager = if let Some(mut tcp_pipe_config) = config.tcp_pipe_config {
        tcp_pipe_config.tcp_multiplexing_limit = config.multi_pipeline;
        Some(tcp::TunnelManager::new(tcp_pipe_config)?)
    } else {
        None
    };
    let extensible_pipe = if config.enable_extend {
        Some(ExtensiblePipe::new())
    } else {
        None
    };
    let pipe = TunnelManager {
        route_table: route_table.clone(),
        udp_tunnel_manager,
        tcp_tunnel_manager,
        extensible_pipe,
    };
    let puncher = Puncher::from(&pipe);
    Ok((
        pipe,
        puncher,
        IdleRouteManager::new(config.route_idle_time, route_table),
    ))
}

pub struct TunnelManager<PeerID> {
    route_table: RouteTable<PeerID>,
    udp_tunnel_manager: Option<udp::TunnelManager>,
    tcp_tunnel_manager: Option<tcp::TunnelManager>,
    extensible_pipe: Option<ExtensiblePipe>,
}

pub enum PipeLine {
    Udp(udp::Tunnel),
    Tcp(tcp::Tunnel),
    Extend(ExtensiblePipeLine),
}

#[derive(Clone)]
pub struct SocketManager<PeerID> {
    route_table: RouteTable<PeerID>,
    udp_socket_manager: Option<Arc<udp::SocketManager>>,
    tcp_socket_manager: Option<Arc<tcp::SocketManager>>,
    extensible_pipe_writer: Option<ExtensiblePipeWriter>,
}

impl<PeerID> TunnelManager<PeerID> {
    /// Accept pipelines from a given `tunnel`
    pub async fn accept(&mut self) -> io::Result<PipeLine> {
        tokio::select! {
            rs=accept_udp(self.udp_tunnel_manager.as_mut())=>{
                rs
            }
            rs=accept_tcp(self.tcp_tunnel_manager.as_mut())=>{
                rs
            }
            rs=accept_extend(self.extensible_pipe.as_mut())=>{
                rs
            }
        }
    }
    pub fn shared_udp_socket_manager(&self) -> Option<Arc<udp::SocketManager>> {
        self.udp_tunnel_manager
            .as_ref()
            .map(|v| v.socket_manager.clone())
    }
    pub fn shared_tcp_socket_manager(&self) -> Option<Arc<tcp::SocketManager>> {
        self.tcp_tunnel_manager
            .as_ref()
            .map(|v| v.socket_manager.clone())
    }
    pub fn socket_manager(&self) -> SocketManager<PeerID> {
        SocketManager {
            route_table: self.route_table.clone(),
            udp_socket_manager: self.shared_udp_socket_manager(),
            tcp_socket_manager: self.shared_tcp_socket_manager(),
            extensible_pipe_writer: None,
        }
    }
}

impl<PeerID> TunnelManager<PeerID> {
    pub fn udp_tunnel_manager_as_mut(&mut self) -> Option<&mut udp::TunnelManager> {
        self.udp_tunnel_manager.as_mut()
    }
    pub fn tcp_tunnel_manager_as_mut(&mut self) -> Option<&mut tcp::TunnelManager> {
        self.tcp_tunnel_manager.as_mut()
    }
    /// Acquire the `route_table` associated with the `tunnel`
    pub fn route_table(&self) -> &RouteTable<PeerID> {
        &self.route_table
    }
}

async fn accept_tcp(tcp: Option<&mut tcp::TunnelManager>) -> io::Result<PipeLine> {
    if let Some(tcp_pipe) = tcp {
        Ok(PipeLine::Tcp(tcp_pipe.accept().await?))
    } else {
        futures::future::pending().await
    }
}
async fn accept_udp(udp_tunnel_manager: Option<&mut udp::TunnelManager>) -> io::Result<PipeLine> {
    if let Some(udp_pipe) = udp_tunnel_manager {
        Ok(PipeLine::Udp(udp_pipe.dispatch().await?))
    } else {
        futures::future::pending().await
    }
}
async fn accept_extend(extend: Option<&mut ExtensiblePipe>) -> io::Result<PipeLine> {
    if let Some(extend) = extend {
        Ok(PipeLine::Extend(extend.accept().await?))
    } else {
        futures::future::pending().await
    }
}

impl<PeerID> SocketManager<PeerID> {
    /// Acquire a owned `writer` for writing to the tunnel established by other extended protocols
    pub fn extensible_pipe_writer(&self) -> Option<&ExtensiblePipeWriter> {
        self.extensible_pipe_writer.as_ref()
    }
    pub fn route_table(&self) -> &RouteTable<PeerID> {
        &self.route_table
    }
}

impl<PeerID> SocketManager<PeerID> {
    /// Writing `buf` to the target denoted by `route_key`
    pub async fn send_to(&self, buf: BytesMut, route_key: &RouteKey) -> io::Result<()> {
        match route_key.protocol() {
            ConnectProtocol::UDP => {
                if let Some(w) = self.udp_socket_manager.as_ref() {
                    return w.send_buf_to(buf, route_key).await;
                }
            }
            ConnectProtocol::TCP => {
                if let Some(w) = self.tcp_socket_manager.as_ref() {
                    return w.send_to(buf, route_key).await;
                }
            }
            ConnectProtocol::Extend => {
                if let Some(w) = self.extensible_pipe_writer.as_ref() {
                    return w.send_to(buf, route_key).await;
                }
            }
        }
        Err(io::Error::from(io::ErrorKind::InvalidInput))
    }

    /// Writing `buf` to the target denoted by SocketAddr with the specified protocol
    pub async fn send_to_addr<A: Into<SocketAddr>>(
        &self,
        connect_protocol: ConnectProtocol,
        buf: BytesMut,
        addr: A,
    ) -> io::Result<()> {
        match connect_protocol {
            ConnectProtocol::UDP => {
                if let Some(w) = self.udp_socket_manager.as_ref() {
                    return w.send_buf_to_addr(buf, addr).await;
                }
            }
            ConnectProtocol::TCP => {
                if let Some(w) = self.tcp_socket_manager.as_ref() {
                    return w.send_to_addr(buf, addr).await;
                }
            }
            ConnectProtocol::Extend => {}
        }
        Err(io::Error::from(io::ErrorKind::InvalidInput))
    }
}
impl<PeerID: Hash + Eq> SocketManager<PeerID> {
    /// Writing `buf` to the target named by `peer_id`
    pub async fn send_to_id(&self, buf: BytesMut, peer_id: &PeerID) -> io::Result<()> {
        let route = self.route_table.get_route_by_id(peer_id)?;
        self.send_to(buf, &route.route_key()).await
    }
    /// Writing `buf` to the target named by `peer_id`
    pub async fn send_to_id_safe(
        &self,
        buf: BytesMut,
        src_id: &PeerID,
        peer_id: &PeerID,
    ) -> io::Result<()> {
        let route = self.route_table.get_route_by_id(peer_id)?;
        if self
            .route_table
            .is_route_of_peer_id(src_id, &route.route_key())
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "loop route"));
        }
        self.send_to(buf, &route.route_key()).await
    }
}

impl PipeLine {
    /// Receving buf from the associated PipeLine
    /// `usize` in the `Ok` branch indicates how many bytes are received
    /// `RouteKey` in the `Ok` branch denotes the source where these bytes are received from
    pub async fn recv_from(&mut self, buf: &mut [u8]) -> Option<io::Result<(usize, RouteKey)>> {
        match self {
            PipeLine::Udp(line) => line.recv_from(buf).await,
            PipeLine::Tcp(line) => Some(line.recv_from(buf).await),
            PipeLine::Extend(line) => Some(line.recv_from(buf).await),
        }
    }
    pub async fn recv_multi_from<B: AsMut<[u8]>>(
        &mut self,
        bufs: &mut [B],
        sizes: &mut [usize],
        addrs: &mut [RouteKey],
    ) -> Option<io::Result<usize>> {
        match self {
            PipeLine::Udp(line) => line.recv_multi_from(bufs, sizes, addrs).await,
            PipeLine::Tcp(line) => {
                if addrs.len() != bufs.len() {
                    return Some(Err(io::Error::new(io::ErrorKind::Other, "addrs error")));
                }
                match line.recv_multi_from(bufs, sizes).await {
                    Ok((n, route_key)) => {
                        addrs[..n].fill(route_key);
                        Some(Ok(n))
                    }
                    Err(e) => Some(Err(e)),
                }
            }
            PipeLine::Extend(line) => {
                let rs = line.recv_from(bufs[0].as_mut()).await;
                match rs {
                    Ok((len, addr)) => {
                        sizes[0] = len;
                        addrs[0] = addr;
                        Some(Ok(1))
                    }
                    Err(e) => Some(Err(e)),
                }
            }
        }
    }
    pub fn done(&mut self) {
        match self {
            PipeLine::Udp(line) => line.done(),
            PipeLine::Tcp(line) => line.done(),
            PipeLine::Extend(line) => line.done(),
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
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        match self {
            PipeLine::Udp(_) => None,
            PipeLine::Tcp(tcp) => Some(tcp.route_key().addr()),
            PipeLine::Extend(_) => None,
        }
    }
}
