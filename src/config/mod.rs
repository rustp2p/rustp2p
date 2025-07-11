use std::io;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use bytes::{Buf, BytesMut};

use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocol::node_id::{GroupCode, NodeID};
use crate::protocol::{NetPacket, HEAD_LEN};
use crate::tunnel::{NodeAddress, PeerNodeAddress, RecvResult};
pub use rust_p2p_core::nat::*;
pub use rust_p2p_core::punch::config::{PunchModel, PunchPolicy, PunchPolicySet};
pub use rust_p2p_core::socket::LocalInterface;
pub use rust_p2p_core::tunnel::config::LoadBalance;
use rust_p2p_core::tunnel::recycle::RecycleBuf;
use rust_p2p_core::tunnel::tcp::{Decoder, Encoder, InitCodec};
pub use rust_p2p_core::tunnel::udp::Model;

pub(crate) mod punch_info;

pub(crate) const ROUTE_IDLE_TIME: Duration = Duration::from_secs(10);

pub struct Config {
    pub load_balance: LoadBalance,
    pub major_socket_count: usize,
    pub route_idle_time: Duration,
    pub udp_tunnel_config: Option<UdpTunnelConfig>,
    pub tcp_tunnel_config: Option<TcpTunnelConfig>,
    pub group_code: Option<GroupCode>,
    pub self_id: Option<NodeID>,
    pub direct_addrs: Option<Vec<PeerNodeAddress>>,
    pub send_buffer_size: usize,
    pub recv_buffer_size: usize,
    pub query_id_interval: Duration,
    pub query_id_max_num: usize,
    pub heartbeat_interval: Duration,
    pub tcp_stun_servers: Option<Vec<String>>,
    pub udp_stun_servers: Option<Vec<String>>,
    pub mapping_addrs: Option<Vec<NodeAddress>>,
    pub dns: Option<Vec<String>>,
    pub recycle_buf_cap: usize,
    #[cfg(any(
        feature = "aes-gcm-openssl",
        feature = "aes-gcm-ring",
        feature = "chacha20-poly1305-openssl",
        feature = "chacha20-poly1305-ring"
    ))]
    pub encryption: Option<crate::cipher::Algorithm>,
    pub default_interface: Option<LocalInterface>,
    pub use_v6: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            load_balance: LoadBalance::MinHopLowestLatency,
            major_socket_count: MAX_MAJOR_SOCKET_COUNT,
            udp_tunnel_config: Some(Default::default()),
            tcp_tunnel_config: Some(Default::default()),
            route_idle_time: ROUTE_IDLE_TIME,
            group_code: None,
            self_id: None,
            direct_addrs: None,
            send_buffer_size: 2048,
            recv_buffer_size: 2048,
            query_id_interval: Duration::from_secs(17),
            query_id_max_num: 3,
            heartbeat_interval: Duration::from_secs(5),
            tcp_stun_servers: Some(vec![
                "stun.flashdance.cx".to_string(),
                "stun.sipnet.net".to_string(),
                "stun.nextcloud.com:443".to_string(),
            ]),
            udp_stun_servers: Some(vec![
                "stun.miwifi.com".to_string(),
                "stun.chat.bilibili.com".to_string(),
                "stun.hitv.com".to_string(),
                "stun.l.google.com:19302".to_string(),
                "stun1.l.google.com:19302".to_string(),
                "stun2.l.google.com:19302".to_string(),
            ]),
            mapping_addrs: None,
            dns: None,
            recycle_buf_cap: 64,
            #[cfg(any(
                feature = "aes-gcm-openssl",
                feature = "aes-gcm-ring",
                feature = "chacha20-poly1305-openssl",
                feature = "chacha20-poly1305-ring"
            ))]
            encryption: None,
            default_interface: None,
            use_v6: rust_p2p_core::tunnel::config::UdpTunnelConfig::default()
                .set_use_v6(true)
                .check()
                .is_ok(),
        }
    }
}

pub(crate) const MAX_MAJOR_SOCKET_COUNT: usize = 2;
pub(crate) const MAX_UDP_SUB_SOCKET_COUNT: usize = 82;

impl Config {
    pub fn none_tcp(self) -> Self {
        self
    }
}

impl Config {
    pub fn empty() -> Self {
        Self::default()
    }
    pub fn set_load_balance(mut self, load_balance: LoadBalance) -> Self {
        self.load_balance = load_balance;
        self
    }
    pub fn set_main_socket_count(mut self, count: usize) -> Self {
        self.major_socket_count = count;
        self
    }

