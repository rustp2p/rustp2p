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
//! use rustp2p_quic::{Endpoint, Identity, PeerId};
//!
//! # #[tokio::main]
//! # async fn main() -> rustp2p_quic::Result<()> {
//! let endpoint = Endpoint::builder()
//!     .identity(Identity::new("node-a", "seed-a")?)
//!     .bind("0.0.0.0:0".parse().unwrap())
//!     .build()
//!     .await?;
//! let (mut send, mut recv) = endpoint.open_bi(PeerId::from("node-b")).await?;
//! send.write_all(b"hello").await?;
//! # Ok(())
//! # }
//! ```

mod cert;
mod config;
mod connection;
mod demux;
mod endpoint;
mod identity;
mod protocol;
mod reliable;

pub use cert::{CertificateVerifier, SkipCertificateVerification};
pub use config::Config;
pub use connection::{Connection, RecvStream, SendStream};
pub use demux::{classify_packet, PacketType, ReceivedPacket};
pub use endpoint::{
    Builder, Endpoint, IncomingBiStream, LinkInfo, LinkMode, NodeAddr, PeerInfo, ReceivedMessage,
};
pub use identity::{Identity, PeerId};
pub use reliable::{ReliableRecvStream, ReliableSendStream};
pub use rust_p2p_core::nat::NatInfo;

/// Re-exported result type.
pub type Result<T> = std::io::Result<T>;
