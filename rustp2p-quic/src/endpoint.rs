use crate::config::Config;
use crate::connection::Connection;
use crate::demux::{ReceivedPacket, SharedUdpSocket};
use rust_p2p_core::nat::NatInfo;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// A peer node address containing node ID and direct addresses.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NodeAddr {
    /// The peer's node ID (32 bytes).
    pub node_id: [u8; 32],
    /// Direct socket addresses to reach the peer.
    pub direct_addresses: Vec<SocketAddr>,
}

impl NodeAddr {
    pub fn new(node_id: [u8; 32], direct_addresses: Vec<SocketAddr>) -> Self {
        Self {
            node_id,
            direct_addresses,
        }
    }

    pub fn from_id(node_id: [u8; 32]) -> Self {
        Self {
            node_id,
            direct_addresses: Vec::new(),
        }
    }
}

/// The main QUIC endpoint for establishing P2P connections.
pub struct Endpoint {
    quinn_endpoint: quinn::Endpoint,
    node_id: [u8; 32],
    stun_servers: Vec<String>,
    nat_info: Arc<parking_lot::RwLock<NatInfo>>,
    shared_socket: Arc<SharedUdpSocket>,
}

impl Endpoint {
    /// Binds a new endpoint on the given address.
    pub async fn bind(bind_addr: SocketAddr) -> crate::Result<Self> {
        Self::bind_with_config(Config {
            bind_addr,
            ..Default::default()
        })
        .await
    }

    /// Binds a new endpoint with custom configuration.
    pub async fn bind_with_config(config: Config) -> crate::Result<Self> {
        use rcgen::KeyPair;
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

        // Generate self-signed certificate
        let key_pair = KeyPair::generate().map_err(|e| std::io::Error::other(e.to_string()))?;
        let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let cert_der = cert.der().clone();
        let key_der = PrivatePkcs8KeyDer::from(key_pair.serialize_der());

        // Derive node ID from certificate
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&cert_der);
        let hash = hasher.finalize();
        let mut node_id = [0u8; 32];
        node_id.copy_from_slice(&hash);

