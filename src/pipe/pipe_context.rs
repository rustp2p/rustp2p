#![allow(clippy::type_complexity)]
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cipher::aes_gcm::AesGcmCipher;
use crate::config::punch_info::NodePunchInfo;
use crate::error::Error;
use crate::extend::dns_query::{dns_query_all, dns_query_txt};
use crate::protocol::node_id::{GroupCode, NodeID};
use anyhow::Context;
use crossbeam_utils::atomic::AtomicCell;
use dashmap::DashMap;
use parking_lot::RwLock;
use rand::seq::SliceRandom;
use rust_p2p_core::punch::{PunchConsultInfo, PunchModelBox};
use rust_p2p_core::route::route_table::RouteTable;
use rust_p2p_core::route::Index;
use rust_p2p_core::socket::LocalInterface;

#[derive(Clone)]
pub struct PipeContext {
    self_node_id: Arc<AtomicCell<Option<NodeID>>>,
    group_code: Arc<AtomicCell<GroupCode>>,
    direct_node_address_list: Arc<RwLock<Vec<(PeerNodeAddress, u16, Vec<NodeAddress>)>>>,
    direct_node_id_map: Arc<DashMap<u16, (GroupCode, NodeID, Instant)>>,
    pub(crate) reachable_nodes:
        Arc<DashMap<GroupCode, DashMap<NodeID, (GroupCode, NodeID, u8, Instant)>>>,
    punch_info: Arc<RwLock<NodePunchInfo>>,
    default_interface: Option<LocalInterface>,
    dns: Vec<String>,
    pub(crate) other_route_table: Arc<DashMap<GroupCode, RouteTable<NodeID>>>,
    pub(crate) aes_gcm_cipher: Option<AesGcmCipher>,
}
pub type DirectNodes = Vec<(NodeAddress, u16, Option<(GroupCode, NodeID)>)>;
impl PipeContext {
    pub(crate) fn new(
        local_udp_ports: Vec<u16>,
        local_tcp_port: u16,
        default_interface: Option<LocalInterface>,
        dns: Option<Vec<String>>,
        aes_gcm_cipher: Option<AesGcmCipher>,
    ) -> Self {
        let punch_info = NodePunchInfo::new(local_udp_ports, local_tcp_port);
        Self {
            self_node_id: Arc::new(Default::default()),
            group_code: Arc::new(Default::default()),
            direct_node_address_list: Arc::new(Default::default()),
            direct_node_id_map: Arc::new(Default::default()),
            reachable_nodes: Arc::new(Default::default()),
            punch_info: Arc::new(RwLock::new(punch_info)),
            default_interface,
            dns: dns.unwrap_or_default(),
            other_route_table: Arc::new(Default::default()),
            aes_gcm_cipher,
        }
    }
    pub fn store_self_id(&self, node_id: NodeID) -> crate::error::Result<()> {
        if node_id.is_unspecified() || node_id.is_broadcast() {
            return Err(Error::InvalidArgument("invalid node id".into()));
        }
        self.self_node_id.store(Some(node_id));
        Ok(())
    }
    pub fn store_group_code(&self, group_code: GroupCode) -> crate::error::Result<()> {
        if group_code.is_unspecified() {
            return Err(Error::InvalidArgument("invalid group code".into()));
        }
        self.group_code.store(group_code);
        Ok(())
    }
    pub fn load_group_code(&self) -> GroupCode {
        self.group_code.load()
    }
    pub fn load_id(&self) -> Option<NodeID> {
        self.self_node_id.load()
    }
    pub fn set_direct_nodes(&self, direct_node: Vec<PeerNodeAddress>) {
        let mut addrs = Vec::new();
        let mut ids: Vec<u16> = (10..u16::MAX).collect();
        ids.shuffle(&mut rand::thread_rng());
        let mut guard = self.direct_node_address_list.write();
        self.direct_node_id_map.clear();
        for (index, addr) in direct_node.into_iter().enumerate() {
            let id = ids[index];
            addrs.push((addr, id, vec![]))
        }
        *guard = addrs;
    }
    pub fn set_direct_nodes_and_id(
        &self,
        direct_node: Vec<(PeerNodeAddress, u16, Vec<NodeAddress>)>,
    ) {
        *self.direct_node_address_list.write() = direct_node;
    }
    pub fn get_direct_nodes(&self) -> Vec<(NodeAddress, Option<(GroupCode, NodeID)>)> {
        let guard = self.direct_node_address_list.read();
        let mut addrs = Vec::new();
        for (_, id, v) in guard.iter() {
            let node_id = self.get_direct_node_id(id);
            for x in v {
                addrs.push((*x, node_id))
            }
        }
        addrs
    }
    pub fn get_direct_nodes_and_id(&self) -> DirectNodes {
        let guard = self.direct_node_address_list.read();
        let mut addrs = Vec::new();
        for (_, id, v) in guard.iter() {
            let node_id = self.get_direct_node_id(id);
            for x in v {
                addrs.push((*x, *id, node_id))
            }
        }
        addrs
    }
    pub fn get_direct_node_id(&self, id: &u16) -> Option<(GroupCode, NodeID)> {
        self.direct_node_id_map
            .get(id)
            .map(|v| (v.value().0, v.value().1))
    }
    pub async fn update_direct_nodes(&self) -> crate::error::Result<()> {
        let mut addrs = self.direct_node_address_list.read().clone();
        for (peer_addr, _id, addr) in &mut addrs {
            *addr = peer_addr
                .to_addr(&self.dns, &self.default_interface)
                .await?;
        }
        self.set_direct_nodes_and_id(addrs);
        Ok(())
    }
    pub fn update_direct_node_id(&self, id: u16, group_code: GroupCode, node_id: NodeID) {
        self.direct_node_id_map
            .insert(id, (group_code, node_id, Instant::now()));
    }

