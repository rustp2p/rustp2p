use crate::endpoint::config::Config;
use crate::endpoint::pool::SocketPool;
use crate::endpoint::transport::Transport;
use bytes::Bytes;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;

/// A received message with data and source transport.
pub struct Received {
    /// The received data (already framed for TCP).
    pub data: Bytes,
    /// The source transport (can be used to send back).
    pub transport: Transport,
}

/// The main P2P endpoint for sending and receiving data.
///
/// # Examples
///
/// ```rust,no_run
/// use rust_p2p_core::endpoint::{EndPoint, Config};
///
/// # #[tokio::main]
/// # async fn main() -> std::io::Result<()> {
/// let mut ep = EndPoint::bind(Config::new().udp_port(3000)).await?;
/// println!("Listening on: {:?}", ep.local_addr().await);
///
/// while let Some(received) = ep.recv().await {
///     println!("From {}: {:?}", received.transport.remote_addr(), received.data);
///     received.transport.send(b"echo").await?;
/// }
/// # Ok(())
/// # }
/// ```
pub struct EndPoint {
    pool: Arc<SocketPool>,
    data_rx: mpsc::Receiver<(Transport, Bytes)>,
    config: Config,
}

impl EndPoint {
    /// Binds an endpoint with the given configuration.
    pub async fn bind(mut config: Config) -> io::Result<Self> {
        let codec: Box<dyn crate::endpoint::codec::InitCodec> = config
            .tcp_codec
            .take()
            .unwrap_or_else(|| Box::new(crate::endpoint::codec::LengthPrefixedInitCodec));

        let mut pool_opt = None;
        let mut data_rx_opt = None;

        if let Some(port) = config.udp_port {
            let addr = format!("0.0.0.0:{port}");
            let socket = UdpSocket::bind(&addr).await?;
            let (pool, data_rx) = SocketPool::new(socket, codec.clone());
            pool_opt = Some(Arc::new(pool));
            data_rx_opt = Some(data_rx);
        }

        let tcp_listener = if let Some(port) = config.tcp_port {
            let addr = format!("0.0.0.0:{port}");
            Some(TcpListener::bind(&addr).await?)
        } else {
            None
        };

        let (pool, data_rx) = match pool_opt {
            Some(p) => (p, data_rx_opt.unwrap()),
            None => {
                let socket = UdpSocket::bind("0.0.0.0:0").await?;
                let (pool, data_rx) = SocketPool::new(socket, codec.clone());
                (Arc::new(pool), data_rx)
            }
        };

        let ep = Self {
            pool,
            data_rx,
            config,
        };

        // Start TCP accept loop
        if let Some(listener) = tcp_listener {
            let pool = ep.pool.clone();
            let mut shutdown_rx = pool.shutdown_rx();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        result = listener.accept() => {
                            match result {
                                Ok((stream, peer_addr)) => {
                                    log::debug!("TCP connection from {peer_addr}");
                                    if let Err(e) = pool.add_tcp(stream, peer_addr).await {
                                        log::warn!("TCP setup error: {e}");
                                    }
                                }
                                Err(e) => {
                                    log::warn!("TCP accept error: {e}");
                                }
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            log::debug!("TCP accept loop shutting down");
                            break;
                        }
                    }
                }
            });
        }

        Ok(ep)
    }

    /// Creates an endpoint from an existing UDP socket.
    pub async fn from_socket(socket: UdpSocket) -> io::Result<Self> {
        let codec: Box<dyn crate::endpoint::codec::InitCodec> =
            Box::new(crate::endpoint::codec::LengthPrefixedInitCodec);
        let (pool, data_rx) = SocketPool::new(socket, codec);
        Ok(Self {
            pool: Arc::new(pool),
            data_rx,
            config: Config::default(),
        })
    }

    /// Receives the next message from any peer.
    pub async fn recv(&mut self) -> Option<Received> {
        let (transport, data) = self.data_rx.recv().await?;
        Some(Received { data, transport })
    }

    /// Returns the local address this endpoint is bound to.
    pub async fn local_addr(&self) -> io::Result<SocketAddr> {
        self.pool.local_addr().await
    }

    /// Returns the configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns a Sender handle for sending data and querying socket state.
    ///
    /// `Sender` is lightweight and cloneable. It provides send methods
    /// and read-only query methods without exposing internal socket management.
    pub fn sender(&self) -> super::pool::Sender {
        super::pool::Sender(self.pool.clone())
    }

    /// Returns a Puncher for NAT hole-punching.
    ///
    /// The Puncher is constructed using the endpoint's internal socket pool.
    pub fn puncher(&self) -> crate::punch::Puncher {
        crate::punch::Puncher::new(self.pool.clone())
    }

    /// Get local UDP ports.
    pub async fn local_udp_ports(&self) -> Vec<u16> {
        self.pool
            .udp_sockets()
            .await
            .iter()
            .map(|s| s.local_addr().unwrap().port())
            .collect()
    }

    /// Get local TCP port from config.
    pub fn local_tcp_port(&self) -> u16 {
        self.config.tcp_port.unwrap_or(0)
    }

    /// Get NAT information using configured STUN servers.
    ///
    /// Uses the stun servers from Config to detect NAT type and public addresses.
    pub async fn nat_info(&self) -> io::Result<crate::nat::NatInfo> {
        let stun_servers = self.config.stun_servers.clone();
        let default_interface = self.config.default_interface.as_ref();

        let stun_result = crate::stun::stun_test_nat(stun_servers, default_interface).await?;

        log::debug!(
            "nat_type:{:?},public_ipv4:{:?},public_ipv6:{:?},public_udp_ports:{:?},port_range:{}",
            stun_result.nat_type,
            stun_result.public_ipv4,
            stun_result.public_ipv6,
            stun_result.public_udp_ports,
            stun_result.port_range
        );

        let local_ipv4 = crate::util::addr::local_ipv4()
            .await
            .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED);

        let local_udp_ports = self.local_udp_ports().await;
        let local_tcp_port = self.local_tcp_port();

        let mut public_udp_ports = local_udp_ports.clone();
        public_udp_ports.fill(0);

        Ok(crate::nat::NatInfo {
            nat_type: stun_result.nat_type,
            public_ips: stun_result.public_ipv4,
            public_udp_ports,
            mapping_tcp_addr: self.config.mapping_tcp_addr.clone(),
            mapping_udp_addr: self.config.mapping_udp_addr.clone(),
            public_port_range: stun_result.port_range,
            local_ipv4,
            local_ipv4s: vec![],
            ipv6: None,
            local_udp_ports,
            local_tcp_port,
            public_tcp_port: 0,
        })
    }

    /// Apply NAT model based on detected NAT type.
    ///
    /// - Symmetric NAT: Add assistant sockets up to configured limit
    /// - Cone NAT: Clean all assistant sockets
    pub async fn apply_nat_model(&self) -> io::Result<crate::nat::NatInfo> {
        let nat_info = self.nat_info().await?;

        match nat_info.nat_type {
            crate::nat::NatType::Symmetric => {
                let current = self.pool.assistant_count().await;
                let target = self.config.max_assistant_sockets;
                if target > current {
                    log::info!(
                        "Symmetric NAT detected, adding {} assistant sockets",
                        target - current
                    );
                    for _ in current..target {
                        let socket = crate::socket::bind_udp("0.0.0.0:0".parse().unwrap(), None)?;
                        let std_socket: std::net::UdpSocket = socket.into();
                        let tokio_socket = tokio::net::UdpSocket::from_std(std_socket)?;
                        self.pool.add_assistant_udp(tokio_socket).await;
                    }
                }
            }
            crate::nat::NatType::Cone => {
                let count = self.pool.assistant_count().await;
                if count > 0 {
                    log::info!("Cone NAT detected, cleaning {} assistant sockets", count);
                    self.pool.clean_assistant_udp();
                }
            }
        }

        Ok(nat_info)
    }
}

impl std::fmt::Debug for EndPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EndPoint").finish_non_exhaustive()
    }
}

impl Drop for EndPoint {
    fn drop(&mut self) {
        self.pool.shutdown();
    }
}
