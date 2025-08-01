pub use crate::config::Config;
use crate::kcp::{DataType, KcpHandle};
use crate::maintain::start_task;
use async_shutdown::ShutdownManager;
use bytes::BytesMut;
use flume::Receiver;
use parking_lot::Mutex;
use rand::seq::SliceRandom;
pub use rust_p2p_core::nat::NatInfo;
use rust_p2p_core::nat::NatType;
pub use rust_p2p_core::punch::config::*;
use rust_p2p_core::punch::Puncher as CorePuncher;
use rust_p2p_core::route::Index;
use rust_p2p_core::socket::LocalInterface;
pub use rust_p2p_core::tunnel::config::*;
pub use rust_p2p_core::tunnel::tcp::{
    BytesCodec, BytesInitCodec, Decoder, Encoder, InitCodec, LengthPrefixedCodec,
    LengthPrefixedInitCodec,
};
use rust_p2p_core::tunnel::tcp::{TcpTunnel, WeakTcpTunnelSender};
use rust_p2p_core::tunnel::udp::{UDPIndex, UdpTunnel};
use rust_p2p_core::tunnel::{SocketManager, Tunnel, TunnelDispatcher};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

mod config;
mod kcp;
mod maintain;

pub async fn from_config(config: Config) -> io::Result<(ReliableTunnelListener, Puncher)> {
    let tunnel_config = config.tunnel_config;
    let tcp_stun_servers = config.tcp_stun_servers;
    let udp_stun_servers = config.udp_stun_servers;
    let default_interface = tunnel_config
        .udp_tunnel_config
        .as_ref()
        .map(|v| v.default_interface.clone())
        .unwrap_or_default();

    let (unified_tunnel_factory, puncher) =
        rust_p2p_core::tunnel::new_tunnel_component(tunnel_config)?;
    let manager = unified_tunnel_factory.socket_manager();
    let shutdown_manager = ShutdownManager::<()>::new();
    let puncher = Puncher::new(
        default_interface,
        tcp_stun_servers,
        udp_stun_servers,
        puncher,
        manager,
    )
    .await?;
    let listener = ReliableTunnelListener::new(
        shutdown_manager.clone(),
        unified_tunnel_factory,
        puncher.punch_context.clone(),
    );
    start_task(shutdown_manager, puncher.clone());
    Ok((listener, puncher))
}
pub struct ReliableTunnelListener {
    shutdown_manager: ShutdownManager<()>,
    punch_context: Arc<PunchContext>,
    unified_tunnel_factory: TunnelDispatcher,
    kcp_receiver: tokio::sync::mpsc::Receiver<KcpMessageHub>,
    kcp_sender: Sender<KcpMessageHub>,
}
#[derive(Clone)]
pub struct Puncher {
    punch_context: Arc<PunchContext>,
    puncher: CorePuncher,
    socket_manager: SocketManager,
}
impl Drop for ReliableTunnelListener {
    fn drop(&mut self) {
        _ = self.shutdown_manager.trigger_shutdown(());
    }
}
pub(crate) struct PunchContext {
    default_interface: Option<LocalInterface>,
    tcp_stun_servers: Vec<String>,
    udp_stun_servers: Vec<String>,
    nat_info: Arc<Mutex<NatInfo>>,
}
impl PunchContext {
    pub fn new(
        default_interface: Option<LocalInterface>,
        tcp_stun_servers: Vec<String>,
        udp_stun_servers: Vec<String>,
        local_udp_ports: Vec<u16>,
        local_tcp_port: u16,
    ) -> Self {
        let public_udp_ports = vec![0; local_udp_ports.len()];
        let nat_info = NatInfo {
            nat_type: Default::default(),
            public_ips: vec![],
            public_udp_ports,
            mapping_tcp_addr: vec![],
            mapping_udp_addr: vec![],
            public_port_range: 0,
            local_ipv4: Ipv4Addr::UNSPECIFIED,
            ipv6: None,
            local_udp_ports,
            local_tcp_port,
            public_tcp_port: 0,
        };
        Self {
            default_interface,
            tcp_stun_servers,
            udp_stun_servers,
            nat_info: Arc::new(Mutex::new(nat_info)),
        }
    }
    pub fn set_public_info(
        &self,
        nat_type: NatType,
        mut ips: Vec<Ipv4Addr>,
        public_port_range: u16,
    ) {
        ips.retain(rust_p2p_core::extend::addr::is_ipv4_global);
        let mut guard = self.nat_info.lock();
        guard.public_ips = ips;
        guard.nat_type = nat_type;
        guard.public_port_range = public_port_range;
    }
    fn mapping_addr(addr: SocketAddr) -> Option<(Ipv4Addr, u16)> {
        match addr {
            SocketAddr::V4(addr) => Some((*addr.ip(), addr.port())),
            SocketAddr::V6(addr) => addr.ip().to_ipv4_mapped().map(|ip| (ip, addr.port())),
        }
    }
    pub fn update_tcp_public_addr(&self, addr: SocketAddr) {
        let (ip, port) = if let Some(r) = Self::mapping_addr(addr) {
            r
        } else {
            return;
        };
        let mut nat_info = self.nat_info.lock();
        if rust_p2p_core::extend::addr::is_ipv4_global(&ip) && !nat_info.public_ips.contains(&ip) {
            nat_info.public_ips.push(ip);
        }
        nat_info.public_tcp_port = port;
    }
    pub fn update_public_addr(&self, index: Index, addr: SocketAddr) {
        let (ip, port) = if let Some(r) = Self::mapping_addr(addr) {
            r
        } else {
            return;
        };
        let mut nat_info = self.nat_info.lock();

        if rust_p2p_core::extend::addr::is_ipv4_global(&ip) {
            if !nat_info.public_ips.contains(&ip) {
                nat_info.public_ips.push(ip);
            }
            match index {
                Index::Udp(index) => {
                    let index = match index {
                        UDPIndex::MainV4(index) => index,
                        UDPIndex::MainV6(index) => index,
                        UDPIndex::SubV4(_) => return,
                    };
                    if let Some(p) = nat_info.public_udp_ports.get_mut(index) {
                        *p = port;
                    }
                }
                Index::Tcp(_) => {
                    nat_info.public_tcp_port = port;
                }
                _ => {}
            }
        } else {
            log::debug!("not public addr: {addr:?}")
        }
    }
    pub async fn update_local_addr(&self) {
        let local_ipv4 = rust_p2p_core::extend::addr::local_ipv4().await;
        let local_ipv6 = rust_p2p_core::extend::addr::local_ipv6().await;
        let mut nat_info = self.nat_info.lock();
        if let Ok(local_ipv4) = local_ipv4 {
            nat_info.local_ipv4 = local_ipv4;
        }
        nat_info.ipv6 = local_ipv6.ok();
    }
    pub async fn update_nat_info(&self) -> io::Result<NatInfo> {
        self.update_local_addr().await;
        let mut udp_stun_servers = self.udp_stun_servers.clone();
        udp_stun_servers.shuffle(&mut rand::rng());
        let udp_stun_servers = if udp_stun_servers.len() > 3 {
            &udp_stun_servers[..3]
        } else {
            &udp_stun_servers
        };
        let (nat_type, ips, port_range) = rust_p2p_core::stun::stun_test_nat(
            udp_stun_servers.to_vec(),
            self.default_interface.as_ref(),
        )
        .await?;
        self.set_public_info(nat_type, ips, port_range);
        Ok(self.nat_info())
    }
    pub fn nat_info(&self) -> NatInfo {
        self.nat_info.lock().clone()
    }
}
impl Puncher {
    async fn new(
        default_interface: Option<LocalInterface>,
        tcp_stun_servers: Vec<String>,
        udp_stun_servers: Vec<String>,
        puncher: CorePuncher,
        socket_manager: SocketManager,
    ) -> io::Result<Self> {
        let local_tcp_port = if let Some(v) = socket_manager.tcp_socket_manager_as_ref() {
            v.local_addr().port()
        } else {
            0
        };
        let local_udp_ports = if let Some(v) = socket_manager.udp_socket_manager_as_ref() {
            v.local_ports()?
        } else {
            vec![]
        };
        let punch_context = Arc::new(PunchContext::new(
            default_interface,
            tcp_stun_servers,
            udp_stun_servers,
            local_udp_ports,
            local_tcp_port,
        ));
        punch_context.update_local_addr().await;
        Ok(Self {
            punch_context,
            puncher,
            socket_manager,
        })
    }

