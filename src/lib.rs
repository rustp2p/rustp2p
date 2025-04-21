mod protocol;

pub mod cipher;
mod config;
mod extend;
mod reliable;
mod tunnel;
pub use protocol::{node_id, NetPacket, HEAD_LEN};
pub use tunnel::{NodeAddress, PeerNodeAddress, RecvUserData};

#[cfg(feature = "use-kcp")]
pub use reliable::*;

pub use crate::config::{DataInterceptor, DefaultInterceptor};
use crate::protocol::protocol_type::ProtocolType;
pub use crate::tunnel::{RecvMetadata, RecvResult};
use async_trait::async_trait;
use cipher::Algorithm;
pub use config::{
    Config, NatType, PunchModel, PunchPolicy, PunchPolicySet, TcpTunnelConfig, UdpTunnelConfig,
};
use flume::{Receiver, Sender, TryRecvError};
use protocol::node_id::{GroupCode, NodeID};
pub use rust_p2p_core::route::RouteKey;
use std::io;
use std::ops::Deref;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tunnel::{Tunnel, TunnelDispatcher, TunnelRouter};

pub struct EndPoint {
    #[cfg(feature = "use-kcp")]
    kcp_context: KcpContext,
    input: Receiver<(RecvUserData, RecvMetadata)>,
    output: TunnelRouter,
    _handle: OwnedJoinHandle,
}

impl EndPoint {
    pub async fn recv_from(&self) -> io::Result<(RecvUserData, RecvMetadata)> {
        self.input
            .recv_async()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "shutdown"))
    }

    pub async fn send_to<D: Into<NodeID>>(&self, buf: &[u8], dest: D) -> io::Result<()> {
        self.output.send_to(buf, dest).await
    }
    pub fn try_recv_from(&self) -> io::Result<(RecvUserData, RecvMetadata)> {
        match self.input.try_recv() {
            Ok(rs) => Ok(rs),
            Err(e) => match e {
                TryRecvError::Empty => Err(io::Error::from(io::ErrorKind::WouldBlock)),
                TryRecvError::Disconnected => {
                    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "shutdown"))
                }
            },
        }
    }
    pub fn try_send_to<D: Into<NodeID>>(&self, buf: &[u8], dest: D) -> io::Result<()> {
        self.output.try_send_to(buf, dest)
    }
    pub async fn send_to_route<D: Into<NodeID>>(
        &self,
        buf: &[u8],
        dest: D,
        route_key: &RouteKey,
    ) -> io::Result<()> {
        let mut send_packet = self.output.allocate_send_packet();
        send_packet.set_payload(buf);
        self.output
            .send_packet_to_route(send_packet, &dest.into(), Some(route_key))
            .await
    }
    pub async fn try_send_to_route<D: Into<NodeID>>(
        &self,
        buf: &[u8],
        dest: D,
        route_key: &RouteKey,
    ) -> io::Result<()> {
        let mut send_packet = self.output.allocate_send_packet();
        send_packet.set_payload(buf);
        self.output
            .try_send_packet_to_route(send_packet, &dest.into(), Some(route_key))
    }

    pub async fn broadcast(&self, buf: &[u8]) -> io::Result<()> {
        let mut send_packet = self.output.allocate_send_packet();
        send_packet.set_payload(buf);
        self.output.broadcast_packet(send_packet).await
    }
}

impl Deref for EndPoint {
    type Target = TunnelRouter;

