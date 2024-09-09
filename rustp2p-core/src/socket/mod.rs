use anyhow::{anyhow, Context};
use network_interface::{NetworkInterface, NetworkInterfaceConfig};
use socket2::Protocol;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub(crate) trait VntSocketTrait {
    fn set_ip_unicast_if(&self, _interface: &LocalInterface) -> crate::error::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct LocalInterface {
    pub index: u32,
    #[cfg(unix)]
    pub name: Option<String>,
}
impl LocalInterface {
    pub fn new(index: u32, #[cfg(unix)] name: Option<String>) -> Self {
        Self {
            index,
            #[cfg(unix)]
            name,
        }
    }
}
#[allow(dead_code)]
pub(crate) async fn connect_tcp(
    addr: SocketAddr,
    bind_port: u16,
    default_interface: Option<&LocalInterface>,
    ttl: Option<u32>,
) -> crate::error::Result<tokio::net::TcpStream> {
    let socket = create_tcp0(addr.is_ipv4(), bind_port, default_interface, ttl)?;
    Ok(socket.connect(addr).await?)
}
#[allow(dead_code)]
pub(crate) fn create_tcp(
    v4: bool,
    default_interface: Option<&LocalInterface>,
) -> crate::error::Result<tokio::net::TcpSocket> {
    create_tcp0(v4, 0, default_interface, None)
}
pub(crate) fn create_tcp0(
    v4: bool,
    bind_port: u16,
    default_interface: Option<&LocalInterface>,
    ttl: Option<u32>,
) -> crate::error::Result<tokio::net::TcpSocket> {
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
    Ok(tokio::net::TcpSocket::from_std_stream(socket.into()))
}
pub(crate) fn create_tcp_listener(addr: SocketAddr) -> anyhow::Result<std::net::TcpListener> {
    let socket = if addr.is_ipv6() {
        let socket = socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::STREAM, None)?;
        socket
            .set_only_v6(false)
            .with_context(|| format!("set_only_v6 failed: {}", &addr))?;
        socket
    } else {
        socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None)?
    };
    socket
        .set_reuse_address(true)
        .context("set_reuse_address")?;
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
) -> anyhow::Result<socket2::Socket> {
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
        socket
            .set_only_v6(only_v6)
            .with_context(|| format!("set_only_v6 failed: {}", &addr))?;
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
) -> anyhow::Result<socket2::Socket> {
    bind_udp_ops(addr, true, default_interface).with_context(|| format!("bind_udp {}", addr))
}

/// Obtain network interface with the specified IP address
pub fn get_interface(dest_ip: Ipv4Addr) -> anyhow::Result<LocalInterface> {
    let network_interfaces = NetworkInterface::show()?;
    for iface in network_interfaces {
        for addr in iface.addr {
            if let IpAddr::V4(ip) = addr.ip() {
                if ip == dest_ip {
                    return Ok(LocalInterface {
                        index: iface.index,
                        #[cfg(unix)]
                        name: Some(iface.name),
                    });
                }
            }
        }
    }
    Err(anyhow!("No network card with IP {} found", dest_ip))
}
