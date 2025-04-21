use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::ops::{Div, Mul};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rand::seq::SliceRandom;
use rand::Rng;

use crate::nat::{NatInfo, NatType};

use crate::tunnel::TunnelDispatcher;
use crate::tunnel::{tcp, udp};
pub use config::*;
pub mod config;

#[derive(Clone)]
pub struct Puncher {
    // 端口顺序
    port_vec: Vec<u16>,
    // 指定IP的打洞记录
    sym_record: Arc<Mutex<HashMap<SocketAddr, usize>>>,
    #[allow(clippy::type_complexity)]
    count_record: Arc<Mutex<HashMap<SocketAddr, (usize, usize, u64)>>>,
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
    pub fn new(
        udp_socket_manager: Option<Arc<udp::UdpSocketManager>>,
        tcp_socket_manager: Option<Arc<tcp::TcpSocketManager>>,
    ) -> Puncher {
        let mut port_vec: Vec<u16> = (1..=65535).collect();
        let mut rng = rand::rng();
        port_vec.shuffle(&mut rng);
        Self {
            port_vec,
            sym_record: Arc::new(Mutex::new(HashMap::new())),
            count_record: Arc::new(Mutex::new(HashMap::new())),
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
        let mut count_record = self.count_record.lock();
        let ten_minutes_ago = now() - 1200;
        count_record.retain(|_addr, &mut (_u1, _u2, timestamp)| timestamp >= ten_minutes_ago);
        let valid_keys: std::collections::HashSet<_> = count_record.keys().cloned().collect();
        let mut sym_map = self.sym_record.lock();
        sym_map.retain(|addr, _| valid_keys.contains(addr));
    }
    /// Call `need_punch` at a certain frequency, and call [`punch_now`](Self::punch_now) after getting true.
    /// Determine whether punching is needed.
    pub fn need_punch(&self, punch_info: &PunchInfo) -> bool {
        let Some(id) = punch_info.peer_nat_info.flag() else {
            return false;
        };
        let (count, _, _) = *self
            .count_record
            .lock()
            .entry(id)
            .and_modify(|(v, _, time)| {
                *v += 1;
                *time = now();
            })
            .or_insert((0, 0, now()));
        if count > 8 {
            //降低频率
            let interval = count / 8;
            return count % interval.min(360) == 0;
        }
        true
    }

    /// Call `punch` at a certain frequency
    pub async fn punch(&self, buf: &[u8], punch_info: PunchInfo) -> io::Result<()> {
        if !self.need_punch(&punch_info) {
            return Ok(());
        }
        self.punch_now(Some(buf), buf, punch_info).await
    }
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
        let (_, count, _) = *self
            .count_record
            .lock()
            .entry(peer)
            .and_modify(|(_, v, time)| {
                *v += 1;
                *time = now();
            })
            .or_insert((0, 0, now()));
        let ttl = if count < 255 { Some(count as u8) } else { None };
        let peer_nat_info = punch_info.peer_nat_info;
        let punch_model = punch_info.punch_model;

        type Scope<'a, T> = async_scoped::TokioScope<'a, T>;
        Scope::scope_and_block(|s| {
            if let Some(tcp_socket_manager) = self.tcp_socket_manager.as_ref() {
                for addr in &peer_nat_info.mapping_tcp_addr {
                    s.spawn(async move {
                        Self::connect_tcp(tcp_socket_manager, tcp_buf, *addr, ttl).await;
                    })
                }
                if punch_model.is_match(PunchModel::IPv4Tcp) {
                    if let Some(addr) = peer_nat_info.local_ipv4_tcp() {
                        s.spawn(async move {
                            Self::connect_tcp(tcp_socket_manager, tcp_buf, addr, ttl).await;
                        })
                    }
                    for addr in peer_nat_info.public_ipv4_tcp() {
                        s.spawn(async move {
                            Self::connect_tcp(tcp_socket_manager, tcp_buf, addr, ttl).await;
                        })
                    }
                }
                if punch_model.is_match(PunchModel::IPv6Tcp) {
                    if let Some(addr) = peer_nat_info.ipv6_tcp_addr() {
                        s.spawn(async move {
                            Self::connect_tcp(tcp_socket_manager, tcp_buf, addr, ttl).await;
                        })
                    }
                }
            }
        });
        self.punch_udp(peer, count, udp_buf, &peer_nat_info, &punch_model)
            .await?;

        Ok(())
    }
    async fn connect_tcp(
        tcp_socket_manager: &tcp::TcpSocketManager,
        buf: Option<&[u8]>,
        addr: SocketAddr,
        ttl: Option<u8>,
    ) {
        let rs = if let Some(buf) = buf {
            tokio::time::timeout(
                Duration::from_secs(3),
                tcp_socket_manager.multi_send_to_impl(buf.into(), addr, ttl),
            )
            .await
        } else {
            tokio::time::timeout(Duration::from_secs(3), async {
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
        punch_model: &PunchModelIntersect,
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

        if punch_model.is_match(PunchModel::IPv6Udp) {
            let v6_addr = peer_nat_info.ipv6_udp_addr();
            udp_socket_manager.try_main_v6_batch_send_to(buf, &v6_addr);
        }
        if !punch_model.is_match(PunchModel::IPv4Udp) {
            return Ok(());
        }
        if peer_nat_info.public_ips.is_empty() {
            return Ok(());
        }

        match peer_nat_info.nat_type {
            NatType::Symmetric => {
                // 假设对方绑定n个端口，通过NAT对外映射出n个 公网ip:公网端口，自己随机尝试k次的情况下
                // 猜中的概率 p = 1-((65535-n)/65535)*((65535-n-1)/(65535-1))*...*((65535-n-k+1)/(65535-k+1))
                // n取76，k取600，猜中的概率就超过50%了
                // 前提 自己是锥形网络，否则猜中了也通信不了

                //预测范围内最多发送max_k1个包
                let max_k1 = 60;
                //全局最多发送max_k2个包
                let mut max_k2: usize = rand::rng().random_range(600..800);
                if count > 8 {
                    //递减探测规模
                    max_k2 = max_k2.mul(8).div(count).max(max_k1 as usize);
                }
                let port = peer_nat_info.public_udp_ports.first().copied().unwrap_or(0);
                if peer_nat_info.public_port_range < max_k1 * 3 {
                    //端口变化不大时，在预测的范围内随机发送
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
                    .sym_record
                    .lock()
                    .get(&peer_id)
                    .cloned()
                    .unwrap_or_default();
                let mut end = start + max_k2;
                if end > self.port_vec.len() {
                    end = self.port_vec.len();
                }
                let mut index = start
                    + self
                        .punch_symmetric(
                            udp_socket_manager,
                            &self.port_vec[start..end],
                            buf,
                            &peer_nat_info.public_ips,
                            max_k2,
                        )
                        .await?;
                if index >= self.port_vec.len() {
                    index = 0
                }
                // 记录这个IP的打洞记录
                self.sym_record.lock().insert(peer_id, index);
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
        ips: &Vec<Ipv4Addr>,
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
