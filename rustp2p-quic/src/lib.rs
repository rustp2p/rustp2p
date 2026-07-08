//! # rustp2p-quic - PeerId QUIC Overlay
//!
//! `rustp2p-quic` provides reliable, encrypted peer-to-peer connections using QUIC.
//! It uses `rustp2p-core` as the transport layer and keeps the high-level API based
//! on `PeerId` rather than socket addresses.
//!
//! ## Architecture
//!
//! ```text
//! application API
//!       |
//!       v
//! Endpoint public API
//!       |
//!       v
//! quic module (quinn + synthetic PeerId addresses)
//!       |
//!       v
//! protocol module (wire packets + control plane)
//!       |
//!       v
//! transport module (rustp2p-core endpoint + PeerId routes)
//!       |
//!       v
//! real transport links and relay forwarding
//! ```
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use rustp2p_quic::{Endpoint, Identity, PeerId};
//!
//! async fn example() -> rustp2p_quic::Result<()> {
//!     let endpoint = Endpoint::builder()
//!         .identity(Identity::new("node-a", "seed-a")?)
//!         .bind("0.0.0.0:0".parse().unwrap())
//!         .build()
//!         .await?;
//!     let (mut send, _recv) = endpoint.open_bi(PeerId::from("node-b")).await?;
//!     send.write_all(b"hello").await?;
//!     Ok(())
//! }
//! ```

mod cert;
mod config;
mod connection;
mod endpoint;
mod identity;
mod protocol;
mod quic;
mod reliable;
mod transport;

pub use cert::{CertificateVerifier, SkipCertificateVerification};
pub use config::Config;
pub use endpoint::{Builder, Endpoint};
pub use identity::{Identity, PeerId};
pub use protocol::{classify_packet, PacketType};
pub use quic::{IncomingBiStream, ReceivedMessage};
pub use reliable::{ReliableRecvStream, ReliableSendStream};
pub use rust_p2p_core::nat::NatInfo;
pub use transport::{LinkInfo, LinkMode, PeerInfo, TransportHandle, TransportMessage};

/// Re-exported result type.
pub type Result<T> = std::io::Result<T>;
