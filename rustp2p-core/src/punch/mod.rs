use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rand::seq::SliceRandom;
use rand::Rng;

use crate::endpoint::pool::SocketPool;
use crate::nat::{NatInfo, NatType};

pub use config::*;
pub mod config;

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
    pool: Arc<SocketPool>,
}

impl Puncher {
    pub fn new(pool: Arc<SocketPool>) -> Puncher {
        let mut shuffled_ports: Vec<u16> = (1..=65535).collect();
        shuffled_ports.shuffle(&mut rand::rng());
        Self {
            shuffled_ports: Arc::new(shuffled_ports),
            port_cursor: Arc::new(Mutex::new(HashMap::new())),
            punch_stats: Arc::new(Mutex::new(HashMap::new())),
            pool,
        }
    }

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

    pub async fn punch(&self, buf: &[u8], punch_info: PunchInfo) -> io::Result<()> {
        if !self.need_punch(&punch_info) {
            return Ok(());
        }
        self.punch_now(None, buf, punch_info).await
    }

    pub async fn punch_now(
        &self,
        tcp_buf: Option<&[u8]>,
        udp_buf: &[u8],
        punch_info: PunchInfo,
    ) -> io::Result<()> {
        let peer = punch_info
            .peer_nat_info
            .flag()
            .unwrap_or(SocketAddr::V4(SocketAddrV4::new(
                std::net::Ipv4Addr::UNSPECIFIED,
                0,
            )));
        {
            let mut stats = self.punch_stats.lock();
            let entry = stats.entry(peer).or_default();
            entry.batch_count += 1;
            entry.last_time = now();
        }
        let stats = self
            .punch_stats
            .lock()
            .get(&peer)
            .cloned()
            .unwrap_or_default();
        let count = stats.batch_count;
        let ttl = if count < 255 { Some(count as u8) } else { None };
        let peer_nat_info = punch_info.peer_nat_info;
        let punch_model = punch_info.punch_model;

        // UDP punch
        self.punch_udp(count, udp_buf, &peer_nat_info, &punch_model)
            .await;

        // TCP punch
        let mut tcp_tasks = Vec::new();
        let tcp_buf_owned: Option<Arc<[u8]>> = tcp_buf.map(Arc::from);
        if !peer_nat_info.mapping_tcp_addr.is_empty() {
            for addr in &peer_nat_info.mapping_tcp_addr {
                let buf = tcp_buf_owned.clone();
                let a = *addr;
                let pool = self.pool.clone();
                tcp_tasks.push(tokio::spawn(async move {
                    Self::connect_tcp(&pool, buf.as_deref(), a, ttl, Duration::from_secs(3)).await;
                }));
            }
        }
        if punch_model.is_match(PunchPolicy::IPv4Tcp) {
            if let Some(addr) = peer_nat_info.local_ipv4_tcp() {
                let buf = tcp_buf_owned.clone();
                let pool = self.pool.clone();
                tcp_tasks.push(tokio::spawn(async move {
                    Self::connect_tcp(&pool, buf.as_deref(), addr, ttl, Duration::from_millis(100))
                        .await;
                }));
            }
            for addr in peer_nat_info.public_ipv4_tcp() {
                let buf = tcp_buf_owned.clone();
                let pool = self.pool.clone();
                tcp_tasks.push(tokio::spawn(async move {
                    Self::connect_tcp(&pool, buf.as_deref(), addr, ttl, Duration::from_secs(3))
                        .await;
                }));
            }
        }
        if punch_model.is_match(PunchPolicy::IPv6Tcp) {
            if let Some(addr) = peer_nat_info.ipv6_tcp_addr() {
                let buf = tcp_buf_owned.clone();
                let pool = self.pool.clone();
                tcp_tasks.push(tokio::spawn(async move {
                    Self::connect_tcp(&pool, buf.as_deref(), addr, ttl, Duration::from_secs(3))
                        .await;
                }));
            }
        }
        for task in tcp_tasks {
            let _ = task.await;
        }
        Ok(())
    }

