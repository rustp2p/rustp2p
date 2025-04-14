#[cfg(feature = "use-kcp")]
mod kcp_stream;

use crate::EndPoint;
#[cfg(feature = "use-kcp")]
pub use kcp_stream::*;

impl EndPoint {
    #[cfg(feature = "use-kcp")]
    pub fn kcp_stream(&self) -> KcpStreamManager {
        self.kcp_stream_manager.clone()
    }
}
