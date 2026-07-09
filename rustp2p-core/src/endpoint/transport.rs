use std::io;
use std::net::SocketAddr;
use std::sync::Weak;
use tokio::net::UdpSocket;

use crate::endpoint::pool::TcpConnection;
use crate::route_table::Protocol;

/// A transport handle to a peer, holding a Weak reference to the socket.
///
/// Transport is a send handle - it does NOT store received data.
/// Data is stored in `Received` alongside the Transport.
///
/// When the socket is dropped by the pool (e.g., environment change),
/// the Weak reference fails and `send()` returns an error.
///
/// # Examples
///
/// ```rust,no_run
/// use rustp2p_core::endpoint::Transport;
///
/// # async fn example(transport: Transport) -> std::io::Result<()> {
/// transport.send(b"hello").await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Transport {
    inner: TransportInner,
    addr: SocketAddr,
}

#[derive(Clone)]
enum TransportInner {
    Udp(Weak<UdpSocket>),
    Tcp(Weak<TcpConnection>),
}

impl Transport {
    /// Creates a UDP transport.
    pub(crate) fn udp(weak: Weak<UdpSocket>, addr: SocketAddr) -> Self {
        Self {
            inner: TransportInner::Udp(weak),
            addr,
        }
    }

    /// Creates a TCP transport.
    pub(crate) fn tcp(weak: Weak<TcpConnection>, addr: SocketAddr) -> Self {
        Self {
            inner: TransportInner::Tcp(weak),
            addr,
        }
    }

    /// Send data to the peer this transport connects to.
    pub async fn send(&self, data: &[u8]) -> io::Result<()> {
        match &self.inner {
            TransportInner::Udp(weak) => {
                let socket = weak
                    .upgrade()
                    .ok_or_else(|| io::Error::other("UDP socket dropped"))?;
                socket.send_to(data, self.addr).await?;
                Ok(())
            }
            TransportInner::Tcp(weak) => {
                let conn = weak
                    .upgrade()
                    .ok_or_else(|| io::Error::other("TCP connection dropped"))?;
                conn.send(data).await
            }
        }
    }

    /// Returns the protocol (UDP or TCP).
    pub fn protocol(&self) -> Protocol {
        match self.inner {
            TransportInner::Udp(_) => Protocol::UDP,
            TransportInner::Tcp(_) => Protocol::TCP,
        }
    }

    /// Returns the remote address.
    pub fn remote_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn is_udp(&self) -> bool {
        matches!(self.inner, TransportInner::Udp(_))
    }

    pub fn is_tcp(&self) -> bool {
        matches!(self.inner, TransportInner::Tcp(_))
    }
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("protocol", &self.protocol())
            .field("addr", &self.addr)
            .finish()
    }
}
