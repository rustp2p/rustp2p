mod kcp_stream;

use crate::EndPoint;
pub use kcp_stream::*;

impl EndPoint {
    pub fn kcp_stream(&self) -> KcpStreamManager {
        self.kcp_stream_manager.clone()
    }
}
