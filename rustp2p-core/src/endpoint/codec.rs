use std::io;
use std::net::SocketAddr;

use async_trait::async_trait;
use dyn_clone::DynClone;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// Factory for creating codec pairs.
pub trait InitCodec: Send + Sync + DynClone {
    fn codec(&self, addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)>;
}
dyn_clone::clone_trait_object!(InitCodec);

/// Decoder for reading framed data.
#[async_trait]
pub trait Decoder: Send + Sync {
    async fn decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize>;
}

/// Encoder for writing framed data.
#[async_trait]
pub trait Encoder: Send + Sync {
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<()>;
}

/// Raw bytes codec (no framing).
#[derive(Clone)]
pub struct BytesCodec;

#[async_trait]
impl Decoder for BytesCodec {
    async fn decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize> {
        read.read(src).await
    }
}

#[async_trait]
impl Encoder for BytesCodec {
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<()> {
        write.write_all(data).await
    }
}

/// InitCodec for raw bytes (no framing).
#[derive(Clone)]
pub struct BytesInitCodec;

impl InitCodec for BytesInitCodec {
    fn codec(&self, _addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
        Ok((Box::new(BytesCodec), Box::new(BytesCodec)))
    }
}

/// Length-prefixed codec (4-byte big-endian length prefix).
#[derive(Clone)]
pub struct LengthPrefixedCodec;

#[async_trait]
impl Decoder for LengthPrefixedCodec {
    async fn decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize> {
        let mut head = [0; 4];
        read.read_exact(&mut head).await?;
        let len = u32::from_be_bytes(head) as usize;
        if len > src.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame too large: {len}"),
            ));
        }
        read.read_exact(&mut src[..len]).await?;
        Ok(len)
    }
}

#[async_trait]
impl Encoder for LengthPrefixedCodec {
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<()> {
        let len = data.len() as u32;
        write.write_all(&len.to_be_bytes()).await?;
        write.write_all(data).await?;
        Ok(())
    }
}

/// InitCodec for length-prefixed framing.
#[derive(Clone)]
pub struct LengthPrefixedInitCodec;

impl InitCodec for LengthPrefixedInitCodec {
    fn codec(&self, _addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
        Ok((Box::new(LengthPrefixedCodec), Box::new(LengthPrefixedCodec)))
    }
}
