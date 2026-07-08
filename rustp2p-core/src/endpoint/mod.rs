//! # Endpoint API - P2P Networking Interface
//!
//! This module provides the primary API for P2P networking.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use rust_p2p_core::endpoint::{EndPoint, Config};
//!
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! let mut ep = EndPoint::bind(Config::new().udp_port(3000)).await?;
//!
//! while let Some(received) = ep.recv().await {
//!     println!("From {}: {:?}", received.transport.remote_addr(), received.data);
//!     received.transport.send(b"echo").await?;
//! }
//! # Ok(())
//! # }
//! ```

mod codec;
mod config;
pub(crate) mod pool;
mod service;
pub(crate) mod transport;

pub use crate::route_table::Protocol;
pub use codec::{BytesInitCodec, Decoder, Encoder, InitCodec, LengthPrefixedInitCodec};
pub use config::{Config, LoadBalance};
pub use pool::Sender;
pub use service::{EndPoint, Received};
pub use transport::Transport;
