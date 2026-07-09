//! STUN (Session Traversal Utilities for NAT) protocol implementation.
//!
//! This module provides STUN client functionality for NAT type detection and
//! public address discovery. STUN is used to determine how the local network
//! appears from the public internet.
//!
//! # Examples
//!
//! ```rust,no_run
//! use rustp2p_core::stun::stun_test_nat;
//!
//! # #[tokio::main]
//! # async fn main() -> std::io::Result<()> {
//! let stun_servers = vec![
//!     "stun.miwifi.com:3478".to_string(),
//!     "stun.chat.bilibili.com:3478".to_string(),
//!     "stun.hitv.com:3478".to_string(),
//! ];
//!
//! let result = stun_test_nat(stun_servers, None).await?;
//! println!("NAT Type: {:?}", result.nat_type);
//! println!("Public IPv4: {:?}", result.public_ipv4);
//! println!("Public IPv6: {:?}", result.public_ipv6);
//! # Ok(())
//! # }
//! ```

use std::collections::HashSet;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

use crate::nat::NatType;
use crate::socket::{bind_udp, LocalInterface};
use rand::RngCore;
use stun_format::Attr;
use tokio::net::UdpSocket;

/// Result of STUN NAT detection.
#[derive(Debug, Clone)]
pub struct StunResult {
    /// Detected NAT type (Cone or Symmetric)
    pub nat_type: NatType,
    /// Public IPv4 addresses discovered
    pub public_ipv4: Vec<Ipv4Addr>,
    /// Public IPv6 address if discovered
    pub public_ipv6: Option<Ipv6Addr>,
    /// Public UDP ports discovered (NAT mapped ports)
    pub public_udp_ports: Vec<u16>,
    /// Port range for symmetric NAT (max_port - min_port)
    pub port_range: u16,
}

/// Tests NAT type and discovers public addresses using STUN servers.
///
/// This function queries multiple STUN servers to determine the NAT type,
/// discover public IP addresses, and measure port allocation patterns.
///
/// # Arguments
///
/// * `stun_servers` - List of STUN server addresses (e.g., "stun.example.com:3478")
/// * `default_interface` - Optional network interface to bind to
///
/// # Returns
///
/// A tuple containing:
/// - `NatType` - Detected NAT type (Cone or Symmetric)
/// - `Vec<Ipv4Addr>` - List of discovered public IPv4 addresses
/// - `u16` - Port allocation range (for symmetric NAT prediction)
///
/// # Examples
///
/// ```rust,no_run
/// use rustp2p_core::stun::stun_test_nat;
///
/// # #[tokio::main]
/// # async fn main() -> std::io::Result<()> {
/// let stun_servers = vec![
///     "stun.miwifi.com:3478".to_string(),
///     "stun.chat.bilibili.com:3478".to_string(),
///     "stun.hitv.com:3478".to_string(),
/// ];
/// let result = stun_test_nat(stun_servers, None).await?;
/// println!("NAT Type: {:?}", result.nat_type);
/// println!("Public IPv4: {:?}", result.public_ipv4);
/// println!("Public IPv6: {:?}", result.public_ipv6);
/// # Ok(())
/// # }
/// ```
pub async fn stun_test_nat(
    stun_servers: Vec<String>,
    default_interface: Option<&LocalInterface>,
) -> io::Result<StunResult> {
    let mut nat_type = NatType::Cone;
    let mut port_range = 0;
    let mut ipv4_set = HashSet::new();
    let mut public_ports = HashSet::new();
    let mut ipv6_addr = None;
    for _ in 0..2 {
        let stun_servers = stun_servers.clone();
        match stun_test_nat0(stun_servers, default_interface).await {
            Ok(result) => {
                if result.nat_type == NatType::Symmetric {
                    nat_type = NatType::Symmetric;
                    break;
                }
                for ip in result.public_ipv4 {
                    ipv4_set.insert(ip);
                }
                for port in result.public_udp_ports {
                    public_ports.insert(port);
                }
                if result.public_ipv6.is_some() && ipv6_addr.is_none() {
                    ipv6_addr = result.public_ipv6;
                }
                if port_range < result.port_range {
                    port_range = result.port_range;
                }
            }
            Err(e) => {
                log::warn!("{e:?}");
            }
        }
    }
    Ok(StunResult {
        nat_type,
        public_ipv4: ipv4_set.into_iter().collect(),
        public_ipv6: ipv6_addr,
        public_udp_ports: public_ports.into_iter().collect(),
        port_range,
    })
}

