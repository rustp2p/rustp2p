use crate::endpoint::config::Config;
use crate::endpoint::pool::SocketPool;
use crate::endpoint::tunnel::Tunnel;
use crate::socket::LocalInterface;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;

/// Binds the shared UDP sockets and TCP listener and yields logical tunnels.
///
/// # Examples
///
/// ```rust,no_run
/// use bytes::Bytes;
/// use rustp2p_core::endpoint::{Config, TunnelIncoming};
///
/// # #[tokio::main]
/// # async fn main() -> std::io::Result<()> {
/// let mut incoming = TunnelIncoming::bind(Config::new().udp_port(3000)).await?;
/// println!("Listening on: {:?}", incoming.local_addr());
///
/// while let Some(mut tunnel) = incoming.next().await {
///     tokio::spawn(async move {
///         while let Some(data) = tunnel.recv().await {
///             println!("From {}: {:?}", tunnel.remote_addr(), data);
///             tunnel.send(Bytes::from_static(b"echo")).await?;
///         }
///         Ok::<_, std::io::Error>(())
///     });
/// }
/// # Ok(())
/// # }
/// ```
pub struct TunnelIncoming {
    pool: Arc<SocketPool>,
    tunnel_rx: mpsc::Receiver<Tunnel>,
    config: Config,
    local_udp_ipv6_addr: Option<SocketAddr>,
    local_tcp_addr: Option<SocketAddr>,
    local_tcp_ipv6_addr: Option<SocketAddr>,
}

impl TunnelIncoming {
    /// Binds an incoming tunnel source with the given configuration.
    pub async fn bind(mut config: Config) -> io::Result<Self> {
        let codec: Box<dyn crate::endpoint::codec::InitCodec> = config
            .tcp_codec
            .take()
            .unwrap_or_else(|| Box::new(crate::endpoint::codec::LengthPrefixedInitCodec));

        let (accept_tx, tunnel_rx) = mpsc::channel(512);
        let (main_v4, main_v6) = bind_main_udp(
            config.udp_port.unwrap_or(0),
            config.enable_ipv6,
            config.bind_ipv4,
            config.bind_ipv6,
            config.default_interface.as_ref(),
        )
        .await?;
        let local_udp_ipv6_addr = main_v6.as_ref().and_then(|s| s.local_addr().ok());
        let udp_v4_port = main_v4.local_addr()?.port();
        let (tcp_v4, tcp_v6) = if let Some(port) = config.tcp_port {
            bind_tcp(
                port,
                udp_v4_port,
                config.enable_ipv6,
                config.bind_ipv4,
                config.bind_ipv6,
                config.default_interface.as_ref(),
            )?
        } else {
            (None, None)
        };

        let local_tcp_addr = tcp_v4.as_ref().and_then(|l| l.local_addr().ok());
        let local_tcp_ipv6_addr = tcp_v6.as_ref().and_then(|l| l.local_addr().ok());

        let pool = Arc::new(SocketPool::new(
            main_v4,
            main_v6,
            accept_tx,
            codec.clone(),
            config.max_udp_datagram_size,
        ));

        let incoming = Self {
            pool,
            tunnel_rx,
            config,
            local_udp_ipv6_addr,
            local_tcp_addr,
            local_tcp_ipv6_addr,
        };

        for tcp_listener in [tcp_v4, tcp_v6].into_iter().flatten() {
            spawn_tcp_accept_loop(
                tcp_listener,
                incoming.pool.clone(),
                incoming.config.default_interface.clone(),
            );
        }

        Ok(incoming)
    }

    /// Returns the next UDP five-tuple or inbound/punched TCP connection, or
    /// `None` after the incoming source is closed.
    pub async fn next(&mut self) -> Option<Tunnel> {
        self.tunnel_rx.recv().await
    }

    /// Stops accepting tunnels and waits for the endpoint's internal workers
    /// to exit and release their TCP and UDP sockets.
    ///
    /// This also cancels pending outgoing TCP attempts and closes
    /// connected sockets not yet registered as tunnels. Outstanding
    /// `Puncher` or `Tunnel` handles do not keep the listeners open.
    ///
    /// The port-release guarantee covers sockets owned by the endpoint.
    /// Before closing, drop any raw sockets obtained from
    /// [`Puncher::udp_sockets`](crate::punch::Puncher::udp_sockets).
    /// This method returns [`io::ErrorKind::AddrInUse`] after shutdown if the
    /// original `Arc<UdpSocket>` still has owners. It cannot detect a socket
    /// taken out of its Arc (e.g. with `Arc::try_unwrap`) or a duplicated
    /// socket handle; those can keep a port bound even after `Ok(())`.
    ///
    /// Dropping the endpoint (or cancelling this future) requests shutdown
    /// without waiting for the workers.
    pub async fn close(mut self) -> io::Result<()> {
        self.tunnel_rx.close();
        self.pool.close().await
    }

    /// Returns the local address this incoming source is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.pool.local_addr()
    }

    /// Returns the configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns a Puncher for NAT hole-punching.
    ///
    /// The handle also provides raw UDP sends, socket queries, and NAT discovery.
    pub fn puncher(&self) -> crate::punch::Puncher {
        crate::punch::Puncher::new(
            self.pool.clone(),
            &self.config,
            self.local_tcp_addr.map(|a| a.port()).unwrap_or(0),
        )
    }

    /// Get local UDP ports.
    pub fn local_udp_ports(&self) -> Vec<u16> {
        self.pool.local_udp_ports()
    }

    /// The local IPv6 UDP socket address, if IPv6 UDP handling is enabled.
    pub fn local_udp_ipv6_addr(&self) -> Option<SocketAddr> {
        self.local_udp_ipv6_addr
    }

    /// The local IPv4 TCP listener address, `None` when TCP or IPv4 handling
    /// is disabled.
    pub fn local_tcp_addr(&self) -> Option<SocketAddr> {
        self.local_tcp_addr
    }

    /// The local IPv6 TCP listener address, if IPv6 TCP handling is enabled.
    pub fn local_tcp_ipv6_addr(&self) -> Option<SocketAddr> {
        self.local_tcp_ipv6_addr
    }
}

