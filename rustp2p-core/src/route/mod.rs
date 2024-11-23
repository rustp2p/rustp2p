use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

pub mod route_table;

pub const DEFAULT_RTT: u32 = 9999;

#[derive(Copy, Clone, Debug)]
pub struct Route {
    index: Index,
    addr: SocketAddr,
    metric: u8,
    rtt: u32,
}
impl Route {
    pub fn from(route_key: RouteKey, metric: u8, rtt: u32) -> Self {
        Self {
            index: route_key.index,
            addr: route_key.addr,
            metric,
            rtt,
        }
    }
    pub fn from_default_rt(route_key: RouteKey, metric: u8) -> Self {
        Self {
            index: route_key.index,
            addr: route_key.addr,
            metric,
            rtt: DEFAULT_RTT,
        }
    }
    pub fn route_key(&self) -> RouteKey {
        RouteKey {
            index: self.index,
            addr: self.addr,
        }
    }
    pub fn sort_key(&self) -> RouteSortKey {
        RouteSortKey {
            metric: self.metric,
            rtt: self.rtt,
        }
    }
    pub fn is_direct(&self) -> bool {
        self.metric == 0
    }
    pub fn is_relay(&self) -> bool {
        self.metric > 0
    }
    pub fn rtt(&self) -> u32 {
        self.rtt
    }
    pub fn metric(&self) -> u8 {
        self.metric
    }
}

impl From<(RouteKey, u8)> for Route {
    fn from((key, metric): (RouteKey, u8)) -> Self {
        Route::from_default_rt(key, metric)
    }
}

use crate::pipe::udp_pipe::UDPIndex;
#[non_exhaustive]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub enum Index {
    Udp(UDPIndex),
    Tcp(usize),
    Extend(usize),
}
impl Index {
    pub fn index(&self) -> usize {
        match self {
            Index::Udp(index) => index.index(),
            Index::Tcp(index) => *index,
            Index::Extend(index) => *index,
        }
    }
    pub fn protocol(&self) -> ConnectProtocol {
        match self {
            Index::Tcp(_) => ConnectProtocol::TCP,
            Index::Udp(_) => ConnectProtocol::UDP,
            Index::Extend(_) => ConnectProtocol::Extend,
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
    Extend,
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
