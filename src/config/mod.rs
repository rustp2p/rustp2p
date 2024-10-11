use std::io;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::time::Duration;

use crate::pipe::{NodeAddress, PeerNodeAddress};
use crate::protocol::node_id::{GroupCode, NodeID};
use crate::protocol::{NetPacket, HEAD_LEN};
use async_trait::async_trait;
use bytes::{Buf, BytesMut};
use rust_p2p_core::pipe::recycle::RecycleBuf;
use rust_p2p_core::pipe::tcp_pipe::{Decoder, Encoder, InitCodec};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

pub(crate) mod punch_info;

pub use rust_p2p_core::pipe::udp_pipe::Model;
pub use rust_p2p_core::socket::LocalInterface;

pub(crate) const ROUTE_IDLE_TIME: Duration = Duration::from_secs(10);

pub struct PipeConfig {
    pub first_latency: bool,
    pub multi_pipeline: usize,
    pub route_idle_time: Duration,
    pub udp_pipe_config: Option<UdpPipeConfig>,
    pub tcp_pipe_config: Option<TcpPipeConfig>,
    pub enable_extend: bool,
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
    pub encryption: Option<String>,
}

impl Default for PipeConfig {
    fn default() -> Self {
        Self {
            first_latency: false,
            multi_pipeline: MULTI_PIPELINE,
            enable_extend: false,
            udp_pipe_config: Some(Default::default()),
            tcp_pipe_config: Some(Default::default()),
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
            encryption: None,
        }
    }
}

pub(crate) const MULTI_PIPELINE: usize = 2;
pub(crate) const UDP_SUB_PIPELINE_NUM: usize = 82;

impl PipeConfig {
    pub fn none_tcp(self) -> Self {
        self
    }
}

