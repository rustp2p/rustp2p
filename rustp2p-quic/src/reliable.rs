use bytes::Bytes;
use std::io;

/// The send half of an end-to-end QUIC stream.
pub struct ReliableSendStream {
    send: quinn::SendStream,
}

impl ReliableSendStream {
    pub(crate) fn new(send: quinn::SendStream) -> Self {
        Self { send }
    }

    pub async fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.send
            .write_all(data)
            .await
            .map_err(|e| io::Error::other(format!("write reliable stream: {e}")))
    }

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

    pub async fn read(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        self.recv
            .read(buf)
            .await
            .map_err(|e| io::Error::other(format!("read reliable stream: {e}")))
    }

    pub async fn read_to_end(&mut self, max_size: usize) -> io::Result<Bytes> {
        let data = self
            .recv
            .read_to_end(max_size)
            .await
            .map_err(|e| io::Error::other(format!("read reliable stream to end: {e}")))?;
        Ok(Bytes::from(data))
    }
}
