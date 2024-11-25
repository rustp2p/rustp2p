use async_io::Async;
use async_std::net::ToSocketAddrs;
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
    pub async fn writable(&self) -> io::Result<()> {
        self.inner.writable().await
    }
    pub async fn readable(&self) -> io::Result<()> {
        self.inner.readable().await
    }

    pub fn try_send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<usize> {
        self.inner.get_ref().send_to(buf, addr)
    }
    pub fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        // Need to handle events
        self.inner.get_ref().recv_from(buf)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }
    pub async fn connect<A: ToSocketAddrs>(&self, addrs: A) -> io::Result<()> {
        let mut last_err = None;
        let addrs = addrs.to_socket_addrs().await?;

        for addr in addrs {
            match self.inner.get_ref().connect(addr) {
                Ok(()) => return Ok(()),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not resolve to any addresses",
            )
        }))
    }
    pub async fn bind<A: ToSocketAddrs>(addrs: A) -> io::Result<UdpSocket> {
        let mut last_err = None;
        let addrs = addrs.to_socket_addrs().await?;

        for addr in addrs {
            match Async::<std::net::UdpSocket>::bind(addr) {
                Ok(socket) => {
                    return Ok(UdpSocket { inner: socket });
                }
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not resolve to any addresses",
            )
        }))
    }
}
