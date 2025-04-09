#[cfg(windows)]
use crate::socket::windows::ignore_conn_reset;
use socket2::Protocol;
use std::io;
use std::net::SocketAddr;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub(crate) trait SocketTrait {
    fn set_ip_unicast_if(&self, _interface: &LocalInterface) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct LocalInterface {
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub index: u32,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub name: String,
}

impl LocalInterface {
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub fn new(index: u32) -> Self {
        Self { index }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

pub(crate) async fn connect_tcp(
    addr: SocketAddr,
    bind_port: u16,
    default_interface: Option<&LocalInterface>,
    ttl: Option<u32>,
) -> io::Result<tokio::net::TcpStream> {
    let socket = create_tcp0(addr, bind_port, default_interface, ttl)?;
    socket.writable().await?;
    Ok(socket)
}

pub(crate) fn create_tcp0(
    addr: SocketAddr,
    bind_port: u16,
    default_interface: Option<&LocalInterface>,
    ttl: Option<u32>,
) -> io::Result<tokio::net::TcpStream> {
    let v4 = addr.is_ipv4();
    let socket = if v4 {
        socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(Protocol::TCP),
        )?
    } else {
        socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::STREAM,
            Some(Protocol::TCP),
        )?
    };
    if v4 && default_interface.is_some() {
        socket.set_ip_unicast_if(default_interface.unwrap())?;
    }
    if bind_port != 0 {
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;
        if v4 {
            let addr: SocketAddr = format!("0.0.0.0:{}", bind_port).parse().unwrap();
            socket.bind(&addr.into())?;
        } else {
            socket.set_only_v6(true)?;
            let addr: SocketAddr = format!("[::]:{}", bind_port).parse().unwrap();
            socket.bind(&addr.into())?;
        }
    }
    if let Some(ttl) = ttl {
        socket.set_ttl(ttl)?;
    }
    socket.set_nonblocking(true)?;
    socket.set_nodelay(true)?;
    let res = socket.connect(&addr.into());
    match res {
        Ok(()) => {}
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
        #[cfg(unix)]
        Err(ref e) if e.raw_os_error() == Some(libc::EINPROGRESS) => {}
        Err(e) => Err(e)?,
    }
    tokio::net::TcpStream::from_std(socket.into())
}

pub(crate) fn create_tcp_listener(addr: SocketAddr) -> io::Result<std::net::TcpListener> {
    let socket = if addr.is_ipv6() {
        let socket = socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::STREAM, None)?;
        socket.set_only_v6(false)?;
        socket
    } else {
        socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None)?
    };
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    if let Err(e) = socket.set_reuse_port(true) {
        log::warn!("set_reuse_port {:?}", e)
    }
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    socket.set_nonblocking(true)?;
    socket.set_nodelay(true)?;
    Ok(socket.into())
}

pub(crate) fn bind_udp_ops(
    addr: SocketAddr,
    only_v6: bool,
    default_interface: Option<&LocalInterface>,
) -> io::Result<socket2::Socket> {
    let socket = if addr.is_ipv4() {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(Protocol::UDP),
        )?;
        if let Some(default_interface) = default_interface {
            socket.set_ip_unicast_if(default_interface)?;
        }
        socket
    } else {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(Protocol::UDP),
        )?;
        socket.set_only_v6(only_v6)?;
        socket
    };
    #[cfg(windows)]
    if let Err(e) = ignore_conn_reset(&socket) {
        log::warn!("ignore_conn_reset {e:?}")
    }
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    Ok(socket)
}

pub fn bind_udp(
    addr: SocketAddr,
    default_interface: Option<&LocalInterface>,
) -> io::Result<socket2::Socket> {
    bind_udp_ops(addr, true, default_interface)
}
