use rust_p2p_core::tunnel::config::TunnelConfig;

pub struct Config {
    pub(crate) tunnel_config: TunnelConfig,
    pub(crate) tcp_stun_servers: Vec<String>,
    pub(crate) udp_stun_servers: Vec<String>,
}
impl Config {
    pub fn new(tunnel_config: TunnelConfig) -> Self {
        Self {
            tunnel_config,
            tcp_stun_servers: vec![
                "stun.flashdance.cx".to_string(),
                "stun.sipnet.net".to_string(),
                "stun.nextcloud.com:443".to_string(),
            ],
            udp_stun_servers: vec![
                "stun.miwifi.com".to_string(),
                "stun.chat.bilibili.com".to_string(),
                "stun.hitv.com".to_string(),
                "stun.l.google.com:19302".to_string(),
                "stun1.l.google.com:19302".to_string(),
                "stun2.l.google.com:19302".to_string(),
            ],
        }
    }
    pub fn set_tcp_stun_servers(&mut self, tcp_stun_servers: Vec<String>) {
        self.tcp_stun_servers = tcp_stun_servers
    }
    pub fn set_udp_stun_servers(&mut self, udp_stun_servers: Vec<String>) {
        self.udp_stun_servers = udp_stun_servers
    }
}
