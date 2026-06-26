use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::protocol::node_id::{GroupCode, NodeID};
use crate::tunnel::{PeerAddr, RecvResult, ResolvedAddr};
pub use rust_p2p_core::nat::*;
pub use rust_p2p_core::punch::config::{PunchModel, PunchPolicy, PunchPolicySet};
use rust_p2p_core::punch::PunchRole;
pub use rust_p2p_core::socket::LocalInterface;
pub use rust_p2p_core::tunnel::config::LoadBalance;
pub use rust_p2p_core::tunnel::config::TcpTunnelConfig;
pub use rust_p2p_core::tunnel::config::UdpTunnelConfig;

pub(crate) mod punch_info;

pub(crate) const ROUTE_IDLE_TIME: Duration = Duration::from_secs(10);

/// Configuration for creating a P2P endpoint.
///
/// `Config` contains all the parameters needed to configure a P2P node,
/// including network settings, encryption, NAT traversal, and routing options.
///
/// # Examples
///
/// ```rust,no_run
/// use rustp2p::Config;
/// use std::net::Ipv4Addr;
///
/// let config = Config::new()
///     .node_id(Ipv4Addr::new(10, 0, 0, 1).into());
/// ```
pub struct Config {
    pub(crate) load_balance: LoadBalance,
    pub(crate) major_socket_count: usize,
    pub(crate) route_idle_time: Duration,
    pub(crate) udp_tunnel_config: Option<UdpTunnelConfig>,
    pub(crate) tcp_tunnel_config: Option<TcpTunnelConfig>,
    pub(crate) group_code: Option<GroupCode>,
    pub(crate) node_id: Option<NodeID>,
    pub(crate) direct_addrs: Option<Vec<PeerAddr>>,
    pub(crate) send_buffer_size: usize,
    pub(crate) recv_buffer_size: usize,
    pub(crate) query_id_interval: Duration,
    pub(crate) query_id_max_num: usize,
    pub(crate) heartbeat_interval: Duration,
    pub(crate) tcp_stun_servers: Option<Vec<String>>,
    pub(crate) udp_stun_servers: Option<Vec<String>>,
    pub(crate) mapping_addrs: Option<Vec<ResolvedAddr>>,
    pub(crate) dns: Option<Vec<String>>,
    pub(crate) recycle_buf_cap: usize,
    #[cfg(any(
        feature = "aes-gcm-openssl",
        feature = "aes-gcm-ring",
        feature = "chacha20-poly1305-openssl",
        feature = "chacha20-poly1305-ring"
    ))]
    pub(crate) encryption: Option<crate::cipher::Algorithm>,
    pub(crate) default_interface: Option<LocalInterface>,
    pub(crate) use_v6: bool,
    pub(crate) punching_policy: Option<Arc<dyn PunchingPolicy>>,
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
            node_id: None,
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
                .use_v6(true)
                .check()
                .is_ok(),
            punching_policy: None,
        }
    }
}

pub(crate) const MAX_MAJOR_SOCKET_COUNT: usize = 2;

impl Config {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn load_balance(mut self, load_balance: LoadBalance) -> Self {
        self.load_balance = load_balance;
        self
    }
    pub fn main_socket_count(mut self, count: usize) -> Self {
        self.major_socket_count = count;
        self
    }

    pub fn udp_tunnel_config(mut self, config: UdpTunnelConfig) -> Self {
        self.udp_tunnel_config.replace(config);
        self
    }
    pub fn tcp_tunnel_config(mut self, config: TcpTunnelConfig) -> Self {
        self.tcp_tunnel_config.replace(config);
        self
    }
    pub fn group_code(mut self, group_code: GroupCode) -> Self {
        self.group_code.replace(group_code);
        self
    }
    pub fn node_id(mut self, node_id: NodeID) -> Self {
        self.node_id.replace(node_id);
        self
    }
    pub fn direct_addrs(mut self, direct_addrs: Vec<PeerAddr>) -> Self {
        self.direct_addrs.replace(direct_addrs);
        self
    }
    pub fn send_buffer_size(mut self, send_buffer_size: usize) -> Self {
        self.send_buffer_size = send_buffer_size;
        self
    }
    pub fn recv_buffer_size(mut self, recv_buffer_size: usize) -> Self {
        self.recv_buffer_size = recv_buffer_size;
        self
    }
    pub fn query_id_interval(mut self, query_id_interval: Duration) -> Self {
        self.query_id_interval = query_id_interval;
        self
    }
    pub fn query_id_max_num(mut self, query_id_max_num: usize) -> Self {
        self.query_id_max_num = query_id_max_num;
        self
    }
    pub fn heartbeat_interval(mut self, heartbeat_interval: Duration) -> Self {
        self.heartbeat_interval = heartbeat_interval;
        self
    }
    pub fn tcp_stun_servers(mut self, tcp_stun_servers: Vec<String>) -> Self {
        self.tcp_stun_servers.replace(tcp_stun_servers);
        self
    }
    pub fn udp_stun_servers(mut self, udp_stun_servers: Vec<String>) -> Self {
        self.udp_stun_servers.replace(udp_stun_servers);
        self
    }
    /// Other nodes will attempt to connect to the current node through this configuration
    pub fn mapping_addrs(mut self, mapping_addrs: Vec<ResolvedAddr>) -> Self {
        self.mapping_addrs.replace(mapping_addrs);
        self
    }
    pub fn dns(mut self, dns: Vec<String>) -> Self {
        self.dns.replace(dns);
        self
    }
    pub fn recycle_buf_cap(mut self, recycle_buf_cap: usize) -> Self {
        self.recycle_buf_cap = recycle_buf_cap;
        self
    }
    #[cfg(any(
        feature = "aes-gcm-openssl",
        feature = "aes-gcm-ring",
        feature = "chacha20-poly1305-openssl",
        feature = "chacha20-poly1305-ring"
    ))]
    pub fn encryption(mut self, encryption: crate::cipher::Algorithm) -> Self {
        self.encryption.replace(encryption);
        self
    }
    /// Bind to this network card
    pub fn default_interface(mut self, default_interface: LocalInterface) -> Self {
        self.default_interface = Some(default_interface.clone());
        self
    }
    /// Whether to use IPv6
    pub fn use_v6(mut self, use_v6: bool) -> Self {
        self.use_v6 = use_v6;
        self
    }
    pub fn punching_policy<T: PunchingPolicy + 'static>(mut self, punching_policy: T) -> Self {
        self.punching_policy = Some(Arc::new(punching_policy));
        self
    }
    pub fn punching_policy_arc(
        mut self,
        punching_policy: Arc<dyn PunchingPolicy + 'static>,
    ) -> Self {
        self.punching_policy = Some(punching_policy);
        self
    }
}

