#[cfg(feature = "use-kcp")]
mod kcp_stream;

use crate::EndPoint;
#[cfg(feature = "use-kcp")]
pub use kcp_stream::*;

impl EndPoint {
    #[cfg(feature = "use-kcp")]
    pub fn kcp_stream(&self) -> KcpStreamHub {
        self.kcp_context.create_manager(self.output.clone())
    }
}
