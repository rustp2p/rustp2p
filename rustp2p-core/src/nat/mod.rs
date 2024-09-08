use crate::extend::addr::is_ipv6_global;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, Default)]
pub enum NatType {
    #[default]
    Cone,
    Symmetric,
}

impl NatType {
    #[inline]
    pub fn is_cone(&self) -> bool {
        self == &NatType::Cone
    }
    #[inline]
    pub fn is_symmetric(&self) -> bool {
        self == &NatType::Symmetric
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NatInfo {
    /// nat type of the network
    pub nat_type: NatType,
    /// the set of public Ipv4
    pub public_ips: Vec<Ipv4Addr>,
    /// the set of public ports mapped from the nat
    pub public_ports: Vec<u16>,
    /// the set of mapped addresses where `TCP` serves on
    pub mapping_tcp_addr: Vec<SocketAddr>,
    /// the set of mapped addresses where `UDP` serves on
    pub mapping_udp_addr: Vec<SocketAddr>,
    /// The predicted range of public ports, it is used when the nat_type is symmertric
    pub public_port_range: u16,
    /// local IP address
    pub local_ipv4: Ipv4Addr,
    /// The public IPv6 addess
    pub ipv6: Option<Ipv6Addr>,
    /// The local ports where the `UDP` services bind
    pub local_udp_ports: Vec<u16>,
    /// The local ports where the `TCP` services bind
    pub local_tcp_port: u16,
    /// The public port of `TCP` service, which works when there is either `nat1` or no `nat` exists
    pub public_tcp_port: u16,
    /// Both parties' seq in the same round of hole punching need to be the same
    pub seq: u32,
}
impl NatInfo {
    pub fn ipv6_addr(&self) -> Vec<SocketAddr> {
        if let Some(ipv6) = self.ipv6 {
            if is_ipv6_global(&ipv6) {
                return self
                    .local_udp_ports
                    .iter()
                    .map(|&port| SocketAddrV6::new(ipv6, port, 0, 0).into())
                    .collect();
            }
        }
        vec![]
    }
    pub fn ipv6_tcp_addr(&self) -> Option<SocketAddr> {
        self.ipv6
            .map(|ipv6| SocketAddrV6::new(ipv6, self.local_tcp_port, 0, 0).into())
    }
    pub fn public_ipv4_addr(&self) -> Vec<SocketAddr> {
        if self.public_ips.is_empty() || self.public_ports.is_empty() {
            return vec![];
        }
        if self.public_ips.len() == 1 {
            let ip = self.public_ips[0];
            return self
                .public_ports
                .iter()
                .map(|&port| SocketAddrV4::new(ip, port).into())
                .collect();
        }
        let port = self.public_ports[0];
        self.public_ips
            .iter()
            .map(|&ip| SocketAddrV4::new(ip, port).into())
            .collect()
    }
    pub fn local_ipv4_addrs(&self) -> Vec<SocketAddr> {
        if self.local_ipv4.is_unspecified()
            || self.local_ipv4.is_multicast()
            || self.local_ipv4.is_broadcast()
        {
            return vec![];
        }
        self.local_udp_ports
            .iter()
            .map(|&port| SocketAddrV4::new(self.local_ipv4, port).into())
            .collect()
    }
    pub fn local_ipv4_tcp(&self) -> Option<SocketAddr> {
        if self.local_ipv4.is_unspecified()
            || self.local_ipv4.is_multicast()
            || self.local_ipv4.is_broadcast()
            || self.local_tcp_port == 0
        {
            return None;
        }
        Some(SocketAddrV4::new(self.local_ipv4, self.local_tcp_port).into())
    }
    pub fn public_ipv4_tcp(&self) -> Vec<SocketAddr> {
        if self.public_tcp_port == 0 {
            return vec![];
        }
        self.public_ips
            .iter()
            .map(|&ip| SocketAddrV4::new(ip, self.public_tcp_port).into())
            .collect()
    }
}
