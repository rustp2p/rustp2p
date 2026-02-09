//! # rustp2p - Decentralized P2P Library
//!
//! `rustp2p` is a decentralized peer-to-peer library written in Rust that provides simple and 
//! efficient NAT traversal and peer-to-peer communication. It supports both UDP and TCP hole 
//! punching, reliable transport over KCP, and secure encryption.
//!
//! ## Features
//!
//! - **UDP Hole Punching**: Works with both Cone NAT and Symmetric NAT
//! - **TCP Hole Punching**: Supports NAT1 scenarios
//! - **Reliable Transport**: Built-in KCP support for reliable UDP communication
//! - **Encryption**: Supports AES-GCM and ChaCha20-Poly1305 encryption
//! - **Simple API**: Easy-to-use builder pattern for configuration
//! - **Auto Route Discovery**: Automatically finds the best path between peers
//!
//! ## Quick Start
//!
//! Here's a minimal example to get you started:
//!
//! ```rust,no_run
//! use rustp2p::Builder;
//! use std::net::Ipv4Addr;
//!
//! #[tokio::main]
//! async fn main() -> std::io::Result<()> {
//!     // Create a node with ID 10.0.0.1
//!     let node_id = Ipv4Addr::new(10, 0, 0, 1);
//!     let endpoint = Builder::new()
//!         .node_id(node_id.into())
//!         .udp_port(8080)
//!         .build()
//!         .await?;
//!
//!     // Receive messages
//!     tokio::spawn(async move {
//!         loop {
//!             if let Ok((data, metadata)) = endpoint.recv_from().await {
//!                 let src: Ipv4Addr = metadata.src_id().into();
//!                 println!("Received from {}: {:?}", src, data.payload());
//!             }
//!         }
//!     });
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Basic Usage
//!
//! ### Creating a P2P Node
//!
//! Use the [`Builder`] to configure and create an [`EndPoint`]:
//!
//! ```rust,no_run
//! use rustp2p::{Builder, PeerNodeAddress};
//! use std::net::Ipv4Addr;
//! use std::str::FromStr;
//!
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! let endpoint = Builder::new()
//!     .node_id(Ipv4Addr::new(10, 0, 0, 1).into())
//!     .udp_port(8080)
//!     .tcp_port(8080)
//!     // Add initial peers to connect to
//!     .peers(vec![
//!         PeerNodeAddress::from_str("udp://192.168.1.100:9090").unwrap()
//!     ])
//!     .build()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Sending Messages
//!
//! Send data to a peer using their node ID:
//!
//! ```rust,no_run
//! # use rustp2p::{Builder, EndPoint};
//! # use std::net::Ipv4Addr;
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! # let endpoint = Builder::new().node_id(Ipv4Addr::new(10, 0, 0, 1).into()).udp_port(8080).build().await?;
//! let peer_id = Ipv4Addr::new(10, 0, 0, 2);
//! endpoint.send_to(b"Hello, peer!", peer_id).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Receiving Messages
//!
//! Receive data from any peer:
//!
//! ```rust,no_run
//! # use rustp2p::{Builder, EndPoint};
//! # use std::net::Ipv4Addr;
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! # let endpoint = Builder::new().node_id(Ipv4Addr::new(10, 0, 0, 1).into()).udp_port(8080).build().await?;
//! loop {
//!     match endpoint.recv_from().await {
//!         Ok((data, metadata)) => {
//!             let src: Ipv4Addr = metadata.src_id().into();
//!             println!("From {}: {:?}", src, data.payload());
//!         }
//!         Err(e) => {
//!             eprintln!("Error receiving: {}", e);
//!             break;
//!         }
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Advanced Features
//!
//! ### Using Encryption
//!
//! Enable secure communication with AES-GCM encryption:
//!
//! ```rust,no_run
//! # #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
//! # {
//! use rustp2p::{Builder, cipher::Algorithm};
//! use std::net::Ipv4Addr;
//!
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! let endpoint = Builder::new()
//!     .node_id(Ipv4Addr::new(10, 0, 0, 1).into())
//!     .udp_port(8080)
//!     .encryption(Algorithm::AesGcm("my-secret-password".to_string()))
//!     .build()
//!     .await?;
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! ### Group Networking
//!
//! Use group codes to create isolated P2P networks:
//!
//! ```rust,no_run
//! use rustp2p::{Builder, GroupCode};
//! use std::net::Ipv4Addr;
//!
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! let endpoint = Builder::new()
//!     .node_id(Ipv4Addr::new(10, 0, 0, 1).into())
//!     .udp_port(8080)
//!     .group_code(GroupCode::try_from("12345").unwrap())
//!     .build()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Broadcasting Messages
//!
//! Send a message to all connected peers:
//!
//! ```rust,no_run
//! # use rustp2p::Builder;
//! # use std::net::Ipv4Addr;
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! # let endpoint = Builder::new().node_id(Ipv4Addr::new(10, 0, 0, 1).into()).udp_port(8080).build().await?;
//! endpoint.broadcast(b"Hello everyone!").await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Non-blocking Operations
//!
//! Use try_* variants for non-blocking operations:
//!
//! ```rust,no_run
//! # use rustp2p::Builder;
//! # use std::net::Ipv4Addr;
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! # let endpoint = Builder::new().node_id(Ipv4Addr::new(10, 0, 0, 1).into()).udp_port(8080).build().await?;
//! let peer_id = Ipv4Addr::new(10, 0, 0, 2);
//! 
//! // Non-blocking send
//! match endpoint.try_send_to(b"Quick message", peer_id) {
//!     Ok(_) => println!("Sent successfully"),
//!     Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
//!         println!("Would block, try again later")
//!     }
//!     Err(e) => eprintln!("Error: {}", e),
//! }
//!
//! // Non-blocking receive
//! match endpoint.try_recv_from() {
//!     Ok((data, metadata)) => {
//!         println!("Received: {:?}", data.payload());
//!     }
//!     Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
//!         println!("No data available")
//!     }
//!     Err(e) => eprintln!("Error: {}", e),
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Architecture
//!
//! The library is organized into several key components:
//!
//! - **EndPoint**: The main interface for sending and receiving data
//! - **Builder**: Fluent API for configuring and creating endpoints
//! - **NodeID**: Unique identifier for each peer (based on IPv4)
//! - **Tunnel**: Underlying transport mechanism (UDP/TCP)
//! - **Cipher**: Optional encryption layer
//!
//! ## How It Works
//!
//! 1. **Peer Discovery**: Nodes connect to known peers and discover others
//! 2. **NAT Traversal**: UDP/TCP hole punching establishes direct connections
//! 3. **Route Selection**: Automatically selects the best path (direct or relayed)
//! 4. **Data Transfer**: Encrypted communication between peers
//!
//! ## Examples
//!
//! See the `examples/` directory for complete working examples:
//! - `node.rs` - Basic P2P node
//! - `node_kcp_stream.rs` - Using KCP for reliable transport
//!
//! ## See Also
//!
//! - [`rustp2p-core`](../rust_p2p_core/index.html) - Core NAT traversal functionality
//! - [`rustp2p-reliable`](../rustp2p_reliable/index.html) - Reliable transport layer

