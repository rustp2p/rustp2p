#[cfg(feature = "async_tokio")]
mod tokio;
#[cfg(feature = "async_tokio")]
pub use tokio::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "async_tokio")]
pub use tokio::{OwnedReadHalf, OwnedWriteHalf};
#[cfg(feature = "async_tokio")]
pub use tokio::{TcpListener, TcpStream};
#[cfg(feature = "async_std")]
mod async_std;
#[cfg(feature = "async_std")]
pub use async_std::{OwnedReadHalf, OwnedWriteHalf};
#[cfg(feature = "async_std")]
pub use async_std::{TcpListener, TcpStream};
#[cfg(feature = "async_std")]
pub use futures_util::{AsyncReadExt, AsyncWriteExt};
