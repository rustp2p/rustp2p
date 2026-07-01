use bytes::Bytes;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Weak};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc, RwLock};

use crate::endpoint::codec::InitCodec;

/// Socket role in the pool.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum SocketRole {
    Main,
    Assistant,
}

/// A managed UDP socket entry with its own shutdown signal.
struct UdpEntry {
    socket: Arc<UdpSocket>,
    role: SocketRole,
    /// Per-socket shutdown sender for assistant sockets. None for main socket.
    _shutdown: Option<broadcast::Sender<()>>,
}

/// A TCP connection with Encoder for writing.
pub struct TcpConnection {
    pub peer_addr: SocketAddr,
    write_tx: mpsc::Sender<Vec<u8>>,
}

impl TcpConnection {
    pub async fn send(&self, data: &[u8]) -> io::Result<()> {
        self.write_tx
            .send(data.to_vec())
            .await
            .map_err(|_| io::Error::other("TCP connection closed"))
    }
}

/// A shared pool of sockets. Owns all Arcs.
pub struct SocketPool {
    udp_sockets: RwLock<Vec<UdpEntry>>,
    tcp_conns: RwLock<HashMap<SocketAddr, Arc<TcpConnection>>>,
    data_tx: mpsc::Sender<(super::transport::Transport, Bytes)>,
    /// Global shutdown - kills ALL tasks (main + sub)
    global_shutdown: broadcast::Sender<()>,
    init_codec: Box<dyn InitCodec>,
    connect_lock: tokio::sync::Mutex<()>,
}

impl SocketPool {
    /// Create a pool from a UDP socket.
    pub fn new(
        socket: UdpSocket,
        init_codec: Box<dyn InitCodec>,
    ) -> (Self, mpsc::Receiver<(super::transport::Transport, Bytes)>) {
        let (data_tx, data_rx) = mpsc::channel(512);
        let (global_shutdown, _) = broadcast::channel(4);
        let socket = Arc::new(socket);

        let mut shutdown_rx = global_shutdown.subscribe();
        let socket_weak = Arc::downgrade(&socket);
        let data_tx_clone = data_tx.clone();
        let s = socket.clone();
        tokio::spawn(async move {
            Self::run_udp_reader(s, socket_weak, data_tx_clone, &mut shutdown_rx).await;
        });

        let entry = UdpEntry {
            socket,
            role: SocketRole::Main,
            _shutdown: None,
        };

        let pool = Self {
            udp_sockets: RwLock::new(vec![entry]),
            tcp_conns: RwLock::new(HashMap::new()),
            data_tx,
            global_shutdown,
            init_codec,
            connect_lock: tokio::sync::Mutex::new(()),
        };
        (pool, data_rx)
    }

    /// Add an assistant UDP socket (for symmetric NAT probing).
    /// Its reader task exits when the assistant socket is removed.
    pub async fn add_assistant_udp(&self, socket: UdpSocket) -> Weak<UdpSocket> {
        let socket = Arc::new(socket);
        let weak = Arc::downgrade(&socket);
        let socket_weak = weak.clone();

        // Per-socket shutdown for this assistant socket
        let (socket_shutdown, mut socket_shutdown_rx) = broadcast::channel(4);
        let data_tx = self.data_tx.clone();
        let s = socket.clone();

        tokio::spawn(async move {
            Self::run_udp_reader(s, socket_weak, data_tx, &mut socket_shutdown_rx).await;
        });

        let entry = UdpEntry {
            socket,
            role: SocketRole::Assistant,
            _shutdown: Some(socket_shutdown),
        };

        let mut sockets = self.udp_sockets.write().await;
        sockets.push(entry);
        drop(sockets);

        weak
    }

    /// Clean all assistant UDP sockets and cancel their reader tasks.
    pub fn clean_assistant_udp(&self) {
        let mut sockets = self.udp_sockets.blocking_write();
        // Dropping UdpEntry drops _shutdown Sender, reader task exits
        sockets.retain(|e| e.role == SocketRole::Main);
    }

    /// Remove a TCP connection from the pool by peer address.
    pub(crate) async fn remove_tcp(&self, addr: SocketAddr) {
        self.tcp_conns.write().await.remove(&addr);
    }