mod protocol;

pub mod cipher;
mod config;
mod extend;
mod reliable;
mod tunnel;
pub use protocol::{
    node_id::{self, GroupCode, NodeID},
    NetPacket, HEAD_LEN,
};
pub use tunnel::{NodeAddress, PeerNodeAddress, RecvUserData};

#[cfg(feature = "use-kcp")]
pub use reliable::*;

use crate::config::PunchingPolicy;
pub use crate::config::{DataInterceptor, DefaultInterceptor};
use crate::protocol::protocol_type::ProtocolType;
pub use crate::tunnel::{RecvMetadata, RecvResult};
use async_trait::async_trait;
use cipher::Algorithm;
pub use config::{
    Config, NatType, PunchModel, PunchPolicy, PunchPolicySet, TcpTunnelConfig, UdpTunnelConfig,
};
use flume::{Receiver, Sender, TryRecvError};
pub use rust_p2p_core::route::RouteKey;
pub use rust_p2p_core::socket::LocalInterface;
use rust_p2p_core::tunnel::config::LoadBalance;
use std::io;
use std::ops::Deref;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tunnel::{Tunnel, TunnelDispatcher, TunnelRouter};

/// The main endpoint for peer-to-peer communication.
///
/// `EndPoint` represents a P2P node that can send and receive data to/from other peers.
/// It handles NAT traversal, routing, and maintains connections with peers.
///
/// # Examples
///
/// ```rust,no_run
/// use rustp2p::Builder;
/// use std::net::Ipv4Addr;
///
/// #[tokio::main]
/// async fn main() -> std::io::Result<()> {
///     let endpoint = Builder::new()
///         .node_id(Ipv4Addr::new(10, 0, 0, 1).into())
///         .udp_port(8080)
///         .build()
///         .await?;
///     
///     // Send to a peer
///     let peer_id = Ipv4Addr::new(10, 0, 0, 2);
///     endpoint.send_to(b"hello", peer_id).await?;
///     
///     // Receive from any peer
///     let (data, metadata) = endpoint.recv_from().await?;
///     println!("Received: {:?}", data.payload());
///     
///     Ok(())
/// }
/// ```
pub struct EndPoint {
    #[cfg(feature = "use-kcp")]
    kcp_context: KcpContext,
    input: Receiver<(RecvUserData, RecvMetadata)>,
    output: TunnelRouter,
    _handle: OwnedJoinHandle,
}

