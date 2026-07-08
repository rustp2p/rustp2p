use std::net::SocketAddr;

/// A QUIC connection to a remote peer.
#[derive(Clone)]
pub struct Connection {
    inner: quinn::Connection,
}

impl Connection {
    pub(crate) fn new(conn: quinn::Connection) -> Self {
        Self { inner: conn }
    }

    /// Returns the remote socket address.
    pub fn remote_addr(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// Check if the connection is closed.
    pub fn is_closed(&self) -> bool {
        self.inner.close_reason().is_some()
    }

    /// Returns the underlying quinn connection.
    pub fn quinn(&self) -> &quinn::Connection {
        &self.inner
    }
}
