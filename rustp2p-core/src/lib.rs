//! # rustp2p-core - Core NAT Traversal Library
//!
//! `rustp2p-core` provides the foundational building blocks for peer-to-peer networking,
//! including NAT traversal, hole punching, routing, and tunnel management. This crate is
//! the underlying engine that powers the higher-level `rustp2p` library.
//!
//! ## Features
//!
//! - **NAT Detection**: Automatically detects NAT type using STUN
//! - **Hole Punching**: UDP and TCP hole punching for direct peer connections
//! - **Route Management**: Dynamic routing with multi-path support
//! - **Tunnel Management**: Unified management of UDP and TCP tunnels
//! - **STUN Protocol**: Built-in STUN client for NAT traversal
//! - **Socket Management**: Advanced socket handling with multi-interface support
//!
//! ## Architecture
//!
//! The library is organized into several key modules:
//!
//! - [`nat`] - NAT type detection and information
//! - [`punch`] - Hole punching for NAT traversal
//! - [`route`] - Routing and path selection
//! - [`socket`] - Low-level socket management
//! - [`stun`] - STUN protocol implementation
//! - [`tunnel`] - Tunnel abstraction for UDP/TCP
//!
//! ## Quick Start
//!
//! ### Creating a Simple Tunnel
//!
//! ```rust,no_run
//! use rust_p2p_core::tunnel::{TunnelConfig, new_tunnel_component};
//!
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! // Create a tunnel configuration
//! let config = TunnelConfig::default();
//!
//! // Create tunnel components
//! let (dispatcher, puncher) = new_tunnel_component(config)?;
//!
//! // Use dispatcher to handle incoming connections
//! while let Ok(tunnel) = dispatcher.dispatch().await {
//!     tokio::spawn(async move {
//!         // Handle the tunnel
//!     });
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## NAT Type Detection
//!
//! Detect your NAT type using STUN servers:
//!
//! ```rust,no_run
//! use rust_p2p_core::stun::stun_test_nat;
//!
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! let stun_servers = vec![
//!     "stun.l.google.com:19302".to_string(),
//!     "stun1.l.google.com:19302".to_string(),
//! ];
//!
//! let (nat_type, public_ips, port_range) =
//!     stun_test_nat(stun_servers, None).await?;
//!
//! println!("NAT Type: {:?}", nat_type);
//! println!("Public IPs: {:?}", public_ips);
//! println!("Port Range: {}", port_range);
//! # Ok(())
//! # }
//! ```
//!
//! ## Hole Punching
//!
//! Perform UDP hole punching to establish direct connections:
//!
//! ```rust,no_run
//! use rust_p2p_core::punch::{Puncher, PunchInfo};
//! use std::net::SocketAddr;
//!
//! # async fn example(puncher: Puncher) -> std::io::Result<()> {
//! // Create punch info for the target peer
//! let punch_info = PunchInfo::default();
//!
//! // Attempt to punch through NAT
//! puncher.punch_now(None, b"punch", punch_info).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Route Management
//!
//! The routing system automatically manages paths between peers:
//!
//! ```rust,no_run
//! use rust_p2p_core::route::{Route, RouteKey};
//! use std::net::SocketAddr;
//!
//! # fn example() {
//! // Routes are managed automatically by the tunnel system
//! // You can query route information using RouteKey
//! # }
//! ```
//!
//! ## Socket Management
//!
//! Low-level socket creation and management:
//!
//! ```rust,no_run
//! use rust_p2p_core::socket::{create_udp_socket, LocalInterface};
//! use std::net::{SocketAddr, Ipv4Addr};
//!
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
//! let socket = create_udp_socket(addr, None)?;
//! # Ok(())
//! # }
//! ```
//!
//! ## UDP vs TCP Tunnels
//!
//! The library supports both UDP and TCP tunnels:
//!
//! - **UDP Tunnels**: Low latency, suitable for real-time communication
//! - **TCP Tunnels**: Reliable, ordered delivery
//!
//! Both tunnel types provide a unified interface through the `Tunnel` enum.
//!
//! ## Advanced Features
//!
//! ### Custom Punch Policies
//!
//! Implement custom NAT traversal strategies by providing your own punch policy.
//!
//! ### Multi-Interface Support
//!
//! Bind to specific network interfaces for more control over routing.
//!
//! ### Route Metrics
//!
//! The routing system considers metrics like latency and packet loss when
//! selecting the best path.
//!
//! ## Thread Safety
//!
//! All public types are thread-safe and can be shared across async tasks.
//! The library uses Tokio for async runtime.
//!
//! ## See Also
//!
//! - [`rustp2p`](../rustp2p/index.html) - High-level P2P library built on top of this crate
//! - [`rustp2p-reliable`](../rustp2p_reliable/index.html) - Reliable transport using KCP

pub mod extend;
pub mod idle;
pub mod nat;
pub mod punch;
pub mod route;
pub mod socket;
pub mod stun;
pub mod tunnel;
