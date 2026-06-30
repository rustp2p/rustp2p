//! Low-level socket creation and management.
//!
//! This module provides functions for creating and configuring UDP and TCP sockets
//! with support for interface binding, port reuse, and other socket options.
//!
//! # Examples
//!
//! ```rust,no_run
//! use rust_p2p_core::socket::{bind_udp, LocalInterface};
//! use std::net::SocketAddr;
//!
//! # fn main() -> std::io::Result<()> {
//! let addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
//! let socket = bind_udp(addr, None)?;
//! # Ok(())
//! # }
//! ```

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

/// Network interface identifier for binding sockets.
///
/// On Linux/Android, this uses the interface name (e.g., "eth0").
/// On other platforms, this uses the interface index.
///
/// # Examples
///
/// ```rust
/// use rust_p2p_core::socket::LocalInterface;
///
/// // On Linux/Android
/// #[cfg(any(target_os = "linux", target_os = "android"))]
/// let iface = LocalInterface::new("eth0".to_string());
///
/// // On other platforms
/// #[cfg(not(any(target_os = "linux", target_os = "android")))]
/// let iface = LocalInterface::new(2); // interface index
/// ```
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
