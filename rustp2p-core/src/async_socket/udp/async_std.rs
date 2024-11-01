use std::io;
use std::net::SocketAddr;

pub struct UdpSocket {
    inner: async_io::Async<std::net::UdpSocket>,
}

impl UdpSocket {
    pub fn from_std(udp: std::net::UdpSocket) -> io::Result<Self> {
        Ok(Self {
            inner: async_io::Async::new(udp)?,
        })
    }

    pub fn try_send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<usize> {
        self.inner.get_ref().send_to(buf, addr)
    }
    pub async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<usize> {
        self.inner.send_to(buf, addr).await
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.recv_from(buf).await
    }
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }
}
