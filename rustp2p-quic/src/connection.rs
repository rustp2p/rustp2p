use bytes::{Bytes, BytesMut};
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

    /// Opens a bidirectional stream.
    pub async fn open_bi(&self) -> crate::Result<(SendStream, RecvStream)> {
        let (send, recv) = self.inner.open_bi().await?;
        Ok((SendStream::new(send), RecvStream::new(recv)))
    }

    /// Accepts an incoming bidirectional stream.
    pub async fn accept_bi(&self) -> crate::Result<(SendStream, RecvStream)> {
        let (send, recv) = self.inner.accept_bi().await?;
        Ok((SendStream::new(send), RecvStream::new(recv)))
    }

    /// Opens a unidirectional stream.
    pub async fn open_uni(&self) -> crate::Result<SendStream> {
        let send = self.inner.open_uni().await?;
        Ok(SendStream::new(send))
    }

    /// Accepts an incoming unidirectional stream.
    pub async fn accept_uni(&self) -> crate::Result<RecvStream> {
        let recv = self.inner.accept_uni().await?;
        Ok(RecvStream::new(recv))
    }

    /// Sends a datagram to the remote peer.
    pub async fn send_datagram(&self, data: Bytes) -> crate::Result<()> {
        self.inner
            .send_datagram(data)
            .map_err(|e| std::io::Error::other(format!("send datagram: {e}")))?;
        Ok(())
    }

    /// Receives a datagram from the remote peer.
    pub async fn recv_datagram(&self) -> crate::Result<Bytes> {
        let data = self.inner.read_datagram().await?;
        Ok(data)
    }

    /// Returns the remote socket address.
    pub fn remote_addr(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// Returns the connection error, if any.
    pub fn close_reason(&self) -> Option<quinn::ConnectionError> {
        self.inner.close_reason()
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

/// A send half of a QUIC stream.
pub struct SendStream {
    inner: quinn::SendStream,
}

impl SendStream {
    pub(crate) fn new(stream: quinn::SendStream) -> Self {
        Self { inner: stream }
    }

    /// Writes data to the stream.
    pub async fn write_all(&mut self, buf: &[u8]) -> crate::Result<()> {
        self.inner.write_all(buf).await?;
        Ok(())
    }

    /// Writes a chunk of data.
    pub async fn write_chunk(&mut self, chunk: Bytes) -> crate::Result<()> {
        self.inner.write_chunk(chunk).await?;
        Ok(())
    }

    /// Finishes the send stream.
    pub fn finish(&mut self) -> crate::Result<()> {
        self.inner
            .finish()
            .map_err(|e| std::io::Error::other(format!("finish stream: {e}")))?;
        Ok(())
    }
}

/// A receive half of a QUIC stream.
pub struct RecvStream {
    inner: quinn::RecvStream,
}

impl RecvStream {
    pub(crate) fn new(stream: quinn::RecvStream) -> Self {
        Self { inner: stream }
    }

    /// Reads data from the stream into a buffer.
    pub async fn read(&mut self, buf: &mut [u8]) -> crate::Result<Option<usize>> {
        let n = self.inner.read(buf).await?;
        Ok(n)
    }

    /// Reads a chunk of data.
    pub async fn read_chunk(&mut self) -> crate::Result<Option<Bytes>> {
        let chunk = self.inner.read_chunk(usize::MAX, true).await?;
        Ok(chunk.map(|c| c.bytes))
    }

    /// Reads all remaining data until the stream is finished.
    pub async fn read_to_end(&mut self, max_size: usize) -> crate::Result<BytesMut> {
        let data = self
            .inner
            .read_to_end(max_size)
            .await
            .map_err(|e| std::io::Error::other(format!("read to end: {e}")))?;
        Ok(BytesMut::from(&data[..]))
    }
}
