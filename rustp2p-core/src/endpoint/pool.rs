use bytes::{Bytes, BytesMut};
use std::io::{self, IoSlice};
use std::net::SocketAddr;
use std::sync::{Arc, Weak};
#[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
use std::{mem, os::fd::AsRawFd};
#[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
use tokio::io::Interest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinSet;

use crate::endpoint::codec::InitCodec;
use crate::endpoint::tunnel::{Tunnel, UdpDispatcher};
use crate::route_table::{Protocol, RouteKey};

#[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
const UDP_BATCH_SIZE: usize = 16;

/// A managed assistant UDP socket entry with its own shutdown signal.
struct UdpEntry {
    socket: Arc<UdpSocket>,
    /// Per-socket shutdown sender for the assistant socket.
    _shutdown: broadcast::Sender<()>,
}

struct SocketState {
    main_udp_v4: Option<Arc<UdpSocket>>,
    main_udp_v6: Option<Arc<UdpSocket>>,
    assistant_udp: Vec<UdpEntry>,
    // Tracks the original Arcs, including retired assistants. It does not
    // track sockets extracted from an Arc or independently duplicated.
    udp_refs: Vec<Weak<UdpSocket>>,
    // None is a permanent shutdown. Task registration and socket creation
    // are serialized with taking this set.
    tasks: Option<JoinSet<()>>,
}

impl SocketState {
    fn udp_sockets(&self) -> impl Iterator<Item = &Arc<UdpSocket>> {
        self.main_udp_v4
            .iter()
            .chain(&self.main_udp_v6)
            .chain(self.assistant_udp.iter().map(|entry| &entry.socket))
    }
}

/// A shared pool of sockets. Retaining a Puncher does not keep sockets alive
/// after the endpoint shuts down.
pub struct SocketPool {
    state: parking_lot::RwLock<SocketState>,
    assistant_model_lock: parking_lot::Mutex<()>,
    accept_tx: mpsc::Sender<Tunnel>,
    udp_dispatcher: Arc<UdpDispatcher>,
    /// Global shutdown - kills ALL tasks (main + sub)
    global_shutdown: broadcast::Sender<()>,
    init_codec: Box<dyn InitCodec>,
    /// Maximum UDP datagram size; receive buffers are sized by this.
    max_udp_datagram_size: usize,
}

impl SocketPool {
    /// Create a pool from the main IPv4 UDP socket and an optional main IPv6
    /// UDP socket. New five-tuples are forwarded through `accept_tx`.
    pub fn new(
        main_udp_v4: UdpSocket,
        main_udp_v6: Option<UdpSocket>,
        accept_tx: mpsc::Sender<Tunnel>,
        init_codec: Box<dyn InitCodec>,
        max_udp_datagram_size: usize,
    ) -> Self {
        let (global_shutdown, _) = broadcast::channel(4);
        let udp_dispatcher = UdpDispatcher::new();

        let main_udp_v4 = Arc::new(main_udp_v4);
        let main_udp_v6 = main_udp_v6.map(Arc::new);
        let mut tasks = JoinSet::new();
        let mut udp_refs = Vec::new();
        for socket in std::iter::once(&main_udp_v4).chain(main_udp_v6.iter()) {
            udp_refs.push(Arc::downgrade(socket));
            Self::spawn_udp_tasks(
                &mut tasks,
                socket.clone(),
                &accept_tx,
                &udp_dispatcher,
                &global_shutdown,
                max_udp_datagram_size,
            );
        }

        Self {
            state: parking_lot::RwLock::new(SocketState {
                main_udp_v4: Some(main_udp_v4),
                main_udp_v6,
                assistant_udp: Vec::new(),
                udp_refs,
                tasks: Some(tasks),
            }),
            assistant_model_lock: parking_lot::Mutex::new(()),
            accept_tx,
            udp_dispatcher,
            global_shutdown,
            init_codec,
            max_udp_datagram_size,
        }
    }

    /// Spawn the read and write tasks for one UDP socket.
    fn spawn_udp_tasks(
        tasks: &mut JoinSet<()>,
        socket: Arc<UdpSocket>,
        accept_tx: &mpsc::Sender<Tunnel>,
        udp_dispatcher: &Arc<UdpDispatcher>,
        global_shutdown: &broadcast::Sender<()>,
        max_udp_datagram_size: usize,
    ) {
        let (write_tx, write_rx) = mpsc::channel::<(Bytes, SocketAddr)>(64);

        let mut global_shutdown_rx = global_shutdown.subscribe();
        let mut socket_shutdown_rx = global_shutdown.subscribe();
        let accept_tx = accept_tx.clone();
        let udp_dispatcher = udp_dispatcher.clone();
        let s = socket.clone();
        tasks.spawn(async move {
            Self::run_udp_reader(
                s,
                write_tx,
                accept_tx,
                udp_dispatcher,
                &mut global_shutdown_rx,
                &mut socket_shutdown_rx,
                max_udp_datagram_size,
            )
            .await;
        });

        let mut global_shutdown_rx = global_shutdown.subscribe();
        let mut socket_shutdown_rx = global_shutdown.subscribe();
        let s = socket.clone();
        tasks.spawn(async move {
            Self::run_udp_writer(
                s,
                write_rx,
                &mut global_shutdown_rx,
                &mut socket_shutdown_rx,
            )
            .await;
        });
    }

