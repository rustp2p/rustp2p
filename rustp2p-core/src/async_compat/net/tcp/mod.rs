#[cfg(feature = "use-tokio")]
mod tokio;
#[cfg(feature = "use-tokio")]
pub use tokio::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "use-tokio")]
pub use tokio::{OwnedReadHalf, OwnedWriteHalf};
#[cfg(feature = "use-tokio")]
pub use tokio::{TcpListener, TcpStream};
#[cfg(feature = "use-async-std")]
pub mod async_std;
#[cfg(feature = "use-async-std")]
pub use async_std::{OwnedReadHalf, OwnedWriteHalf};
#[cfg(feature = "use-async-std")]
pub use async_std::{TcpListener, TcpStream};
#[cfg(feature = "use-async-std")]
pub use futures_util::{AsyncReadExt, AsyncWriteExt};
