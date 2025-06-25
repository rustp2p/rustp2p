use std::io;
use std::time::Duration;

use crate::socket::LocalInterface;
use crate::tunnel::recycle::RecycleBuf;
use crate::tunnel::tcp::{BytesInitCodec, InitCodec};
use crate::tunnel::udp::Model;

pub(crate) const MAX_SYMMETRIC_SOCKET_COUNT: usize = 200;
pub(crate) const MAX_MAIN_SOCKET_COUNT: usize = 10;
pub(crate) const ROUTE_IDLE_TIME: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Eq, PartialEq, Debug, Default)]
pub enum LoadBalance {
    /// Use the route with the lowest latency among those with the fewest hops.
    #[default]
    MinHopLowestLatency,
    /// Round-robin the route list.
    RoundRobin,
    /// Use the most recently added route.
    MostRecent,
    /// Use the route with the lowest latency.
    LowestLatency,
}
#[derive(Clone)]
pub struct TunnelConfig {
    pub major_socket_count: usize,
    pub udp_tunnel_config: Option<UdpTunnelConfig>,
    pub tcp_tunnel_config: Option<TcpTunnelConfig>,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            major_socket_count: MAX_MAJOR_SOCKET_COUNT,
            udp_tunnel_config: Some(Default::default()),
            tcp_tunnel_config: Some(Default::default()),
        }
    }
}

pub(crate) const MAX_MAJOR_SOCKET_COUNT: usize = 2;
pub(crate) const MAX_UDP_SUB_SOCKET_COUNT: usize = 82;

impl TunnelConfig {
    pub fn new(tcp_init_codec: Box<dyn InitCodec>) -> TunnelConfig {
        let udp_tunnel_config = Some(UdpTunnelConfig::default());
        let tcp_tunnel_config = Some(TcpTunnelConfig::new(tcp_init_codec));
        Self {
            major_socket_count: MAX_MAJOR_SOCKET_COUNT,
            udp_tunnel_config,
            tcp_tunnel_config,
        }
    }
}
impl TunnelConfig {
    pub fn none_tcp(self) -> Self {
        self
    }
}
impl TunnelConfig {
    pub fn empty() -> Self {
        Self {
            major_socket_count: MAX_MAJOR_SOCKET_COUNT,
            udp_tunnel_config: None,
            tcp_tunnel_config: None,
        }
    }

    pub fn set_tcp_multi_count(mut self, count: usize) -> Self {
        self.major_socket_count = count;
        self
    }

