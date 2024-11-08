use std::io;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};

use async_io::Async;
use futures_util::io::{ReadHalf, WriteHalf};
use futures_util::AsyncReadExt;

pub type OwnedReadHalf = ReadHalf<Async<std::net::TcpStream>>;
pub type OwnedWriteHalf = WriteHalf<Async<std::net::TcpStream>>;

#[repr(transparent)]
pub struct TcpListener {
    inner: Async<std::net::TcpListener>,
}

impl TcpListener {
    pub async fn accept(&mut self) -> io::Result<(TcpStream, SocketAddr)> {
        let (stream, addr) = self.inner.accept().await?;
        Ok((TcpStream::new(stream), addr))
    }
}

impl TcpListener {
    pub fn from_std(tcp_listener: std::net::TcpListener) -> io::Result<Self> {
        Ok(Self {
            inner: Async::new(tcp_listener)?,
        })
    }
}

#[repr(transparent)]
pub struct TcpStream {
    inner: Async<std::net::TcpStream>,
}

impl TcpStream {
    pub(crate) fn new(inner: Async<std::net::TcpStream>) -> Self {
        Self { inner }
    }
}

impl TcpStream {
    pub fn from_std(stream: std::net::TcpStream) -> io::Result<Self> {
        Ok(Self {
            inner: Async::new(stream)?,
        })
    }
    pub async fn writable(&self) -> io::Result<()> {
        self.inner.writable().await
    }
    pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        self.inner.split()
    }
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.inner.get_ref().set_nodelay(nodelay)
    }
    pub fn try_write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.get_ref().write(buf)
    }
    pub fn try_read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.get_ref().read(buf)
    }
}

impl Deref for TcpStream {
    type Target = Async<std::net::TcpStream>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for TcpStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