impl std::fmt::Debug for TunnelIncoming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelIncoming").finish_non_exhaustive()
    }
}

/// Bind the main UDP sockets: IPv4 on `0.0.0.0:port` and, when IPv6 is
/// enabled and the system supports it, an IPv6-only socket on `[::]:port`
/// sharing the same port.
///
/// With port 0, the IPv6 socket is bound first (its own random port); binding
/// it fails exactly when the system has no IPv6 support, in which case we
/// bind an IPv4 socket on port 0 directly - no retries needed. When v6 binds
/// successfully, a v4 socket is paired on v6's port, retrying with a fresh
/// v6 port on conflict (up to 20 attempts).
///
/// A missing second socket (IPv6 unsupported or disabled) is a silent
/// downgrade to IPv4 only, not an error.
async fn bind_main_udp(
    port: u16,
    enable_ipv6: bool,
    bind_ipv4: Option<Ipv4Addr>,
    bind_ipv6: Option<Ipv6Addr>,
    default_interface: Option<&LocalInterface>,
) -> io::Result<(UdpSocket, Option<UdpSocket>)> {
    if !enable_ipv6 {
        let main_v4 = bind_udp_v4(v4_addr(bind_ipv4, port), default_interface).await?;
        return Ok((main_v4, None));
    }
    if port != 0 {
        let main_v4 = bind_udp_v4(v4_addr(bind_ipv4, port), default_interface).await?;
        return bind_v6_same_port(
            main_v4,
            v6_addr(bind_ipv6, port),
            bind_ipv6.is_some(),
            default_interface,
        )
        .await;
    }
    // Bind an IPv6-only socket on port 0 first. On systems without IPv6 the
    // bind fails, so this doubles as a capability probe - no retry needed.
    match bind_udp_v6(v6_addr(bind_ipv6, 0), default_interface) {
        Ok(mut main_v6) => {
            // IPv6 is supported. Pair the v4 socket on the same port; when
            // the v4 bind conflicts, re-bind v6 for a fresh port and retry,
            // up to 20 attempts.
            for _ in 0..20 {
                let port = main_v6.local_addr()?.port();
                if let Ok(main_v4) = bind_udp_v4(v4_addr(bind_ipv4, port), default_interface).await
                {
                    return Ok((main_v4, Some(main_v6)));
                }
                main_v6 = bind_udp_v6(v6_addr(bind_ipv6, 0), default_interface)?;
            }
            let main_v4 = bind_udp_v4(v4_addr(bind_ipv4, 0), default_interface).await?;
            Ok((main_v4, Some(main_v6)))
        }
        Err(e) if bind_ipv6.is_none() => {
            log::warn!("IPv6 main socket unavailable, using IPv4 only: {e}");
            let main_v4 = bind_udp_v4(v4_addr(bind_ipv4, 0), default_interface).await?;
            Ok((main_v4, None))
        }
        Err(e) => Err(e),
    }
}

/// Bind the IPv6 socket on the v4 socket's port, downgrading to IPv4 only
/// when the system has no IPv6 support or the port is unavailable.
async fn bind_v6_same_port(
    main_v4: UdpSocket,
    addr: SocketAddr,
    explicit: bool,
    default_interface: Option<&LocalInterface>,
) -> io::Result<(UdpSocket, Option<UdpSocket>)> {
    match bind_udp_v6(addr, default_interface) {
        Ok(main_v6) => Ok((main_v4, Some(main_v6))),
        Err(e) if !explicit => {
            log::warn!("IPv6 main socket unavailable, falling back to IPv4 only: {e}");
            Ok((main_v4, None))
        }
        Err(e) => Err(e),
    }
}

/// Bind an IPv6-only UDP socket on `[::]:port`.
///
/// v6-only so a v4 socket bound to the same port does not conflict.
async fn bind_udp_v4(
    addr: SocketAddr,
    default_interface: Option<&LocalInterface>,
) -> io::Result<UdpSocket> {
    if default_interface.is_none() {
        return UdpSocket::bind(addr).await;
    }
    let socket = crate::socket::bind_udp_ops(addr, true, default_interface)?;
    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}

fn bind_udp_v6(
    addr: SocketAddr,
    default_interface: Option<&LocalInterface>,
) -> io::Result<UdpSocket> {
    let socket = crate::socket::bind_udp_ops(addr, true, default_interface)?;
    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}

impl Drop for TunnelIncoming {
    fn drop(&mut self) {
        self.pool.shutdown();
    }
}