    pub fn set_udp_tunnel_config(mut self, config: UdpTunnelConfig) -> Self {
        self.udp_tunnel_config.replace(config);
        self
    }
    pub fn set_tcp_tunnel_config(mut self, config: TcpTunnelConfig) -> Self {
        self.tcp_tunnel_config.replace(config);
        self
    }
    pub fn check(&self) -> io::Result<()> {
        if let Some(udp_tunnel_config) = self.udp_tunnel_config.as_ref() {
            udp_tunnel_config.check()?;
        }
        if let Some(tcp_tunnel_config) = self.tcp_tunnel_config.as_ref() {
            tcp_tunnel_config.check()?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct TcpTunnelConfig {
    pub route_idle_time: Duration,
    pub tcp_multiplexing_limit: usize,
    pub default_interface: Option<LocalInterface>,
    pub tcp_port: u16,
    pub use_v6: bool,
    pub init_codec: Box<dyn InitCodec>,
    pub recycle_buf: Option<RecycleBuf>,
}

impl Default for TcpTunnelConfig {
    fn default() -> Self {
        Self {
            route_idle_time: ROUTE_IDLE_TIME,
            tcp_multiplexing_limit: MAX_MAJOR_SOCKET_COUNT,
            default_interface: None,
            tcp_port: 0,
            use_v6: true,
            init_codec: Box::new(BytesInitCodec),
            recycle_buf: None,
        }
    }
}

impl TcpTunnelConfig {
    pub fn new(init_codec: Box<dyn InitCodec>) -> TcpTunnelConfig {
        Self {
            route_idle_time: ROUTE_IDLE_TIME,
            tcp_multiplexing_limit: MAX_MAJOR_SOCKET_COUNT,
            default_interface: None,
            tcp_port: 0,
            use_v6: true,
            init_codec,
            recycle_buf: None,
        }
    }
    pub fn check(&self) -> io::Result<()> {
        if self.tcp_multiplexing_limit == 0 {
            return Err(io::Error::other("tcp_multiplexing_limit cannot be 0"));
        }
        if self.tcp_multiplexing_limit > MAX_MAIN_SOCKET_COUNT {
            return Err(io::Error::other("tcp_multiplexing_limit cannot too large"));
        }
        if self.use_v6 {
            socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::STREAM, None)?;
        }
        Ok(())
    }
    pub fn set_tcp_multiplexing_limit(mut self, tcp_multiplexing_limit: usize) -> Self {
        self.tcp_multiplexing_limit = tcp_multiplexing_limit;
        self
    }
    pub fn set_route_idle_time(mut self, route_idle_time: Duration) -> Self {
        self.route_idle_time = route_idle_time;
        self
    }
    pub fn set_default_interface(mut self, default_interface: LocalInterface) -> Self {
        self.default_interface = Some(default_interface.clone());
        self
    }
    pub fn set_tcp_port(mut self, tcp_port: u16) -> Self {
        self.tcp_port = tcp_port;
        self
    }
    pub fn set_use_v6(mut self, use_v6: bool) -> Self {
        self.use_v6 = use_v6;
        self
    }
}

#[derive(Clone)]
pub struct UdpTunnelConfig {
    pub main_udp_count: usize,
    pub sub_udp_count: usize,
    pub model: Model,
    pub default_interface: Option<LocalInterface>,
    pub udp_ports: Vec<u16>,
    pub use_v6: bool,
    pub recycle_buf: Option<RecycleBuf>,
}

impl Default for UdpTunnelConfig {
    fn default() -> Self {
        Self {
            main_udp_count: MAX_MAJOR_SOCKET_COUNT,
            sub_udp_count: MAX_UDP_SUB_SOCKET_COUNT,
            model: Model::Low,
            default_interface: None,
            udp_ports: vec![0, 0],
            use_v6: true,
            recycle_buf: None,
        }
    }
}

impl UdpTunnelConfig {
    pub fn check(&self) -> io::Result<()> {
        if self.main_udp_count == 0 {
            return Err(io::Error::other("main socket count cannot be 0"));
        }
        if self.main_udp_count > MAX_MAIN_SOCKET_COUNT {
            return Err(io::Error::other("main socket count is too large"));
        }
        if self.sub_udp_count > MAX_SYMMETRIC_SOCKET_COUNT {
            return Err(io::Error::other(
                "socket count for symmetric nat is too large",
            ));
        }
        if self.use_v6 {
            socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::DGRAM, None)?;
        }
        Ok(())
    }
    pub fn set_main_udp_count(mut self, count: usize) -> Self {
        self.main_udp_count = count;
        self
    }
    pub fn set_sub_udp_count(mut self, count: usize) -> Self {
        self.sub_udp_count = count;
        self
    }
    pub fn set_model(mut self, model: Model) -> Self {
        self.model = model;
        self
    }
    pub fn set_default_interface(mut self, default_interface: LocalInterface) -> Self {
        self.default_interface = Some(default_interface.clone());
        self
    }
    pub fn set_udp_ports(mut self, udp_ports: Vec<u16>) -> Self {
        self.udp_ports = udp_ports;
        self
    }
    pub fn set_simple_udp_port(mut self, udp_port: u16) -> Self {
        self.udp_ports = vec![udp_port];
        self
    }
    pub fn set_use_v6(mut self, use_v6: bool) -> Self {
        self.use_v6 = use_v6;
        self
    }
}
