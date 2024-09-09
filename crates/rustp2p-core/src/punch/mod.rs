use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::ops;
use std::ops::{Div, Mul};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rand::seq::SliceRandom;
use rand::Rng;

use crate::nat::{NatInfo, NatType};
use crate::pipe::tcp_pipe::TcpPipeWriter;
use crate::pipe::udp_pipe::UdpPipeWriter;
use crate::pipe::Pipe;
use crate::route::route_table::RouteTable;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum PunchModel {
    IPv4Tcp,
    IPv4Udp,
    IPv6Tcp,
    IPv6Udp,
}

impl ops::BitOr<PunchModel> for PunchModel {
    type Output = PunchModelBox;

    fn bitor(self, rhs: PunchModel) -> Self::Output {
        let mut model = PunchModelBox::empty();
        model.or(self);
        model.or(rhs);
        model
    }
}

#[derive(Clone, Debug)]
pub struct PunchModelBox {
    models: HashSet<PunchModel>,
}

impl ops::BitOr<PunchModel> for PunchModelBox {
    type Output = PunchModelBox;

    fn bitor(mut self, rhs: PunchModel) -> Self::Output {
        self.or(rhs);
        self
    }
}

impl PunchModelBox {
    pub fn all() -> Self {
        PunchModel::IPv4Tcp | PunchModel::IPv4Udp | PunchModel::IPv6Tcp | PunchModel::IPv6Udp
    }
    pub fn ipv4() -> Self {
        PunchModel::IPv4Tcp | PunchModel::IPv4Udp
    }
    pub fn ipv6() -> Self {
        PunchModel::IPv6Tcp | PunchModel::IPv6Udp
    }
    pub fn empty() -> Self {
        Self {
            models: Default::default(),
        }
    }
    pub fn or(&mut self, punch_model: PunchModel) {
        self.models.insert(punch_model);
    }
    pub fn is_match(&self, punch_model: PunchModel) -> bool {
        self.models.contains(&punch_model)
    }
}

#[derive(Clone, Debug)]
pub struct PunchModelBoxes {
    boxes: Vec<PunchModelBox>,
}
impl ops::BitAnd<PunchModelBox> for PunchModelBox {
    type Output = PunchModelBoxes;

    fn bitand(self, rhs: PunchModelBox) -> Self::Output {
        let mut boxes = PunchModelBoxes::empty();
        boxes.and(rhs);
        boxes
    }
}
impl PunchModelBoxes {
    pub fn all() -> Self {
        Self {
            boxes: vec![PunchModelBox::all()],
        }
    }
    pub fn empty() -> Self {
        Self { boxes: Vec::new() }
    }
    pub fn and(&mut self, punch_model_box: PunchModelBox) {
        self.boxes.push(punch_model_box)
    }
    pub fn is_match(&self, punch_model: PunchModel) -> bool {
        if self.boxes.is_empty() {
            return false;
        }
        for x in &self.boxes {
            if !x.is_match(punch_model) {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Debug)]
pub struct PunchInfo {
    initiate_by_oneself: bool,
    punch_model: PunchModelBoxes,
    peer_nat_info: NatInfo,
}

impl PunchInfo {
    pub fn new(
        initiate_by_oneself: bool,
        punch_model: PunchModelBoxes,
        peer_nat_info: NatInfo,
    ) -> Self {
        Self {
            initiate_by_oneself,
            punch_model,
            peer_nat_info,
        }
    }
    pub fn new_by_oneself(punch_model: PunchModelBoxes, peer_nat_info: NatInfo) -> Self {
        Self {
            initiate_by_oneself: true,
            punch_model,
            peer_nat_info,
        }
    }
    pub fn new_by_other(punch_model: PunchModelBoxes, peer_nat_info: NatInfo) -> Self {
        Self {
            initiate_by_oneself: false,
            punch_model,
            peer_nat_info,
        }
    }
    pub(crate) fn use_ttl(&self) -> bool {
        self.initiate_by_oneself ^ (self.peer_nat_info.seq % 2 == 0)
    }
}

impl FromStr for PunchModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "ipv4-tcp" => Ok(PunchModel::IPv4Tcp),
            "ipv4-udp" => Ok(PunchModel::IPv4Udp),
            "ipv6-tcp" => Ok(PunchModel::IPv6Tcp),
            "ipv6-udp" => Ok(PunchModel::IPv6Udp),
            _ => Err(format!(
                "not match '{}', enum: ipv4-tcp/ipv4-udp/ipv6-tcp/ipv6-udp",
                s
            )),
        }
    }
}