    pub fn set_udp_tunnel_config(mut self, config: UdpTunnelConfig) -> Self {
        self.udp_tunnel_config.replace(config);
        self
    }
    pub fn set_tcp_tunnel_config(mut self, config: TcpTunnelConfig) -> Self {
        self.tcp_tunnel_config.replace(config);
        self
    }
    pub fn set_group_code(mut self, group_code: GroupCode) -> Self {
        self.group_code.replace(group_code);
        self
    }
    pub fn set_node_id(mut self, self_id: NodeID) -> Self {
        self.self_id.replace(self_id);
        self
    }
    pub fn set_direct_addrs(mut self, direct_addrs: Vec<PeerNodeAddress>) -> Self {
        self.direct_addrs.replace(direct_addrs);
        self
    }
    pub fn set_send_buffer_size(mut self, send_buffer_size: usize) -> Self {
        self.send_buffer_size = send_buffer_size;
        self
    }
    pub fn set_recv_buffer_size(mut self, recv_buffer_size: usize) -> Self {
        self.recv_buffer_size = recv_buffer_size;
        self
    }
    pub fn set_query_id_interval(mut self, query_id_interval: Duration) -> Self {
        self.query_id_interval = query_id_interval;
        self
    }
    pub fn set_query_id_max_num(mut self, query_id_max_num: usize) -> Self {
        self.query_id_max_num = query_id_max_num;
        self
    }
    pub fn set_heartbeat_interval(mut self, heartbeat_interval: Duration) -> Self {
        self.heartbeat_interval = heartbeat_interval;
        self
    }
    pub fn set_tcp_stun_servers(mut self, tcp_stun_servers: Vec<String>) -> Self {
        self.tcp_stun_servers.replace(tcp_stun_servers);
        self
    }
    pub fn set_udp_stun_servers(mut self, udp_stun_servers: Vec<String>) -> Self {
        self.udp_stun_servers.replace(udp_stun_servers);
        self
    }
    /// Other nodes will attempt to connect to the current node through this configuration
    pub fn set_mapping_addrs(mut self, mapping_addrs: Vec<NodeAddress>) -> Self {
        self.mapping_addrs.replace(mapping_addrs);
        self
    }
    pub fn set_dns(mut self, dns: Vec<String>) -> Self {
        self.dns.replace(dns);
        self
    }
    pub fn set_recycle_buf_cap(mut self, recycle_buf_cap: usize) -> Self {
        self.recycle_buf_cap = recycle_buf_cap;
        self
    }
    #[cfg(any(
        feature = "aes-gcm-openssl",
        feature = "aes-gcm-ring",
        feature = "chacha20-poly1305-openssl",
        feature = "chacha20-poly1305-ring"
    ))]
    pub fn set_encryption(mut self, encryption: crate::cipher::Algorithm) -> Self {
        self.encryption.replace(encryption);
        self
    }
    /// Bind to this network card
    pub fn set_default_interface(mut self, default_interface: LocalInterface) -> Self {
        self.default_interface = Some(default_interface.clone());
        self
    }
    /// Whether to use IPv6
    pub fn set_use_v6(mut self, use_v6: bool) -> Self {
        self.use_v6 = use_v6;
        self
    }
}

pub struct TcpTunnelConfig {
    pub route_idle_time: Duration,
    pub tcp_multiplexing_limit: usize,
    pub tcp_port: u16,
}

impl Default for TcpTunnelConfig {
    fn default() -> Self {
        Self {
            route_idle_time: ROUTE_IDLE_TIME,
            tcp_multiplexing_limit: MAX_MAJOR_SOCKET_COUNT,
            tcp_port: 0,
        }
    }
}

impl TcpTunnelConfig {
    pub fn set_tcp_multiplexing_limit(mut self, tcp_multiplexing_limit: usize) -> Self {
        self.tcp_multiplexing_limit = tcp_multiplexing_limit;
        self
    }
    pub fn set_route_idle_time(mut self, route_idle_time: Duration) -> Self {
        self.route_idle_time = route_idle_time;
        self
    }
    pub fn set_tcp_port(mut self, tcp_port: u16) -> Self {
        self.tcp_port = tcp_port;
        self
    }
}

