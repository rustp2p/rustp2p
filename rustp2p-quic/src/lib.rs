//! # rustp2p-quic - QUIC-based Reliable P2P Transport
//!
//! `rustp2p-quic` provides reliable, encrypted peer-to-peer connections using QUIC.
//! It integrates with `rustp2p-core` by sharing the UDP socket via `AsyncUdpSocket`,
//! allowing STUN hole-punching and QUIC connections to coexist on the same socket.
//!
//! ## Architecture
//!
//! ```text
//!                 ┌─────────────────────────┐
//!                 │    Shared UDP Socket      │
//!                 │  (Single local port)     │
//!                 └──────────┬──────────────┘
//!                            │
//!                 ┌──────────▼──────────────┐
//!                 │  AsyncUdpSocket impl     │
//!                 │  (Packet demuxing)       │
//!                 └──────────┬──────────────┘
//!                            │ First byte check
//!                 ┌──────────┼──────────────┐
//!                 ▼          ▼              ▼
//!            STUN (0x01)  QUIC (0x80+)   Other (0x02-0x7f)
//!          rustp2p-core    quinn       custom handler
//! ```
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use rustp2p_quic::{Endpoint, NodeAddr};
//!
//! # #[tokio::main]
//! # async fn main() -> rustp2p_quic::Result<()> {
//! let endpoint = Endpoint::bind("0.0.0.0:0".parse()?).await?;
//! let node_addr = NodeAddr::new([0u8; 32], vec!["1.2.3.4:4433".parse()?]);
//! let connection = endpoint.connect(node_addr).await?;
//! let (mut send, mut recv) = connection.open_bi().await?;
//! send.write_all(b"hello").await?;
//! # Ok(())
//! # }
//! ```

mod config;
mod connection;
mod demux;
mod endpoint;

pub use config::Config;
pub use connection::{Connection, RecvStream, SendStream};
pub use endpoint::{Endpoint, NodeAddr};
pub use rust_p2p_core::nat::NatInfo;

/// Re-exported result type.
pub type Result<T> = std::io::Result<T>;