/// Tests NAT type using an existing socket.
///
/// This is useful when you want to use the main UDP socket for STUN testing
/// to ensure the discovered public port matches the actual communication port.
pub async fn stun_test_nat_with_socket(
    socket: &UdpSocket,
    stun_servers: Vec<String>,
) -> io::Result<StunResult> {
    let mut nat_type = NatType::Cone;
    let mut port_range = 0;
    let mut ipv4_set = HashSet::new();
    let mut public_ports = HashSet::new();
    let mut ipv6_addr = None;
    for _ in 0..2 {
        let stun_servers = stun_servers.clone();
        match stun_test_nat_with_socket0(socket, stun_servers).await {
            Ok(result) => {
                if result.nat_type == NatType::Symmetric {
                    nat_type = NatType::Symmetric;
                    break;
                }
                for ip in result.public_ipv4 {
                    ipv4_set.insert(ip);
                }
                for port in result.public_udp_ports {
                    public_ports.insert(port);
                }
                if result.public_ipv6.is_some() && ipv6_addr.is_none() {
                    ipv6_addr = result.public_ipv6;
                }
                if port_range < result.port_range {
                    port_range = result.port_range;
                }
            }
            Err(e) => {
                log::warn!("{e:?}");
            }
        }
    }
    Ok(StunResult {
        nat_type,
        public_ipv4: ipv4_set.into_iter().collect(),
        public_ipv6: ipv6_addr,
        public_udp_ports: public_ports.into_iter().collect(),
        port_range,
    })
}

pub(crate) async fn stun_test_nat0(
    stun_servers: Vec<String>,
    default_interface: Option<&LocalInterface>,
) -> io::Result<StunResult> {
    let udp = bind_udp("0.0.0.0:0".parse().unwrap(), default_interface)?;
    let udp = UdpSocket::from_std(udp.into())?;
    let mut nat_type = NatType::Cone;
    let mut min_port = u16::MAX;
    let mut max_port = 0;
    let mut ipv4_set = HashSet::new();
    let mut public_ports = HashSet::new();
    let mut ipv6_addr = None;
    let mut pub_addrs = HashSet::new();
    for x in &stun_servers {
        match test_nat(&udp, x).await {
            Ok(addr) => {
                pub_addrs.extend(addr);
            }
            Err(e) => {
                log::warn!("stun {x} error {e:?} ");
            }
        }
    }
    if pub_addrs.len() > 1 {
        nat_type = NatType::Symmetric;
    }
    for addr in &pub_addrs {
        match addr {
            SocketAddr::V4(v4) => {
                ipv4_set.insert(*v4.ip());
                public_ports.insert(v4.port());
            }
            SocketAddr::V6(v6) => {
                if ipv6_addr.is_none() {
                    ipv6_addr = Some(*v6.ip());
                }
                public_ports.insert(v6.port());
            }
        }
        if min_port > addr.port() {
            min_port = addr.port()
        }
        if max_port < addr.port() {
            max_port = addr.port()
        }
    }
    Ok(StunResult {
        nat_type,
        public_ipv4: ipv4_set.into_iter().collect(),
        public_ipv6: ipv6_addr,
        public_udp_ports: public_ports.into_iter().collect(),
        port_range: max_port.saturating_sub(min_port),
    })
}

pub(crate) async fn stun_test_nat_with_socket0(
    socket: &UdpSocket,
    stun_servers: Vec<String>,
) -> io::Result<StunResult> {
    let mut nat_type = NatType::Cone;
    let mut min_port = u16::MAX;
    let mut max_port = 0;
    let mut ipv4_set = HashSet::new();
    let mut public_ports = HashSet::new();
    let mut ipv6_addr = None;
    let mut pub_addrs = HashSet::new();
    for x in &stun_servers {
        match test_nat(socket, x).await {
            Ok(addr) => {
                pub_addrs.extend(addr);
            }
            Err(e) => {
                log::warn!("stun {x} error {e:?} ");
            }
        }
    }
    if pub_addrs.len() > 1 {
        nat_type = NatType::Symmetric;
    }
    for addr in &pub_addrs {
        match addr {
            SocketAddr::V4(v4) => {
                ipv4_set.insert(*v4.ip());
                public_ports.insert(v4.port());
            }
            SocketAddr::V6(v6) => {
                if ipv6_addr.is_none() {
                    ipv6_addr = Some(*v6.ip());
                }
                public_ports.insert(v6.port());
            }
        }
        if min_port > addr.port() {
            min_port = addr.port()
        }
        if max_port < addr.port() {
            max_port = addr.port()
        }
    }
    Ok(StunResult {
        nat_type,
        public_ipv4: ipv4_set.into_iter().collect(),
        public_ipv6: ipv6_addr,
        public_udp_ports: public_ports.into_iter().collect(),
        port_range: max_port.saturating_sub(min_port),
    })
}

