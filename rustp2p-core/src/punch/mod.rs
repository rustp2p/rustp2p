//! NAT hole punching for establishing direct peer-to-peer connections.
//!
//! This module implements UDP and TCP hole punching techniques to traverse NAT
//! and establish direct connections between peers. It handles both Cone and
//! Symmetric NAT types.
//!
//! # Examples
//!
//! ```rust,no_run
//! use rust_p2p_core::punch::{Puncher, PunchInfo};
//!
//! # async fn example(puncher: Puncher) -> std::io::Result<()> {
//! let punch_info = PunchInfo::default();
//!
//! // Check if punching is needed
//! if puncher.need_punch(&punch_info) {
//!     // Perform hole punching
//!     puncher.punch_now(None, b"punch", punch_info).await?;
//! }
//! # Ok(())
//! # }
//! ```

use bytes::Bytes;
use parking_lot::Mutex;
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::ops::{Div, Mul};
use std::sync::Arc;
use std::time::Duration;

use crate::nat::{NatInfo, NatType};

use crate::transport::TunnelDispatcher;
use crate::transport::{tcp, udp};
pub use config::*;
pub mod config;

/// NAT hole punching coordinator.
///
/// `Puncher` manages the hole punching process, tracking attempts and
/// adapting strategies based on NAT type and previous results.
///
/// # Examples
///
/// ```rust,no_run
/// use rust_p2p_core::punch::Puncher;
///
/// let puncher = Puncher::new(None, None);
/// ```

#[derive(Default, Clone)]
struct PunchStats {
    total_count: usize,
    batch_count: usize,
    last_time: u64,
}

#[derive(Clone)]
pub struct Puncher {
    shuffled_ports: Arc<Vec<u16>>,
    port_cursor: Arc<Mutex<HashMap<SocketAddr, usize>>>,
    punch_stats: Arc<Mutex<HashMap<SocketAddr, PunchStats>>>,
    udp_socket_manager: Option<Arc<udp::UdpSocketManager>>,
    tcp_socket_manager: Option<Arc<tcp::TcpSocketManager>>,
}

impl From<&TunnelDispatcher> for Puncher {
    fn from(value: &TunnelDispatcher) -> Self {
        let tcp_socket_manager = value.shared_tcp_socket_manager();
        let udp_socket_manager = value.shared_udp_socket_manager();
        Self::new(udp_socket_manager, tcp_socket_manager)
    }
}

impl Puncher {
    /// Creates a new Puncher with the given socket managers.
    ///
    /// # Arguments
    ///
    /// * `udp_socket_manager` - Optional UDP socket manager for UDP punching
    /// * `tcp_socket_manager` - Optional TCP socket manager for TCP punching
    pub fn new(
        udp_socket_manager: Option<Arc<udp::UdpSocketManager>>,
        tcp_socket_manager: Option<Arc<tcp::TcpSocketManager>>,
    ) -> Puncher {
        let mut shuffled_ports: Vec<u16> = (1..=65535).collect();
        let mut rng = rand::rng();
        shuffled_ports.shuffle(&mut rng);
        Self {
            shuffled_ports: Arc::new(shuffled_ports),
            port_cursor: Arc::new(Mutex::new(HashMap::new())),
            punch_stats: Arc::new(Mutex::new(HashMap::new())),
            udp_socket_manager,
            tcp_socket_manager,
        }
    }
}
fn now() -> u64 {
    let now = std::time::SystemTime::now();
    let time = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    time.as_secs()
}
impl Puncher {
    fn clean(&self) {
        let mut punch_stats = self.punch_stats.lock();
        let expire_threshold = now() - 1200;
        punch_stats.retain(|_addr, stats| stats.last_time >= expire_threshold);
        let valid_keys: std::collections::HashSet<_> = punch_stats.keys().cloned().collect();
        let mut port_cursor = self.port_cursor.lock();
        port_cursor.retain(|addr, _| valid_keys.contains(addr));
    }
    /// Determines whether hole punching is needed for a peer.
    ///
    /// Call this method periodically. It uses adaptive frequency based on
    /// previous attempts to avoid excessive punching.
    ///
    /// # Arguments
    ///
    /// * `punch_info` - Information about the peer to punch
    ///
    /// # Returns
    ///
    /// `true` if punching should be attempted, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use rust_p2p_core::punch::{Puncher, PunchInfo};
    /// # async fn example(puncher: Puncher, punch_info: PunchInfo) {
    /// if puncher.need_punch(&punch_info) {
    ///     // Perform punching
    /// }
    /// # }
    /// ```
    pub fn need_punch(&self, punch_info: &PunchInfo) -> bool {
        let Some(id) = punch_info.peer_nat_info.flag() else {
            return false;
        };
        let stats = self.punch_stats.lock().entry(id).or_default().clone();
        if stats.total_count > 8 {
            let interval = stats.total_count / 8;
            return stats.total_count.is_multiple_of(interval.min(360));
        }
        true
    }

