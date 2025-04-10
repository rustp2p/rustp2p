mod kcp_stream;

use crate::protocol::node_id::NodeID;
use crate::EndPoint;
pub use kcp_stream::KcpStream;
use std::io;

impl EndPoint {
    pub async fn accept_kcp_stream(&self, node_id: NodeID) -> io::Result<(KcpStream, NodeID)> {
        todo!()
    }
    pub async fn new_kcp_stream(&self, conv: u32, node_id: NodeID) -> io::Result<KcpStream> {
        todo!()
    }
}