    pub async fn punch(&self, punch_info: PunchInfo) -> io::Result<()> {
        self.punch_conv(0, punch_info).await
    }

    pub async fn punch_conv(&self, kcp_conv: u32, punch_info: PunchInfo) -> io::Result<()> {
        let mut punch_udp_buf = [0; 8];
        punch_udp_buf[..4].copy_from_slice(&kcp_conv.to_le_bytes());
        // kcp flag
        punch_udp_buf[0] = 0x02;
        if rust_p2p_core::stun::is_stun_response(&punch_udp_buf) {
            return Err(io::Error::other("kcp_conv error"));
        }
        if !self.puncher.need_punch(&punch_info) {
            return Ok(());
        }
        self.puncher
            .punch_now(None, &punch_udp_buf, punch_info)
            .await
    }
    pub fn nat_info(&self) -> NatInfo {
        self.punch_context.nat_info()
    }
}

pub enum ReliableTunnel {
    Tcp(TcpMessageHub),
    Kcp(KcpMessageHub),
}
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ReliableTunnelType {
    Tcp,
    Kcp,
}

impl ReliableTunnelListener {
    fn new(
        shutdown_manager: ShutdownManager<()>,
        unified_tunnel_factory: TunnelDispatcher,
        punch_context: Arc<PunchContext>,
    ) -> Self {
        let (kcp_sender, kcp_receiver) = tokio::sync::mpsc::channel(64);
        Self {
            shutdown_manager,
            punch_context,
            unified_tunnel_factory,
            kcp_receiver,
            kcp_sender,
        }
    }
    pub async fn accept(&mut self) -> io::Result<ReliableTunnel> {
        loop {
            tokio::select! {
                rs=self.unified_tunnel_factory.dispatch()=>{
                    let unified_tunnel = rs?;
                    match unified_tunnel {
                        Tunnel::Udp(udp) => {
                            handle_udp(self.shutdown_manager.clone(), udp, self.kcp_sender.clone(), self.punch_context.clone())?;
                        }
                        Tunnel::Tcp(tcp) => {
                            let local_addr = tcp.local_addr();
                            let remote_addr = tcp.route_key().addr();
                            let sender = tcp.sender()?;
                            let receiver = handle_tcp(self.shutdown_manager.clone(),tcp).await?;
                            let hub = TcpMessageHub::new(local_addr,remote_addr,sender,receiver);
                            return Ok(ReliableTunnel::Tcp(hub))
                        }
                    }
                }
                rs=self.kcp_receiver.recv()=>{
                    return if let Some(hub) = rs{
                        Ok(ReliableTunnel::Kcp(hub))
                    }else{
                        Err(io::Error::from(io::ErrorKind::UnexpectedEof))
                    }
                }
            }
        }
    }
}