    /// Attempts hole punching if needed (convenience method).
    ///
    /// This combines `need_punch` and `punch_now` into a single call.
    ///
    /// # Arguments
    ///
    /// * `buf` - The data to send during punching
    /// * `punch_info` - Information about the peer to punch
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use rust_p2p_core::punch::{Puncher, PunchInfo};
    /// # async fn example(puncher: Puncher) -> std::io::Result<()> {
    /// let punch_info = PunchInfo::default();
    /// puncher.punch(b"punch_data", punch_info).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn punch(&self, buf: &[u8], punch_info: PunchInfo) -> io::Result<()> {
        if !self.need_punch(&punch_info) {
            return Ok(());
        }
        self.punch_now(Some(buf), buf, punch_info).await
    }

    /// Performs hole punching immediately without checking if it's needed.
    ///
    /// Attempts both TCP and UDP hole punching based on available socket managers
    /// and the peer's NAT information.
    ///
    /// # Arguments
    ///
    /// * `tcp_buf` - Optional TCP punch data
    /// * `udp_buf` - UDP punch data
    /// * `punch_info` - Information about the peer to punch
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use rust_p2p_core::punch::{Puncher, PunchInfo};
    /// # async fn example(puncher: Puncher) -> std::io::Result<()> {
    /// let punch_info = PunchInfo::default();
    /// puncher.punch_now(Some(b"tcp"), b"udp", punch_info).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn punch_now(
        &self,
        tcp_buf: Option<&[u8]>,
        udp_buf: &[u8],
        punch_info: PunchInfo,
    ) -> io::Result<()> {
        self.clean();
        let peer = punch_info
            .peer_nat_info
            .flag()
            .unwrap_or(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)));
        let stats = self
            .punch_stats
            .lock()
            .entry(peer)
            .and_modify(|stats| {
                stats.batch_count += 1;
                stats.last_time = now();
            })
            .or_insert_with(|| PunchStats {
                total_count: 0,
                batch_count: 1,
                last_time: now(),
            })
            .clone();
        let count = stats.batch_count;
        let ttl = if count < 255 { Some(count as u8) } else { None };
        let peer_nat_info = punch_info.peer_nat_info;
        let punch_model = punch_info.punch_model;
        self.punch_udp(peer, count, udp_buf, &peer_nat_info, &punch_model)
            .await?;

        let mut tcp_tasks = Vec::new();
        let tcp_buf_owned: Option<Arc<[u8]>> = tcp_buf.map(Arc::from);
        if let Some(tcp_socket_manager) = self.tcp_socket_manager.as_ref() {
            for addr in &peer_nat_info.mapping_tcp_addr {
                let mgr = tcp_socket_manager.clone();
                let buf = tcp_buf_owned.clone();
                let a = *addr;
                tcp_tasks.push(tokio::spawn(async move {
                    Self::connect_tcp(&mgr, buf.as_deref(), a, ttl, Duration::from_secs(3)).await;
                }));
            }
            if punch_model.is_match(PunchPolicy::IPv4Tcp) {
                if let Some(addr) = peer_nat_info.local_ipv4_tcp() {
                    let mgr = tcp_socket_manager.clone();
                    let buf = tcp_buf_owned.clone();
                    tcp_tasks.push(tokio::spawn(async move {
                        Self::connect_tcp(
                            &mgr,
                            buf.as_deref(),
                            addr,
                            ttl,
                            Duration::from_millis(100),
                        )
                        .await;
                    }));
                }
                for addr in peer_nat_info.public_ipv4_tcp() {
                    let mgr = tcp_socket_manager.clone();
                    let buf = tcp_buf_owned.clone();
                    tcp_tasks.push(tokio::spawn(async move {
                        Self::connect_tcp(&mgr, buf.as_deref(), addr, ttl, Duration::from_secs(3))
                            .await;
                    }));
                }
            }
            if punch_model.is_match(PunchPolicy::IPv6Tcp) {
                if let Some(addr) = peer_nat_info.ipv6_tcp_addr() {
                    let mgr = tcp_socket_manager.clone();
                    let buf = tcp_buf_owned.clone();
                    tcp_tasks.push(tokio::spawn(async move {
                        Self::connect_tcp(&mgr, buf.as_deref(), addr, ttl, Duration::from_secs(3))
                            .await;
                    }));
                }
            }
        }
        for task in tcp_tasks {
            let _ = task.await;
        }

        Ok(())
    }
    async fn connect_tcp(
        tcp_socket_manager: &tcp::TcpSocketManager,
        buf: Option<&[u8]>,
        addr: SocketAddr,
        ttl: Option<u8>,
        timeout: Duration,
    ) {
        let rs = if let Some(buf) = buf {
            tokio::time::timeout(
                timeout,
                tcp_socket_manager.multi_send_to_impl(Bytes::copy_from_slice(buf), addr, ttl),
            )
            .await
        } else {
            tokio::time::timeout(timeout, async {
                tcp_socket_manager.connect_ttl(addr, ttl).await?;
                Ok(())
            })
            .await
        };
        match rs {
            Ok(rs) => {
                if let Err(e) = rs {
                    log::warn!("tcp connect {addr},{e:?}");
                }
            }
            Err(_) => {
                log::warn!("tcp connect timeout {addr}");
            }
        }
    }
    async fn punch_udp(
        &self,
        peer_id: SocketAddr,
        count: usize,
        buf: &[u8],
        peer_nat_info: &NatInfo,
        punch_model: &PunchModel,
    ) -> io::Result<()> {
        let udp_socket_manager = if let Some(udp_socket_manager) = self.udp_socket_manager.as_ref()
        {
            udp_socket_manager
        } else {
            return Ok(());
        };
        if !peer_nat_info.mapping_udp_addr.is_empty() {
            let mapping_udp_v4_addr: Vec<SocketAddr> = peer_nat_info
                .mapping_udp_addr
                .iter()
                .filter(|a| a.is_ipv4())
                .copied()
                .collect();
            udp_socket_manager.try_main_v4_batch_send_to(buf, &mapping_udp_v4_addr);

            let mapping_udp_v6_addr: Vec<SocketAddr> = peer_nat_info
                .mapping_udp_addr
                .iter()
                .filter(|a| a.is_ipv6())
                .copied()
                .collect();
            udp_socket_manager.try_main_v6_batch_send_to(buf, &mapping_udp_v6_addr);
        }
        let local_ipv4_addrs = peer_nat_info.local_ipv4_addrs();
        if !local_ipv4_addrs.is_empty() {
            udp_socket_manager.try_main_v4_batch_send_to(buf, &local_ipv4_addrs);
        }

        if punch_model.is_match(PunchPolicy::IPv6Udp) {
            let v6_addr = peer_nat_info.ipv6_udp_addr();
            udp_socket_manager.try_main_v6_batch_send_to(buf, &v6_addr);
        }
        if !punch_model.is_match(PunchPolicy::IPv4Udp) {
            return Ok(());
        }
        if peer_nat_info.public_ips.is_empty() {
            return Ok(());
        }

        match peer_nat_info.nat_type {
            NatType::Symmetric => {
                // Assume peer binds n ports, NAT maps them to n public ip:port pairs.
                // With k random guesses, probability of match:
                // p = 1 - ((65535-n)/65535) * ((65535-n-1)/(65535-1)) * ... * ((65535-n-k+1)/(65535-k+1))
                // With n=76, k=600, probability > 50%.
                // Prerequisite: local NAT is Cone type, otherwise even a match won't enable communication.

                // Max packets to send within predicted range
                let max_k1 = 60;
                // Max packets to send globally
                let mut max_k2: usize = rand::rng().random_range(600..800);
                if count > 8 {
                    // Reduce probe count for repeated attempts
                    max_k2 = max_k2.mul(8).div(count).max(max_k1 as usize);
                }
                let port = peer_nat_info.public_udp_ports.first().copied().unwrap_or(0);
                if peer_nat_info.public_port_range < max_k1 * 3 {
                    // When port range is small, probe within predicted range
                    let min_port = if port > peer_nat_info.public_port_range {
                        port - peer_nat_info.public_port_range
                    } else {
                        1
                    };
                    let (max_port, overflow) =
                        port.overflowing_add(peer_nat_info.public_port_range);
                    let max_port = if overflow { 65535 } else { max_port };
                    let k = if max_port - min_port + 1 > max_k1 {
                        max_k1 as usize
                    } else {
                        (max_port - min_port + 1) as usize
                    };
                    let mut nums: Vec<u16> = (min_port..=max_port).collect();
                    nums.shuffle(&mut rand::rng());
                    self.punch_symmetric(
                        udp_socket_manager,
                        &nums[..k],
                        buf,
                        &peer_nat_info.public_ips,
                        max_k1 as usize,
                    )
                    .await?;
                }
                let start = self
                    .port_cursor
                    .lock()
                    .get(&peer_id)
                    .cloned()
                    .unwrap_or_default();
                let mut end = start + max_k2;
                if end > self.shuffled_ports.len() {
                    end = self.shuffled_ports.len();
                }
                let mut index = start
                    + self
                        .punch_symmetric(
                            udp_socket_manager,
                            &self.shuffled_ports[start..end],
                            buf,
                            &peer_nat_info.public_ips,
                            max_k2,
                        )
                        .await?;
                if index >= self.shuffled_ports.len() {
                    index = 0
                }
                self.port_cursor.lock().insert(peer_id, index);
            }
            NatType::Cone => {
                let addr = peer_nat_info.public_ipv4_addr();
                if addr.is_empty() {
                    return Ok(());
                }
                udp_socket_manager.try_main_v4_batch_send_to(buf, &addr);
                udp_socket_manager.try_sub_batch_send_to(buf, addr[0]);
            }
        }
        Ok(())
    }

    async fn punch_symmetric(
        &self,
        udp_socket_manager: &udp::UdpSocketManager,
        ports: &[u16],
        buf: &[u8],
        ips: &[Ipv4Addr],
        max: usize,
    ) -> io::Result<usize> {
        let mut count = 0;
        for (index, port) in ports.iter().enumerate() {
            for pub_ip in ips {
                count += 1;
                if count == max {
                    return Ok(index);
                }
                let addr: SocketAddr = SocketAddr::V4(SocketAddrV4::new(*pub_ip, *port));
                if let Err(e) = udp_socket_manager.try_send_to(buf, addr) {
                    log::info!("{addr},{e:?}");
                }
                tokio::time::sleep(Duration::from_millis(2)).await
            }
        }
        Ok(ports.len())
    }
}
