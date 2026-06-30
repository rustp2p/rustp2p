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
        let mut pool_opt = None;
        let mut data_rx_opt = None;

        if let Some(port) = config.udp_port {
            let addr = format!("0.0.0.0:{port}");
            let socket = UdpSocket::bind(&addr).await?;
            let (pool, data_rx) = SocketPool::new(socket);
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
                let (pool, data_rx) = SocketPool::new(socket);
                (Arc::new(pool), data_rx)
            }
        };

        let codec: Box<dyn crate::endpoint::codec::InitCodec> = config
            .tcp_codec
            .take()
            .unwrap_or_else(|| Box::new(crate::endpoint::codec::LengthPrefixedInitCodec));

        let ep = Self {
            pool,
            data_rx,
            config,
        };

        // Start TCP accept loop
        if let Some(listener) = tcp_listener {
            let pool = ep.pool.clone();
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, peer_addr)) => {
                            log::debug!("TCP connection from {peer_addr}");
                            if let Err(e) = pool.add_tcp(stream, peer_addr, codec.as_ref()).await {
                                log::warn!("TCP setup error: {e}");
                            }
                        }
                        Err(e) => {
                            log::warn!("TCP accept error: {e}");
                        }
                    }
                }
            });
        }

        Ok(ep)
    }

    /// Creates an endpoint from an existing UDP socket.
    pub async fn from_socket(socket: UdpSocket) -> io::Result<Self> {
        let (pool, data_rx) = SocketPool::new(socket);
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

    /// Get local TCP port.
    pub async fn local_tcp_port(&self) -> u16 {
        self.pool
            .last_tcp()
            .await
            .map(|c| c.peer_addr.port())
            .unwrap_or(0)
    }

    /// Get NAT information using configured STUN servers.
    ///
    /// Uses the stun servers from Config to detect NAT type and public addresses.
    pub async fn nat_info(&self) -> io::Result<crate::nat::NatInfo> {
        let stun_servers = self.config.stun_servers.clone();
        let (nat_type, public_ips, port_range) =
            crate::stun::stun_test_nat(stun_servers, None).await?;
        log::info!("nat_type:{nat_type:?},public_ips:{public_ips:?},port_range={port_range}");

        let local_ipv4 = crate::util::addr::local_ipv4()
            .await
            .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED);

        let local_udp_ports = self.local_udp_ports().await;
        let local_tcp_port = self.local_tcp_port().await;

        let mut public_ports = local_udp_ports.clone();
        public_ports.fill(0);

        Ok(crate::nat::NatInfo {
            nat_type,
            public_ips,
            public_udp_ports: public_ports,
            mapping_tcp_addr: vec![],
            mapping_udp_addr: vec![],
            public_port_range: port_range,
            local_ipv4,
            local_ipv4s: vec![],
            ipv6: None,
            local_udp_ports,
            local_tcp_port,
            public_tcp_port: 0,
        })
    }
}

impl std::fmt::Debug for EndPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EndPoint").finish_non_exhaustive()
    }
}
