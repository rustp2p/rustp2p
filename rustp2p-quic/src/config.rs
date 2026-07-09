use crate::{CertificateVerifier, Identity, PeerId, SkipCertificateVerification};
use rustp2p_core::endpoint::LoadBalance;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the QUIC endpoint.
///
/// Most users should prefer [`Endpoint::builder`](crate::Endpoint::builder).
/// This struct remains available for callers that need to construct or store a
/// complete endpoint configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// Local core transport address to bind to.
    pub bind_addr: SocketAddr,
    /// STUN servers used for NAT type and port-range estimation.
    pub stun_servers: Vec<String>,
    /// Interval used by NAT maintenance tasks.
    pub stun_timeout: Duration,
    /// QUIC ALPN protocols to accept.
    pub alpns: Vec<Vec<u8>>,
    /// QUIC application datagram receive buffer size. `None` disables QUIC datagrams.
    pub datagram_receive_buffer_size: Option<usize>,
    /// QUIC application datagram send buffer size.
    pub datagram_send_buffer_size: usize,
    /// Local high-level P2P identity. If absent, a random identity is generated.
    pub identity: Option<Identity>,
    /// Initial directly reachable bootstrap addresses.
    ///
    /// These addresses are entry points only; discovered peers are later
    /// addressed by `PeerId`.
    pub bootstrap: Vec<SocketAddr>,
    /// Whether to enable the underlying core TCP transport.
    pub enable_tcp: bool,
    /// Optional TCP listen port. `None` reuses the UDP bind port value.
    pub tcp_port: Option<u16>,
    /// Externally mapped UDP addresses to advertise and use for punching.
    pub mapping_udp_addrs: Vec<SocketAddr>,
    /// Externally mapped TCP addresses to advertise and use for punching.
    pub mapping_tcp_addrs: Vec<SocketAddr>,
    /// Route selection strategy for multiple available routes.
    pub load_balance: LoadBalance,
    /// Initial peers allowed to perform direct hole punching.
    pub punch_whitelist: Vec<PeerId>,
    /// Direct peers allowed to act as public address observers. Empty means any direct peer.
    pub nat_observers: Vec<PeerId>,
    /// Maximum assistant UDP sockets enabled when the detected local NAT is symmetric.
    pub max_assistant_sockets: usize,
    /// QUIC certificate verifier used for both server and client certificates.
    pub certificate_verifier: Arc<dyn CertificateVerifier>,
    /// Maximum forwarding TTL for high-level packets.
    pub max_ttl: u8,
    /// Whether to start high-level P2P dispatch/maintenance tasks.
    ///
    /// `Endpoint::builder` enables this automatically.
    pub high_level: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:0".parse().unwrap(),
            stun_servers: Vec::new(),
            stun_timeout: Duration::from_secs(10),
            alpns: vec![b"rustp2p-quic".to_vec()],
            datagram_receive_buffer_size: Some(1024 * 1024),
            datagram_send_buffer_size: 1024 * 1024,
            identity: None,
            bootstrap: Vec::new(),
            enable_tcp: true,
            tcp_port: None,
            mapping_udp_addrs: Vec::new(),
            mapping_tcp_addrs: Vec::new(),
            load_balance: LoadBalance::MinHopLowestLatency,
            punch_whitelist: Vec::new(),
            nat_observers: Vec::new(),
            max_assistant_sockets: 0,
            certificate_verifier: Arc::new(SkipCertificateVerification),
            max_ttl: 8,
            high_level: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn default_config_has_no_stun_servers() {
        assert!(Config::default().stun_servers.is_empty());
    }
}
