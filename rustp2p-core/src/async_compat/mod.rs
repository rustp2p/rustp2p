pub mod mpsc;
pub mod net;
pub mod task;
pub mod time;

#[cfg(feature = "use-async-std")]
use std::future::Future;

pub use task::*;

#[cfg(feature = "use-tokio")]
pub use tokio::task::JoinHandle;

#[cfg(feature = "use-tokio")]
pub use ::tokio::spawn;

#[cfg(feature = "use-async-std")]
use ::async_std::task::JoinHandle;

#[cfg(feature = "use-async-std")]
pub fn spawn<F>(f: F) -> JoinHandle<Result<F::Output, std::io::Error>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    ::async_std::task::spawn(async move { Ok(f.await) })
}