    /// Add an assistant UDP socket (for symmetric NAT probing).
    /// Its reader task exits when the assistant socket is removed.
    pub fn add_assistant_udp(&self, socket: UdpSocket) -> io::Result<()> {
        let mut state = self.state.write();
        let tasks = state.tasks.as_mut().ok_or_else(Self::closed)?;
        Self::reap_finished(tasks);
        let socket = Arc::new(socket);
        let weak = Arc::downgrade(&socket);

        // Per-socket shutdown for this assistant socket
        let (socket_shutdown, mut socket_shutdown_rx) = broadcast::channel(4);
        let (write_tx, write_rx) = mpsc::channel::<(Bytes, SocketAddr)>(64);
        let accept_tx = self.accept_tx.clone();
        let udp_dispatcher = self.udp_dispatcher.clone();
        let mut global_shutdown_rx = self.global_shutdown.subscribe();
        let s = socket.clone();
        let max_udp_datagram_size = self.max_udp_datagram_size;

        tasks.spawn(async move {
            Self::run_udp_reader(
                s,
                write_tx,
                accept_tx,
                udp_dispatcher,
                &mut global_shutdown_rx,
                &mut socket_shutdown_rx,
                max_udp_datagram_size,
            )
            .await;
        });

        let mut writer_shutdown_rx = socket_shutdown.subscribe();
        let mut writer_global_shutdown_rx = self.global_shutdown.subscribe();
        let s = socket.clone();
        tasks.spawn(async move {
            Self::run_udp_writer(
                s,
                write_rx,
                &mut writer_global_shutdown_rx,
                &mut writer_shutdown_rx,
            )
            .await;
        });

        let entry = UdpEntry {
            socket,
            _shutdown: socket_shutdown,
        };

        state.assistant_udp.push(entry);
        state.udp_refs.retain(|socket| socket.strong_count() != 0);
        state.udp_refs.push(weak);
        Ok(())
    }

    /// Drop the per-socket shutdown senders to stop assistant tasks.
    pub fn clean_assistant_udp(&self) {
        self.state.write().assistant_udp.clear();
    }

    /// Locks a complete assistant-socket model transition across all Puncher
    /// handles backed by this pool.
    pub(crate) fn lock_assistant_model(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.assistant_model_lock.lock()
    }

