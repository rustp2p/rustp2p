use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

pub mod route_table;

pub const DEFAULT_RTT: u32 = 9999;

use crate::tunnel::udp::UDPIndex;
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
    #[inline]
    pub fn protocol(&self) -> ConnectProtocol {
        self.index.protocol()
    }
    #[inline]
    pub fn index(&self) -> Index {
        self.index
    }
    #[inline]
    pub fn index_usize(&self) -> usize {
        self.index.index()
    }
    #[inline]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct RouteSortKey {
    metric: u8,
    rtt: u32,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ConnectProtocol {
    UDP,
    TCP,
}
impl ConnectProtocol {
    #[inline]
    pub fn is_tcp(&self) -> bool {
        self == &ConnectProtocol::TCP
    }
    #[inline]
    pub fn is_udp(&self) -> bool {
        self == &ConnectProtocol::UDP
    }
}
