#[cfg(feature = "use-tokio")]
mod use_tokio;
#[cfg(feature = "use-tokio")]
pub use use_tokio::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "use-tokio")]
pub use use_tokio::{OwnedReadHalf, OwnedWriteHalf};
#[cfg(feature = "use-tokio")]
pub use use_tokio::{TcpListener, TcpStream};
#[cfg(feature = "use-async-std")]
pub mod use_async_std;
#[cfg(feature = "use-async-std")]
pub use futures_util::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "use-async-std")]
pub use use_async_std::{OwnedReadHalf, OwnedWriteHalf};
#[cfg(feature = "use-async-std")]
pub use use_async_std::{TcpListener, TcpStream};
