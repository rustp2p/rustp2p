use std::time::Duration;

use anyhow::{anyhow, Context};

use crate::pipe::tcp_pipe::{BytesInitCodec, InitCodec};
use crate::pipe::udp_pipe::Model;
use crate::socket::LocalInterface;

pub(crate) const MAX_SYMMETRIC_PIPELINE_NUM: usize = 200;
pub(crate) const MAX_MAIN_PIPELINE_NUM: usize = 10;
pub(crate) const ROUTE_IDLE_TIME: Duration = Duration::from_secs(10);

pub struct PipeConfig {
    pub first_latency: bool,
    pub multi_pipeline: usize,
    pub route_idle_time: Duration,
    pub udp_pipe_config: Option<UdpPipeConfig>,
    pub tcp_pipe_config: Option<TcpPipeConfig>,
    pub enable_extend: bool,
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
        }
    }
}

pub(crate) const MULTI_PIPELINE: usize = 2;
pub(crate) const UDP_SUB_PIPELINE_NUM: usize = 82;

impl PipeConfig {
    pub fn new(tcp_init_codec: Box<dyn InitCodec>) -> PipeConfig {
        let udp_pipe_config = Some(UdpPipeConfig::default());
        let tcp_pipe_config = Some(TcpPipeConfig::new(tcp_init_codec));
        Self {
            first_latency: false,
            multi_pipeline: MULTI_PIPELINE,
            enable_extend: false,
            udp_pipe_config,
            tcp_pipe_config,
            route_idle_time: ROUTE_IDLE_TIME,
        }
    }
}
impl PipeConfig {
    pub fn none_tcp(self) -> Self {
        self
    }
}
impl PipeConfig {
    pub fn empty() -> Self {
        Self {
            first_latency: false,
            multi_pipeline: MULTI_PIPELINE,
            enable_extend: false,
            udp_pipe_config: None,
            tcp_pipe_config: None,
            route_idle_time: ROUTE_IDLE_TIME,
        }
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
    pub fn check(&self) -> anyhow::Result<()> {
        if let Some(udp_pipe_config) = self.udp_pipe_config.as_ref() {
            udp_pipe_config.check()?;
        }
        if let Some(tcp_pipe_config) = self.tcp_pipe_config.as_ref() {
            tcp_pipe_config.check()?;
        }
        Ok(())
    }
}

pub struct TcpPipeConfig {
    pub route_idle_time: Duration,
    pub tcp_multiplexing_limit: usize,
    pub default_interface: Option<LocalInterface>,
    pub tcp_port: u16,
    pub use_v6: bool,
    pub init_codec: Box<dyn InitCodec>,
}

impl Default for TcpPipeConfig {
    fn default() -> Self {
        Self {
            route_idle_time: ROUTE_IDLE_TIME,
            tcp_multiplexing_limit: MULTI_PIPELINE,
            default_interface: None,
            tcp_port: 0,
            use_v6: true,
            init_codec: Box::new(BytesInitCodec),
        }
    }
}

impl TcpPipeConfig {
    pub fn new(init_codec: Box<dyn InitCodec>) -> TcpPipeConfig {
        Self {
            route_idle_time: ROUTE_IDLE_TIME,
            tcp_multiplexing_limit: MULTI_PIPELINE,
            default_interface: None,
            tcp_port: 0,
            use_v6: true,
            init_codec,
        }
    }
    pub fn check(&self) -> anyhow::Result<()> {
        if self.tcp_multiplexing_limit == 0 {
            return Err(anyhow!("tcp_multiplexing_limit cannot be 0"));
        }
        if self.tcp_multiplexing_limit > MAX_MAIN_PIPELINE_NUM {
            return Err(anyhow!("tcp_multiplexing_limit cannot too large"));
        }
        if self.use_v6 {
            socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::STREAM, None)
                .context("Does not support IPV6")?;
        }
        Ok(())
    }
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
    pub fn check(&self) -> anyhow::Result<()> {
        if self.main_pipeline_num == 0 {
            return Err(anyhow!("main_pipeline_num cannot be 0"));
        }
        if self.main_pipeline_num > MAX_MAIN_PIPELINE_NUM {
            return Err(anyhow!("main_pipeline_num cannot too large"));
        }
        if self.sub_pipeline_num > MAX_SYMMETRIC_PIPELINE_NUM {
            return Err(anyhow!("symmetric_pipeline_num is too large"));
        }
        if self.use_v6 {
            socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::DGRAM, None)
                .context("Does not support IPV6")?;
        }
        Ok(())
    }
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