/// Runs the accept loop for one TCP listener until global shutdown, publishing
/// each accepted connection to the pool.
fn spawn_tcp_accept_loop(
    tcp_listener: TcpListener,
    pool: Arc<SocketPool>,
    default_interface: Option<LocalInterface>,
) {
    let handle = Arc::downgrade(&pool);
    pool.spawn_listener(async move {
        loop {
            match tcp_listener.accept().await {
                Ok((stream, peer_addr)) => {
                    let Some(pool) = handle.upgrade() else {
                        break;
                    };
                    log::debug!("TCP connection from {peer_addr}");
                    if let Err(e) = crate::socket::configure_accepted_tcp_stream(
                        &stream,
                        default_interface.as_ref(),
                    ) {
                        log::warn!("TCP stream setup error for {peer_addr}: {e}");
                        continue;
                    }
                    if let Err(e) = pool.publish_tcp(stream, peer_addr).await {
                        if pool.ensure_open().is_err() {
                            break;
                        }
                        log::warn!("TCP setup error: {e}");
                    }
                }
                Err(e) => {
                    log::warn!("TCP accept error: {e}");
                    // Back off on persistent errors (e.g. fd exhaustion).
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
    });
}

/// Bind IPv4 and IPv6 TCP listeners. IPv6 is v6-only so both listeners can
/// bind concrete addresses on the same port.
fn bind_tcp(
    configured_port: u16,
    udp_port: u16,
    enable_ipv6: bool,
    bind_ipv4: Option<Ipv4Addr>,
    bind_ipv6: Option<Ipv6Addr>,
    default_interface: Option<&LocalInterface>,
) -> io::Result<(Option<TcpListener>, Option<TcpListener>)> {
    let bind_v4 =
        |port| crate::socket::bind_tcp_listener(v4_addr(bind_ipv4, port), false, default_interface);
    let bind_v6 =
        |port| crate::socket::bind_tcp_listener(v6_addr(bind_ipv6, port), true, default_interface);
    bind_tcp_with(
        configured_port,
        udp_port,
        enable_ipv6,
        bind_ipv6.is_some(),
        bind_v4,
        bind_v6,
    )
}

fn bind_tcp_with<F4, F6>(
    configured_port: u16,
    udp_port: u16,
    enable_ipv6: bool,
    explicit_ipv6: bool,
    bind_v4: F4,
    bind_v6: F6,
) -> io::Result<(Option<TcpListener>, Option<TcpListener>)>
where
    F4: Fn(u16) -> io::Result<TcpListener>,
    F6: Fn(u16) -> io::Result<TcpListener>,
{
    let preferred_port = if configured_port == 0 {
        udp_port
    } else {
        configured_port
    };

    if !enable_ipv6 {
        return bind_v4_with_fallback(bind_v4, preferred_port, configured_port)
            .map(|listener| (Some(listener), None));
    }

    // First try the requested UDP/shared port. For port zero this preserves
    // the existing preference for the main UDP port.
    let v4 = match bind_v4(preferred_port) {
        Ok(listener) => listener,
        Err(error) if configured_port == 0 && is_tcp_port_conflict(&error) => bind_v4(0)?,
        Err(error) => return Err(error),
    };
    let v4_port = v4.local_addr()?.port();
    match bind_v6(v4_port) {
        Ok(v6) => return Ok((Some(v4), Some(v6))),
        Err(error) if explicit_ipv6 && (configured_port != 0 || !is_tcp_port_conflict(&error)) => {
            return Err(error);
        }
        Err(error) if configured_port != 0 || !is_tcp_port_conflict(&error) => {
            log::warn!("IPv6 TCP listener unavailable, using IPv4 only: {error}");
            return Ok((Some(v4), None));
        }
        Err(error) => {
            log::debug!("TCP IPv4/IPv6 port pairing retry: {error}");
            drop(v4);
        }
    }

    // A total of 20 pairing attempts are allowed. TCP must use a shared port
    // across address families. An explicit IPv6 bind reports pairing failure;
    // an implicit IPv6 bind eventually falls back to IPv4 only.
    let mut attempts = 1;
    let last_pair_error = loop {
        let v4 = bind_v4(0)?;
        let v4_port = v4.local_addr()?.port();
        match bind_v6(v4_port) {
            Ok(v6) => return Ok((Some(v4), Some(v6))),
            Err(error) if explicit_ipv6 && !is_tcp_port_conflict(&error) => {
                return Err(error);
            }
            Err(error) if !is_tcp_port_conflict(&error) => {
                log::warn!("IPv6 TCP listener unavailable, using IPv4 only: {error}");
                return Ok((Some(v4), None));
            }
            Err(error) => {
                log::debug!("TCP IPv4/IPv6 port pairing retry: {error}");
                if attempts == 20 {
                    if !explicit_ipv6 {
                        log::warn!(
                            "IPv6 TCP listener could not share a port, using IPv4 only: {error}"
                        );
                        return Ok((Some(v4), None));
                    }
                    break error;
                }
                attempts += 1;
            }
        }
    };
    Err(last_pair_error)
}

fn bind_v4_with_fallback<F>(
    bind: F,
    preferred_port: u16,
    configured_port: u16,
) -> io::Result<TcpListener>
where
    F: Fn(u16) -> io::Result<TcpListener>,
{
    match bind(preferred_port) {
        Ok(listener) => Ok(listener),
        Err(error) if configured_port == 0 && is_tcp_port_conflict(&error) => bind(0),
        Err(error) => Err(error),
    }
}

fn v4_addr(ip: Option<Ipv4Addr>, port: u16) -> SocketAddr {
    SocketAddr::from((ip.unwrap_or(Ipv4Addr::UNSPECIFIED), port))
}

fn v6_addr(ip: Option<Ipv6Addr>, port: u16) -> SocketAddr {
    SocketAddr::from((ip.unwrap_or(Ipv6Addr::UNSPECIFIED), port))
}

fn is_tcp_port_conflict(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::AddrInUse {
        return true;
    }

    // Windows can report WSAEACCES instead of WSAEADDRINUSE when another
    // socket has the port reserved with exclusive address use.
    #[cfg(windows)]
    if error.raw_os_error() == Some(10013) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::{bind_tcp_with, TunnelIncoming};
    use crate::endpoint::Config;
    use crate::nat::NatType;
    use bytes::Bytes;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream, UdpSocket};

    async fn udp_incoming() -> TunnelIncoming {
        TunnelIncoming::bind(Config::udp(0).enable_ipv6(false))
            .await
            .unwrap()
    }

    fn loopback(addr: SocketAddr) -> SocketAddr {
        SocketAddr::new("127.0.0.1".parse().unwrap(), addr.port())
    }

    #[tokio::test]
    async fn close_releases_ports_with_live_puncher_and_tunnels() {
        let config = Config::new()
            .enable_ipv6(false)
            .bind_ipv4(std::net::Ipv4Addr::LOCALHOST)
            .udp_port(0)
            .tcp_port(0)
            .max_assistant_sockets(2);
        let mut endpoint = TunnelIncoming::bind(config).await.unwrap();
        let puncher = endpoint.puncher();
        let udp_addr = endpoint.local_addr().unwrap();
        let tcp_addr = endpoint.local_tcp_addr().unwrap();
        puncher.apply_nat_model(NatType::Symmetric).unwrap();
        let assistants = puncher
            .udp_sockets()
            .into_iter()
            .skip(1)
            .map(|socket| socket.local_addr().unwrap())
            .collect::<Vec<_>>();

        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        udp.send_to(b"first packet", udp_addr).await.unwrap();
        let mut udp_tunnel = tokio::time::timeout(Duration::from_secs(2), endpoint.next())
            .await
            .unwrap()
            .unwrap();
        assert!(udp_tunnel.is_udp());
        assert!(udp_tunnel.recv().await.is_some());
        let _peer = TcpStream::connect(tcp_addr).await.unwrap();
        let mut tcp_tunnel = tokio::time::timeout(Duration::from_secs(2), endpoint.next())
            .await
            .unwrap()
            .unwrap();
        assert!(tcp_tunnel.is_tcp());

        // Also wait for workers from an assistant generation retired earlier.
        puncher.apply_nat_model(NatType::Cone).unwrap();
        tokio::time::timeout(Duration::from_secs(2), endpoint.close())
            .await
            .unwrap()
            .unwrap();

        assert!(udp_tunnel.recv().await.is_none());
        assert!(tcp_tunnel.recv().await.is_none());
        assert!(puncher.udp_sockets().is_empty());
        assert_eq!(
            puncher.send_to(b"closed", udp_addr).unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            puncher
                .apply_nat_model(NatType::Symmetric)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::BrokenPipe
        );

        // No sleeps or retries: the original ports must be available now.
        let replacement = TunnelIncoming::bind(
            Config::new()
                .enable_ipv6(false)
                .bind_ipv4(std::net::Ipv4Addr::LOCALHOST)
                .udp_port(udp_addr.port())
                .tcp_port(tcp_addr.port()),
        )
        .await
        .unwrap();
        let _assistants = assistants
            .into_iter()
            .map(|addr| std::net::UdpSocket::bind(addr).unwrap())
            .collect::<Vec<_>>();
        replacement.close().await.unwrap();
    }

    #[tokio::test]
    async fn retained_puncher_cannot_connect_after_close() {
        use crate::nat::NatInfo;
        use crate::punch::{PunchInfo, PunchModel};

        let endpoint = udp_incoming().await;
        let puncher = endpoint.puncher();
        let peer = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = peer.local_addr().unwrap();
        let info = PunchInfo::new(
            PunchModel::all(),
            NatInfo {
                mapping_tcp_addr: vec![addr],
                ..Default::default()
            },
        );
        endpoint.close().await.unwrap();
        assert_eq!(
            puncher.connect_tcp(addr, None).await.unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            puncher
                .punch_now(None, Bytes::new(), info)
                .await
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), peer.accept())
                .await
                .is_err()
        );
    }

    // Poll just enough to start the dial. This joins its worker without
    // letting the caller consume the success notification.
    async fn complete_outgoing_dial(
        endpoint: &TunnelIncoming,
        mut connecting: std::pin::Pin<&mut impl std::future::Future>,
        peer: &TcpListener,
    ) -> TcpStream {
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(connecting.as_mut().poll(&mut cx).is_pending());
        tokio::time::timeout(
            Duration::from_secs(5),
            endpoint.pool.join_tcp_dial_for_test(),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), peer.accept())
            .await
            .unwrap()
            .unwrap()
            .0
    }

    async fn assert_peer_eof(mut stream: TcpStream) {
        let mut bytes = [0; 1];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), stream.read(&mut bytes))
                .await
                .expect("peer should see EOF, even if the caller is still paused")
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn dropping_connect_after_dial_does_not_publish_tunnel() {
        let mut endpoint = udp_incoming().await;
        let puncher = endpoint.puncher();
        let peer = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut connecting = Box::pin(puncher.connect_tcp(
            peer.local_addr().unwrap(),
            Some(Bytes::from_static(b"initial data")),
        ));
        let stream = complete_outgoing_dial(&endpoint, connecting.as_mut(), &peer).await;

        drop(connecting);
        assert!(matches!(
            endpoint.tunnel_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        assert_peer_eof(stream).await;
        endpoint.close().await.unwrap();
    }

    #[tokio::test]
    async fn close_reclaims_outgoing_tcp_before_caller_resumes() {
        let endpoint = udp_incoming().await;
        let puncher = endpoint.puncher();
        let peer = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut connecting = Box::pin(puncher.connect_tcp(peer.local_addr().unwrap(), None));
        let stream = complete_outgoing_dial(&endpoint, connecting.as_mut(), &peer).await;

        endpoint.close().await.unwrap();
        assert_peer_eof(stream).await;
        // Only now let the caller resume. The earlier dial success cannot
        // produce a tunnel after the endpoint closed.
        assert_eq!(
            connecting.await.unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }

    #[tokio::test]
    async fn outgoing_tcp_handoff_registers_the_connection_for_close() {
        let mut endpoint = udp_incoming().await;
        let puncher = endpoint.puncher();
        let peer = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = peer.local_addr().unwrap();
        let data = Bytes::from_static(b"hello");
        let mut connecting = Box::pin(puncher.connect_tcp(addr, Some(data.clone())));
        let mut stream = complete_outgoing_dial(&endpoint, connecting.as_mut(), &peer).await;

        connecting.await.unwrap();
        let tunnel = tokio::time::timeout(Duration::from_secs(5), endpoint.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tunnel.remote_addr(), addr);
        let mut frame = [0; 9]; // four-byte length prefix, then "hello"
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut frame))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&frame[..4], &5u32.to_be_bytes());
        assert_eq!(&frame[4..], data.as_ref());

        endpoint.close().await.unwrap();
        assert_peer_eof(stream).await;
        drop(tunnel);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_reclaims_outgoing_tcp_during_codec_init() {
        use crate::endpoint::codec::{BytesInitCodec, Decoder, Encoder, InitCodec};
        use std::io;
        use std::sync::Arc;

        #[derive(Clone)]
        struct PausedCodec {
            entered: Arc<parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
            resume: Arc<parking_lot::Mutex<std::sync::mpsc::Receiver<()>>>,
        }

        impl InitCodec for PausedCodec {
            fn codec(&self, peer: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
                self.entered.lock().take().unwrap().send(()).unwrap();
                self.resume
                    .lock()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                BytesInitCodec.codec(peer)
            }
        }

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let endpoint = TunnelIncoming::bind(Config::udp(0).enable_ipv6(false).tcp_codec(Box::new(
            PausedCodec {
                entered: Arc::new(parking_lot::Mutex::new(Some(entered_tx))),
                resume: Arc::new(parking_lot::Mutex::new(resume_rx)),
            },
        )))
        .await
        .unwrap();
        let peer = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = peer.local_addr().unwrap();
        let puncher = endpoint.puncher();
        let connecting = tokio::spawn(async move { puncher.connect_tcp(addr, None).await });
        tokio::time::timeout(Duration::from_secs(5), entered_rx)
            .await
            .unwrap()
            .unwrap();
        let (stream, _) = tokio::time::timeout(Duration::from_secs(5), peer.accept())
            .await
            .unwrap()
            .unwrap();

        endpoint.close().await.unwrap();
        assert_peer_eof(stream).await;
        resume_tx.send(()).unwrap();
        assert_eq!(
            connecting.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[tokio::test]
    async fn close_cancels_and_joins_in_flight_tcp_attempt() {
        let endpoint = udp_incoming().await;
        let pool = endpoint.pool.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (resource, mut released) = tokio::sync::oneshot::channel::<()>();
        let connecting = tokio::spawn(async move {
            pool.connect_outgoing_tcp(SocketAddr::from(([127, 0, 0, 1], 0)), None, async move {
                let _resource = resource;
                let _ = started_tx.send(());
                std::future::pending().await
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), endpoint.close())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            released.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ));
        assert_eq!(
            connecting.await.unwrap().unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }

    #[tokio::test]
    async fn extracted_udp_socket_remains_callers_responsibility() {
        let endpoint =
            TunnelIncoming::bind(Config::udp(0).enable_ipv6(false).max_assistant_sockets(1))
                .await
                .unwrap();
        let puncher = endpoint.puncher();
        puncher.apply_nat_model(NatType::Symmetric).unwrap();
        let assistant = puncher.udp_sockets().pop().unwrap();
        puncher.apply_nat_model(NatType::Cone).unwrap();
        let socket = tokio::time::timeout(Duration::from_secs(2), async {
            let mut assistant = assistant;
            loop {
                match std::sync::Arc::try_unwrap(assistant) {
                    Ok(socket) => break socket,
                    Err(shared) => assistant = shared,
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let addr = socket.local_addr().unwrap();
        endpoint.close().await.unwrap();
        // This socket is now owned directly by the caller, not an endpoint Arc.
        assert!(UdpSocket::bind(addr).await.is_err());
        drop(socket);
        let _rebound = UdpSocket::bind(addr).await.unwrap();
    }

    #[tokio::test]
    async fn close_reports_outstanding_raw_udp_socket() {
        let endpoint = udp_incoming().await;
        let addr = endpoint.local_addr().unwrap();
        let sockets = endpoint.puncher().udp_sockets();
        let error = endpoint.close().await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        drop(sockets);
        let _rebound = UdpSocket::bind(addr).await.unwrap();
    }

    #[tokio::test]
    async fn close_releases_ipv6_listeners_when_available() {
        if UdpSocket::bind("[::1]:0").await.is_err() || TcpListener::bind("[::1]:0").await.is_err()
        {
            return;
        }
        let endpoint = TunnelIncoming::bind(
            Config::new()
                .enable_ipv6(true)
                .bind_ipv4(std::net::Ipv4Addr::LOCALHOST)
                .bind_ipv6(std::net::Ipv6Addr::LOCALHOST)
                .udp_port(0)
                .tcp_port(0),
        )
        .await
        .unwrap();
        let udp = endpoint.local_udp_ipv6_addr().unwrap();
        let tcp = endpoint.local_tcp_ipv6_addr().unwrap();
        let _puncher = endpoint.puncher();
        endpoint.close().await.unwrap();
        let _udp = UdpSocket::bind(udp).await.unwrap();
        let _tcp = TcpListener::bind(tcp).await.unwrap();
    }

    #[tokio::test]
    async fn zero_tcp_port_prefers_the_main_udp_port() {
        let listener =
            TunnelIncoming::bind(Config::new().udp_port(0).tcp_port(0).enable_ipv6(false))
                .await
                .unwrap();

        assert_ne!(listener.local_tcp_addr().unwrap().port(), 0);
        assert_eq!(
            listener.local_tcp_addr().unwrap().port(),
            listener.local_addr().unwrap().port()
        );
    }

    #[tokio::test]
    async fn zero_tcp_port_falls_back_when_the_udp_port_is_taken_for_tcp() {
        let blocker = TcpListener::bind("0.0.0.0:0").await.unwrap();
        let occupied_port = blocker.local_addr().unwrap().port();
        let listener = TunnelIncoming::bind(
            Config::new()
                .udp_port(occupied_port)
                .tcp_port(0)
                .enable_ipv6(false),
        )
        .await
        .unwrap();

        assert_eq!(listener.local_addr().unwrap().port(), occupied_port);
        assert_ne!(listener.local_tcp_addr().unwrap().port(), 0);
        assert_ne!(listener.local_tcp_addr().unwrap().port(), occupied_port);
    }

    #[tokio::test]
    async fn apply_nat_model_uses_external_nat_type() {
        let listener = TunnelIncoming::bind(
            Config::new()
                .udp_port(0)
                .tcp_port(0)
                .max_assistant_sockets(2),
        )
        .await
        .unwrap();
        let puncher = listener.puncher();

        puncher.apply_nat_model(NatType::Symmetric).unwrap();
        assert_eq!(puncher.assistant_count(), 2);

        puncher.apply_nat_model(NatType::Cone).unwrap();
        assert_eq!(puncher.assistant_count(), 0);
    }

    #[tokio::test]
    async fn puncher_owns_udp_socket_queries_and_nat_info() {
        let mapping_tcp_addr: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        let mapping_udp_addr: SocketAddr = "127.0.0.1:42000".parse().unwrap();
        let listener = TunnelIncoming::bind(
            Config::new()
                .udp_port(0)
                .tcp_port(0)
                .enable_ipv6(false)
                .mapping_tcp_addr(vec![mapping_tcp_addr])
                .mapping_udp_addr(vec![mapping_udp_addr]),
        )
        .await
        .unwrap();
        let puncher = listener.puncher();

        assert_eq!(
            puncher.local_addr().unwrap(),
            listener.local_addr().unwrap()
        );
        assert_eq!(puncher.assistant_count(), 0);
        assert_eq!(puncher.udp_sockets().len(), 1);

        let nat_info = puncher.nat_info().await.unwrap();
        assert_eq!(nat_info.mapping_tcp_addr, vec![mapping_tcp_addr]);
        assert_eq!(nat_info.mapping_udp_addr, vec![mapping_udp_addr]);
        assert_eq!(
            nat_info.local_tcp_port,
            listener.local_tcp_addr().unwrap().port()
        );
        assert_eq!(nat_info.local_udp_ports, listener.local_udp_ports());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn bind_applies_default_interface_to_the_main_socket() {
        let result = TunnelIncoming::bind(
            Config::udp(0)
                .enable_ipv6(false)
                .default_interface(crate::socket::LocalInterface::new("rp2pnone".to_owned())),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn bind_ipv4_applies_to_main_assistant_and_tcp_sockets() {
        let bind_ip = std::net::Ipv4Addr::LOCALHOST;
        let listener = TunnelIncoming::bind(
            Config::new()
                .udp_port(0)
                .tcp_port(0)
                .enable_ipv6(false)
                .bind_ipv4(bind_ip)
                .max_assistant_sockets(1),
        )
        .await
        .unwrap();
        assert_eq!(
            listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(bind_ip)
        );
        assert!(listener.local_udp_ipv6_addr().is_none());
        assert_eq!(
            listener.local_tcp_addr().unwrap().ip(),
            std::net::IpAddr::V4(bind_ip)
        );
        assert!(listener.local_tcp_ipv6_addr().is_none());

        let puncher = listener.puncher();
        puncher.apply_nat_model(NatType::Symmetric).unwrap();
        assert!(puncher
            .udp_sockets()
            .iter()
            .all(|socket| socket.local_addr().unwrap().ip() == std::net::IpAddr::V4(bind_ip)));

        let remote = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote_addr = remote.local_addr().unwrap();
        let (connect_result, accepted) =
            tokio::join!(puncher.connect_tcp(remote_addr, None), remote.accept(),);
        connect_result.unwrap();
        assert_eq!(accepted.unwrap().1.ip(), std::net::IpAddr::V4(bind_ip));

        let info = puncher.nat_info().await.unwrap();
        assert_eq!(info.local_ipv4s, vec![bind_ip]);
    }

    #[tokio::test]
    async fn explicit_ipv6_binding_uses_separate_listener_on_the_udp_port() {
        if UdpSocket::bind("[::1]:0").await.is_err() {
            return; // IPv6 is unavailable on this test host.
        }
        let listener = TunnelIncoming::bind(
            Config::new()
                .udp_port(0)
                .tcp_port(0)
                .bind_ipv4(std::net::Ipv4Addr::LOCALHOST)
                .bind_ipv6(std::net::Ipv6Addr::LOCALHOST),
        )
        .await
        .unwrap();
        assert_eq!(
            listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );
        let udp_v6 = listener.local_udp_ipv6_addr().unwrap();
        assert_eq!(
            udp_v6.ip(),
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        );
        assert_eq!(udp_v6.port(), listener.local_addr().unwrap().port());
        assert_eq!(
            listener.local_tcp_addr().unwrap().ip(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );
        let tcp_v6 = listener.local_tcp_ipv6_addr().unwrap();
        assert_eq!(
            tcp_v6.ip(),
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        );
        assert_eq!(tcp_v6.port(), listener.local_tcp_addr().unwrap().port());
    }

    #[tokio::test]
    async fn unavailable_implicit_ipv6_tcp_falls_back_to_ipv4() {
        let bind_v4 = |port| {
            crate::socket::bind_tcp_listener(
                SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)),
                false,
                None,
            )
        };
        let bind_v6 = |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "IPv6 unavailable",
            ))
        };

        let (v4, v6) = bind_tcp_with(0, 0, true, false, bind_v4, bind_v6).unwrap();

        assert!(v4.is_some());
        assert!(v6.is_none());
    }

    #[tokio::test]
    async fn explicit_unavailable_ip_does_not_fall_back_to_wildcard() {
        let result = TunnelIncoming::bind(
            Config::udp(0)
                .enable_ipv6(false)
                .bind_ipv4("203.0.113.1".parse().unwrap()),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn disabling_ipv6_skips_explicit_ipv6_binding() {
        let listener = TunnelIncoming::bind(
            Config::udp(0)
                .enable_ipv6(false)
                .bind_ipv4(std::net::Ipv4Addr::LOCALHOST)
                .bind_ipv6("2001:db8::1".parse().unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(
            listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );
        assert_eq!(listener.puncher().udp_sockets().len(), 1);
    }

    #[tokio::test]
    async fn udp_five_tuples_are_accepted_once_and_demultiplexed() {
        let mut listener = udp_incoming().await;
        let target = loopback(listener.local_addr().unwrap());
        let client_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        client_a.send_to(b"a1", target).await.unwrap();
        let mut tunnel_a = listener.next().await.unwrap();
        assert_eq!(tunnel_a.remote_addr(), client_a.local_addr().unwrap());
        assert_eq!(&tunnel_a.recv().await.unwrap()[..], b"a1");

        client_b.send_to(b"b1", target).await.unwrap();
        let mut tunnel_b = listener.next().await.unwrap();
        assert_eq!(tunnel_b.remote_addr(), client_b.local_addr().unwrap());
        assert_eq!(&tunnel_b.recv().await.unwrap()[..], b"b1");

        client_a.send_to(b"a2", target).await.unwrap();
        client_b.send_to(b"b2", target).await.unwrap();
        assert_eq!(&tunnel_a.recv().await.unwrap()[..], b"a2");
        assert_eq!(&tunnel_b.recv().await.unwrap()[..], b"b2");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.next())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn dropping_udp_tunnel_allows_same_tuple_to_be_accepted_again() {
        let mut listener = udp_incoming().await;
        let target = loopback(listener.local_addr().unwrap());
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        client.send_to(b"first", target).await.unwrap();
        let mut first = listener.next().await.unwrap();
        assert_eq!(&first.recv().await.unwrap()[..], b"first");
        let route_key = first.route_key();
        drop(first);

        client.send_to(b"second", target).await.unwrap();
        let mut second = listener.next().await.unwrap();
        assert_eq!(second.route_key(), route_key);
        assert_eq!(&second.recv().await.unwrap()[..], b"second");
    }

    #[tokio::test]
    async fn udp_tunnel_split_supports_independent_read_and_write_halves() {
        let mut listener = udp_incoming().await;
        let target = loopback(listener.local_addr().unwrap());
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        client.send_to(b"open", target).await.unwrap();
        let tunnel = listener.next().await.unwrap();
        let route_key = tunnel.route_key();
        let (mut reader, writer) = tunnel.split();
        assert_eq!(reader.route_key(), route_key);
        assert_eq!(writer.route_key(), route_key);
        assert_eq!(&reader.recv().await.unwrap()[..], b"open");

        writer.send(Bytes::from_static(b"reply")).await.unwrap();
        let mut buffer = [0_u8; 16];
        let (len, peer) = client.recv_from(&mut buffer).await.unwrap();
        assert_eq!(peer, target);
        assert_eq!(&buffer[..len], b"reply");

        // Dropping only the reader unregisters the five-tuple. The independent
        // writer remains usable, while the next inbound packet creates a new tunnel.
        drop(reader);
        writer
            .send(Bytes::from_static(b"still-open"))
            .await
            .unwrap();
        client.recv_from(&mut buffer).await.unwrap();
        client.send_to(b"reopen", target).await.unwrap();
        let mut reopened = listener.next().await.unwrap();
        assert_eq!(reopened.route_key(), route_key);
        assert_eq!(&reopened.recv().await.unwrap()[..], b"reopen");
    }

    #[tokio::test]
    async fn full_udp_tunnel_does_not_block_other_five_tuples() {
        let mut listener = udp_incoming().await;
        let target = loopback(listener.local_addr().unwrap());
        let slow_client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let fast_client = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        slow_client.send_to(b"start", target).await.unwrap();
        let _slow_tunnel = listener.next().await.unwrap();
        for _ in 0..crate::endpoint::tunnel::TUNNEL_CHANNEL_CAPACITY + 64 {
            slow_client.send_to(b"overflow", target).await.unwrap();
        }

        fast_client.send_to(b"fast", target).await.unwrap();
        let mut fast_tunnel = tokio::time::timeout(Duration::from_secs(1), listener.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fast_tunnel.remote_addr(), fast_client.local_addr().unwrap());
        assert_eq!(&fast_tunnel.recv().await.unwrap()[..], b"fast");
    }

    #[tokio::test]
    async fn tcp_accept_and_drop_follow_tunnel_lifetime() {
        let mut server = TunnelIncoming::bind(Config::tcp(0).enable_ipv6(false))
            .await
            .unwrap();
        let target = SocketAddr::new(
            "127.0.0.1".parse().unwrap(),
            server.local_tcp_addr().unwrap().port(),
        );

        let mut client = TcpStream::connect(target).await.unwrap();
        let mut server_tunnel = server.next().await.unwrap();
        assert!(server_tunnel.is_tcp());

        client.write_all(&6_u32.to_be_bytes()).await.unwrap();
        client.write_all(b"client").await.unwrap();
        assert_eq!(&server_tunnel.recv().await.unwrap()[..], b"client");
        server_tunnel
            .send(Bytes::from_static(b"server"))
            .await
            .unwrap();
        let mut header = [0_u8; 4];
        client.read_exact(&mut header).await.unwrap();
        assert_eq!(u32::from_be_bytes(header), 6);
        let mut payload = [0_u8; 6];
        client.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload, b"server");

        drop(client);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), server_tunnel.recv())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn tcp_listens_on_ipv6_when_enabled() {
        let mut server = TunnelIncoming::bind(Config::tcp(0).enable_ipv6(true))
            .await
            .unwrap();
        let Some(addr) = server.local_tcp_ipv6_addr() else {
            // Host without IPv6 support: silently downgraded to IPv4 only.
            return;
        };

        // An IPv6 connection is accepted and tunneled like any other TCP stream.
        let target = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], addr.port()));
        let mut client = TcpStream::connect(target).await.unwrap();
        let mut server_tunnel = server.next().await.unwrap();
        assert!(server_tunnel.is_tcp());
        assert!(server_tunnel.remote_addr().is_ipv6());

        client.write_all(&2_u32.to_be_bytes()).await.unwrap();
        client.write_all(b"v6").await.unwrap();
        assert_eq!(&server_tunnel.recv().await.unwrap()[..], b"v6");
        server_tunnel
            .send(Bytes::from_static(b"pong"))
            .await
            .unwrap();
        let mut header = [0_u8; 4];
        client.read_exact(&mut header).await.unwrap();
        assert_eq!(u32::from_be_bytes(header), 4);
        let mut payload = [0_u8; 4];
        client.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload, b"pong");

        // The IPv4 listener stays available alongside, sharing one port.
        let v4_target = SocketAddr::new(
            "127.0.0.1".parse().unwrap(),
            server.local_tcp_addr().unwrap().port(),
        );
        let mut v4_client = TcpStream::connect(v4_target).await.unwrap();
        let mut v4_tunnel = server.next().await.unwrap();
        assert!(v4_tunnel.remote_addr().is_ipv4());
        v4_client.write_all(&2_u32.to_be_bytes()).await.unwrap();
        v4_client.write_all(b"v4").await.unwrap();
        assert_eq!(&v4_tunnel.recv().await.unwrap()[..], b"v4");
    }

    /// Port-conflict behavior differs on Windows, so this is only tested on
    /// platforms with Unix socket binding semantics.
    #[cfg(not(windows))]
    #[tokio::test]
    async fn tcp_ipv4_ipv6_pair_falls_back_when_the_udp_port_is_taken() {
        // Another TCP process holds the UDP port. Block the port with a
        // plain listener bound to `[::]` itself — that conflicts with our
        // dual-stack bind on unix; the listener then falls back to an
        // OS-assigned port while still serving both families.
        let blocker = std::net::TcpListener::bind(SocketAddr::from(([0; 16], 0))).unwrap();
        let occupied_port = blocker.local_addr().unwrap().port();
        let server = TunnelIncoming::bind(
            Config::new()
                .udp_port(occupied_port)
                .tcp_port(0)
                .enable_ipv6(true),
        )
        .await
        .unwrap();

        // UDP still binds the occupied port (TCP blocker does not block UDP).
        assert_eq!(server.local_addr().unwrap().port(), occupied_port);
        let Some(addr) = server.local_tcp_ipv6_addr() else {
            // Host without IPv6 support: nothing to pair.
            return;
        };
        // The TCP listener moved off the occupied port.
        assert_ne!(addr.port(), occupied_port);
    }

    #[tokio::test]
    async fn dropping_incoming_closes_udp_tunnels() {
        let mut listener = udp_incoming().await;
        let target = loopback(listener.local_addr().unwrap());
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(b"open", target).await.unwrap();
        let mut tunnel = listener.next().await.unwrap();
        assert_eq!(&tunnel.recv().await.unwrap()[..], b"open");

        drop(listener);
        assert!(tokio::time::timeout(Duration::from_secs(1), tunnel.recv())
            .await
            .unwrap()
            .is_none());
        tokio::task::yield_now().await;
        assert!(tunnel.send(Bytes::from_static(b"closed")).await.is_err());
    }

    #[tokio::test]
    async fn removing_assistant_socket_closes_its_udp_tunnels() {
        let mut listener =
            TunnelIncoming::bind(Config::udp(0).enable_ipv6(false).max_assistant_sockets(1))
                .await
                .unwrap();
        let puncher = listener.puncher();
        puncher.apply_nat_model(NatType::Symmetric).unwrap();
        let sockets = puncher.udp_sockets();
        let target = loopback(sockets[1].local_addr().unwrap());
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(b"assistant", target).await.unwrap();
        let mut tunnel = listener.next().await.unwrap();
        assert_eq!(&tunnel.recv().await.unwrap()[..], b"assistant");

        puncher.apply_nat_model(NatType::Cone).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), tunnel.recv())
            .await
            .unwrap()
            .is_none());
    }
}
