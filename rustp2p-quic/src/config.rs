use std::net::SocketAddr;
use std::time::Duration;

/// Configuration for the QUIC endpoint.
#[derive(Clone, Debug)]
pub struct Config {
    /// Local address to bind to.
    pub bind_addr: SocketAddr,
    /// STUN servers for NAT detection.
    pub stun_servers: Vec<String>,
    /// Timeout for NAT detection.
    pub stun_timeout: Duration,
    /// ALPN protocols to accept.
    pub alpns: Vec<Vec<u8>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:0".parse().unwrap(),
            stun_servers: vec![
                "stun.l.google.com:19302".to_string(),
                "stun1.l.google.com:19302".to_string(),
                "stun2.l.google.com:19302".to_string(),
            ],
            stun_timeout: Duration::from_secs(10),
            alpns: vec![b"rustp2p-quic".to_vec()],
        }
    }
}
