use std::time::Duration;

use crate::endpoint::codec::InitCodec;

/// Load balance strategy for route selection.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Default)]
pub enum LoadBalance {
    #[default]
    MinHopLowestLatency,
    RoundRobin,
    MostRecent,
    LowestLatency,
}

/// Main configuration for creating an EndPoint.
pub struct Config {
    pub(crate) stun_servers: Vec<String>,
    pub(crate) udp_port: Option<u16>,
    pub(crate) tcp_port: Option<u16>,
    pub(crate) tcp_codec: Option<Box<dyn InitCodec>>,
    pub(crate) load_balance: LoadBalance,
    pub(crate) route_idle_timeout: Duration,
    pub(crate) max_assistant_sockets: usize,
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
            max_assistant_sockets: 0,
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

    pub fn max_assistant_sockets(mut self, max: usize) -> Self {
        self.max_assistant_sockets = max;
        self
    }
}