    fn deref(&self) -> &Self::Target {
        &self.output
    }
}
struct OwnedJoinHandle {
    handle: JoinHandle<()>,
}
impl Drop for OwnedJoinHandle {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub struct Builder {
    udp_port: Option<u16>,
    tcp_port: Option<u16>,
    peers: Option<Vec<PeerNodeAddress>>,
    group_code: Option<GroupCode>,
    encryption: Option<Algorithm>,
    node_id: Option<NodeID>,
    interceptor: Option<Interceptor>,
}
impl Builder {
    pub fn new() -> Self {
        Self {
            udp_port: None,
            tcp_port: None,
            peers: None,
            group_code: None,
            encryption: None,
            node_id: None,
            interceptor: None,
        }
    }
    pub fn udp_port(mut self, port: u16) -> Self {
        self.udp_port = Some(port);
        self
    }
    pub fn tcp_port(mut self, port: u16) -> Self {
        self.tcp_port = Some(port);
        self
    }
    pub fn peers(mut self, peers: Vec<PeerNodeAddress>) -> Self {
        self.peers = Some(peers);
        self
    }
    pub fn group_code(mut self, group_code: GroupCode) -> Self {
        self.group_code = Some(group_code);
        self
    }
    pub fn encryption(mut self, encryption: Algorithm) -> Self {
        self.encryption = Some(encryption);
        self
    }
    pub fn node_id(mut self, node_id: NodeID) -> Self {
        self.node_id = Some(node_id);
        self
    }
    pub fn interceptor<I: DataInterceptor + 'static>(mut self, interceptor: I) -> Self {
        self.interceptor = Some(Interceptor {
            inner: Arc::new(interceptor),
        });
        self
    }
    pub async fn build(self) -> io::Result<EndPoint> {
        let mut config = Config::empty()
            .set_udp_tunnel_config(
                UdpTunnelConfig::default().set_simple_udp_port(self.udp_port.unwrap_or_default()),
            )
            .set_tcp_tunnel_config(
                TcpTunnelConfig::default().set_tcp_port(self.tcp_port.unwrap_or(0)),
            )
            .set_node_id(self.node_id.ok_or(io::Error::new(
                io::ErrorKind::InvalidInput,
                "node_id is required",
            ))?);
        if let Some(group_code) = self.group_code {
            config = config.set_group_code(group_code);
        }
        if let Some(peers) = self.peers {
            config = config.set_direct_addrs(peers);
        }
        #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
        if let Some(encryption) = self.encryption {
            config = config.set_encryption(encryption);
        }

        let tunnel_manager = TunnelDispatcher::new(config).await?;

        EndPoint::with_interceptor_impl(tunnel_manager, self.interceptor).await
    }
}
impl EndPoint {
    pub async fn new(config: Config) -> io::Result<Self> {
        let tunnel_hub = TunnelDispatcher::new(config).await?;
        EndPoint::with_interceptor_impl(tunnel_hub, None).await
    }
    pub async fn with_interceptor<T: DataInterceptor + 'static>(
        config: Config,
        interceptor: T,
    ) -> io::Result<Self> {
        let tunnel_hub = TunnelDispatcher::new(config).await?;
        let interceptor = Some(Interceptor {
            inner: Arc::new(interceptor),
        });
        EndPoint::with_interceptor_impl(tunnel_hub, interceptor).await
    }
    async fn with_interceptor_impl(
        mut tunnel_hub: TunnelDispatcher,
        interceptor: Option<Interceptor>,
    ) -> io::Result<Self> {
        let (sender, receiver) = flume::unbounded();
        let writer = tunnel_hub.tunnel_router();
        #[cfg(feature = "use-kcp")]
        let kcp_context = KcpContext::default();
        #[cfg(feature = "use-kcp")]
        let kcp_data_input = kcp_context.clone();
        let handle = tokio::spawn(async move {
            while let Ok(tunnel_rx) = tunnel_hub.dispatch().await {
                tokio::spawn(handle(
                    tunnel_rx,
                    sender.clone(),
                    #[cfg(feature = "use-kcp")]
                    kcp_data_input.clone(),
                    interceptor.clone(),
                ));
            }
        });
        let _handle = OwnedJoinHandle { handle };
        Ok(EndPoint {
            #[cfg(feature = "use-kcp")]
            kcp_context,
            output: writer,
            input: receiver,
            _handle,
        })
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

async fn handle(
    mut tunnel_rx: Tunnel,
    sender: Sender<(RecvUserData, RecvMetadata)>,
    #[cfg(feature = "use-kcp")] kcp_data_input: KcpContext,
    interceptor: Option<Interceptor>,
) {
    let mut list = Vec::with_capacity(16);
    let interceptor = interceptor.as_ref();
    loop {
        let rs = match tunnel_rx
            .preprocess_batch_recv(interceptor, &mut list)
            .await
        {
            Ok(rs) => rs,
            Err(e) => {
                log::debug!(
                    "recv_from {e:?},{:?} {:?}",
                    tunnel_rx.protocol(),
                    tunnel_rx.remote_addr()
                );
                return;
            }
        };
        if let Err(e) = rs {
            log::debug!("recv_data_handle {e:?}");
            continue;
        };
        for (data, meta_data) in list.drain(..) {
            if meta_data.protocol() == ProtocolType::KcpData.into() {
                #[cfg(feature = "use-kcp")]
                kcp_data_input
                    .input(data.payload(), meta_data.src_id())
                    .await;
            } else if sender.send_async((data, meta_data)).await.is_err() {
                break;
            }
        }
    }
}
#[derive(Clone)]
pub(crate) struct Interceptor {
    inner: Arc<dyn DataInterceptor>,
}
#[async_trait]
impl DataInterceptor for Interceptor {
    async fn pre_handle(&self, data: &mut RecvResult) -> bool {
        self.inner.pre_handle(data).await
    }
}
