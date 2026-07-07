use bytes::Bytes;
use std::io;

/// A reliable end-to-end QUIC stream.
pub struct ReliableStream {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
}

impl ReliableStream {
    pub(crate) fn new(send: quinn::SendStream, recv: quinn::RecvStream) -> Self {
        Self { send, recv }
    }

    pub async fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.send
            .write_all(data)
            .await
            .map_err(|e| io::Error::other(format!("write reliable stream: {e}")))
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

    pub fn finish(&mut self) -> io::Result<()> {
        self.send
            .finish()
            .map_err(|e| io::Error::other(format!("finish reliable stream: {e}")))
    }
}
