use bytes::BytesMut;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use crate::tunnel::config::TunnelConfig;

use crate::punch::Puncher;
use crate::route::{ConnectProtocol, RouteKey};
use std::sync::Arc;

pub mod config;

pub mod recycle;
pub mod tcp;
pub mod udp;
pub const DEFAULT_ADDRESS_V4: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
pub const DEFAULT_ADDRESS_V6: SocketAddr =
    SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0));

/// Construct the needed components for p2p communication with the given tunnel configuration
pub fn new_tunnel_component(config: TunnelConfig) -> io::Result<(TunnelDispatcher, Puncher)> {
    let udp_tunnel_dispatcher = if let Some(mut udp_tunnel_config) = config.udp_tunnel_config {
        udp_tunnel_config.main_udp_count = config.major_socket_count;
        Some(udp::UdpTunnelDispatcher::new(udp_tunnel_config)?)
    } else {
        None
    };
    let tcp_tunnel_dispatcher = if let Some(mut tcp_tunnel_config) = config.tcp_tunnel_config {
        tcp_tunnel_config.tcp_multiplexing_limit = config.major_socket_count;
        Some(tcp::TcpTunnelDispatcher::new(tcp_tunnel_config)?)
    } else {
        None
    };

    let tunnel_dispatcher = TunnelDispatcher {
        udp_tunnel_dispatcher,
        tcp_tunnel_dispatcher,
    };
    let puncher = Puncher::from(&tunnel_dispatcher);
    Ok((tunnel_dispatcher, puncher))
}

pub struct TunnelDispatcher {
    udp_tunnel_dispatcher: Option<udp::UdpTunnelDispatcher>,
    tcp_tunnel_dispatcher: Option<tcp::TcpTunnelDispatcher>,
}

pub enum Tunnel {
    Udp(udp::UdpTunnel),
    Tcp(tcp::TcpTunnel),
}

#[derive(Clone)]
pub struct SocketManager {
    udp_socket_manager: Option<Arc<udp::UdpSocketManager>>,
    tcp_socket_manager: Option<Arc<tcp::TcpSocketManager>>,
}

impl TunnelDispatcher {
    /// Accept tunnels from a given `factory`
    pub async fn dispatch(&mut self) -> io::Result<Tunnel> {
        tokio::select! {
            rs=dispatch_udp_tunnel(self.udp_tunnel_dispatcher.as_mut())=>{
                rs
            }
            rs=accept_tcp(self.tcp_tunnel_dispatcher.as_mut())=>{
                rs
            }
        }
    }
    pub fn shared_udp_socket_manager(&self) -> Option<Arc<udp::UdpSocketManager>> {
        self.udp_tunnel_dispatcher
            .as_ref()
            .map(|v| v.socket_manager.clone())
    }
    pub fn shared_tcp_socket_manager(&self) -> Option<Arc<tcp::TcpSocketManager>> {
        self.tcp_tunnel_dispatcher
            .as_ref()
            .map(|v| v.socket_manager.clone())
    }
    pub fn socket_manager(&self) -> SocketManager {
        SocketManager {
            udp_socket_manager: self.shared_udp_socket_manager(),
            tcp_socket_manager: self.shared_tcp_socket_manager(),
        }
    }
    pub fn udp_socket_manager_as_ref(&self) -> Option<&Arc<udp::UdpSocketManager>> {
        self.udp_tunnel_dispatcher
            .as_ref()
            .map(|v| &v.socket_manager)
    }
    pub fn tcp_socket_manager_as_ref(&self) -> Option<&Arc<tcp::TcpSocketManager>> {
        self.tcp_tunnel_dispatcher
            .as_ref()
            .map(|v| &v.socket_manager)
    }
}

impl TunnelDispatcher {
    pub fn udp_tunnel_manager_as_mut(&mut self) -> Option<&mut udp::UdpTunnelDispatcher> {
        self.udp_tunnel_dispatcher.as_mut()
    }
    pub fn tcp_tunnel_manager_as_mut(&mut self) -> Option<&mut tcp::TcpTunnelDispatcher> {
        self.tcp_tunnel_dispatcher.as_mut()
    }
}

async fn accept_tcp(tcp: Option<&mut tcp::TcpTunnelDispatcher>) -> io::Result<Tunnel> {
    if let Some(tcp_tunnel_factory) = tcp {
        Ok(Tunnel::Tcp(tcp_tunnel_factory.dispatch().await?))
    } else {
        futures::future::pending().await
    }
}
async fn dispatch_udp_tunnel(
    udp_tunnel_factory: Option<&mut udp::UdpTunnelDispatcher>,
) -> io::Result<Tunnel> {
    if let Some(udp_tunnel_factory) = udp_tunnel_factory {
        Ok(Tunnel::Udp(udp_tunnel_factory.dispatch().await?))
    } else {
        futures::future::pending().await
    }
}

