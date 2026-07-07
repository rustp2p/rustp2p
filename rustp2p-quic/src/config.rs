use crate::{CertificateVerifier, Identity, SkipCertificateVerification};
use std::net::SocketAddr;
use std::sync::Arc;
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
    /// QUIC application datagram receive buffer size. `None` disables QUIC datagrams.
    pub datagram_receive_buffer_size: Option<usize>,
    /// QUIC application datagram send buffer size.
    pub datagram_send_buffer_size: usize,
    /// Local high-level P2P identity.
    pub identity: Option<Identity>,
    /// Initial directly reachable bootstrap addresses.
    pub bootstrap: Vec<SocketAddr>,
    /// QUIC server certificate verifier.
    pub certificate_verifier: Arc<dyn CertificateVerifier>,
    /// Maximum forwarding TTL for high-level packets.
    pub max_ttl: u8,
    /// Whether to start high-level P2P dispatch/maintenance tasks.
    pub high_level: bool,
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
            datagram_receive_buffer_size: Some(1024 * 1024),
            datagram_send_buffer_size: 1024 * 1024,
            identity: None,
            bootstrap: Vec::new(),
            certificate_verifier: Arc::new(SkipCertificateVerification),
            max_ttl: 8,
            high_level: false,
        }
    }
}
