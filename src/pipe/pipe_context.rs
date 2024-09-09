use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_utils::atomic::AtomicCell;
use dashmap::DashMap;
use parking_lot::RwLock;

use crate::error::Error;
use crate::protocol::node_id::NodeID;

#[derive(Clone, Default)]
pub struct PipeContext {
    self_node_id: Arc<AtomicCell<Option<NodeID>>>,
    direct_node_address_list: Arc<RwLock<Vec<(NodeAddress, Option<NodeID>)>>>,
    reachable_nodes: Arc<DashMap<NodeID, Vec<(NodeID, u8, Instant)>>>,
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
}

#[derive(Copy, Clone, Debug)]
pub enum NodeAddress {
    Tcp(SocketAddr),
    Udp(SocketAddr),
}