    async fn connect_tcp(
        pool: &Arc<SocketPool>,
        buf: Option<&[u8]>,
        addr: SocketAddr,
        ttl: Option<u8>,
        timeout: Duration,
    ) {
        match tokio::time::timeout(timeout, async {
            let stream = crate::socket::connect_tcp(addr, 0, None, ttl).await?;
            let weak = pool
                .add_tcp(stream, addr, &crate::endpoint::LengthPrefixedInitCodec)
                .await?;
            if let Some(data) = buf {
                if let Some(conn) = weak.upgrade() {
                    let _ = conn.send(data).await;
                }
            }
            Ok::<(), io::Error>(())
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("tcp punch error: {e}"),
            Err(_) => log::warn!("tcp punch timeout"),
        }
    }

    async fn punch_udp(
        &self,
        count: usize,
        buf: &[u8],
        peer_nat_info: &NatInfo,
        _punch_model: &PunchModel,
    ) {
        // Send to mapped addresses
        if !peer_nat_info.mapping_udp_addr.is_empty() {
            let addrs: Vec<SocketAddr> = peer_nat_info
                .mapping_udp_addr
                .iter()
                .filter(|a| a.is_ipv4())
                .copied()
                .collect();
            for addr in &addrs {
                let _ = self.pool.send_to(buf, *addr).await;
            }
        }
        if !peer_nat_info.local_ipv4_addrs().is_empty() {
            let addrs = peer_nat_info.local_ipv4_addrs();
            for addr in &addrs {
                let _ = self.pool.send_to(buf, *addr).await;
            }
        }

        match peer_nat_info.nat_type {
            NatType::Symmetric => {
                let max_k1 = 60;
                let mut max_k2: usize = rand::rng().random_range(600..800);
                if count > 8 {
                    max_k2 = ((max_k2 * 8) / count.max(1)).max(max_k1 as usize);
                }
                let port = peer_nat_info.public_udp_ports.first().copied().unwrap_or(0);
                if peer_nat_info.public_port_range < max_k1 * 3 {
                    let min_port = if port > peer_nat_info.public_port_range {
                        port - peer_nat_info.public_port_range
                    } else {
                        1
                    };
                    let (max_port, overflow) =
                        port.overflowing_add(peer_nat_info.public_port_range);
                    let max_port = if overflow { 65535 } else { max_port };
                    let local_range = min_port..=max_port;
                    let default_addr =
                        SocketAddr::V4(SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 0));
                    let pub_addr = peer_nat_info
                        .public_ipv4_addr()
                        .into_iter()
                        .next()
                        .unwrap_or(default_addr);
                    let start = self.port_cursor.lock().get(&pub_addr).copied().unwrap_or(0);
                    let end = (start + max_k1 as usize).min(self.shuffled_ports.len());
                    let ports = &self.shuffled_ports[start..end];
                    let valid_ports: Vec<u16> = ports
                        .iter()
                        .filter(|p| local_range.contains(p))
                        .copied()
                        .collect();
                    if !valid_ports.is_empty() {
                        let pub_ip = peer_nat_info
                            .public_ips
                            .first()
                            .copied()
                            .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED);
                        self.punch_symmetric(&valid_ports, buf, &[pub_ip], max_k1 as usize)
                            .await;
                    }
                }
                let default_addr =
                    SocketAddr::V4(SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 0));
                let pub_addr = peer_nat_info
                    .public_ipv4_addr()
                    .into_iter()
                    .next()
                    .unwrap_or(default_addr);
                let start = self.port_cursor.lock().get(&pub_addr).copied().unwrap_or(0);
                let end = (start + max_k2).min(self.shuffled_ports.len());
                let mut index = start
                    + self
                        .punch_symmetric(
                            &self.shuffled_ports[start..end],
                            buf,
                            &peer_nat_info.public_ips,
                            max_k2,
                        )
                        .await;
                if index >= self.shuffled_ports.len() {
                    index = 0;
                }
                if let Some(addr) = peer_nat_info.public_ipv4_addr().into_iter().next() {
                    self.port_cursor.lock().insert(addr, index);
                }
            }
            NatType::Cone => {
                if let Some(addr) = peer_nat_info.public_ipv4_addr().into_iter().next() {
                    self.pool.try_send_via_all(buf, addr).await;
                }
            }
        }
    }

    async fn punch_symmetric(
        &self,
        ports: &[u16],
        buf: &[u8],
        ips: &[std::net::Ipv4Addr],
        max: usize,
    ) -> usize {
        let mut count = 0;
        for (index, port) in ports.iter().enumerate() {
            for pub_ip in ips {
                count += 1;
                if count == max {
                    return index;
                }
                let addr = SocketAddr::V4(SocketAddrV4::new(*pub_ip, *port));
                let _ = self.pool.try_send_via_all(buf, addr).await;
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
        ports.len()
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
