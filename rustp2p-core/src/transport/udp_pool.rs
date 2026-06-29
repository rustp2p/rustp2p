use bytes::Bytes;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, RwLock};

/// A pool of UDP sockets shared between Transport and Puncher.
///
/// - Users add their own socket via `new(socket)`
/// - Puncher adds sockets via `add_socket()` (for symmetric NAT)
/// - Transport reads from all sockets via the shared receiver
/// - All sends go through the pool
pub struct UdpPool {
    sockets: RwLock<Vec<Arc<UdpSocket>>>,
    data_tx: mpsc::Sender<(Bytes, SocketAddr, usize)>,
}

impl UdpPool {
    /// Creates a pool from a user's socket and returns a receiver for incoming data.
    ///
    /// The receiver yields `(data, from_addr, socket_index)` tuples.
    /// `socket_index` identifies which socket received the data.
    pub fn new(socket: UdpSocket) -> (Self, mpsc::Receiver<(Bytes, SocketAddr, usize)>) {
        let (data_tx, data_rx) = mpsc::channel(512);
        let socket = Arc::new(socket);
        let sockets = RwLock::new(vec![socket.clone()]);

        // Start read loop for the first socket
        Self::spawn_reader(socket, 0, data_tx.clone());

        (
            Self { sockets, data_tx },
            data_rx,
        )
    }

    /// Add a new socket to the pool and start reading from it.
    ///
    /// Returns the index of the newly added socket.
    pub async fn add_socket(&self, socket: UdpSocket) -> usize {
        let socket = Arc::new(socket);
        let mut sockets = self.sockets.write().await;
        let index = sockets.len();
        sockets.push(socket.clone());

        // Start read loop for the new socket
        Self::spawn_reader(socket, index, self.data_tx.clone());

        index
    }

    /// Send data through a specific socket.
    pub async fn send_to(
        &self,
        index: usize,
        buf: Bytes,
        addr: SocketAddr,
    ) -> io::Result<()> {
        let sockets = self.sockets.read().await;
        let socket = sockets
            .get(index)
            .ok_or_else(|| io::Error::other(format!("socket index {index} out of range")))?;
        socket.send_to(&buf, addr).await?;
        Ok(())
    }

    /// Send data through a specific socket (non-blocking).
    pub fn try_send_to(
        &self,
        index: usize,
        buf: &[u8],
        addr: SocketAddr,
    ) -> io::Result<()> {
        let sockets = self.sockets.blocking_read();
        let socket = sockets
            .get(index)
            .ok_or_else(|| io::Error::other(format!("socket index {index} out of range")))?;
        socket.try_send_to(buf, addr)?;
        Ok(())
    }

    /// Send data through ALL sockets (for hole-punching).
    pub fn try_send_all(&self, buf: &[u8], addr: SocketAddr) {
        let sockets = self.sockets.blocking_read();
        for socket in sockets.iter() {
            let _ = socket.try_send_to(buf, addr);
        }
    }

    /// Get the number of sockets in the pool.
    pub async fn len(&self) -> usize {
        self.sockets.read().await.len()
    }

    /// Get the local address of the first socket.
    pub async fn local_addr(&self) -> io::Result<SocketAddr> {
        let sockets = self.sockets.read().await;
        sockets
            .first()
            .ok_or_else(|| io::Error::other("no sockets"))?
            .local_addr()
    }

    fn spawn_reader(
        socket: Arc<UdpSocket>,
        index: usize,
        data_tx: mpsc::Sender<(Bytes, SocketAddr, usize)>,
    ) {
        tokio::spawn(async move {
            let mut buf = [0u8; 65536];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok(0) => break,
                    Ok(len) => {
                        let data = Bytes::copy_from_slice(&buf[..len]);
                        let addr = socket
                            .peer_addr()
                            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
                        if data_tx.send((data, addr, index)).await.is_err() {
                            break; // Receiver dropped
                        }
                    }
                    Err(e) => {
                        log::warn!("UDP recv error on socket {index}: {e}");
                        break;
                    }
                }
            }
        });
    }
}

impl std::fmt::Debug for UdpPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UdpPool").finish_non_exhaustive()
    }
}