impl ReliableTunnel {
    pub async fn send(&self, buf: BytesMut) -> io::Result<()> {
        match &self {
            ReliableTunnel::Tcp(tcp) => tcp.send(buf).await,
            ReliableTunnel::Kcp(kcp) => kcp.send(buf).await,
        }
    }
    pub async fn send_raw(&self, buf: BytesMut) -> io::Result<()> {
        match &self {
            ReliableTunnel::Tcp(tcp) => tcp.send(buf).await,
            ReliableTunnel::Kcp(kcp) => kcp.send_raw(buf).await,
        }
    }
    pub async fn next(&self) -> io::Result<BytesMut> {
        match &self {
            ReliableTunnel::Tcp(tcp) => tcp.next().await,
            ReliableTunnel::Kcp(kcp) => kcp.next().await,
        }
    }
    pub fn local_addr(&self) -> SocketAddr {
        match &self {
            ReliableTunnel::Tcp(tcp) => tcp.local_addr,
            ReliableTunnel::Kcp(kcp) => kcp.local_addr,
        }
    }
    pub fn remote_addr(&self) -> SocketAddr {
        match &self {
            ReliableTunnel::Tcp(tcp) => tcp.remote_addr,
            ReliableTunnel::Kcp(kcp) => kcp.remote_addr,
        }
    }
    pub fn tunnel_type(&self) -> ReliableTunnelType {
        match &self {
            ReliableTunnel::Tcp(_tcp) => ReliableTunnelType::Tcp,
            ReliableTunnel::Kcp(_kcp) => ReliableTunnelType::Kcp,
        }
    }
}
pub struct TcpMessageHub {
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    input: WeakTcpTunnelSender,
    output: Receiver<BytesMut>,
}
impl TcpMessageHub {
    pub(crate) fn new(
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
        input: WeakTcpTunnelSender,
        output: Receiver<BytesMut>,
    ) -> Self {
        Self {
            local_addr,
            remote_addr,
            input,
            output,
        }
    }
    pub async fn send(&self, buf: BytesMut) -> io::Result<()> {
        self.input.send(buf).await
    }
    pub async fn next(&self) -> io::Result<BytesMut> {
        self.output
            .recv_async()
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))
    }
}
pub struct KcpMessageHub {
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    input: Sender<DataType>,
    output: Receiver<BytesMut>,
}