    pub(crate) fn clear_timeout_reachable_nodes(&self, query_id_interval: Duration) {
        let now = Instant::now();
        if let Some(timeout) = now.checked_sub(query_id_interval * 3) {
            self.direct_node_id_map
                .retain(|_, (_, _, time)| *time > timeout);
            for mut val in self.reachable_nodes.iter_mut() {
                val.value_mut()
                    .retain(|_k, (_, _, _, time)| *time > timeout);
            }
            self.reachable_nodes.retain(|_, v| !v.is_empty());
        }
    }

    pub fn update_reachable_nodes(
        &self,
        reachable_group_code: GroupCode,
        reachable_id: NodeID,
        src_group_code: GroupCode,
        src_id: NodeID,
        metric: u8,
    ) {
        let now = Instant::now();
        self.reachable_nodes
            .entry(reachable_group_code)
            .or_default()
            .entry(reachable_id)
            .and_modify(|(group_code, node, m, time)| {
                if *m < metric {
                    return;
                }
                *m = metric;
                *time = now;
                *group_code = src_group_code;
                *node = src_id;
            })
            .or_insert_with(|| (src_group_code, src_id, metric, now));
    }
    pub fn reachable_node(
        &self,
        group_code: &GroupCode,
        dest_id: &NodeID,
    ) -> Option<(GroupCode, NodeID)> {
        if let Some(v) = self.reachable_nodes.get(group_code) {
            v.get(dest_id).map(|v| (v.value().0, v.value().1))
        } else {
            None
        }
    }
    pub fn default_route(&self) -> Option<NodeAddress> {
        let guard = self.direct_node_address_list.read();
        if let Some((_, _, v)) = guard.first() {
            v.first().cloned()
        } else {
            None
        }
    }
    pub(crate) fn exists_nat_info(&self) -> bool {
        self.punch_info.read().exists_nat_info()
    }
    pub fn punch_info(&self) -> &Arc<RwLock<NodePunchInfo>> {
        &self.punch_info
    }
    pub(crate) fn gen_punch_info(&self, seq: u32) -> PunchConsultInfo {
        self.punch_info.read().punch_consult_info(seq)
    }
    pub fn punch_model_box(&self) -> PunchModelBox {
        self.punch_info.read().punch_model_box.clone()
    }
    pub fn set_mapping_addrs(&self, mapping_addrs: Vec<NodeAddress>) {
        let tcp_addr: Vec<SocketAddr> = mapping_addrs
            .iter()
            .filter(|v| v.is_tcp())
            .map(|v| *v.addr())
            .collect();
        let udp_addr: Vec<SocketAddr> = mapping_addrs
            .iter()
            .filter(|v| !v.is_tcp())
            .map(|v| *v.addr())
            .collect();
        let mut guard = self.punch_info.write();
        guard.mapping_tcp_addr = tcp_addr;
        guard.mapping_udp_addr = udp_addr;
    }
    pub fn update_public_addr(&self, index: Index, addr: SocketAddr) {
        self.punch_info.write().update_public_addr(index, addr);
    }
    pub fn update_tcp_public_addr(&self, addr: SocketAddr) {
        self.punch_info.write().update_tcp_public_port(addr);
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum NodeAddress {
    Tcp(SocketAddr),
    Udp(SocketAddr),
}

impl NodeAddress {
    pub fn is_tcp(&self) -> bool {
        match self {
            NodeAddress::Tcp(_) => true,
            NodeAddress::Udp(_) => false,
        }
    }
    pub fn addr(&self) -> &SocketAddr {
        match self {
            NodeAddress::Tcp(addr) => addr,
            NodeAddress::Udp(addr) => addr,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerNodeAddress {
    Tcp(SocketAddr),
    Udp(SocketAddr),
    TcpDomain(String),
    UdpDomain(String),
    TxtDomain(String),
}
impl PeerNodeAddress {
    pub async fn to_addr(
        &self,
        name_servers: &Vec<String>,
        default_interface: &Option<LocalInterface>,
    ) -> crate::error::Result<Vec<NodeAddress>> {
        let addrs = match self {
            PeerNodeAddress::Tcp(addr) => vec![NodeAddress::Tcp(*addr)],
            PeerNodeAddress::Udp(addr) => vec![NodeAddress::Udp(*addr)],
            PeerNodeAddress::TcpDomain(domain) => {
                let addrs = dns_query_all(domain, name_servers, default_interface).await?;
                addrs.into_iter().map(NodeAddress::Tcp).collect()
            }
            PeerNodeAddress::UdpDomain(domain) => {
                let addrs = dns_query_all(domain, name_servers, default_interface).await?;
                addrs.into_iter().map(NodeAddress::Udp).collect()
            }
            PeerNodeAddress::TxtDomain(domain) => {
                let txt = dns_query_txt(domain, name_servers.clone(), default_interface).await?;
                let mut addrs = Vec::with_capacity(txt.len());
                for x in txt {
                    let x = x.to_lowercase();
                    let addr = if let Some(v) = x.strip_prefix("udp://") {
                        NodeAddress::Udp(
                            SocketAddr::from_str(v).context("record type txt is not SocketAddr")?,
                        )
                    } else if let Some(v) = x.strip_prefix("tcp://") {
                        NodeAddress::Tcp(
                            SocketAddr::from_str(v).context("record type txt is not SocketAddr")?,
                        )
                    } else {
                        NodeAddress::Tcp(
                            SocketAddr::from_str(&x)
                                .context("record type txt is not SocketAddr")?,
                        )
                    };
                    addrs.push(addr);
                }
                addrs
            }
        };
        Ok(addrs)
    }
}
impl FromStr for PeerNodeAddress {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let domain = s.to_lowercase();
        let addr = if let Some(v) = domain.strip_prefix("tcp://") {
            match SocketAddr::from_str(v) {
                Ok(addr) => PeerNodeAddress::Tcp(addr),
                Err(_) => PeerNodeAddress::TcpDomain(v.to_string()),
            }
        } else if let Some(v) = domain.strip_prefix("udp://") {
            match SocketAddr::from_str(v) {
                Ok(addr) => PeerNodeAddress::Udp(addr),
                Err(_) => PeerNodeAddress::UdpDomain(v.to_string()),
            }
        } else if let Some(v) = domain.strip_prefix("txt://") {
            PeerNodeAddress::TxtDomain(v.to_string())
        } else {
            match SocketAddr::from_str(&domain) {
                Ok(addr) => PeerNodeAddress::Udp(addr),
                Err(_) => PeerNodeAddress::UdpDomain(domain),
            }
        };
        Ok(addr)
    }
}