impl PipeConfig {
    pub fn empty() -> Self {
        Self::default()
    }
    pub fn set_first_latency(mut self, first_latency: bool) -> Self {
        self.first_latency = first_latency;
        self
    }
    pub fn set_main_pipeline_num(mut self, main_pipeline_num: usize) -> Self {
        self.multi_pipeline = main_pipeline_num;
        self
    }
    pub fn set_enable_extend(mut self, enable_extend: bool) -> Self {
        self.enable_extend = enable_extend;
        self
    }
    pub fn set_udp_pipe_config(mut self, udp_pipe_config: UdpPipeConfig) -> Self {
        self.udp_pipe_config.replace(udp_pipe_config);
        self
    }
    pub fn set_tcp_pipe_config(mut self, tcp_pipe_config: TcpPipeConfig) -> Self {
        self.tcp_pipe_config.replace(tcp_pipe_config);
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
    pub fn set_encryption(mut self, encryption: String) -> Self {
        self.encryption.replace(encryption);
        self
    }
}

pub struct TcpPipeConfig {
    pub route_idle_time: Duration,
    pub tcp_multiplexing_limit: usize,
    pub default_interface: Option<LocalInterface>,
    pub tcp_port: u16,
    pub use_v6: bool,
}

impl Default for TcpPipeConfig {
    fn default() -> Self {
        Self {
            route_idle_time: ROUTE_IDLE_TIME,
            tcp_multiplexing_limit: MULTI_PIPELINE,
            default_interface: None,
            tcp_port: 0,
            use_v6: true,
        }
    }
}

impl TcpPipeConfig {
    pub fn set_tcp_multiplexing_limit(mut self, tcp_multiplexing_limit: usize) -> Self {
        self.tcp_multiplexing_limit = tcp_multiplexing_limit;
        self
    }
    pub fn set_route_idle_time(mut self, route_idle_time: Duration) -> Self {
        self.route_idle_time = route_idle_time;
        self
    }
    pub fn set_default_interface(mut self, default_interface: LocalInterface) -> Self {
        self.default_interface = Some(default_interface.clone());
        self
    }
    pub fn set_tcp_port(mut self, tcp_port: u16) -> Self {
        self.tcp_port = tcp_port;
        self
    }
    pub fn set_use_v6(mut self, use_v6: bool) -> Self {
        self.use_v6 = use_v6;
        self
    }
}

#[derive(Clone)]
pub struct UdpPipeConfig {
    pub main_pipeline_num: usize,
    pub sub_pipeline_num: usize,
    pub model: Model,
    pub default_interface: Option<LocalInterface>,
    pub udp_ports: Vec<u16>,
    pub use_v6: bool,
}

impl Default for UdpPipeConfig {
    fn default() -> Self {
        Self {
            main_pipeline_num: MULTI_PIPELINE,
            sub_pipeline_num: UDP_SUB_PIPELINE_NUM,
            model: Model::Low,
            default_interface: None,
            udp_ports: vec![0, 0],
            use_v6: true,
        }
    }
}

impl UdpPipeConfig {
    pub fn set_main_pipeline_num(mut self, main_pipeline_num: usize) -> Self {
        self.main_pipeline_num = main_pipeline_num;
        self
    }
    pub fn set_sub_pipeline_num(mut self, sub_pipeline_num: usize) -> Self {
        self.sub_pipeline_num = sub_pipeline_num;
        self
    }
    pub fn set_model(mut self, model: Model) -> Self {
        self.model = model;
        self
    }
    pub fn set_default_interface(mut self, default_interface: LocalInterface) -> Self {
        self.default_interface = Some(default_interface.clone());
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
    pub fn set_use_v6(mut self, use_v6: bool) -> Self {
        self.use_v6 = use_v6;
        self
    }
}

impl From<PipeConfig> for rust_p2p_core::pipe::config::PipeConfig {
    fn from(value: PipeConfig) -> Self {
        let recycle_buf = if value.recycle_buf_cap > 0 {
            Some(RecycleBuf::new(
                value.recycle_buf_cap,
                value.send_buffer_size..value.send_buffer_size + 1,
            ))
        } else {
            None
        };
        let udp_pipe_config = value.udp_pipe_config.map(|v| {
            let mut config: rust_p2p_core::pipe::config::UdpPipeConfig = v.into();
            config.recycle_buf = recycle_buf.clone();
            config
        });
        let tcp_pipe_config = value.tcp_pipe_config.map(|v| {
            let mut config: rust_p2p_core::pipe::config::TcpPipeConfig = v.into();
            config.recycle_buf = recycle_buf;
            config
        });
        rust_p2p_core::pipe::config::PipeConfig {
            first_latency: value.first_latency,
            multi_pipeline: value.multi_pipeline,
            route_idle_time: value.route_idle_time,
            udp_pipe_config,
            tcp_pipe_config,
            enable_extend: value.enable_extend,
        }
    }
}

impl From<UdpPipeConfig> for rust_p2p_core::pipe::config::UdpPipeConfig {
    fn from(value: UdpPipeConfig) -> Self {
        rust_p2p_core::pipe::config::UdpPipeConfig {
            main_pipeline_num: value.main_pipeline_num,
            sub_pipeline_num: value.sub_pipeline_num,
            model: value.model,
            default_interface: value.default_interface,
            udp_ports: value.udp_ports,
            use_v6: value.use_v6,
            recycle_buf: None,
        }
    }
}

impl From<TcpPipeConfig> for rust_p2p_core::pipe::config::TcpPipeConfig {
    fn from(value: TcpPipeConfig) -> Self {
        rust_p2p_core::pipe::config::TcpPipeConfig {
            route_idle_time: value.route_idle_time,
            tcp_multiplexing_limit: value.tcp_multiplexing_limit,
            default_interface: value.default_interface,
            tcp_port: value.tcp_port,
            use_v6: value.use_v6,
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
            return Err(io::Error::new(io::ErrorKind::Other, "too short"));
        }
        let mut offset = 0;
        loop {
            if self.buf.is_empty() {
                let len = read.read(&mut src[offset..]).await?;
                offset += len;
                if offset < HEAD_LEN {
                    continue;
                }
                let packet = NetPacket::unchecked(&src);
                let data_length = packet.data_length() as usize;
                if data_length > src.len() {
                    return Err(io::Error::new(io::ErrorKind::Other, "too short"));
                }
                match data_length.cmp(&offset) {
                    std::cmp::Ordering::Less => {
                        self.buf.extend_from_slice(&src[data_length..offset]);
                        return Ok(data_length);
                    }
                    std::cmp::Ordering::Equal => {
                        return Ok(data_length);
                    }
                    std::cmp::Ordering::Greater => {
                        continue;
                    }
                }
            } else {
                let len = self.buf.len();
                if len < HEAD_LEN {
                    src[..len].copy_from_slice(self.buf.as_ref());
                    offset += len;
                    self.buf.clear();
                    continue;
                }
                let packet = NetPacket::unchecked(self.buf.as_ref());
                let data_length = packet.data_length() as usize;
                if data_length > src.len() {
                    return Err(io::Error::new(io::ErrorKind::Other, "too short"));
                }
                if data_length > len {
                    src[..len].copy_from_slice(self.buf.as_ref());
                    offset += len;
                    self.buf.clear();
                    continue;
                } else {
                    src[..data_length].copy_from_slice(&self.buf[..data_length]);
                    if data_length == len {
                        self.buf.clear();
                    } else {
                        self.buf.advance(data_length);
                    }
                    return Ok(data_length);
                }
            }
        }
    }
}

#[async_trait]
impl Encoder for LengthPrefixedEncoder {
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<()> {
        let len = data.len();
        let packet = NetPacket::unchecked(data);
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
                    if index == bufs.len() - 1 {
                        write.write_all(buf).await?;
                        return Ok(());
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

pub(crate) struct LengthPrefixedInitCodec;

impl InitCodec for LengthPrefixedInitCodec {
    fn codec(&self, _addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
        Ok((
            Box::new(LengthPrefixedDecoder::new()),
            Box::new(LengthPrefixedEncoder::new()),
        ))
    }
}