    /// Build one TCP tunnel with independent framing and lifetime.
    fn tcp_tunnel(
        &self,
        stream: tokio::net::TcpStream,
        peer_addr: SocketAddr,
    ) -> io::Result<Tunnel> {
        // A dual-stack listener reports IPv4 peers as IPv4-mapped `::ffff:`
        // addresses. A TCP connection's peer address is fixed for its whole
        // life, so canonicalizing once at the tunnel boundary keeps the route
        // key and `remote_addr` consistent with the view of a real IPv4
        // socket (and with the outbound punch path).
        let peer_addr = SocketAddr::new(peer_addr.ip().to_canonical(), peer_addr.port());
        let (read_half, mut write_half) = stream.into_split();
        let local_addr = read_half
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        let (mut decoder, encoder) = self.init_codec.codec(peer_addr)?;
        let mut state = self.state.write();
        let tasks = state.tasks.as_mut().ok_or_else(Self::closed)?;
        Self::reap_finished(tasks);
        let (write_tx, mut write_rx) = mpsc::channel::<Bytes>(64);
        let (data_tx, data_rx) = mpsc::channel(64);
        let (read_shutdown, _) = broadcast::channel(1);
        let mut read_global_shutdown = self.global_shutdown.subscribe();
        let mut read_tunnel_shutdown = read_shutdown.subscribe();

        // Read loop using Decoder
        tasks.spawn(async move {
            let mut read = read_half;
            let mut buf = BytesMut::with_capacity(64 * 1024);
            'read: loop {
                tokio::select! {
                    result = read.read_buf(&mut buf) => {
                        match result {
                            Ok(0) => break,
                            Ok(_) => {
                                loop {
                                    match decoder.decode(&mut buf) {
                                        Ok(Some(data)) => {
                                            if data_tx.send(data).await.is_err() {
                                                break 'read;
                                            }
                                        }
                                        Ok(None) => break,
                                        Err(e) => {
                                            log::warn!("TCP decode error: {e}");
                                            break 'read;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!("TCP read error: {e}");
                                break;
                            }
                        }
                    }
                    _ = read_global_shutdown.recv() => {
                        log::debug!("TCP read task shutting down (global)");
                        break;
                    }
                    _ = read_tunnel_shutdown.recv() => {
                        log::debug!("TCP read task shutting down (tunnel)");
                        break;
                    }
                }
            }
        });

        // Write loop using Encoder
        let mut write_global_shutdown = self.global_shutdown.subscribe();
        tasks.spawn(async move {
            let mut encoder = encoder;
            loop {
                tokio::select! {
                    data = write_rx.recv() => {
                        match data {
                            Some(data) => {
                                // `iov` borrows `encoder` and `data`, so it is
                                // scoped to a single frame.
                                let mut iov = Vec::with_capacity(2);
                                let result = match encoder.encode(&data, &mut iov) {
                                    Ok(()) => write_all_vectored(&mut write_half, &mut iov).await,
                                    Err(e) => Err(e),
                                };
                                if let Err(e) = result {
                                    log::warn!("TCP encode error: {e}");
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    _ = write_global_shutdown.recv() => {
                        log::debug!("TCP write task shutting down (global)");
                        break;
                    }
                }
            }
        });

        Ok(Tunnel::tcp(
            write_tx,
            RouteKey::new(Protocol::TCP, local_addr, peer_addr),
            data_rx,
            read_shutdown,
        ))
    }

    /// Publish an accepted or punched TCP connection to `TunnelIncoming::next`.
    pub async fn publish_tcp(
        &self,
        stream: tokio::net::TcpStream,
        peer_addr: SocketAddr,
        initial_data: Option<Bytes>,
    ) -> io::Result<()> {
        let tunnel = self.tcp_tunnel(stream, peer_addr)?;
        if let Some(data) = initial_data {
            tunnel.send(data).await?;
        }
        self.accept_tx
            .send(tunnel)
            .await
            .map_err(|_| io::Error::other("tunnel incoming source closed"))
    }

    /// Send data through all assistant UDP sockets (IPv4 only) to a specific
    /// address. Does nothing for IPv6 targets.
    pub fn try_send_via_assistants(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        if addr.is_ipv6() {
            return Ok(());
        }
        let state = self.state.read();
        for entry in &state.assistant_udp {
            // Best effort per socket, mirroring try_send_via_all: a transient
            // failure on one assistant must not skip the remaining ones,
            // punch probes need the full coverage.
            if let Err(e) = entry.socket.try_send_to(buf, addr) {
                log::debug!("assistant send to {addr} failed: {e}");
            }
        }
        Ok(())
    }

    /// Send data to an address via the matching family main UDP socket.
    pub fn send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        let state = self.state.read();
        let v4 = state.main_udp_v4.as_ref().ok_or_else(Self::closed)?;
        let socket = if addr.is_ipv4() {
            v4
        } else {
            state
                .main_udp_v6
                .as_ref()
                .ok_or_else(|| io::Error::other("IPv6 main socket not available"))?
        };
        socket
            .try_send_to(buf, addr)
            .map(|_| ())
            .map_err(|e| io::Error::other(format!("send failed: {e}")))
    }

    /// Best-effort send through the matching main UDP socket and assistants.
    pub fn try_send_via_all(&self, buf: &[u8], addr: SocketAddr) {
        let state = self.state.read();
        if addr.is_ipv4() {
            if let Some(v4) = &state.main_udp_v4 {
                let _ = v4.try_send_to(buf, addr);
            }
            for entry in &state.assistant_udp {
                let _ = entry.socket.try_send_to(buf, addr);
            }
        } else if let Some(v6) = &state.main_udp_v6 {
            let _ = v6.try_send_to(buf, addr);
        }
    }

    fn closed() -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, "tunnel endpoint is closed")
    }

    pub(crate) fn ensure_open(&self) -> io::Result<()> {
        self.state
            .read()
            .tasks
            .as_ref()
            .map(|_| ())
            .ok_or_else(Self::closed)
    }

    pub(crate) fn subscribe_shutdown(&self) -> io::Result<broadcast::Receiver<()>> {
        // Subscribe while holding the same lock used to publish shutdown.
        let state = self.state.read();
        state.tasks.as_ref().ok_or_else(Self::closed)?;
        Ok(self.global_shutdown.subscribe())
    }

    /// Register an outgoing TCP attempt so close can abort and join it,
    /// including time spent connecting or waiting to publish the tunnel.
    pub(crate) async fn run_tcp_attempt(
        &self,
        attempt: impl std::future::Future<Output = io::Result<()>> + Send + 'static,
    ) -> io::Result<()> {
        let (mut result_tx, result_rx) = oneshot::channel();
        {
            let mut state = self.state.write();
            let tasks = state.tasks.as_mut().ok_or_else(Self::closed)?;
            Self::reap_finished(tasks);
            tasks.spawn(async move {
                tokio::select! {
                    biased;
                    // Also stop when the caller drops or times out the attempt.
                    _ = result_tx.closed() => {}
                    result = attempt => { let _ = result_tx.send(result); }
                }
            });
        }
        result_rx.await.map_err(|_| Self::closed())?
    }

    fn reap_finished(tasks: &mut JoinSet<()>) {
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                log::debug!("endpoint worker exited: {error}");
            }
        }
    }

    pub(crate) fn spawn_listener(
        &self,
        worker: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        if let Some(tasks) = self.state.write().tasks.as_mut() {
            tasks.spawn(worker);
        }
    }

    /// Prevent new tasks, drop the pool's socket references and abort workers.
    /// Dropping the returned set is sufficient for non-blocking Drop.
    fn begin_shutdown(&self) -> Option<(JoinSet<()>, Vec<Weak<UdpSocket>>)> {
        let _model_guard = self.lock_assistant_model();
        let mut state = self.state.write();
        let mut tasks = state.tasks.take()?;
        let _ = self.global_shutdown.send(());
        tasks.abort_all();
        state.main_udp_v4 = None;
        state.main_udp_v6 = None;
        state.assistant_udp.clear();
        Some((tasks, std::mem::take(&mut state.udp_refs)))
    }

    pub(crate) fn shutdown(&self) {
        if self.begin_shutdown().is_some() {
            self.udp_dispatcher.clear();
        }
    }

    pub(crate) async fn close(&self) -> io::Result<()> {
        let Some((mut tasks, refs)) = self.begin_shutdown() else {
            return Ok(());
        };
        // Clear on cancellation as well as on successful completion.
        struct ClearRoutes<'a>(&'a UdpDispatcher);
        impl Drop for ClearRoutes<'_> {
            fn drop(&mut self) {
                self.0.clear();
            }
        }
        let _clear = ClearRoutes(&self.udp_dispatcher);
        tasks.shutdown().await;
        if refs.iter().any(|socket| socket.strong_count() != 0) {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "endpoint closed, but a shared Arc<UdpSocket> still holds a port",
            ));
        }
        Ok(())
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.state
            .read()
            .main_udp_v4
            .as_ref()
            .ok_or_else(Self::closed)?
            .local_addr()
    }

    /// Get all UDP sockets: main IPv4, main IPv6 (if present), then assistants.
    pub fn udp_sockets(&self) -> Vec<Arc<UdpSocket>> {
        self.state.read().udp_sockets().cloned().collect()
    }

    pub fn local_udp_ports(&self) -> Vec<u16> {
        self.state
            .read()
            .udp_sockets()
            .filter_map(|socket| socket.local_addr().ok().map(|addr| addr.port()))
            .collect()
    }

    pub fn assistant_count(&self) -> usize {
        self.state.read().assistant_udp.len()
    }

    async fn run_udp_reader(
        socket: Arc<UdpSocket>,
        write_tx: mpsc::Sender<(Bytes, SocketAddr)>,
        accept_tx: mpsc::Sender<Tunnel>,
        udp_dispatcher: Arc<UdpDispatcher>,
        global_shutdown_rx: &mut broadcast::Receiver<()>,
        socket_shutdown_rx: &mut broadcast::Receiver<()>,
        max_udp_datagram_size: usize,
    ) {
        let local_addr = socket
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        #[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
        {
            Self::run_udp_reader_mmsg(
                socket,
                write_tx,
                accept_tx,
                udp_dispatcher.clone(),
                global_shutdown_rx,
                socket_shutdown_rx,
                max_udp_datagram_size,
            )
            .await;
        }
        #[cfg(not(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android"))))]
        {
            Self::run_udp_reader_single(
                socket,
                write_tx,
                accept_tx,
                udp_dispatcher.clone(),
                global_shutdown_rx,
                socket_shutdown_rx,
                max_udp_datagram_size,
            )
            .await;
        }
        udp_dispatcher.unregister_local(local_addr);
    }

    #[cfg(not(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android"))))]
    async fn run_udp_reader_single(
        socket: Arc<UdpSocket>,
        write_tx: mpsc::Sender<(Bytes, SocketAddr)>,
        accept_tx: mpsc::Sender<Tunnel>,
        udp_dispatcher: Arc<UdpDispatcher>,
        global_shutdown_rx: &mut broadcast::Receiver<()>,
        socket_shutdown_rx: &mut broadcast::Receiver<()>,
        max_udp_datagram_size: usize,
    ) {
        let mut buf = vec![0u8; max_udp_datagram_size];
        let local_addr = socket
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, addr)) => {
                            let data = BytesMut::from(&buf[..len]);
                            let route_key = RouteKey::new(Protocol::UDP, local_addr, addr);
                            if !udp_dispatcher.dispatch(&accept_tx, &write_tx, route_key, data) {
                                return;
                            }
                        }
                        Err(e) if is_recoverable_udp_recv_error(&e) => {
                            log::debug!("Ignoring recoverable UDP recv error: {e}");
                            continue;
                        }
                        Err(e) => {
                            log::warn!("UDP recv error: {e}");
                            break;
                        }
                    }
                }
                _ = global_shutdown_rx.recv() => {
                    log::debug!("UDP read task shutting down (global)");
                    break;
                }
                _ = socket_shutdown_rx.recv() => {
                    log::debug!("UDP read task shutting down (socket)");
                    break;
                }
            }
        }
    }

    #[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
    async fn run_udp_reader_mmsg(
        socket: Arc<UdpSocket>,
        write_tx: mpsc::Sender<(Bytes, SocketAddr)>,
        accept_tx: mpsc::Sender<Tunnel>,
        udp_dispatcher: Arc<UdpDispatcher>,
        global_shutdown_rx: &mut broadcast::Receiver<()>,
        socket_shutdown_rx: &mut broadcast::Receiver<()>,
        max_udp_datagram_size: usize,
    ) {
        let mut buffers = (0..UDP_BATCH_SIZE)
            .map(|_| Vec::with_capacity(max_udp_datagram_size))
            .collect::<Vec<_>>();
        let mut peer_addrs = [SocketAddr::from(([0, 0, 0, 0], 0)); UDP_BATCH_SIZE];
        let local_addr = socket
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        let fd = socket.as_raw_fd();

        loop {
            let count = tokio::select! {
                result = socket.async_io(Interest::READABLE, || {
                    recv_mmsg(fd, &mut buffers, &mut peer_addrs)
                }) => {
                    match result {
                        Ok(count) => count,
                        Err(e) if is_recoverable_udp_recv_error(&e) => {
                            log::debug!("Ignoring recoverable UDP recvmmsg error: {e}");
                            continue;
                        }
                        Err(e) => {
                            log::warn!("UDP recvmmsg error: {e}");
                            break;
                        }
                    }
                }
                _ = global_shutdown_rx.recv() => {
                    log::debug!("UDP read task shutting down (global)");
                    break;
                }
                _ = socket_shutdown_rx.recv() => {
                    log::debug!("UDP read task shutting down (socket)");
                    break;
                }
            };

            for index in 0..count {
                let data = BytesMut::from(&buffers[index][..]);
                let route_key = RouteKey::new(Protocol::UDP, local_addr, peer_addrs[index]);
                if !udp_dispatcher.dispatch(&accept_tx, &write_tx, route_key, data) {
                    return;
                }
            }
        }
    }

    /// Writer task for a UDP socket: drains `(data, peer)` messages from the
    /// channel and sends them through the socket. Exits when the channel
    /// closes or either shutdown signal fires.
    async fn run_udp_writer(
        socket: Arc<UdpSocket>,
        write_rx: mpsc::Receiver<(Bytes, SocketAddr)>,
        global_shutdown_rx: &mut broadcast::Receiver<()>,
        socket_shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        #[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
        {
            Self::run_udp_writer_mmsg(socket, write_rx, global_shutdown_rx, socket_shutdown_rx)
                .await;
        }
        #[cfg(not(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android"))))]
        {
            Self::run_udp_writer_single(socket, write_rx, global_shutdown_rx, socket_shutdown_rx)
                .await;
        }
    }

    #[cfg(not(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android"))))]
    async fn run_udp_writer_single(
        socket: Arc<UdpSocket>,
        mut write_rx: mpsc::Receiver<(Bytes, SocketAddr)>,
        global_shutdown_rx: &mut broadcast::Receiver<()>,
        socket_shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                msg = write_rx.recv() => {
                    match msg {
                        Some((data, addr)) => {
                            if let Err(e) = socket.send_to(&data, addr).await {
                                log::warn!("UDP send error: {e}");
                            }
                        }
                        None => break,
                    }
                }
                _ = global_shutdown_rx.recv() => {
                    log::debug!("UDP write task shutting down (global)");
                    break;
                }
                _ = socket_shutdown_rx.recv() => {
                    log::debug!("UDP write task shutting down (socket)");
                    break;
                }
            }
        }
        // Keep socket alive until writer exits
        drop(socket);
    }

    #[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
    async fn run_udp_writer_mmsg(
        socket: Arc<UdpSocket>,
        mut write_rx: mpsc::Receiver<(Bytes, SocketAddr)>,
        global_shutdown_rx: &mut broadcast::Receiver<()>,
        socket_shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        let fd = socket.as_raw_fd();
        let mut batch = Vec::with_capacity(UDP_BATCH_SIZE);

        'writer: loop {
            batch.clear();
            let first = tokio::select! {
                msg = write_rx.recv() => msg,
                _ = global_shutdown_rx.recv() => {
                    log::debug!("UDP write task shutting down (global)");
                    break;
                }
                _ = socket_shutdown_rx.recv() => {
                    log::debug!("UDP write task shutting down (socket)");
                    break;
                }
            };
            let Some(first) = first else {
                break;
            };
            batch.push(first);
            while batch.len() < UDP_BATCH_SIZE {
                match write_rx.try_recv() {
                    Ok(message) => batch.push(message),
                    Err(_) => break,
                }
            }

            let mut sent = 0;
            while sent < batch.len() {
                if batch.len() - sent == 1 {
                    let (data, addr) = &batch[sent];
                    let result = tokio::select! {
                        result = socket.send_to(data, *addr) => Some(result),
                        _ = global_shutdown_rx.recv() => {
                            log::debug!("UDP write task shutting down (global)");
                            None
                        }
                        _ = socket_shutdown_rx.recv() => {
                            log::debug!("UDP write task shutting down (socket)");
                            None
                        }
                    };
                    let Some(result) = result else {
                        break 'writer;
                    };
                    if let Err(e) = result {
                        log::warn!("UDP send error: {e}");
                    }
                    sent += 1;
                    continue;
                }

                let result = tokio::select! {
                    result = socket.async_io(Interest::WRITABLE, || {
                        send_mmsg(fd, &batch[sent..])
                    }) => Some(result),
                    _ = global_shutdown_rx.recv() => {
                        log::debug!("UDP write task shutting down (global)");
                        None
                    }
                    _ = socket_shutdown_rx.recv() => {
                        log::debug!("UDP write task shutting down (socket)");
                        None
                    }
                };
                let Some(result) = result else {
                    break 'writer;
                };
                match result {
                    Ok(count) => sent += count,
                    Err(batch_error) => {
                        // sendmmsg returns an error only if it sent no messages.
                        // Retry the first message separately so one bad datagram
                        // does not cause the rest of the batch to be discarded.
                        let (data, addr) = &batch[sent];
                        let result = tokio::select! {
                            result = socket.send_to(data, *addr) => Some(result),
                            _ = global_shutdown_rx.recv() => {
                                log::debug!("UDP write task shutting down (global)");
                                None
                            }
                            _ = socket_shutdown_rx.recv() => {
                                log::debug!("UDP write task shutting down (socket)");
                                None
                            }
                        };
                        let Some(result) = result else {
                            break 'writer;
                        };
                        if let Err(e) = result {
                            log::warn!(
                                "UDP sendmmsg error: {batch_error}; single send to {addr} failed: {e}"
                            );
                        } else {
                            log::warn!(
                                "UDP sendmmsg error: {batch_error}; retried {addr} successfully"
                            );
                        }
                        sent += 1;
                    }
                }
            }
        }
        drop(socket);
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn is_recoverable_udp_recv_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Interrupted
        || matches!(
            error.raw_os_error(),
            Some(
                libc::ECONNREFUSED
                    | libc::ECONNRESET
                    | libc::ECONNABORTED
                    | libc::EHOSTUNREACH
                    | libc::ENETUNREACH
                    | libc::EADDRNOTAVAIL
                    | libc::EPROTO
                    | libc::EMSGSIZE
            )
        )
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn is_recoverable_udp_recv_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Interrupted
}