/// TCP tunnel configuration.
///
/// Controls TCP connection behavior including multiplexing and idle timeouts.
///
/// # Examples
///
/// ```rust
/// use rustp2p::TcpTunnelConfig;
/// use std::time::Duration;
///
/// let config = TcpTunnelConfig::default()
///     .tcp_port(8080)
///     .multiplex_limit(4);
/// ```

/// UDP tunnel configuration.
///
/// Controls UDP socket behavior including port allocation and socket model.
///
/// # Examples
///
/// ```rust
/// use rustp2p::UdpTunnelConfig;
///
/// let config = UdpTunnelConfig::default()
///     .simple_udp_port(8080);
/// ```

impl From<Config> for rust_p2p_core::tunnel::config::TunnelConfig {
    fn from(value: Config) -> Self {
        let udp_tunnel_config = value.udp_tunnel_config.map(|mut config| {
            config.use_v6 = value.use_v6;
            config
                .default_interface
                .clone_from(&value.default_interface);
            config
        });
        let tcp_tunnel_config = value.tcp_tunnel_config.map(|mut config| {
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

#[async_trait]
/// Trait for intercepting and preprocessing received data.
///
/// Implement this trait to inspect or modify data before it's delivered
/// to the application. Useful for logging, filtering, or custom protocol handling.
///
/// # Examples
///
/// ```rust,no_run
/// use rustp2p::RecvResult;
/// use async_trait::async_trait;
///
/// // Note: DataInterceptor is not directly exported, use through Builder
/// // struct MyInterceptor;
/// // impl DataInterceptor for MyInterceptor { ... }
/// ```
pub trait DataInterceptor: Send + Sync {
    /// Preprocesses received data before delivery.
    ///
    /// # Arguments
    ///
    /// * `data` - The received data to process
    ///
    /// # Returns
    ///
    /// `true` to drop the packet, `false` to continue processing.
    async fn pre_handle(&self, data: &mut RecvResult) -> bool;
}

/// Default data interceptor that performs no interception.
#[derive(Clone)]
pub struct DefaultInterceptor;

#[async_trait]
impl DataInterceptor for DefaultInterceptor {
    async fn pre_handle(&self, _data: &mut RecvResult) -> bool {
        false
    }
}

/// Trait for customizing NAT hole punching behavior.
///
/// Implement this trait to control which peers to attempt hole punching with.
///
/// # Examples
///
/// ```rust,no_run
/// use rustp2p::NodeID;
/// // Note: PunchingPolicy is not directly exported, use through Builder
/// // struct MyPolicy;
/// // impl PunchingPolicy for MyPolicy { ... }
/// ```
pub trait PunchingPolicy: Send + Sync {
    /// Determines whether to attempt hole punching with a peer.
    ///
    /// # Arguments
    ///
    /// * `punch_role` - The role in the punching process
    /// * `node_id` - The peer's node ID
    ///
    /// # Returns
    ///
    /// `true` to attempt punching, `false` to skip.
    fn should_punch(&self, punch_role: PunchRole, node_id: &NodeID) -> bool;
}

/// Default punching policy that always attempts hole punching.
#[allow(dead_code)]
pub struct DefaultPunchingPolicy;
impl PunchingPolicy for DefaultPunchingPolicy {
    fn should_punch(&self, _punch_role: PunchRole, _node_id: &NodeID) -> bool {
        true
    }
}
