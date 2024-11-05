pub struct Elapsed;
#[cfg(feature = "use-async-std")]
pub use async_std::future::timeout;
#[cfg(feature = "use-tokio")]
pub use tokio::time::timeout;

#[cfg(feature = "use-tokio")]
pub async fn sleep(duration: std::time::Duration) {
    tokio::time::sleep(duration).await
}

#[cfg(feature = "use-async-std")]
pub async fn sleep(duration: std::time::Duration) {
    async_std::task::sleep(duration).await
}
