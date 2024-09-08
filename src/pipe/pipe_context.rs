use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::punch_info::PunchInfo;
use crate::error::Error;
use crate::protocol::node_id::NodeID;
use crossbeam_utils::atomic::AtomicCell;
use dashmap::DashMap;
use parking_lot::RwLock;
use rust_p2p_core::route::Index;

#[derive(Clone)]
pub struct PipeContext {
    self_node_id: Arc<AtomicCell<Option<NodeID>>>,
    direct_node_address_list: Arc<RwLock<Vec<(NodeAddress, Option<NodeID>)>>>,
    reachable_nodes: Arc<DashMap<NodeID, Vec<(NodeID, u8, Instant)>>>,
    punch_info: Arc<RwLock<PunchInfo>>,
}

impl PipeContext {
    pub(crate) fn new(local_udp_ports: Vec<u16>, local_tcp_port: u16) -> Self {
        let punch_info = PunchInfo::new(local_udp_ports, local_tcp_port);
        Self {
            self_node_id: Arc::new(Default::default()),
            direct_node_address_list: Arc::new(Default::default()),
            reachable_nodes: Arc::new(Default::default()),
            punch_info: Arc::new(RwLock::new(punch_info)),
        }
    }
    pub fn store_self_id(&self, node_id: NodeID) -> crate::error::Result<()> {
        if node_id.is_unspecified() || node_id.is_broadcast() {
            return Err(Error::InvalidArgument("invalid node id".into()));
        }
        self.self_node_id.store(Some(node_id));
        Ok(())
    }
    pub fn load_id(&self) -> Option<NodeID> {
        self.self_node_id.load()
    }
    pub fn set_direct_nodes(&self, direct_node: Vec<(NodeAddress, Option<NodeID>)>) {
        *self.direct_node_address_list.write() = direct_node;
    }
    pub fn get_direct_nodes(&self) -> Vec<(NodeAddress, Option<NodeID>)> {
        self.direct_node_address_list.read().clone()
    }
    pub fn update_reachable_nodes(&self, src_id: NodeID, reachable_id: NodeID, metric: u8) {
        let now = Instant::now();
        self.reachable_nodes
            .entry(reachable_id)
            .and_modify(|v| {
                for (node, m, time) in &mut *v {
                    if node == &src_id {
                        *m = metric;
                        *time = now;
                        v.sort_by_key(|(_, m, _)| *m);
                        return;
                    }
                }
                v.push((src_id, metric, now));
                v.sort_by_key(|(_, m, _)| *m);
            })
            .or_insert_with(|| vec![(src_id, metric, now)]);
    }
    pub(crate) fn clear_timeout_reachable_nodes(&self) {
        let timeout = if let Some(time) = Instant::now().checked_sub(Duration::from_secs(60)) {
            time
        } else {
            return;
        };
        for mut val in self.reachable_nodes.iter_mut() {
            val.value_mut().retain(|(_, _, time)| time > &timeout);
        }
        self.reachable_nodes.retain(|_, v| !v.is_empty());
    }
    pub fn reachable_node(&self, dest_id: &NodeID) -> Option<NodeID> {
        if let Some(v) = self.reachable_nodes.get(dest_id) {
            v.value().get(0).map(|(v, _, _)| *v)
        } else {
            None
        }
    }
    pub fn default_route(&self) -> Option<NodeAddress> {
        self.direct_node_address_list.read().get(0).map(|(v, _)| *v)
    }
    pub(crate) fn exists_nat_info(&self) -> bool {
        self.punch_info.read().exists_nat_info()
    }
    pub fn punch_info(&self) -> &Arc<RwLock<PunchInfo>> {
        &self.punch_info
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

#[derive(Copy, Clone, Debug)]
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