#[derive(Clone)]
pub struct Puncher<PeerID> {
    route_table: RouteTable<PeerID>,
    // 端口顺序
    port_vec: Vec<u16>,
    // 指定IP的打洞记录
    sym_record: Arc<Mutex<HashMap<PeerID, usize>>>,
    count_record: Arc<Mutex<HashMap<PeerID, usize>>>,
    udp_pipe_writer: Option<UdpPipeWriter>,
    tcp_pipe_writer: Option<TcpPipeWriter>,
}
impl<PeerID> From<&Pipe<PeerID>> for Puncher<PeerID> {
    fn from(value: &Pipe<PeerID>) -> Self {
        let writer_ref = value.writer_ref();
        let tcp_pipe_writer = writer_ref.tcp_pipe_writer_ref().map(|v| v.to_owned());
        let udp_pipe_writer = writer_ref.udp_pipe_writer_ref().map(|v| v.to_owned());
        Self::new(
            value.route_table().clone(),
            udp_pipe_writer,
            tcp_pipe_writer,
        )
    }
}
impl<PeerID> Puncher<PeerID> {
    pub fn new(
        route_table: RouteTable<PeerID>,
        udp_pipe_writer: Option<UdpPipeWriter>,
        tcp_pipe_writer: Option<TcpPipeWriter>,
    ) -> Puncher<PeerID> {
        let mut port_vec: Vec<u16> = (1..=65535).collect();
        let mut rng = rand::thread_rng();
        port_vec.shuffle(&mut rng);
        Self {
            route_table,
            port_vec,
            sym_record: Arc::new(Mutex::new(HashMap::new())),
            count_record: Arc::new(Mutex::new(HashMap::new())),
            udp_pipe_writer,
            tcp_pipe_writer,
        }
    }
}
impl<PeerID: Hash + Eq + Clone> Puncher<PeerID> {
    pub fn reset_record(&self, peer_id: &PeerID) {
        self.sym_record.lock().remove(peer_id);
        self.count_record.lock().remove(peer_id);
    }
    /// Call `need_punch` at a certain frequency, and call [`punch_now`](Self::punch_now) after getting true.
    /// Determine whether punching is needed.
    pub fn need_punch(&self, id: &PeerID) -> bool {
        let need = self.route_table.need_punch(id);
        if !need {
            self.reset_record(id);
            return false;
        }
        let count = *self
            .count_record
            .lock()
            .entry(id.clone())
            .and_modify(|v| *v += 1)
            .or_insert(0);
        if count > 8 {
            //降低频率
            let interval = count / 8;
            return count % interval.min(360) == 0;
        }
        true
    }