impl SocketManager {
    pub fn udp_socket_manager_as_ref(&self) -> Option<&Arc<udp::UdpSocketManager>> {
        self.udp_socket_manager.as_ref()
    }
    pub fn tcp_socket_manager_as_ref(&self) -> Option<&Arc<tcp::TcpSocketManager>> {
        self.tcp_socket_manager.as_ref()
    }
}

impl SocketManager {
    /// Writing `buf` to the target denoted by `route_key`
    pub async fn send_to(&self, buf: BytesMut, route_key: &RouteKey) -> io::Result<()> {
        match route_key.protocol() {
            ConnectProtocol::UDP => {
                if let Some(w) = self.udp_socket_manager.as_ref() {
                    return w.send_bytes_to(buf, route_key).await;
                }
            }
            ConnectProtocol::TCP => {
                if let Some(w) = self.tcp_socket_manager.as_ref() {
                    return w.send_to(buf, route_key).await;
                }
            }
        }
        Err(io::Error::from(io::ErrorKind::InvalidInput))
    }
    pub fn try_send_to(&self, buf: BytesMut, route_key: &RouteKey) -> io::Result<()> {
        match route_key.protocol() {
            ConnectProtocol::UDP => {
                if let Some(w) = self.udp_socket_manager.as_ref() {
                    return w.try_send_bytes_to(buf, route_key);
                }
            }
            ConnectProtocol::TCP => {
                if let Some(w) = self.tcp_socket_manager.as_ref() {
                    return w.try_send_to(buf, route_key);
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
                    return w.send_bytes_to(buf, addr).await;
                }
            }
            ConnectProtocol::TCP => {
                if let Some(w) = self.tcp_socket_manager.as_ref() {
                    return w.send_to_addr(buf, addr).await;
                }
            }
        }
        Err(io::Error::from(io::ErrorKind::InvalidInput))
    }
}
// impl<PeerID: Hash + Eq> UnifiedSocketManager<PeerID> {
//     /// Writing `buf` to the target named by `peer_id`
//     pub async fn send_to_id(&self, buf: BytesMut, peer_id: &PeerID) -> io::Result<()> {
//         let route = self.route_table.get_route_by_id(peer_id)?;
//         self.send_to(buf, &route.route_key()).await
//     }
//     /// Writing `buf` to the target named by `peer_id`
//     pub async fn avoid_loop_send_to_id(
//         &self,
//         buf: BytesMut,
//         src_id: &PeerID,
//         peer_id: &PeerID,
//     ) -> io::Result<()> {
//         let route = self.route_table.get_route_by_id(peer_id)?;
//         if self
//             .route_table
//             .is_route_of_peer_id(src_id, &route.route_key())
//         {
//             return Err(io::Error::new(io::ErrorKind::InvalidInput, "loop route"));
//         }
//         self.send_to(buf, &route.route_key()).await
//     }
// }

impl Tunnel {
    /// Receiving buf from the associated tunnel
    /// `usize` in the `Ok` branch indicates how many bytes are received
    /// `RouteKey` in the `Ok` branch denotes the source where these bytes are received from
    pub async fn recv_from(&mut self, buf: &mut [u8]) -> Option<io::Result<(usize, RouteKey)>> {
        match self {
            Tunnel::Udp(tunnel) => tunnel.recv_from(buf).await,
            Tunnel::Tcp(tunnel) => Some(tunnel.recv_from(buf).await),
        }
    }
    pub async fn batch_recv_from<B: AsMut<[u8]>>(
        &mut self,
        bufs: &mut [B],
        sizes: &mut [usize],
        addrs: &mut [RouteKey],
    ) -> Option<io::Result<usize>> {
        match self {
            Tunnel::Udp(tunnel) => tunnel.batch_recv_from(bufs, sizes, addrs).await,
            Tunnel::Tcp(tunnel) => {
                if addrs.len() != bufs.len() {
                    return Some(Err(io::Error::other("addrs error")));
                }
                match tunnel.batch_recv_from(bufs, sizes).await {
                    Ok((n, route_key)) => {
                        addrs[..n].fill(route_key);
                        Some(Ok(n))
                    }
                    Err(e) => Some(Err(e)),
                }
            }
        }
    }
    pub async fn send_to<A: Into<SocketAddr>>(&self, buf: BytesMut, addr: A) -> io::Result<()> {
        match self {
            Tunnel::Udp(tunnel) => tunnel.send_bytes_to(buf, addr).await,
            Tunnel::Tcp(tunnel) => tunnel.send(buf).await,
        }
    }
    pub fn done(&mut self) {
        match self {
            Tunnel::Udp(tunnel) => tunnel.done(),
            Tunnel::Tcp(tunnel) => tunnel.done(),
        }
    }
}

impl Tunnel {
    /// The protocol this tunnel is using
    pub fn protocol(&self) -> ConnectProtocol {
        match self {
            Tunnel::Udp(_) => ConnectProtocol::UDP,
            Tunnel::Tcp(_) => ConnectProtocol::TCP,
        }
    }
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        match self {
            Tunnel::Udp(_) => None,
            Tunnel::Tcp(tcp) => Some(tcp.route_key().addr()),
        }
    }
}