impl KcpMessageHub {
    pub(crate) fn new(
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
        input: Sender<DataType>,
        output: Receiver<BytesMut>,
    ) -> Self {
        Self {
            local_addr,
            remote_addr,
            input,
            output,
        }
    }
    pub async fn send(&self, buf: BytesMut) -> io::Result<()> {
        self.input
            .send(DataType::Kcp(buf))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::WriteZero))
    }
    pub async fn send_raw(&self, buf: BytesMut) -> io::Result<()> {
        self.input
            .send(DataType::Raw(buf))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::WriteZero))
    }
    pub async fn next(&self) -> io::Result<BytesMut> {
        self.output
            .recv_async()
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))
    }
}
async fn handle_tcp(
    shutdown_manager: ShutdownManager<()>,
    mut tcp_tunnel: TcpTunnel,
) -> io::Result<Receiver<BytesMut>> {
    let (sender, receiver) = flume::bounded(128);
    tokio::spawn(async move {
        let mut buf = [0; 65536];
        while let Ok(Ok(len)) = shutdown_manager
            .wrap_cancel(tcp_tunnel.recv(&mut buf))
            .await
        {
            if sender.send_async(buf[..len].into()).await.is_err() {
                break;
            }
        }
    });
    Ok(receiver)
}

fn handle_udp(
    shutdown_manager: ShutdownManager<()>,
    mut udp_tunnel: UdpTunnel,
    sender: Sender<KcpMessageHub>,
    punch_context: Arc<PunchContext>,
) -> io::Result<()> {
    let mut kcp_handle = KcpHandle::new(udp_tunnel.local_addr(), udp_tunnel.sender()?, sender);
    tokio::spawn(async move {
        let mut buf = [0; 65536];

        while let Ok(Some(rs)) = shutdown_manager
            .wrap_cancel(udp_tunnel.recv_from(&mut buf))
            .await
        {
            let (len, route_key) = match rs {
                Ok(rs) => rs,
                Err(e) => {
                    log::warn!("udp_tunnel.recv_from {e:?}");
                    continue;
                }
            };
            // check stun data
            if rust_p2p_core::stun::is_stun_response(&buf[..len]) {
                if let Some(pub_addr) = rust_p2p_core::stun::recv_stun_response(&buf[..len]) {
                    punch_context.update_public_addr(route_key.index(), pub_addr);
                    continue;
                }
            }
            kcp_handle.handle(&buf[..len], route_key).await;
        }
    });
    Ok(())
}
