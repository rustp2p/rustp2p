//! Routing logic for selecting optimal paths between peers.
//!
//! This module manages routing decisions, tracking metrics like RTT (Round Trip Time)
//! and hop count to select the best available path for communication.
//!
//! # Examples
//!
//! ```rust
//! use rust_p2p_core::route_table::{RouteKey, Protocol};
//!
//! # fn example() {
//! // Create a RouteKey from protocol and address
//! let key = RouteKey::new(Protocol::UDP, "127.0.0.1:3000".parse().unwrap());
//! assert!(key.protocol().is_udp());
//! assert_eq!(key.addr().port(), 3000);
//! # }
//! ```

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use crate::endpoint::transport::Transport;

pub mod route_table;

pub use route_table::Route;

pub const DEFAULT_RTT: u32 = 9999;

/// Identifies a specific route to a peer.
///
/// `RouteKey` uniquely identifies a path by combining the
/// protocol (UDP/TCP) and remote address.
///
/// # Examples
///
/// ```rust
/// use rust_p2p_core::route_table::{RouteKey, Protocol};
///
/// # fn example() {
/// // Create from protocol and address
/// let key = RouteKey::new(Protocol::UDP, "127.0.0.1:3000".parse().unwrap());
///
/// // Or from a Transport
/// // let key = RouteKey::from_transport(&transport);
/// # }
/// ```
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct RouteKey {
    protocol: Protocol,
    addr: SocketAddr,
}
impl Default for RouteKey {
    fn default() -> Self {
        Self {
            protocol: Protocol::TCP,
            addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
        }
    }
}
impl RouteKey {
    /// Creates a new RouteKey from protocol and address.
    pub const fn new(protocol: Protocol, addr: SocketAddr) -> Self {
        Self { protocol, addr }
    }

    /// Creates a RouteKey from a Transport handle.
    pub fn from_transport(transport: &Transport) -> Self {
        Self {
            protocol: transport.protocol(),
            addr: transport.remote_addr(),
        }
    }

    /// Returns the protocol (UDP or TCP).
    #[inline]
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Returns the remote socket address.
    #[inline]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

/// Sorting key for comparing route quality.
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct RouteSortKey {
    metric: u8,
    rtt: u32,
}

/// Protocol type (UDP or TCP).
///
/// # Examples
///
/// ```rust
/// use rust_p2p_core::route_table::Protocol;
///
/// let proto = Protocol::UDP;
/// assert!(proto.is_udp());
/// assert!(!proto.is_tcp());
/// ```
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Protocol {
    UDP,
    TCP,
}
impl Protocol {
    /// Returns true if this is TCP.
    #[inline]
    pub fn is_tcp(&self) -> bool {
        self == &Protocol::TCP
    }

    /// Returns true if this is UDP.
    #[inline]
    pub fn is_udp(&self) -> bool {
        self == &Protocol::UDP
    }
}

impl fmt::Display for RouteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let protocol = self.protocol();
        write!(f, "{}://{}", protocol, self.addr())
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Protocol::UDP => write!(f, "udp"),
            Protocol::TCP => write!(f, "tcp"),
        }
    }
}