#[derive(Clone)]
pub struct UdpTunnelConfig {
    pub main_socket_count: usize,
    pub sub_socket_count: usize,
    pub model: Model,
    pub udp_ports: Vec<u16>,
}

impl Default for UdpTunnelConfig {
    fn default() -> Self {
        Self {
            main_socket_count: MAX_MAJOR_SOCKET_COUNT,
            sub_socket_count: MAX_UDP_SUB_SOCKET_COUNT,
            model: Model::Low,
            udp_ports: vec![0, 0],
        }
    }
}

impl UdpTunnelConfig {
    pub fn set_main_socket_count(mut self, count: usize) -> Self {
        self.main_socket_count = count;
        self
    }
    pub fn set_sub_socket_count(mut self, count: usize) -> Self {
        self.sub_socket_count = count;
        self
    }
    pub fn set_model(mut self, model: Model) -> Self {
        self.model = model;
        self
    }

    pub fn set_udp_ports(mut self, udp_ports: Vec<u16>) -> Self {
        self.udp_ports = udp_ports;
        self
    }
    pub fn set_simple_udp_port(mut self, udp_port: u16) -> Self {
        self.udp_ports = vec![udp_port];
        self
    }
}

impl From<Config> for rust_p2p_core::tunnel::config::TunnelConfig {
    fn from(value: Config) -> Self {
        let recycle_buf = if value.recycle_buf_cap > 0 {
            Some(RecycleBuf::new(
                value.recycle_buf_cap,
                value.send_buffer_size..usize::MAX,
            ))
        } else {
            None
        };
        let udp_tunnel_config = value.udp_tunnel_config.map(|v| {
            let mut config: rust_p2p_core::tunnel::config::UdpTunnelConfig = v.into();
            config.recycle_buf.clone_from(&recycle_buf);
            config.use_v6 = value.use_v6;
            config
                .default_interface
                .clone_from(&value.default_interface);
            config
        });
        let tcp_tunnel_config = value.tcp_tunnel_config.map(|v| {
            let mut config: rust_p2p_core::tunnel::config::TcpTunnelConfig = v.into();
            config.recycle_buf = recycle_buf;
            config.use_v6 = value.use_v6;
            config
                .default_interface
                .clone_from(&value.default_interface);
            config
        });
        rust_p2p_core::tunnel::config::TunnelConfig {
            major_socket_count: value.major_socket_count,
            udp_tunnel_config,
            tcp_tunnel_config,
        }
    }
}

impl From<UdpTunnelConfig> for rust_p2p_core::tunnel::config::UdpTunnelConfig {
    fn from(value: UdpTunnelConfig) -> Self {
        rust_p2p_core::tunnel::config::UdpTunnelConfig {
            main_udp_count: value.main_socket_count,
            sub_udp_count: value.sub_socket_count,
            model: value.model,
            default_interface: None,
            udp_ports: value.udp_ports,
            use_v6: false,
            recycle_buf: None,
        }
    }
}

impl From<TcpTunnelConfig> for rust_p2p_core::tunnel::config::TcpTunnelConfig {
    fn from(value: TcpTunnelConfig) -> Self {
        rust_p2p_core::tunnel::config::TcpTunnelConfig {
            route_idle_time: value.route_idle_time,
            tcp_multiplexing_limit: value.tcp_multiplexing_limit,
            default_interface: None,
            tcp_port: value.tcp_port,
            use_v6: false,
            init_codec: Box::new(LengthPrefixedInitCodec),
            recycle_buf: None,
        }
    }
}

/// Fixed-length prefix encoder/decoder.
pub(crate) struct LengthPrefixedEncoder {}

pub(crate) struct LengthPrefixedDecoder {
    buf: BytesMut,
}

impl LengthPrefixedEncoder {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl LengthPrefixedDecoder {
    pub(crate) fn new() -> Self {
        Self {
            buf: Default::default(),
        }
    }
}

#[async_trait]
impl Decoder for LengthPrefixedDecoder {
    async fn decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize> {
        if src.len() < HEAD_LEN {
            return Err(io::Error::other("too short"));
        }
        let mut offset = 0;
        loop {
            if self.buf.is_empty() {
                let len = read.read(&mut src[offset..]).await?;
                if len == 0 {
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                }
                offset += len;
                if let Some(rs) = self.process_packet(src, offset) {
                    return rs;
                }
            } else if let Some(rs) = self.process_buf(src, &mut offset) {
                return rs;
            }
        }
    }