    /// Call `punch` at a certain frequency
    pub async fn punch(
        &self,
        peer_id: PeerID,
        buf: &[u8],
        punch_info: PunchInfo,
    ) -> anyhow::Result<()> {
        if !self.need_punch(&peer_id) {
            return Ok(());
        }
        self.punch_now(peer_id, buf, punch_info).await
    }
    pub async fn punch_now(
        &self,
        peer_id: PeerID,
        buf: &[u8],
        punch_info: PunchInfo,
    ) -> anyhow::Result<()> {
        let count = self
            .count_record
            .lock()
            .get(&peer_id)
            .cloned()
            .unwrap_or_default();
        let ttl = if punch_info.use_ttl() && count < 255 {
            Some(count.max(3) as u32)
        } else {
            None
        };
        let peer_nat_info = punch_info.peer_nat_info;
        let punch_model = punch_info.punch_model;

        async_scoped::TokioScope::scope_and_block(|s| {
            if let Some(tcp_pipe_writer) = self.tcp_pipe_writer.as_ref() {
                for addr in &peer_nat_info.mapping_tcp_addr {
                    s.spawn(async move {
                        Self::connect_tcp(tcp_pipe_writer, buf, *addr, ttl).await;
                    })
                }
                if punch_model.is_match(PunchModel::IPv4Tcp) {
                    if let Some(addr) = peer_nat_info.local_ipv4_tcp() {
                        s.spawn(async move {
                            Self::connect_tcp(tcp_pipe_writer, buf, addr, ttl).await;
                        })
                    }
                    for addr in peer_nat_info.public_ipv4_tcp() {
                        s.spawn(async move {
                            Self::connect_tcp(tcp_pipe_writer, buf, addr, ttl).await;
                        })
                    }
                }
                if punch_model.is_match(PunchModel::IPv6Tcp) {
                    if let Some(addr) = peer_nat_info.ipv6_tcp_addr() {
                        s.spawn(async move {
                            Self::connect_tcp(tcp_pipe_writer, buf, addr, ttl).await;
                        })
                    }
                }
            }
        });
        self.punch_udp(peer_id, count, buf, &peer_nat_info, &punch_model)
            .await?;

        Ok(())
    }
    async fn connect_tcp(
        tcp_pipe_writer: &TcpPipeWriter,
        buf: &[u8],
        addr: SocketAddr,
        ttl: Option<u32>,
    ) {
        match tokio::time::timeout(
            Duration::from_secs(3),
            tcp_pipe_writer.send_to_addr_multi0(buf, addr, ttl),
        )
        .await
        {
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
        peer_id: PeerID,
        count: usize,
        buf: &[u8],
        peer_nat_info: &NatInfo,
        punch_model: &PunchModelBoxes,
    ) -> anyhow::Result<()> {
        let udp_pipe_writer = if let Some(udp_pipe_writer) = self.udp_pipe_writer.as_ref() {
            udp_pipe_writer
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
            udp_pipe_writer.try_main_send_to_addr(buf, &mapping_udp_v4_addr);

            let mapping_udp_v6_addr: Vec<SocketAddr> = peer_nat_info
                .mapping_udp_addr
                .iter()
                .filter(|a| a.is_ipv6())
                .copied()
                .collect();
            udp_pipe_writer.try_main_send_to_addr(buf, &mapping_udp_v6_addr);
        }
        let local_ipv4_addrs = peer_nat_info.local_ipv4_addrs();
        if !local_ipv4_addrs.is_empty() {
            udp_pipe_writer.try_main_send_to_addr(buf, &local_ipv4_addrs);
        }

        if punch_model.is_match(PunchModel::IPv6Udp) {
            let v6_addr = peer_nat_info.ipv6_addr();
            udp_pipe_writer.try_main_send_to_addr(buf, &v6_addr);
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
                let mut max_k2: usize = rand::thread_rng().gen_range(600..800);
                if count > 8 {
                    //递减探测规模
                    max_k2 = max_k2.mul(8).div(count).max(max_k1 as usize);
                }
                let port = peer_nat_info.public_ports.first().copied().unwrap_or(0);
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
                    nums.shuffle(&mut rand::thread_rng());
                    self.punch_symmetric(
                        udp_pipe_writer,
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
                            udp_pipe_writer,
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
                udp_pipe_writer.try_main_send_to_addr(buf, &addr);
                udp_pipe_writer.try_sub_send_to_addr_v4(buf, addr[0]);
            }
        }
        Ok(())
    }

    async fn punch_symmetric(
        &self,
        udp_pipe_writer: &UdpPipeWriter,
        ports: &[u16],
        buf: &[u8],
        ips: &Vec<Ipv4Addr>,
        max: usize,
    ) -> anyhow::Result<usize> {
        let mut count = 0;
        for (index, port) in ports.iter().enumerate() {
            for pub_ip in ips {
                count += 1;
                if count == max {
                    return Ok(index);
                }
                let addr: SocketAddr = SocketAddr::V4(SocketAddrV4::new(*pub_ip, *port));
                if let Err(e) = udp_pipe_writer.try_send_to_addr(buf, addr) {
                    log::info!("{addr},{e:?}");
                }
                tokio::time::sleep(Duration::from_millis(2)).await
            }
        }
        Ok(ports.len())
    }
}
