#[cfg(feature = "use-kcp")]
mod kcp_stream;

#[cfg(feature = "use-kcp")]
use crate::protocol::node_id::NodeID;
use crate::EndPoint;
#[cfg(feature = "use-kcp")]
pub use kcp_stream::*;

impl EndPoint {
    #[cfg(feature = "use-kcp")]
    pub fn kcp_listener(&self) -> KcpListener {
        self.kcp_context.create_kcp_hub(self.output.clone())
    }
    #[cfg(feature = "use-kcp")]
    pub fn open_kcp_stream(&self, node_id: NodeID) -> std::io::Result<KcpStream> {
        self.kcp_context.new_stream(self.output.clone(), node_id)
    }
}