    fn try_decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize> {
        if src.len() < HEAD_LEN {
            return Err(io::Error::other("too short"));
        }
        let mut offset = 0;
        loop {
            if self.buf.is_empty() {
                match read.try_read(&mut src[offset..]) {
                    Ok(len) => {
                        if len == 0 {
                            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                        }
                        offset += len;
                    }
                    Err(e) => {
                        if e.kind() == io::ErrorKind::WouldBlock && offset > 0 {
                            self.buf.extend_from_slice(&src[..offset]);
                        }
                        return Err(e);
                    }
                }
                if let Some(rs) = self.process_packet(src, offset) {
                    return rs;
                }
            } else if let Some(rs) = self.process_buf(src, &mut offset) {
                return rs;
            }
        }
    }
}
impl LengthPrefixedDecoder {
    fn process_buf(&mut self, src: &mut [u8], offset: &mut usize) -> Option<io::Result<usize>> {
        let len = self.buf.len();
        if len < HEAD_LEN {
            src[..len].copy_from_slice(self.buf.as_ref());
            *offset += len;
            self.buf.clear();
            return None;
        }
        let packet = unsafe { NetPacket::new_unchecked(self.buf.as_ref()) };
        let data_length = packet.data_length() as usize;
        if data_length > src.len() {
            return Some(Err(io::Error::other("too short")));
        }
        if data_length > len {
            src[..len].copy_from_slice(self.buf.as_ref());
            *offset += len;
            self.buf.clear();
            None
        } else {
            src[..data_length].copy_from_slice(&self.buf[..data_length]);
            if data_length == len {
                self.buf.clear();
            } else {
                self.buf.advance(data_length);
            }
            Some(Ok(data_length))
        }
    }
    fn process_packet(&mut self, src: &mut [u8], offset: usize) -> Option<io::Result<usize>> {
        if offset < HEAD_LEN {
            return None;
        }
        let packet = unsafe { NetPacket::new_unchecked(&src) };
        let data_length = packet.data_length() as usize;
        if data_length > src.len() {
            return Some(Err(io::Error::other("too short")));
        }
        match data_length.cmp(&offset) {
            std::cmp::Ordering::Less => {
                self.buf.extend_from_slice(&src[data_length..offset]);
                Some(Ok(data_length))
            }
            std::cmp::Ordering::Equal => Some(Ok(data_length)),
            std::cmp::Ordering::Greater => None,
        }
    }
}
#[async_trait]
impl Encoder for LengthPrefixedEncoder {
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<()> {
        let len = data.len();
        let packet = unsafe { NetPacket::new_unchecked(data) };
        if packet.data_length() as usize != len {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        write.write_all(data).await
    }

    async fn encode_multiple(
        &mut self,
        write: &mut OwnedWriteHalf,
        bufs: &[IoSlice<'_>],
    ) -> io::Result<()> {
        let mut index = 0;
        let mut total_written = 0;
        let total: usize = bufs.iter().map(|v| v.len()).sum();
        loop {
            if index == bufs.len() - 1 {
                write.write_all(&bufs[index]).await?;
                return Ok(());
            }
            let len = write.write_vectored(&bufs[index..]).await?;
            if len == 0 {
                return Err(io::Error::from(io::ErrorKind::WriteZero));
            }
            total_written += len;
            if total_written == total {
                return Ok(());
            }
            let mut written = len;
            for buf in &bufs[index..] {
                if buf.len() > written {
                    if written != 0 {
                        index += 1;
                        total_written += buf.len() - written;
                        write.write_all(&buf[written..]).await?;
                        if index == bufs.len() {
                            return Ok(());
                        }
                    }
                    break;
                } else {
                    index += 1;
                    written -= buf.len();
                }
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct LengthPrefixedInitCodec;

impl InitCodec for LengthPrefixedInitCodec {
    fn codec(&self, _addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
        Ok((
            Box::new(LengthPrefixedDecoder::new()),
            Box::new(LengthPrefixedEncoder::new()),
        ))
    }
}
#[async_trait]
pub trait DataInterceptor: Send + Sync {
    async fn pre_handle(&self, data: &mut RecvResult) -> bool;
}
#[derive(Clone)]
pub struct DefaultInterceptor;

#[async_trait]
impl DataInterceptor for DefaultInterceptor {
    async fn pre_handle(&self, _data: &mut RecvResult) -> bool {
        false
    }
}
