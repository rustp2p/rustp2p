use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

use crate::endpoint::codec::{BytesInitCodec, InitCodec};
use crate::socket::LocalInterface;

/// Default IPv4 address (0.0.0.0:0).
pub const DEFAULT_ADDRESS_V4: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));

/// Default IPv6 address ([::]:0).
pub const DEFAULT_ADDRESS_V6: SocketAddr =
    SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0));

const MAX_SYMMETRIC_SOCKET_COUNT: usize = 200;
const MAX_MAIN_SOCKET_COUNT: usize = 10;
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// UDP socket management model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Model {
    /// Few sockets (Cone NAT).
    Low,
    /// Many sockets (Symmetric NAT).
    High,
}

impl Default for Model {
    fn default() -> Self {
        Model::Low
    }
}

/// Load balance strategy for route selection.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Default)]
pub enum LoadBalance {
    #[default]
    MinHopLowestLatency,
    RoundRobin,
    MostRecent,
    LowestLatency,
}

/// UDP socket configuration.
#[derive(Clone)]
pub struct UdpConfig {
    pub main_count: usize,
    pub assistant_count: usize,
    pub model: Model,
    pub default_interface: Option<LocalInterface>,
    pub udp_ports: Vec<u16>,
    pub use_v6: bool,
}

impl Default for UdpConfig {
    fn default() -> Self {
        Self {
            main_count: MAX_MAIN_SOCKET_COUNT,
            assistant_count: MAX_SYMMETRIC_SOCKET_COUNT,
            model: Model::Low,
            default_interface: None,
            udp_ports: vec![0, 0],
            use_v6: true,
        }
    }
}

impl UdpConfig {
    pub fn from_socket(socket: std::net::UdpSocket) -> Self {
        let port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
        Self {
            main_count: 1,
            assistant_count: MAX_SYMMETRIC_SOCKET_COUNT,
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
        if self.assistant_count > MAX_SYMMETRIC_SOCKET_COUNT {
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
    pub fn assistant_count(mut self, count: usize) -> Self {
        self.assistant_count = count;
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

/// TCP connection configuration.
#[derive(Clone)]
pub struct TcpConfig {
    pub idle_timeout: Duration,
    pub multiplex_limit: usize,
    pub default_interface: Option<LocalInterface>,
    pub tcp_port: u16,
    pub use_v6: bool,
    pub codec: Box<dyn InitCodec>,
}

impl Default for TcpConfig {
    fn default() -> Self {
        Self {
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            multiplex_limit: MAX_MAIN_SOCKET_COUNT,
            default_interface: None,
            tcp_port: 0,
            use_v6: true,
            codec: Box::new(BytesInitCodec),
        }
    }
}

impl TcpConfig {
    pub fn new(codec: Box<dyn InitCodec>) -> Self {
        Self {
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            multiplex_limit: MAX_MAIN_SOCKET_COUNT,
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

/// Top-level configuration combining UDP and TCP settings.
pub struct TunnelConfig {
    pub major_socket_count: usize,
    pub udp_config: Option<UdpConfig>,
    pub tcp_config: Option<TcpConfig>,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            major_socket_count: MAX_MAIN_SOCKET_COUNT,
            udp_config: Some(Default::default()),
            tcp_config: Some(Default::default()),
        }
    }
}

impl TunnelConfig {
    pub fn simple(udp_port: u16, tcp_port: u16) -> Self {
        Self {
            major_socket_count: MAX_MAIN_SOCKET_COUNT,
            udp_config: Some(UdpConfig::default().simple_udp_port(udp_port)),
            tcp_config: Some(TcpConfig::default().tcp_port(tcp_port)),
        }
    }

    pub fn udp_only(port: u16) -> Self {
        Self {
            major_socket_count: MAX_MAIN_SOCKET_COUNT,
            udp_config: Some(UdpConfig::default().simple_udp_port(port)),
            tcp_config: None,
        }
    }

    pub fn tcp_only(port: u16) -> Self {
        Self {
            major_socket_count: MAX_MAIN_SOCKET_COUNT,
            udp_config: None,
            tcp_config: Some(TcpConfig::default().tcp_port(port)),
        }
    }

    pub fn udp_socket(socket: std::net::UdpSocket) -> Self {
        Self {
            major_socket_count: 1,
            udp_config: Some(UdpConfig::from_socket(socket)),
            tcp_config: None,
        }
    }

    pub fn new(codec: Box<dyn InitCodec>) -> Self {
        Self {
            major_socket_count: MAX_MAIN_SOCKET_COUNT,
            udp_config: Some(UdpConfig::default()),
            tcp_config: Some(TcpConfig::new(codec)),
        }
    }

    pub fn major_socket_count(mut self, count: usize) -> Self {
        self.major_socket_count = count;
        self
    }
    pub fn udp_config(mut self, config: UdpConfig) -> Self {
        self.udp_config = Some(config);
        self
    }
    pub fn tcp_config(mut self, config: TcpConfig) -> Self {
        self.tcp_config = Some(config);
        self
    }
}

/// Main configuration for creating an EndPoint.
pub struct Config {
    pub(crate) stun_servers: Vec<String>,
    pub(crate) udp_port: Option<u16>,
    pub(crate) tcp_port: Option<u16>,
    pub(crate) tcp_codec: Option<Box<dyn InitCodec>>,
    pub(crate) load_balance: LoadBalance,
    pub(crate) route_idle_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            stun_servers: vec![
                "stun.l.google.com:19302".to_string(),
                "stun1.l.google.com:19302".to_string(),
            ],
            udp_port: Some(0),
            tcp_port: Some(0),
            tcp_codec: None,
            load_balance: LoadBalance::MinHopLowestLatency,
            route_idle_timeout: Duration::from_secs(12),
        }
    }
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn udp(port: u16) -> Self {
        Self {
            udp_port: Some(port),
            tcp_port: None,
            ..Default::default()
        }
    }
    pub fn tcp(port: u16) -> Self {
        Self {
            udp_port: None,
            tcp_port: Some(port),
            ..Default::default()
        }
    }
    pub fn udp_port(mut self, port: u16) -> Self {
        self.udp_port = Some(port);
        self
    }
    pub fn tcp_port(mut self, port: u16) -> Self {
        self.tcp_port = Some(port);
        self
    }
    pub fn tcp_codec(mut self, codec: Box<dyn InitCodec>) -> Self {
        self.tcp_codec = Some(codec);
        self
    }
    pub fn stun_servers(mut self, servers: Vec<String>) -> Self {
        self.stun_servers = servers;
        self
    }
    pub fn load_balance(mut self, lb: LoadBalance) -> Self {
        self.load_balance = lb;
        self
    }
    pub fn route_idle_timeout(mut self, timeout: Duration) -> Self {
        self.route_idle_timeout = timeout;
        self
    }
}