    /// Add a TCP connection with Decoder/Encoder.
    pub async fn add_tcp(
        self: &Arc<Self>,
        stream: tokio::net::TcpStream,
        peer_addr: SocketAddr,
    ) -> io::Result<Weak<TcpConnection>> {
        let (read_half, mut write_half) = stream.into_split();
        let (mut decoder, _encoder) = self.init_codec.codec(peer_addr)?;
        let (write_tx, mut write_rx) = mpsc::channel::<Vec<u8>>(64);
        let data_tx = self.data_tx.clone();
        let mut shutdown_rx = self.global_shutdown.subscribe();

        // Create Arc<TcpConnection> first so we can get a real Weak reference
        let conn = Arc::new(TcpConnection {
            peer_addr,
            write_tx,
        });
        let conn_weak = Arc::downgrade(&conn);

        // Read loop using Decoder
        let pool_for_read = self.clone();
        let conn_weak_for_read = conn_weak.clone();
        tokio::spawn(async move {
            let mut read = read_half;
            let mut data_buf = vec![0u8; 65536];
            loop {
                tokio::select! {
                    result = decoder.decode(&mut read, &mut data_buf) => {
                        match result {
                            Ok(len) => {
                                let data = Bytes::copy_from_slice(&data_buf[..len]);
                                let route = super::transport::Transport::tcp(conn_weak_for_read.clone(), peer_addr);
                                let _ = data_tx.send((route, data)).await;
                            }
                            Err(e) => {
                                if e.kind() != io::ErrorKind::UnexpectedEof {
                                    log::warn!("TCP decode error: {e}");
                                }
                                break;
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        log::debug!("TCP read task shutting down");
                        break;
                    }
                }
            }
            pool_for_read.remove_tcp(peer_addr).await;
        });

        // Write loop using Encoder
        let pool_for_write = self.clone();
        let mut shutdown_rx = self.global_shutdown.subscribe();
        let enc = Arc::new(tokio::sync::Mutex::new(_encoder));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    data = write_rx.recv() => {
                        match data {
                            Some(data) => {
                                let mut enc = enc.lock().await;
                                if let Err(e) = enc.encode(&mut write_half, &data).await {
                                    log::warn!("TCP encode error: {e}");
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        log::debug!("TCP write task shutting down");
                        break;
                    }
                }
            }
            pool_for_write.remove_tcp(peer_addr).await;
        });

        let weak = Arc::downgrade(&conn);
        self.tcp_conns.write().await.insert(peer_addr, conn);
        Ok(weak)
    }

    /// Send data through ALL assistant UDP sockets to a specific address.
    pub async fn send_via_assistants(&self, buf: &[u8], addr: SocketAddr) {
        let sockets = self.udp_sockets.read().await;
        for entry in sockets.iter() {
            if entry.role == SocketRole::Assistant {
                let _ = entry.socket.try_send_to(buf, addr);
            }
        }
    }

    /// Send data to an address via the main UDP socket.
    pub async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        let sockets = self.udp_sockets.read().await;
        let main_socket = sockets
            .iter()
            .find(|e| e.role == SocketRole::Main)
            .ok_or_else(|| io::Error::other("no main UDP socket"))?;
        main_socket
            .socket
            .try_send_to(buf, addr)
            .map(|_| ())
            .map_err(|e| io::Error::other(format!("send failed: {e}")))
    }

    /// Send data through ALL UDP sockets (main + assistant) to a specific address.
    pub async fn try_send_via_all(&self, buf: &[u8], addr: SocketAddr) {
        let sockets = self.udp_sockets.read().await;
        for entry in sockets.iter() {
            let _ = entry.socket.try_send_to(buf, addr);
        }
    }

    /// Shutdown all tasks (program exit).
    pub fn shutdown(&self) {
        let _ = self.global_shutdown.send(());
    }

    /// Get a shutdown receiver to listen for shutdown signals.
    pub fn shutdown_rx(&self) -> broadcast::Receiver<()> {
        self.global_shutdown.subscribe()
    }

    /// Get local address of first UDP socket.
    pub async fn local_addr(&self) -> io::Result<SocketAddr> {
        self.udp_sockets
            .read()
            .await
            .first()
            .ok_or_else(|| io::Error::other("no UDP sockets"))?
            .socket
            .local_addr()
    }

    /// Find a TCP connection by peer address.
    pub async fn find_tcp(&self, addr: SocketAddr) -> Option<Arc<TcpConnection>> {
        self.tcp_conns.read().await.get(&addr).cloned()
    }

    /// Get or create a TCP connection to the given address (with concurrency protection).
    pub async fn connect_tcp_internal(
        self: &Arc<Self>,
        addr: SocketAddr,
    ) -> io::Result<Arc<TcpConnection>> {
        if let Some(conn) = self.find_tcp(addr).await {
            return Ok(conn);
        }
        let _guard = self.connect_lock.lock().await;
        if let Some(conn) = self.find_tcp(addr).await {
            return Ok(conn);
        }
        let stream = crate::socket::connect_tcp(addr, 0, None, None).await?;
        let weak = self.add_tcp(stream, addr).await?;
        weak.upgrade()
            .ok_or_else(|| io::Error::other("connection dropped immediately"))
    }

    /// Get all TCP connections.
    pub async fn tcp_connections(&self) -> Vec<Arc<TcpConnection>> {
        self.tcp_conns.read().await.values().cloned().collect()
    }

