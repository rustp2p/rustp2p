use rust_p2p_core::nat::{NatInfo, NatType};
use rust_p2p_core::punch::{PunchConsultInfo, PunchModelBox};
use rust_p2p_core::route::Index;
use rust_p2p_core::tunnel::udp::UDPIndex;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Debug, Clone)]
pub struct NodePunchInfo {
    pub punch_model_box: PunchModelBox,
    pub local_udp_ports: Vec<u16>,
    pub local_tcp_port: u16,
    pub public_udp_ports: Vec<u16>,
    pub public_tcp_port: u16,

    pub mapping_tcp_addr: Vec<SocketAddr>,
    pub mapping_udp_addr: Vec<SocketAddr>,
    pub public_port_range: u16,
    pub local_ipv4: Ipv4Addr,
    pub ipv6: Option<Ipv6Addr>,
    pub nat_type: NatType,
    pub public_ips: Vec<Ipv4Addr>,
}

impl NodePunchInfo {
    pub fn new(local_udp_ports: Vec<u16>, local_tcp_port: u16) -> Self {
        let public_udp_ports = vec![0; local_udp_ports.len()];
        Self {
            punch_model_box: Default::default(),
            local_udp_ports,
            local_tcp_port,
            public_udp_ports,
            public_tcp_port: 0,
            mapping_tcp_addr: vec![],
            mapping_udp_addr: vec![],
            public_port_range: 0,
            local_ipv4: Ipv4Addr::UNSPECIFIED,
            ipv6: None,
            nat_type: Default::default(),
            public_ips: vec![],
        }
    }
    pub fn exists_nat_info(&self) -> bool {
        !self.public_ips.is_empty()
    }
    pub fn update_tcp_public_port(&mut self, addr: SocketAddr) {
        let (ip, port) = if let Some(r) = Self::mapped_ip_port(addr) {
            r
        } else {
            return;
        };
        if rust_p2p_core::extend::addr::is_ipv4_global(&ip) && !self.public_ips.contains(&ip) {
            self.public_ips.push(ip);
        }
        self.public_tcp_port = port;
    }
    fn mapped_ip_port(addr: SocketAddr) -> Option<(Ipv4Addr, u16)> {
        match addr {
            SocketAddr::V4(addr) => Some((*addr.ip(), addr.port())),
            SocketAddr::V6(addr) => addr.ip().to_ipv4_mapped().map(|ip| (ip, addr.port())),
        }
    }
    pub fn update_public_addr(&mut self, index: Index, addr: SocketAddr) {
        let (ip, port) = if let Some(r) = Self::mapped_ip_port(addr) {
            r
        } else {
            return;
        };
        if rust_p2p_core::extend::addr::is_ipv4_global(&ip) {
            if !self.public_ips.contains(&ip) {
                self.public_ips.push(ip);
            }
            match index {
                Index::Udp(index) => {
                    let index = match index {
                        UDPIndex::MainV4(index) => index,
                        UDPIndex::MainV6(index) => index,
                        UDPIndex::SubV4(_) => return,
                    };
                    if let Some(p) = self.public_udp_ports.get_mut(index) {
                        *p = port;
                    }
                }
                Index::Tcp(_) => {
                    self.public_tcp_port = port;
                }
                _ => {}
            }
        } else {
            log::debug!("not public addr: {addr:?}")
        }
    }
    pub fn set_public_ip(&mut self, mut ips: Vec<Ipv4Addr>) {
        ips.retain(rust_p2p_core::extend::addr::is_ipv4_global);
        self.public_ips = ips;
    }
}

impl NodePunchInfo {
    pub fn nat_info(&self) -> NatInfo {
        NatInfo {
            nat_type: self.nat_type,
            public_ips: self.public_ips.clone(),
            public_udp_ports: self.public_udp_ports.clone(),
            mapping_tcp_addr: self.mapping_tcp_addr.clone(),
            mapping_udp_addr: self.mapping_udp_addr.clone(),
            public_port_range: self.public_port_range,
            local_ipv4: self.local_ipv4,
            ipv6: self.ipv6,
            local_udp_ports: self.local_udp_ports.clone(),
            local_tcp_port: self.local_tcp_port,
            public_tcp_port: self.public_tcp_port,
        }
    }
    pub(crate) fn punch_consult_info(&self) -> PunchConsultInfo {
        PunchConsultInfo::new(self.punch_model_box.clone(), self.nat_info())
    }
}