#[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
fn recv_mmsg(
    fd: std::os::fd::RawFd,
    buffers: &mut [Vec<u8>],
    peer_addrs: &mut [SocketAddr],
) -> io::Result<usize> {
    let count = buffers.len().min(peer_addrs.len()).min(UDP_BATCH_SIZE);
    if count == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "recvmmsg requires at least one buffer",
        ));
    }

    let mut iovecs: [libc::iovec; UDP_BATCH_SIZE] = unsafe { mem::zeroed() };
    let mut messages: [libc::mmsghdr; UDP_BATCH_SIZE] = unsafe { mem::zeroed() };
    let mut addresses: [libc::sockaddr_storage; UDP_BATCH_SIZE] = unsafe { mem::zeroed() };

    for index in 0..count {
        buffers[index].clear();
        iovecs[index].iov_base = buffers[index].as_mut_ptr().cast();
        iovecs[index].iov_len = buffers[index].capacity();
        messages[index].msg_hdr.msg_iov = &mut iovecs[index];
        messages[index].msg_hdr.msg_iovlen = 1;
        messages[index].msg_hdr.msg_name =
            (&mut addresses[index] as *mut libc::sockaddr_storage).cast::<libc::c_void>();
        messages[index].msg_hdr.msg_namelen =
            mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    }

    let received = loop {
        let result = unsafe {
            libc::recvmmsg(
                fd,
                messages.as_mut_ptr(),
                count as libc::c_uint,
                libc::MSG_DONTWAIT as _,
                std::ptr::null_mut(),
            )
        };
        if result >= 0 {
            break result as usize;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    if received == 0 {
        return Err(io::Error::from(io::ErrorKind::WouldBlock));
    }

    for index in 0..received {
        peer_addrs[index] =
            sockaddr_to_socket_addr(&addresses[index], messages[index].msg_hdr.msg_namelen)?;
        let length = (messages[index].msg_len as usize).min(buffers[index].capacity());
        // SAFETY: recvmmsg initialized exactly `msg_len` bytes in this
        // buffer, capped above by the iovec capacity supplied to the kernel.
        unsafe {
            buffers[index].set_len(length);
        }
    }
    Ok(received)
}

#[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
fn send_mmsg(fd: std::os::fd::RawFd, buffers: &[(Bytes, SocketAddr)]) -> io::Result<usize> {
    let count = buffers.len().min(UDP_BATCH_SIZE);
    if count == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sendmmsg requires at least one message",
        ));
    }

    let mut iovecs: [libc::iovec; UDP_BATCH_SIZE] = unsafe { mem::zeroed() };
    let mut messages: [libc::mmsghdr; UDP_BATCH_SIZE] = unsafe { mem::zeroed() };
    let mut addresses: [libc::sockaddr_storage; UDP_BATCH_SIZE] = unsafe { mem::zeroed() };

    for (index, (buffer, addr)) in buffers[..count].iter().enumerate() {
        let (storage, addr_len) = socket_addr_to_sockaddr(addr);
        addresses[index] = storage;
        iovecs[index].iov_base = buffer.as_ptr() as *mut libc::c_void;
        iovecs[index].iov_len = buffer.len();
        messages[index].msg_hdr.msg_iov = &mut iovecs[index];
        messages[index].msg_hdr.msg_iovlen = 1;
        messages[index].msg_hdr.msg_name =
            (&mut addresses[index] as *mut libc::sockaddr_storage).cast::<libc::c_void>();
        messages[index].msg_hdr.msg_namelen = addr_len;
    }

    loop {
        let sent = unsafe {
            libc::sendmmsg(
                fd,
                messages.as_mut_ptr(),
                count as libc::c_uint,
                libc::MSG_DONTWAIT as _,
            )
        };
        if sent > 0 {
            return Ok(sent as usize);
        }
        if sent == 0 {
            return Err(io::Error::from(io::ErrorKind::WriteZero));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
fn socket_addr_to_sockaddr(addr: &SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    match addr {
        SocketAddr::V4(addr) => {
            let raw = (&mut storage as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in>();
            unsafe {
                (*raw).sin_family = libc::AF_INET as libc::sa_family_t;
                (*raw).sin_port = addr.port().to_be();
                (*raw).sin_addr.s_addr = u32::from_ne_bytes(addr.ip().octets());
            }
            (
                storage,
                mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        }
        SocketAddr::V6(addr) => {
            let raw = (&mut storage as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in6>();
            unsafe {
                (*raw).sin6_family = libc::AF_INET6 as libc::sa_family_t;
                (*raw).sin6_port = addr.port().to_be();
                (*raw).sin6_flowinfo = addr.flowinfo();
                (*raw).sin6_addr.s6_addr = addr.ip().octets();
                (*raw).sin6_scope_id = addr.scope_id();
            }
            (
                storage,
                mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
            )
        }
    }
}

#[cfg(all(feature = "sendmmsg", any(target_os = "linux", target_os = "android")))]
fn sockaddr_to_socket_addr(
    storage: &libc::sockaddr_storage,
    len: libc::socklen_t,
) -> io::Result<SocketAddr> {
    match storage.ss_family as libc::c_int {
        libc::AF_INET if (len as usize) >= mem::size_of::<libc::sockaddr_in>() => {
            let addr = unsafe { &*(storage as *const _ as *const libc::sockaddr_in) };
            Ok(SocketAddr::V4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr)),
                u16::from_be(addr.sin_port),
            )))
        }
        libc::AF_INET6 if (len as usize) >= mem::size_of::<libc::sockaddr_in6>() => {
            let addr = unsafe { &*(storage as *const _ as *const libc::sockaddr_in6) };
            Ok(SocketAddr::V6(std::net::SocketAddrV6::new(
                std::net::Ipv6Addr::from(addr.sin6_addr.s6_addr),
                u16::from_be(addr.sin6_port),
                addr.sin6_flowinfo,
                addr.sin6_scope_id,
            )))
        }
        family => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported sockaddr family or length: family={family}, len={len}"),
        )),
    }
}

