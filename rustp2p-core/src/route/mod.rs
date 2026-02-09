//! Routing logic for selecting optimal paths between peers.
//!
//! This module manages routing decisions, tracking metrics like RTT (Round Trip Time)
//! and hop count to select the best available path for communication.
//!
//! # Examples
//!
//! ```rust
//! use rust_p2p_core::route::{RouteKey, ConnectProtocol};
//!
//! // RouteKey identifies a specific connection path
//! // It combines protocol (UDP/TCP) and socket address
//! ```

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

pub mod route_table;

pub const DEFAULT_RTT: u32 = 9999;

use crate::tunnel::udp::UDPIndex;

/// Socket index identifying a specific socket in a socket manager.
#[non_exhaustive]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub enum Index {
    Udp(UDPIndex),
    Tcp(usize),
}
impl Index {
    pub fn index(&self) -> usize {
        match self {
            Index::Udp(index) => index.index(),
            Index::Tcp(index) => *index,
        }
    }
    pub fn protocol(&self) -> ConnectProtocol {
        match self {
            Index::Tcp(_) => ConnectProtocol::TCP,
            Index::Udp(_) => ConnectProtocol::UDP,
        }
    }
}

/// Identifies a specific route to a peer.
///
/// `RouteKey` uniquely identifies a connection path by combining the
/// socket index and remote address.
///
/// # Examples
///
/// ```rust
/// use rust_p2p_core::route::{RouteKey, ConnectProtocol};
///
/// # fn example(route: RouteKey) {
/// // Check the protocol
/// if route.protocol() == ConnectProtocol::UDP {
///     println!("UDP route to {}", route.addr());
/// }
/// # }
/// ```
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct RouteKey {
    index: Index,
    addr: SocketAddr,
}
impl Default for RouteKey {
    fn default() -> Self {
        Self {
            index: Index::Tcp(0),
            addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
        }
    }
}
impl RouteKey {
    pub(crate) const fn new(index: Index, addr: SocketAddr) -> Self {
        Self { index, addr }
    }
    
    /// Returns the connection protocol (UDP or TCP).
    #[inline]
    pub fn protocol(&self) -> ConnectProtocol {
        self.index.protocol()
    }
    
    /// Returns the socket index.
    #[inline]
    pub fn index(&self) -> Index {
        self.index
    }
    
    /// Returns the socket index as usize.
    #[inline]
    pub fn index_usize(&self) -> usize {
        self.index.index()
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

/// Connection protocol type.
///
/// # Examples
///
/// ```rust
/// use rust_p2p_core::route::ConnectProtocol;
///
/// let proto = ConnectProtocol::UDP;
/// assert!(proto.is_udp());
/// assert!(!proto.is_tcp());
/// ```
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ConnectProtocol {
    UDP,
    TCP,
}
impl ConnectProtocol {
    /// Returns true if this is TCP.
    #[inline]
    pub fn is_tcp(&self) -> bool {
        self == &ConnectProtocol::TCP
    }
    
    /// Returns true if this is UDP.
    #[inline]
    pub fn is_udp(&self) -> bool {
        self == &ConnectProtocol::UDP
    }
}

impl fmt::Display for RouteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let protocol = self.protocol();
        write!(f, "{}://{}", protocol, self.addr())
    }
}

impl fmt::Display for ConnectProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectProtocol::UDP => write!(f, "udp"),
            ConnectProtocol::TCP => write!(f, "tcp"),
        }
    }
}