    /// Get a UDP socket by index.
    pub async fn udp_socket(&self, index: usize) -> Option<Arc<UdpSocket>> {
        self.udp_sockets
            .read()
            .await
            .get(index)
            .map(|e| e.socket.clone())
    }

    /// Get all UDP sockets.
    pub async fn udp_sockets(&self) -> Vec<Arc<UdpSocket>> {
        self.udp_sockets
            .read()
            .await
            .iter()
            .map(|e| e.socket.clone())
            .collect()
    }

    /// Get the number of assistant sockets.
    pub async fn assistant_count(&self) -> usize {
        self.udp_sockets
            .read()
            .await
            .iter()
            .filter(|e| e.role == SocketRole::Assistant)
            .count()
    }

    /// Get the main UDP socket.
    pub async fn main_socket(&self) -> Option<Arc<UdpSocket>> {
        self.udp_sockets
            .read()
            .await
            .iter()
            .find(|e| e.role == SocketRole::Main)
            .map(|e| e.socket.clone())
    }

    async fn run_udp_reader(
        socket: Arc<UdpSocket>,
        weak: Weak<UdpSocket>,
        data_tx: mpsc::Sender<(super::transport::Transport, Bytes)>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        let mut buf = [0u8; 65536];
        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((0, _)) => break,
                        Ok((len, addr)) => {
                            let data = Bytes::copy_from_slice(&buf[..len]);
                            let route = super::transport::Transport::udp(weak.clone(), addr);
                            if data_tx.send((route, data)).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            log::warn!("UDP recv error: {e}");
                            break;
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    log::debug!("UDP read task shutting down");
                    break;
                }
            }
        }
        // Keep socket alive until reader exits
        drop(socket);
    }
}

/// A lightweight handle for sending data and querying socket state.
///
/// `Sender` is cloneable and can be moved into async tasks.
/// It provides send methods and read-only query methods without
/// exposing internal socket management (add/remove/clean).
///
/// # Examples
///
/// ```rust,no_run
/// use rust_p2p_core::endpoint::{EndPoint, Config};
///
/// # #[tokio::main]
/// # async fn main() -> std::io::Result<()> {
/// let ep = EndPoint::bind(Config::new().udp_port(3000)).await?;
/// let sender = ep.sender();
///
/// // Send to a known address
/// sender.try_send_via_all(b"hello", "127.0.0.1:4000".parse().unwrap()).await;
///
/// // Query local address
/// println!("Listening on: {:?}", sender.local_addr().await);
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Sender(pub(crate) Arc<SocketPool>);

impl Sender {
    // === Send methods ===

    /// Send data through ALL UDP sockets (main + assistant) to a specific address.
    pub async fn try_send_via_all(&self, buf: &[u8], addr: SocketAddr) {
        self.0.try_send_via_all(buf, addr).await;
    }

    /// Send data to an address via the main UDP socket.
    pub async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        self.0.send_to(buf, addr).await
    }

    /// Send data through ALL assistant UDP sockets to a specific address.
    pub async fn send_via_assistants(&self, buf: &[u8], addr: SocketAddr) {
        self.0.send_via_assistants(buf, addr).await;
    }

    // === Read-only query methods ===

    /// Get local address of first UDP socket.
    pub async fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr().await
    }

    /// Get the number of assistant sockets.
    pub async fn assistant_count(&self) -> usize {
        self.0.assistant_count().await
    }

    /// Get all UDP sockets.
    pub async fn udp_sockets(&self) -> Vec<Arc<UdpSocket>> {
        self.0.udp_sockets().await
    }

    /// Get a UDP socket by index.
    pub async fn udp_socket(&self, index: usize) -> Option<Arc<UdpSocket>> {
        self.0.udp_socket(index).await
    }

    /// Find a TCP connection by peer address.
    pub async fn find_tcp(&self, addr: SocketAddr) -> Option<Arc<TcpConnection>> {
        self.0.find_tcp(addr).await
    }

    /// Get all TCP connections.
    pub async fn tcp_connections(&self) -> Vec<Arc<TcpConnection>> {
        self.0.tcp_connections().await
    }

    // === TCP connection methods ===

    /// Establish a TCP connection to the given address.
    /// If a connection already exists, returns immediately.
    pub async fn connect(&self, addr: SocketAddr) -> io::Result<()> {
        self.0.connect_tcp_internal(addr).await?;
        Ok(())
    }

    /// Send data to the given address via TCP.
    /// Automatically establishes a connection if none exists.
    pub async fn write_to(&self, data: &[u8], addr: SocketAddr) -> io::Result<()> {
        let conn = self.0.connect_tcp_internal(addr).await?;
        conn.send(data).await
    }
}