/// Write all of `iov` with vectored writes, advancing across partial writes.
async fn write_all_vectored(write: &mut OwnedWriteHalf, iov: &mut [IoSlice<'_>]) -> io::Result<()> {
    let mut slices = iov;
    while !slices.is_empty() {
        match write.write_vectored(slices).await {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write whole frame",
                ));
            }
            Ok(n) => IoSlice::advance_slices(&mut slices, n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(all(
    test,
    feature = "sendmmsg",
    any(target_os = "linux", target_os = "android")
))]
mod mmsg_tests {
    use super::{
        is_recoverable_udp_recv_error, recv_mmsg, send_mmsg, sockaddr_to_socket_addr,
        socket_addr_to_sockaddr, Interest, SocketAddr, UdpSocket, UDP_BATCH_SIZE,
    };
    use bytes::Bytes;
    use std::io;
    use std::net::{Ipv6Addr, SocketAddrV6};
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    /// Test receive buffer size; the messages sent here are tiny.
    const TEST_RECV_BUFFER_SIZE: usize = 2048;

    #[test]
    fn sockaddr_round_trip_preserves_ipv4_and_ipv6_fields() -> io::Result<()> {
        let addresses = [
            SocketAddr::from(([127, 0, 0, 1], 32123)),
            SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 32124, 0x12345, 7)),
        ];

        for expected in addresses {
            let (storage, len) = socket_addr_to_sockaddr(&expected);
            if let SocketAddr::V6(expected) = expected {
                let raw = unsafe {
                    &*(&storage as *const libc::sockaddr_storage as *const libc::sockaddr_in6)
                };
                assert_eq!(raw.sin6_flowinfo, expected.flowinfo());
            }
            assert_eq!(sockaddr_to_socket_addr(&storage, len)?, expected);
        }
        Ok(())
    }

    #[test]
    fn classifies_async_network_errors_as_recoverable() {
        for code in [
            libc::ECONNREFUSED,
            libc::ECONNRESET,
            libc::EHOSTUNREACH,
            libc::ENETUNREACH,
            libc::EMSGSIZE,
        ] {
            assert!(is_recoverable_udp_recv_error(
                &io::Error::from_raw_os_error(code)
            ));
        }
        assert!(!is_recoverable_udp_recv_error(
            &io::Error::from_raw_os_error(libc::EBADF)
        ));
    }

    #[tokio::test]
    async fn sendmmsg_and_recvmmsg_round_trip_batch() -> io::Result<()> {
        let receiver = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let sender = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let receiver_addr = receiver.local_addr()?;
        let sender_addr = sender.local_addr()?;
        let messages = [
            (Bytes::from_static(b"first"), receiver_addr),
            (Bytes::new(), receiver_addr),
            (Bytes::from_static(b"third"), receiver_addr),
        ];

        let mut sent = 0;
        while sent < messages.len() {
            sent += sender
                .async_io(Interest::WRITABLE, || {
                    send_mmsg(sender.as_raw_fd(), &messages[sent..])
                })
                .await?;
        }

        let received = tokio::time::timeout(Duration::from_secs(1), async {
            let mut buffers = (0..UDP_BATCH_SIZE)
                .map(|_| Vec::with_capacity(TEST_RECV_BUFFER_SIZE))
                .collect::<Vec<_>>();
            let mut peer_addrs = [SocketAddr::from(([0, 0, 0, 0], 0)); UDP_BATCH_SIZE];
            let mut received = Vec::new();

            while received.len() < messages.len() {
                let count = receiver
                    .async_io(Interest::READABLE, || {
                        recv_mmsg(receiver.as_raw_fd(), &mut buffers, &mut peer_addrs)
                    })
                    .await?;
                for index in 0..count {
                    received.push((Bytes::copy_from_slice(&buffers[index]), peer_addrs[index]));
                }
            }
            Ok::<_, io::Error>(received)
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "mmsg round trip timed out"))??;

        assert_eq!(
            received,
            vec![
                (Bytes::from_static(b"first"), sender_addr),
                (Bytes::new(), sender_addr),
                (Bytes::from_static(b"third"), sender_addr),
            ]
        );
        Ok(())
    }
}
