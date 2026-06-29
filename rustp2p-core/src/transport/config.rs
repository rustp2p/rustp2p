use std::io;
use std::time::Duration;

use crate::socket::LocalInterface;
use crate::transport::tcp::{BytesInitCodec, InitCodec};
use crate::transport::udp::Model;

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
    /// Creates a config with both UDP and TCP on the given ports.
    ///
    /// This is the simplest way to create a working configuration.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rust_p2p_core::tunnel::config::TunnelConfig;
    /// let config = TunnelConfig::simple(3000, 3000);
    /// ```
    pub fn simple(udp_port: u16, tcp_port: u16) -> Self {
        Self {
            major_socket_count: MAX_MAJOR_SOCKET_COUNT,
            udp_tunnel_config: Some(UdpTunnelConfig::default().simple_udp_port(udp_port)),
            tcp_tunnel_config: Some(TcpTunnelConfig::default().tcp_port(tcp_port)),
        }
    }

    /// Creates a config with only UDP on the given port.
    pub fn udp_only(port: u16) -> Self {
        Self {
            major_socket_count: MAX_MAJOR_SOCKET_COUNT,
            udp_tunnel_config: Some(UdpTunnelConfig::default().simple_udp_port(port)),
            tcp_tunnel_config: None,
        }
    }

    /// Creates a config with only TCP on the given port.
    pub fn tcp_only(port: u16) -> Self {
        Self {
            major_socket_count: MAX_MAJOR_SOCKET_COUNT,
            udp_tunnel_config: None,
            tcp_tunnel_config: Some(TcpTunnelConfig::default().tcp_port(port)),
        }
    }

    /// Creates a config with a user-provided UDP socket.
    ///
    /// The socket will be used as the primary UDP channel.
    /// Additional sockets may be created by Puncher for symmetric NAT.
    pub fn udp_socket(socket: std::net::UdpSocket) -> Self {
        Self {
            major_socket_count: 1,
            udp_tunnel_config: Some(UdpTunnelConfig::from_socket(socket)),
            tcp_tunnel_config: None,
        }
    }

    /// Creates a config using the given codec for TCP.
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
    pub fn empty() -> Self {
        Self {
            major_socket_count: MAX_MAJOR_SOCKET_COUNT,
            udp_tunnel_config: None,
            tcp_tunnel_config: None,
        }
    }

    pub fn major_socket_count(mut self, count: usize) -> Self {
        self.major_socket_count = count;
        self
    }

    pub fn udp_tunnel_config(mut self, config: UdpTunnelConfig) -> Self {
        self.udp_tunnel_config.replace(config);
        self
    }
    pub fn tcp_tunnel_config(mut self, config: TcpTunnelConfig) -> Self {
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
    pub idle_timeout: Duration,
    pub multiplex_limit: usize,
    pub default_interface: Option<LocalInterface>,
    pub tcp_port: u16,
    pub use_v6: bool,
    pub codec: Box<dyn InitCodec>,
}

impl Default for TcpTunnelConfig {
    fn default() -> Self {
        Self {
            idle_timeout: ROUTE_IDLE_TIME,
            multiplex_limit: MAX_MAJOR_SOCKET_COUNT,
            default_interface: None,
            tcp_port: 0,
            use_v6: true,
            codec: Box::new(BytesInitCodec),
        }
    }
}

impl TcpTunnelConfig {
    pub fn new(codec: Box<dyn InitCodec>) -> TcpTunnelConfig {
        Self {
            idle_timeout: ROUTE_IDLE_TIME,
            multiplex_limit: MAX_MAJOR_SOCKET_COUNT,
            default_interface: None,
            tcp_port: 0,
            use_v6: true,
            codec,
        }
    }
    pub fn check(&self) -> io::Result<()> {
        if self.multiplex_limit == 0 {
            return Err(io::Error::other("multiplex_limit cannot be 0"));
        }
        if self.multiplex_limit > MAX_MAIN_SOCKET_COUNT {
            return Err(io::Error::other("multiplex_limit cannot be too large"));
        }
        if self.use_v6 {
            socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::STREAM, None)?;
        }
        Ok(())
    }
    pub fn multiplex_limit(mut self, limit: usize) -> Self {
        self.multiplex_limit = limit;
        self
    }
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }
    pub fn default_interface(mut self, default_interface: LocalInterface) -> Self {
        self.default_interface = Some(default_interface);
        self
    }
    pub fn tcp_port(mut self, tcp_port: u16) -> Self {
        self.tcp_port = tcp_port;
        self
    }
    pub fn use_v6(mut self, use_v6: bool) -> Self {
        self.use_v6 = use_v6;
        self
    }
}

#[derive(Clone)]
pub struct UdpTunnelConfig {
    pub main_count: usize,
    pub sub_count: usize,
    pub model: Model,
    pub default_interface: Option<LocalInterface>,
    pub udp_ports: Vec<u16>,
    pub use_v6: bool,
}

impl Default for UdpTunnelConfig {
    fn default() -> Self {
        Self {
            main_count: MAX_MAJOR_SOCKET_COUNT,
            sub_count: MAX_UDP_SUB_SOCKET_COUNT,
            model: Model::Low,
            default_interface: None,
            udp_ports: vec![0, 0],
            use_v6: true,
        }
    }
}

impl UdpTunnelConfig {
    /// Creates a config from a user-provided socket.
    ///
    /// The socket is used as the primary UDP channel.
    /// `sub_count` controls how many additional sockets Puncher may create.
    pub fn from_socket(socket: std::net::UdpSocket) -> Self {
        let port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
        Self {
            main_count: 1,
            sub_count: MAX_UDP_SUB_SOCKET_COUNT,
            model: Model::Low,
            default_interface: None,
            udp_ports: vec![port],
            use_v6: false,
        }
    }

    pub fn check(&self) -> io::Result<()> {
        if self.main_count == 0 {
            return Err(io::Error::other("main socket count cannot be 0"));
        }
        if self.main_count > MAX_MAIN_SOCKET_COUNT {
            return Err(io::Error::other("main socket count is too large"));
        }
        if self.sub_count > MAX_SYMMETRIC_SOCKET_COUNT {
            return Err(io::Error::other(
                "socket count for symmetric nat is too large",
            ));
        }
        if self.use_v6 {
            socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::DGRAM, None)?;
        }
        Ok(())
    }
    pub fn main_count(mut self, count: usize) -> Self {
        self.main_count = count;
        self
    }
    pub fn sub_count(mut self, count: usize) -> Self {
        self.sub_count = count;
        self
    }
    pub fn model(mut self, model: Model) -> Self {
        self.model = model;
        self
    }
    pub fn default_interface(mut self, default_interface: LocalInterface) -> Self {
        self.default_interface = Some(default_interface);
        self
    }
    pub fn udp_ports(mut self, udp_ports: Vec<u16>) -> Self {
        self.udp_ports = udp_ports;
        self
    }
    pub fn simple_udp_port(mut self, udp_port: u16) -> Self {
        self.udp_ports = vec![udp_port];
        self
    }
    pub fn use_v6(mut self, use_v6: bool) -> Self {
        self.use_v6 = use_v6;
        self
    }
}
