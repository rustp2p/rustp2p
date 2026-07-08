use bytes::Bytes;
use std::io;

/// The send half of an end-to-end QUIC stream.
///
/// Data written here is carried by the QUIC connection to the remote `PeerId`.
/// The underlying path may be direct or relayed, but the stream is still a QUIC
/// stream between the two endpoint peers.
pub struct ReliableSendStream {
    send: quinn::SendStream,
}

impl ReliableSendStream {
    pub(crate) fn new(send: quinn::SendStream) -> Self {
        Self { send }
    }

    /// Writes the entire buffer to the QUIC stream.
    pub async fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.send
            .write_all(data)
            .await
            .map_err(|e| io::Error::other(format!("write reliable stream: {e}")))
    }

    /// Finishes the send side of the stream.
    ///
    /// The peer can continue sending on its own half of the bidirectional stream.
    pub fn finish(&mut self) -> io::Result<()> {
        self.send
            .finish()
            .map_err(|e| io::Error::other(format!("finish reliable stream: {e}")))
    }
}

/// The receive half of an end-to-end QUIC stream.
pub struct ReliableRecvStream {
    recv: quinn::RecvStream,
}

impl ReliableRecvStream {
    pub(crate) fn new(recv: quinn::RecvStream) -> Self {
        Self { recv }
    }

    /// Reads one chunk from the stream.
    ///
    /// Returns `Ok(None)` when the peer has finished sending.
    pub async fn read(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        self.recv
            .read(buf)
            .await
            .map_err(|e| io::Error::other(format!("read reliable stream: {e}")))
    }

    /// Reads until the peer finishes the stream, up to `max_size` bytes.
    ///
    /// This waits for stream EOF. For interactive protocols or unknown-length
    /// messages, prefer repeated calls to [`read`](Self::read).
    pub async fn read_to_end(&mut self, max_size: usize) -> io::Result<Bytes> {
        let data = self
            .recv
            .read_to_end(max_size)
            .await
            .map_err(|e| io::Error::other(format!("read reliable stream to end: {e}")))?;
        Ok(Bytes::from(data))
    }
}
