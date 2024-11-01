use std::io;
use std::ops::Deref;
use async_io::Async;

#[repr(transparent)]
pub struct TcpStream {
    inner: Async<std::net::TcpStream>,
}

impl Deref for TcpStream {
    type Target = Async<std::net::TcpStream>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl TcpStream {
    pub async fn writable(&self) -> io::Result<()> {
        self.inner.writable().await
    }
}