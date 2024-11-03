use async_io::Async;
use std::io;
use std::net::SocketAddr;
use std::ops::Deref;

#[repr(transparent)]
pub struct UdpSocket {
    inner: Async<std::net::UdpSocket>,
}
impl Deref for UdpSocket {
    type Target = Async<std::net::UdpSocket>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl UdpSocket {
    pub fn from_std(udp: std::net::UdpSocket) -> io::Result<Self> {
        Ok(Self {
            inner: Async::new(udp)?,
        })
    }

    pub fn try_send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<usize> {
        self.inner.get_ref().send_to(buf, addr)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }
}
