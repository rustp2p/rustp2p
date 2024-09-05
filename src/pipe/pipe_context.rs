use crate::error::Error;
use crate::protocol::node_id::NodeID;
use crossbeam_utils::atomic::AtomicCell;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct PipeContext {
    self_node_id: Arc<AtomicCell<Option<NodeID>>>,
    direct_node_address_list: Arc<RwLock<Vec<(NodeAddress, Option<NodeID>)>>>,
    reachable_nodes: Arc<DashMap<NodeID, Vec<(NodeID, u8)>>>,
}

impl PipeContext {
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
        self.reachable_nodes
            .entry(reachable_id)
            .and_modify(|v| {
                for (node, m) in &mut *v {
                    if node == &src_id {
                        *m = metric;
                        v.sort_by_key(|(_, m)| *m);
                        return;
                    }
                }
                v.push((src_id, metric));
                v.sort_by_key(|(_, m)| *m);
            })
            .or_insert_with(|| vec![(src_id, metric)]);
    }
}

#[derive(Clone, Debug)]
pub enum NodeAddress {
    Tcp(SocketAddr),
    Udp(SocketAddr),
}
