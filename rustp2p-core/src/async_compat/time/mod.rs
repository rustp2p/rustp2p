pub struct Elapsed;
#[cfg(feature = "use-async-std")]
pub use async_std::future::timeout;
#[cfg(feature = "use-tokio")]
pub use tokio::time::timeout;

#[cfg(feature = "use-tokio")]
pub use tokio::time::sleep;

#[cfg(feature = "use-async-std")]
pub use async_std::task::sleep;
