//! NAT (Network Address Translation) detection and information.
//!
//! This module provides types and functions for detecting and working with NAT.
//! It helps identify the NAT type (Cone vs Symmetric) and tracks public/private
//! address mappings.
//!
//! # Examples
//!
//! ```rust,no_run
//! use rust_p2p_core::nat::{NatType, NatInfo};
//!
//! let nat_info = NatInfo {
//!     nat_type: NatType::Cone,
//!     public_ips: vec![],
//!     // ... other fields
//! #   public_udp_ports: vec![],
//! #   mapping_tcp_addr: vec![],
//! #   mapping_udp_addr: vec![],
//! #   public_port_range: 0,
//! #   local_ipv4: std::net::Ipv4Addr::UNSPECIFIED,
//! #   local_ipv4s: vec![],
//! #   ipv6: None,
//! #   local_udp_ports: vec![],
//! #   local_tcp_port: 0,
//! #   public_tcp_port: 0,
//! };
//!
//! if nat_info.nat_type.is_cone() {
//!     println!("Cone NAT - easier to traverse");
//! }
//! ```

use crate::extend::addr::is_ipv6_global;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

/// Type of NAT (Network Address Translation).
///
/// # Variants
///
/// - `Cone` - Port mapping is consistent, easier for hole punching
/// - `Symmetric` - Port mapping changes per destination, harder to traverse
///
/// # Examples
///
/// ```rust
/// use rust_p2p_core::nat::NatType;
///
/// let nat = NatType::Cone;
/// assert!(nat.is_cone());
/// assert!(!nat.is_symmetric());
/// ```
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, Default)]
pub enum NatType {
    #[default]
    Cone,
    Symmetric,
}

impl NatType {
    /// Returns true if this is a Cone NAT.
    #[inline]
    pub fn is_cone(&self) -> bool {
        self == &NatType::Cone
    }
    
    /// Returns true if this is a Symmetric NAT.
    #[inline]
    pub fn is_symmetric(&self) -> bool {
        self == &NatType::Symmetric
    }
}

/// Comprehensive NAT information about the local network.
///
/// Contains details about NAT type, public/private addresses, and port mappings
/// discovered through STUN and other NAT traversal techniques.
///
/// # Fields
///
/// - `nat_type` - The detected NAT type (Cone or Symmetric)
/// - `public_ips` - List of public IPv4 addresses
/// - `public_udp_ports` - Public ports mapped for UDP
/// - `local_ipv4` - Primary local IPv4 address
/// - `ipv6` - Public IPv6 address if available
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NatInfo {
    /// nat type of the network
    pub nat_type: NatType,
    /// the set of public Ipv4
    pub public_ips: Vec<Ipv4Addr>,
    /// the set of public ports mapped from the nat
    pub public_udp_ports: Vec<u16>,
    /// the set of mapped addresses where `TCP` serves on
    pub mapping_tcp_addr: Vec<SocketAddr>,
    /// the set of mapped addresses where `UDP` serves on
    pub mapping_udp_addr: Vec<SocketAddr>,
    /// The predicted range of public ports, it is used when the nat_type is symmetric
    pub public_port_range: u16,
    /// local IP address
    pub local_ipv4: Ipv4Addr,
    #[serde(default)]
    pub local_ipv4s: Vec<Ipv4Addr>,
    /// The public IPv6 address
    pub ipv6: Option<Ipv6Addr>,
    /// The local ports where the `UDP` services bind
    pub local_udp_ports: Vec<u16>,
    /// The local ports where the `TCP` services bind
    pub local_tcp_port: u16,
    /// The public port of `TCP` service, which works when there is either `nat1` or no `nat` exists
    pub public_tcp_port: u16,
}
impl NatInfo {
    pub(crate) fn flag(&self) -> Option<SocketAddr> {
        let vec = self.public_ipv4_addr();
        if let Some(v) = vec.first() {
            return Some(*v);
        }
        let vec = self.public_ipv4_tcp();
        if let Some(v) = vec.first() {
            return Some(*v);
        }
        let vec = self.local_ipv4_addrs();
        if let Some(v) = vec.first() {
            return Some(*v);
        }
        let option = self.local_ipv4_tcp();
        if option.is_some() {
            return option;
        }
        None
    }
    pub fn ipv6_udp_addr(&self) -> Vec<SocketAddr> {
        if let Some(ipv6) = self.ipv6 {
            if is_ipv6_global(&ipv6) {
                return self
                    .local_udp_ports
                    .iter()
                    .map(|&port| SocketAddrV6::new(ipv6, port, 0, 0).into())
                    .collect();
            }
        }
        vec![]
    }
    pub fn ipv6_tcp_addr(&self) -> Option<SocketAddr> {
        self.ipv6
            .map(|ipv6| SocketAddrV6::new(ipv6, self.local_tcp_port, 0, 0).into())
    }
    pub fn public_ipv4_addr(&self) -> Vec<SocketAddr> {
        if self.public_ips.is_empty() || self.public_udp_ports.is_empty() {
            return vec![];
        }
        if self.public_ips.len() == 1 {
            let ip = self.public_ips[0];
            return self
                .public_udp_ports
                .iter()
                .map(|&port| SocketAddrV4::new(ip, port).into())
                .collect();
        }
        let port = self.public_udp_ports[0];
        self.public_ips
            .iter()
            .map(|&ip| SocketAddrV4::new(ip, port).into())
            .collect()
    }
    pub fn local_ipv4_addrs(&self) -> Vec<SocketAddr> {
        if self.local_udp_ports.is_empty() {
            return vec![];
        }
        if !self.local_ipv4s.is_empty() {
            let mut rs = Vec::with_capacity(self.local_ipv4s.len() * self.local_udp_ports.len());
            for ip in self.local_ipv4s.iter() {
                if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
                    continue;
                }
                for port in self.local_udp_ports.iter() {
                    rs.push(SocketAddrV4::new(*ip, *port).into());
                }
            }
            return rs;
        }
        if self.local_ipv4.is_unspecified()
            || self.local_ipv4.is_multicast()
            || self.local_ipv4.is_broadcast()
        {
            return vec![];
        }
        self.local_udp_ports
            .iter()
            .map(|&port| SocketAddrV4::new(self.local_ipv4, port).into())
            .collect()
    }
    pub fn local_ipv4_tcp(&self) -> Option<SocketAddr> {
        if self.local_ipv4.is_unspecified()
            || self.local_ipv4.is_multicast()
            || self.local_ipv4.is_broadcast()
            || self.local_tcp_port == 0
        {
            return None;
        }
        Some(SocketAddrV4::new(self.local_ipv4, self.local_tcp_port).into())
    }
    pub fn public_ipv4_tcp(&self) -> Vec<SocketAddr> {
        if self.public_tcp_port == 0 {
            return vec![];
        }
        self.public_ips
            .iter()
            .map(|&ip| SocketAddrV4::new(ip, self.public_tcp_port).into())
            .collect()
    }
}