async fn test_nat(udp: &UdpSocket, stun_server: &str) -> io::Result<HashSet<SocketAddr>> {
    udp.connect(stun_server).await?;
    let tid = rand::rng().next_u64() as u128;
    let mut addr = HashSet::new();
    let (mapped_addr1, changed_addr1) = test_nat_(udp, stun_server, true, true, tid).await?;
    // Collect both IPv4 and IPv6 mapped addresses
    addr.insert(mapped_addr1);
    if let Some(changed_addr1) = changed_addr1 {
        if udp.connect(changed_addr1).await.is_ok() {
            match test_nat_(udp, stun_server, false, false, tid + 1).await {
                Ok((mapped_addr2, _)) => {
                    addr.insert(mapped_addr2);
                }
                Err(e) => {
                    log::warn!("stun {stun_server} error {e:?} ");
                }
            }
        }
    }
    log::debug!("stun {stun_server} mapped_addr {addr:?}  changed_addr {changed_addr1:?}",);

    Ok(addr)
}

async fn test_nat_(
    udp: &UdpSocket,
    stun_server: &str,
    change_ip: bool,
    change_port: bool,
    tid: u128,
) -> io::Result<(SocketAddr, Option<SocketAddr>)> {
    for _ in 0..2 {
        let mut buf = [0u8; 28];
        let mut msg = stun_format::MsgBuilder::from(buf.as_mut_slice());
        msg.typ(stun_format::MsgType::BindingRequest);
        msg.tid(tid);
        msg.add_attr(Attr::ChangeRequest {
            change_ip,
            change_port,
        });
        udp.send(msg.as_bytes()).await?;
        let mut buf = [0; 10240];
        let (len, _addr) =
            match tokio::time::timeout(Duration::from_secs(3), udp.recv_from(&mut buf)).await {
                Ok(rs) => rs?,
                Err(e) => {
                    log::warn!("stun {stun_server} error {e:?}");
                    continue;
                }
            };
        let msg = stun_format::Msg::from(&buf[..len]);
        let mut mapped_addr = None;
        let mut changed_addr = None;
        for x in msg.attrs_iter() {
            match x {
                Attr::MappedAddress(addr) if mapped_addr.is_none() => {
                    let _ = mapped_addr.insert(stun_addr(addr));
                }
                Attr::ChangedAddress(addr) if changed_addr.is_none() => {
                    let _ = changed_addr.insert(stun_addr(addr));
                }
                Attr::XorMappedAddress(addr) if mapped_addr.is_none() => {
                    let _ = mapped_addr.insert(stun_addr(addr));
                }
                _ => {}
            }
            if let Some(mapped_addr) = mapped_addr {
                if changed_addr.is_some() {
                    return Ok((mapped_addr, changed_addr));
                }
            }
        }
        if let Some(addr) = mapped_addr {
            return Ok((addr, changed_addr));
        }
    }
    Err(io::Error::other("stun response err"))
}

fn stun_addr(addr: stun_format::SocketAddr) -> SocketAddr {
    match addr {
        stun_format::SocketAddr::V4(ip, port) => {
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip), port))
        }
        stun_format::SocketAddr::V6(ip, port) => {
            SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(ip), port, 0, 0))
        }
    }
}

const TAG: u128 = 1827549368 << 64;

pub fn send_stun_request() -> Vec<u8> {
    let mut buf = [0u8; 28];
    let mut msg = stun_format::MsgBuilder::from(buf.as_mut_slice());
    msg.typ(stun_format::MsgType::BindingRequest);
    let id = rand::rng().next_u64() as u128;
    msg.tid(id | TAG);
    msg.add_attr(Attr::ChangeRequest {
        change_ip: false,
        change_port: false,
    });
    msg.as_bytes().to_vec()
}
pub fn is_stun_response(buf: &[u8]) -> bool {
    !buf.is_empty() && buf[0] == 0x01
}
pub fn recv_stun_response(buf: &[u8]) -> Option<SocketAddr> {
    let msg = stun_format::Msg::from(buf);
    if let Some(tid) = msg.tid() {
        if tid & TAG != TAG {
            return None;
        }
    }
    for x in msg.attrs_iter() {
        match x {
            Attr::MappedAddress(addr) => {
                return Some(stun_addr(addr));
            }
            Attr::XorMappedAddress(addr) => {
                return Some(stun_addr(addr));
            }
            _ => {}
        }
    }
    None
}