        // Create TLS server config
        let key_provider = rustls::crypto::ring::default_provider();
        let mut server_crypto =
            rustls::ServerConfig::builder_with_provider(Arc::new(key_provider.clone()))
                .with_safe_default_protocol_versions()
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert_der.clone()],
                    PrivateKeyDer::Pkcs8(key_der.clone_key()),
                )
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        server_crypto.alpn_protocols = config.alpns.clone();

        let quic_server_config =
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
                .map_err(|e| std::io::Error::other(format!("quic server config: {e}")))?;

        // Create TLS client config
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(cert_der)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let client_crypto = rustls::ClientConfig::builder_with_provider(Arc::new(key_provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();

        let quic_client_config =
            quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)
                .map_err(|e| std::io::Error::other(format!("quic client config: {e}")))?;
        let _client_config = quinn::ClientConfig::new(Arc::new(quic_client_config));

        // Create the shared UDP socket
        let std_socket = std::net::UdpSocket::bind(config.bind_addr)?;
        std_socket.set_nonblocking(true)?;
        let tokio_socket = Arc::new(UdpSocket::from_std(std_socket)?);

        // Create channels for non-QUIC packets
        let (non_quic_tx, non_quic_rx) = mpsc::channel(256);

        // Create the shared socket with demuxing
        let shared_socket = SharedUdpSocket::new(tokio_socket.clone(), non_quic_tx);

        // Create quinn endpoint using our shared socket
        let endpoint_config = quinn::EndpointConfig::default();
        let server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_server_config));
        let quinn_endpoint = quinn::Endpoint::new_with_abstract_socket(
            endpoint_config,
            Some(server_config),
            shared_socket.clone(),
            Arc::new(quinn::TokioRuntime),
        )?;

        let nat_info = NatInfo::default();
        let endpoint = Self {
            quinn_endpoint,
            node_id,
            stun_servers: config.stun_servers,
            nat_info: Arc::new(parking_lot::RwLock::new(nat_info)),
            shared_socket,
        };

        // Start background NAT detection
        endpoint.start_nat_detection(config.stun_timeout);

        // Start non-QUIC packet handler
        endpoint.start_non_quic_handler(non_quic_rx);

        Ok(endpoint)
    }

    pub fn node_id(&self) -> [u8; 32] {
        self.node_id
    }

    pub fn addr(&self) -> Option<SocketAddr> {
        self.quinn_endpoint.local_addr().ok()
    }

    pub fn nat_info(&self) -> NatInfo {
        self.nat_info.read().clone()
    }

    /// Connects to a remote peer.
    pub async fn connect(&self, node_addr: NodeAddr) -> crate::Result<Connection> {
        let mut last_err = None;
        for addr in &node_addr.direct_addresses {
            match self.quinn_endpoint.connect(*addr, "localhost") {
                Ok(connecting) => match connecting.await {
                    Ok(conn) => return Ok(Connection::new(conn)),
                    Err(e) => {
                        log::debug!("Handshake failed with {addr}: {e}");
                        last_err = Some(std::io::Error::other(format!("handshake: {e}")));
                    }
                },
                Err(e) => {
                    log::debug!("Connect failed to {addr}: {e}");
                    last_err = Some(std::io::Error::other(format!("connect: {e}")));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| std::io::Error::other("no addresses to connect to")))
    }

    /// Accepts an incoming QUIC connection.
    pub async fn accept(&self) -> Option<Connection> {
        let incoming = self.quinn_endpoint.accept().await?;
        match incoming.await {
            Ok(conn) => Some(Connection::new(conn)),
            Err(e) => {
                log::warn!("Failed to accept connection: {e}");
                None
            }
        }
    }

    /// Returns a channel for incoming connections.
    pub fn accept_channel(&self) -> mpsc::Receiver<Connection> {
        let (tx, rx) = mpsc::channel(64);
        let endpoint = self.clone();
        tokio::spawn(async move {
            while let Some(conn) = endpoint.accept().await {
                if tx.send(conn).await.is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// Send raw UDP data through the shared socket (for STUN, punch, etc.)
    pub async fn send_raw(&self, buf: &[u8], addr: SocketAddr) -> crate::Result<()> {
        self.shared_socket.send_raw(buf, addr).await
    }

    /// Try to send raw UDP data (non-blocking)
    pub fn try_send_raw(&self, buf: &[u8], addr: SocketAddr) -> crate::Result<()> {
        self.shared_socket.try_send_raw(buf, addr)
    }

    pub async fn close(&self) {
        self.quinn_endpoint.close(0u32.into(), b"shutdown");
    }

    pub fn quinn(&self) -> &quinn::Endpoint {
        &self.quinn_endpoint
    }

    /// Starts background NAT detection using STUN.
    fn start_nat_detection(&self, timeout: std::time::Duration) {
        let stun_servers = self.stun_servers.clone();
        let nat_info = self.nat_info.clone();
        let _shared_socket = self.shared_socket.clone();

        tokio::spawn(async move {
            loop {
                // Use a temporary UDP socket for STUN queries
                // (STUN responses come back on our shared socket)
                match rust_p2p_core::stun::stun_test_nat(stun_servers.clone(), None).await {
                    Ok((nat_type, ips, port_range)) => {
                        let mut info = nat_info.write();
                        info.nat_type = nat_type;
                        info.public_ips = ips;
                        info.public_port_range = port_range;
                        log::info!("NAT: type={nat_type:?}, ips={:?}", info.public_ips);
                    }
                    Err(e) => {
                        log::debug!("NAT detection failed: {e}");
                    }
                }
                tokio::time::sleep(timeout).await;
            }
        });
    }

    /// Handles non-QUIC packets (STUN, punch, custom protocols).
    fn start_non_quic_handler(&self, mut non_quic_rx: mpsc::Receiver<ReceivedPacket>) {
        tokio::spawn(async move {
            while let Some(packet) = non_quic_rx.recv().await {
                match packet.packet_type {
                    crate::demux::PacketType::Stun => {
                        log::debug!(
                            "STUN packet from {}: {} bytes",
                            packet.addr,
                            packet.data.len()
                        );
                        // TODO: Forward to rustp2p-core STUN handler
                    }
                    crate::demux::PacketType::Punch => {
                        log::debug!(
                            "Punch packet from {}: {} bytes",
                            packet.addr,
                            packet.data.len()
                        );
                        // TODO: Forward to punch handler
                    }
                    crate::demux::PacketType::Other(protocol) => {
                        log::debug!(
                            "Protocol 0x{protocol:02x} from {}: {} bytes",
                            packet.addr,
                            packet.data.len()
                        );
                        // TODO: Forward to custom protocol handler
                    }
                    crate::demux::PacketType::Quic => unreachable!(),
                }
            }
        });
    }
}

impl Clone for Endpoint {
    fn clone(&self) -> Self {
        Self {
            quinn_endpoint: self.quinn_endpoint.clone(),
            node_id: self.node_id,
            stun_servers: self.stun_servers.clone(),
            nat_info: self.nat_info.clone(),
            shared_socket: self.shared_socket.clone(),
        }
    }
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("node_id", &hex::encode(self.node_id))
            .field("addr", &self.addr())
            .finish()
    }
}