impl EndPoint {
    /// Receives data from any connected peer (blocking).
    ///
    /// This method blocks until data is received or an error occurs.
    ///
    /// # Returns
    ///
    /// Returns a tuple containing:
    /// - `RecvUserData`: The received data payload
    /// - `RecvMetadata`: Metadata about the message (source, destination, relay info)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use rustp2p::Builder;
    /// # use std::net::Ipv4Addr;
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// # let endpoint = Builder::new().node_id(Ipv4Addr::new(10, 0, 0, 1).into()).udp_port(8080).build().await?;
    /// loop {
    ///     let (data, metadata) = endpoint.recv_from().await?;
    ///     let src: Ipv4Addr = metadata.src_id().into();
    ///     println!("From {}: {:?}", src, data.payload());
    /// }
    /// # }
    /// ```
    pub async fn recv_from(&self) -> io::Result<(RecvUserData, RecvMetadata)> {
        self.input
            .recv_async()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "shutdown"))
    }
    /// Sends data to a specific peer (blocking).
    ///
    /// # Arguments
    ///
    /// * `buf` - The data to send
    /// * `dest` - The destination node ID (can be `NodeID` or `Ipv4Addr`)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use rustp2p::Builder;
    /// # use std::net::Ipv4Addr;
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// # let endpoint = Builder::new().node_id(Ipv4Addr::new(10, 0, 0, 1).into()).udp_port(8080).build().await?;
    /// let peer_id = Ipv4Addr::new(10, 0, 0, 2);
    /// endpoint.send_to(b"Hello, peer!", peer_id).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send_to<D: Into<NodeID>>(&self, buf: &[u8], dest: D) -> io::Result<()> {
        self.output.send_to(buf, dest).await
    }
    /// Attempts to receive data without blocking.
    ///
    /// Returns immediately with `ErrorKind::WouldBlock` if no data is available.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use rustp2p::Builder;
    /// # use std::net::Ipv4Addr;
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// # let endpoint = Builder::new().node_id(Ipv4Addr::new(10, 0, 0, 1).into()).udp_port(8080).build().await?;
    /// match endpoint.try_recv_from() {
    ///     Ok((data, metadata)) => {
    ///         println!("Received: {:?}", data.payload());
    ///     }
    ///     Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
    ///         println!("No data available");
    ///     }
    ///     Err(e) => return Err(e),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_recv_from(&self) -> io::Result<(RecvUserData, RecvMetadata)> {
        match self.input.try_recv() {
            Ok(rs) => Ok(rs),
            Err(e) => match e {
                TryRecvError::Empty => Err(io::Error::from(io::ErrorKind::WouldBlock)),
                TryRecvError::Disconnected => {
                    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "shutdown"))
                }
            },
        }
    }
    /// Attempts to send data without blocking.
    ///
    /// Returns immediately with `ErrorKind::WouldBlock` if unable to send.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use rustp2p::Builder;
    /// # use std::net::Ipv4Addr;
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// # let endpoint = Builder::new().node_id(Ipv4Addr::new(10, 0, 0, 1).into()).udp_port(8080).build().await?;
    /// let peer_id = Ipv4Addr::new(10, 0, 0, 2);
    /// match endpoint.try_send_to(b"Quick message", peer_id) {
    ///     Ok(_) => println!("Sent successfully"),
    ///     Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
    ///         println!("Would block, try again later")
    ///     }
    ///     Err(e) => return Err(e),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_send_to<D: Into<NodeID>>(&self, buf: &[u8], dest: D) -> io::Result<()> {
        self.output.try_send_to(buf, dest)
    }
    /// Sends data to a peer using a specific route.
    ///
    /// This allows you to explicitly control which path the data takes to reach the destination.
    ///
    /// # Arguments
    ///
    /// * `buf` - The data to send
    /// * `dest` - The destination node ID
    /// * `route_key` - The specific route to use
    pub async fn send_to_route<D: Into<NodeID>>(
        &self,
        buf: &[u8],
        dest: D,
        route_key: &RouteKey,
    ) -> io::Result<()> {
        let mut send_packet = self.output.allocate_send_packet();
        send_packet.set_payload(buf);
        self.output
            .send_packet_to_route(send_packet, &dest.into(), Some(route_key))
            .await
    }
    /// Attempts to send data to a peer using a specific route without blocking.
    pub async fn try_send_to_route<D: Into<NodeID>>(
        &self,
        buf: &[u8],
        dest: D,
        route_key: &RouteKey,
    ) -> io::Result<()> {
        let mut send_packet = self.output.allocate_send_packet();
        send_packet.set_payload(buf);
        self.output
            .try_send_packet_to_route(send_packet, &dest.into(), Some(route_key))
    }

    /// Broadcasts data to all connected peers.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use rustp2p::Builder;
    /// # use std::net::Ipv4Addr;
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// # let endpoint = Builder::new().node_id(Ipv4Addr::new(10, 0, 0, 1).into()).udp_port(8080).build().await?;
    /// endpoint.broadcast(b"Hello everyone!").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn broadcast(&self, buf: &[u8]) -> io::Result<()> {
        let mut send_packet = self.output.allocate_send_packet();
        send_packet.set_payload(buf);
        self.output.broadcast_packet(send_packet).await
    }
}

