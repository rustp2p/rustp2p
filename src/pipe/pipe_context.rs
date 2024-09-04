use crate::error::Error;
use crate::protocol::node_id::NodeID;
use crossbeam_utils::atomic::AtomicCell;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct PipeContext {
    self_node_id: Arc<AtomicCell<Option<NodeID>>>,
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
}