impl Deref for EndPoint {
    type Target = TunnelRouter;

    fn deref(&self) -> &Self::Target {
        &self.output
    }
}
struct OwnedJoinHandle {
    handle: JoinHandle<()>,
}
impl Drop for OwnedJoinHandle {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Builder for creating a P2P endpoint with custom configuration.
///
/// The `Builder` uses the builder pattern to provide a fluent API for configuring
/// all aspects of a P2P endpoint before creating it.
///
/// # Examples
///
/// ## Basic Configuration
///
/// ```rust,no_run
/// use rustp2p::Builder;
/// use std::net::Ipv4Addr;
///
/// # #[tokio::main]
/// # async fn main() -> std::io::Result<()> {
/// let endpoint = Builder::new()
///     .node_id(Ipv4Addr::new(10, 0, 0, 1).into())
///     .udp_port(8080)
///     .build()
///     .await?;
/// # Ok(())
/// # }
/// ```
///
/// ## Full Configuration
///
/// ```rust,no_run
/// use rustp2p::{Builder, PeerNodeAddress, GroupCode};
/// # #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
/// use rustp2p::cipher::Algorithm;
/// use std::net::Ipv4Addr;
/// use std::str::FromStr;
///
/// # #[tokio::main]
/// # async fn main() -> std::io::Result<()> {
/// # #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
/// # {
/// let endpoint = Builder::new()
///     .node_id(Ipv4Addr::new(10, 0, 0, 1).into())
///     .udp_port(8080)
///     .tcp_port(8080)
///     .peers(vec![
///         PeerNodeAddress::from_str("udp://192.168.1.100:9090").unwrap()
///     ])
///     .group_code(GroupCode::try_from("12345").unwrap())
///     .encryption(Algorithm::AesGcm("password".to_string()))
///     .build()
///     .await?;
/// # }
/// # #[cfg(not(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring")))]
/// # {
/// # let endpoint = Builder::new()
/// #     .node_id(Ipv4Addr::new(10, 0, 0, 1).into())
/// #     .udp_port(8080)
/// #     .build()
/// #     .await?;
/// # }
/// # Ok(())
/// # }
/// ```
pub struct Builder {
    udp_port: Option<u16>,
    tcp_port: Option<u16>,
    peers: Option<Vec<PeerNodeAddress>>,
    group_code: Option<GroupCode>,
    encryption: Option<Algorithm>,
    node_id: Option<NodeID>,
    interceptor: Option<Interceptor>,
    punching_policy: Option<Arc<dyn PunchingPolicy>>,
    load_balance: Option<LoadBalance>,
    interface: Option<LocalInterface>,
    config: Option<Config>,
}
impl Builder {
    /// Creates a new builder with default settings.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::Builder;
    ///
    /// let builder = Builder::new();
    /// ```
    pub fn new() -> Self {
        Self {
            udp_port: None,
            tcp_port: None,
            peers: None,
            group_code: None,
            encryption: None,
            node_id: None,
            interceptor: None,
            punching_policy: None,
            load_balance: None,
            interface: None,
            config: None,
        }
    }
    /// Creates a builder from an existing configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - A pre-configured `Config` object
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rustp2p::{Builder, Config};
    ///
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// let config = Config::empty();
    /// let builder = Builder::from_config(config);
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_config(config: Config) -> Self {
        Self {
            udp_port: None,
            tcp_port: None,
            peers: None,
            group_code: None,
            encryption: None,
            node_id: None,
            interceptor: None,
            punching_policy: None,
            load_balance: None,
            interface: None,
            config: Some(config),
        }
    }
    /// Sets the UDP port for this endpoint.
    ///
    /// # Arguments
    ///
    /// * `port` - The port number to bind to (0 for random)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::Builder;
    ///
    /// let builder = Builder::new().udp_port(8080);
    /// ```
    pub fn udp_port(mut self, port: u16) -> Self {
        self.udp_port = Some(port);
        self
    }
    /// Sets the TCP port for this endpoint.
    ///
    /// # Arguments
    ///
    /// * `port` - The port number to bind to (0 for random)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::Builder;
    ///
    /// let builder = Builder::new().tcp_port(8080);
    /// ```
    pub fn tcp_port(mut self, port: u16) -> Self {
        self.tcp_port = Some(port);
        self
    }
    /// Sets the initial list of peers to connect to.
    ///
    /// These peers will be contacted when the endpoint starts to help with
    /// peer discovery and NAT traversal.
    ///
    /// # Arguments
    ///
    /// * `peers` - A vector of peer addresses
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rustp2p::{Builder, PeerNodeAddress};
    /// use std::str::FromStr;
    ///
    /// let builder = Builder::new()
    ///     .peers(vec![
    ///         PeerNodeAddress::from_str("udp://192.168.1.100:9090").unwrap(),
    ///         PeerNodeAddress::from_str("tcp://10.0.0.1:8080").unwrap(),
    ///     ]);
    /// ```
    pub fn peers(mut self, peers: Vec<PeerNodeAddress>) -> Self {
        self.peers = Some(peers);
        self
    }
    /// Sets the group code for this endpoint.
    ///
    /// Endpoints with different group codes will not be able to communicate.
    /// This allows you to create isolated P2P networks.
    ///
    /// # Arguments
    ///
    /// * `group_code` - A unique group identifier
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::Builder;
    /// use rustp2p::GroupCode;
    ///
    /// let builder = Builder::new()
    ///     .group_code(GroupCode::try_from("12345").unwrap());
    /// ```
    pub fn group_code(mut self, group_code: GroupCode) -> Self {
        self.group_code = Some(group_code);
        self
    }
    /// Sets the encryption algorithm for this endpoint.
    ///
    /// When encryption is enabled, all communication will be encrypted with the
    /// specified algorithm and password. All peers must use the same encryption
    /// settings to communicate.
    ///
    /// # Arguments
    ///
    /// * `encryption` - The encryption algorithm and password
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
    /// # {
    /// use rustp2p::{Builder, cipher::Algorithm};
    ///
    /// let builder = Builder::new()
    ///     .encryption(Algorithm::AesGcm("my-secret-password".to_string()));
    /// # }
    /// ```
    pub fn encryption(mut self, encryption: Algorithm) -> Self {
        self.encryption = Some(encryption);
        self
    }
    /// Sets the node ID for this endpoint (required).
    ///
    /// Each peer in the network must have a unique node ID. The node ID is used
    /// to identify and route messages to the correct destination.
    ///
    /// # Arguments
    ///
    /// * `node_id` - A unique identifier for this node
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::Builder;
    /// use std::net::Ipv4Addr;
    ///
    /// let builder = Builder::new()
    ///     .node_id(Ipv4Addr::new(10, 0, 0, 1).into());
    /// ```
    pub fn node_id(mut self, node_id: NodeID) -> Self {
        self.node_id = Some(node_id);
        self
    }
    /// Sets a custom data interceptor.
    ///
    /// The interceptor can inspect and modify data before it is processed.
    ///
    /// # Arguments
    ///
    /// * `interceptor` - An implementation of the `DataInterceptor` trait
    pub fn interceptor<I: DataInterceptor + 'static>(mut self, interceptor: I) -> Self {
        self.interceptor = Some(Interceptor {
            inner: Arc::new(interceptor),
        });
        self
    }
    /// Sets a custom punching policy.
    ///
    /// The punching policy determines how NAT traversal is attempted.
    ///
    /// # Arguments
    ///
    /// * `punching_policy` - An implementation of the `PunchingPolicy` trait
    pub fn punching_policy<I: PunchingPolicy + 'static>(mut self, punching_policy: I) -> Self {
        self.punching_policy = Some(Arc::new(punching_policy));
        self
    }
    /// Sets the load balancing strategy.
    ///
    /// Determines how traffic is distributed across multiple available routes.
    ///
    /// # Arguments
    ///
    /// * `load_balance` - The load balancing strategy
    pub fn load_balance(mut self, load_balance: LoadBalance) -> Self {
        self.load_balance = Some(load_balance);
        self
    }
    /// Binds to a specific network interface.
    ///
    /// # Arguments
    ///
    /// * `interface` - The network interface to bind to
    pub fn bind_interface(mut self, interface: LocalInterface) -> Self {
        self.interface = Some(interface);
        self
    }
    /// Builds and returns the configured endpoint.
    ///
    /// This method consumes the builder and creates an `EndPoint` with all
    /// the specified configuration options.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The node ID was not set
    /// - Unable to bind to the specified ports
    /// - Other configuration errors
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rustp2p::Builder;
    /// use std::net::Ipv4Addr;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// let endpoint = Builder::new()
    ///     .node_id(Ipv4Addr::new(10, 0, 0, 1).into())
    ///     .udp_port(8080)
    ///     .build()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn build(self) -> io::Result<EndPoint> {
        let mut config = if let Some(config) = self.config {
            config
        } else {
            Config::empty()
                .set_udp_tunnel_config(
                    UdpTunnelConfig::default()
                        .set_simple_udp_port(self.udp_port.unwrap_or_default()),
                )
                .set_tcp_tunnel_config(
                    TcpTunnelConfig::default().set_tcp_port(self.tcp_port.unwrap_or(0)),
                )
                .set_node_id(self.node_id.ok_or(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "node_id is required",
                ))?)
        };
        if let Some(interface) = self.interface {
            config = config.set_default_interface(interface);
        }
        if let Some(punching_policy) = self.punching_policy {
            config = config.set_punching_policy_arc(punching_policy);
        }
        if let Some(load_balance) = self.load_balance {
            config = config.set_load_balance(load_balance);
        }
        if let Some(group_code) = self.group_code {
            config = config.set_group_code(group_code);
        }
        if let Some(peers) = self.peers {
            config = config.set_direct_addrs(peers);
        }
        #[cfg(any(
            feature = "aes-gcm-openssl",
            feature = "aes-gcm-ring",
            feature = "chacha20-poly1305-openssl",
            feature = "chacha20-poly1305-ring"
        ))]
        if let Some(encryption) = self.encryption {
            config = config.set_encryption(encryption);
        }

        let tunnel_manager = TunnelDispatcher::new(config).await?;

        EndPoint::with_interceptor_impl(tunnel_manager, self.interceptor).await
    }
}
impl EndPoint {
    /// Creates a new endpoint from a configuration object.
    ///
    /// For most use cases, prefer using [`Builder`] instead.
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration for this endpoint
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rustp2p::{EndPoint, Config};
    ///
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// let config = Config::empty();
    /// let endpoint = EndPoint::new(config).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(config: Config) -> io::Result<Self> {
        let tunnel_hub = TunnelDispatcher::new(config).await?;
        EndPoint::with_interceptor_impl(tunnel_hub, None).await
    }
    /// Creates a new endpoint with a custom data interceptor.
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration for this endpoint
    /// * `interceptor` - A custom data interceptor
    pub async fn with_interceptor<T: DataInterceptor + 'static>(
        config: Config,
        interceptor: T,
    ) -> io::Result<Self> {
        let tunnel_hub = TunnelDispatcher::new(config).await?;
        let interceptor = Some(Interceptor {
            inner: Arc::new(interceptor),
        });
        EndPoint::with_interceptor_impl(tunnel_hub, interceptor).await
    }
    async fn with_interceptor_impl(
        mut tunnel_hub: TunnelDispatcher,
        interceptor: Option<Interceptor>,
    ) -> io::Result<Self> {
        let (sender, receiver) = flume::unbounded();
        let writer = tunnel_hub.tunnel_router();
        #[cfg(feature = "use-kcp")]
        let kcp_context = KcpContext::default();
        #[cfg(feature = "use-kcp")]
        let kcp_data_input = kcp_context.clone();
        let handle = tokio::spawn(async move {
            while let Ok(tunnel_rx) = tunnel_hub.dispatch().await {
                tokio::spawn(handle(
                    tunnel_rx,
                    sender.clone(),
                    #[cfg(feature = "use-kcp")]
                    kcp_data_input.clone(),
                    interceptor.clone(),
                ));
            }
        });
        let _handle = OwnedJoinHandle { handle };
        Ok(EndPoint {
            #[cfg(feature = "use-kcp")]
            kcp_context,
            output: writer,
            input: receiver,
            _handle,
        })
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

async fn handle(
    mut tunnel_rx: Tunnel,
    sender: Sender<(RecvUserData, RecvMetadata)>,
    #[cfg(feature = "use-kcp")] kcp_data_input: KcpContext,
    interceptor: Option<Interceptor>,
) {
    let mut list = Vec::with_capacity(16);
    let interceptor = interceptor.as_ref();
    loop {
        let rs = match tunnel_rx
            .preprocess_batch_recv(interceptor, &mut list)
            .await
        {
            Ok(rs) => rs,
            Err(e) => {
                log::debug!(
                    "recv_from {e:?},{:?} {:?}",
                    tunnel_rx.protocol(),
                    tunnel_rx.remote_addr()
                );
                return;
            }
        };
        if let Err(e) = rs {
            log::debug!("recv_data_handle {e:?}");
            continue;
        };
        for (data, meta_data) in list.drain(..) {
            if meta_data.protocol() == ProtocolType::KcpData.into() {
                #[cfg(feature = "use-kcp")]
                kcp_data_input
                    .input(data.payload(), meta_data.src_id())
                    .await;
            } else if sender.send_async((data, meta_data)).await.is_err() {
                break;
            }
        }
    }
}
#[derive(Clone)]
pub(crate) struct Interceptor {
    inner: Arc<dyn DataInterceptor>,
}
#[async_trait]
impl DataInterceptor for Interceptor {
    async fn pre_handle(&self, data: &mut RecvResult) -> bool {
        self.inner.pre_handle(data).await
    }
}
