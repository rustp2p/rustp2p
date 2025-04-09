pub mod protocol;

pub mod cipher;
pub mod config;
pub mod extend;
pub mod tunnel;

use crate::config::DataInterceptor;
use crate::tunnel::{RecvMetadata, RecvResult};
use async_trait::async_trait;
use cipher::Algorithm;
use config::{TcpTunnelConfig, TunnelManagerConfig, UdpTunnelConfig};
use flume::{Receiver, Sender};
use protocol::node_id::{GroupCode, NodeID};
use std::io;
use std::ops::Deref;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tunnel::{PeerNodeAddress, RecvUserData, Tunnel, TunnelManager, TunnelTransmitHub};

#[derive(Clone)]
pub struct EndPoint {
    receiver: Receiver<(RecvUserData, RecvMetadata)>,
    sender: Arc<TunnelTransmitHub>,
    _handle: Arc<HandleOwner>,
}
struct HandleOwner {
    handle: JoinHandle<()>,
}

impl EndPoint {
    pub async fn recv_from(&self) -> io::Result<(RecvUserData, RecvMetadata)> {
        self.receiver
            .recv_async()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "shutdown"))
    }

    pub async fn send_to<D: Into<NodeID>>(&self, buf: &[u8], dest: D) -> io::Result<()> {
        let mut send_packet = self.sender.allocate_send_packet();
        send_packet.set_payload(buf);
        self.sender.send_packet_to(send_packet, &dest.into()).await
    }

    pub async fn broadcast(&self, buf: &[u8]) -> io::Result<()> {
        let mut send_packet = self.sender.allocate_send_packet();
        send_packet.set_payload(buf);
        self.sender.broadcast_packet(send_packet).await
    }
}

impl Deref for EndPoint {
    type Target = TunnelTransmitHub;

    fn deref(&self) -> &Self::Target {
        &self.sender
    }
}

impl Drop for HandleOwner {
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
    pub fn udp_ports(mut self, port: u16) -> Self {
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
        let mut config = TunnelManagerConfig::empty()
            .set_udp_tunnel_config(
                UdpTunnelConfig::default().set_simple_udp_port(self.udp_port.unwrap_or_default()),
            )
            .set_tcp_tunnel_config(
                TcpTunnelConfig::default().set_tcp_port(self.tcp_port.unwrap_or(0)),
            )
            .set_group_code(self.group_code.ok_or(io::Error::new(
                io::ErrorKind::InvalidInput,
                "group_code is required",
            ))?)
            .set_node_id(self.node_id.ok_or(io::Error::new(
                io::ErrorKind::InvalidInput,
                "node_id is required",
            ))?);
        if let Some(peers) = self.peers {
            config = config.set_direct_addrs(peers);
        }
        #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
        if let Some(encryption) = self.encryption {
            config = config.set_encryption(encryption);
        }

        let tunnel_manager = TunnelManager::new(config).await?;

        EndPoint::from_interceptor0(tunnel_manager, self.interceptor).await
    }
}
impl EndPoint {
    pub async fn from(tunnel_manager: TunnelManager) -> io::Result<Self> {
        EndPoint::from_interceptor0(tunnel_manager, None).await
    }
    pub async fn from_interceptor<T: DataInterceptor + 'static>(
        tunnel_manager: TunnelManager,
        interceptor: T,
    ) -> io::Result<Self> {
        let interceptor = Some(Interceptor {
            inner: Arc::new(interceptor),
        });
        EndPoint::from_interceptor0(tunnel_manager, interceptor).await
    }
    async fn from_interceptor0(
        mut tunnel_manager: TunnelManager,
        interceptor: Option<Interceptor>,
    ) -> io::Result<Self> {
        let (sender, receiver) = flume::unbounded();
        let writer = Arc::new(tunnel_manager.tunnel_send_hub());
        let handle = tokio::spawn(async move {
            while let Ok(tunnel_rx) = tunnel_manager.dispatch().await {
                tokio::spawn(handle(tunnel_rx, sender.clone(), interceptor.clone()));
            }
        });
        Ok(EndPoint {
            sender: writer,
            receiver,
            _handle: Arc::new(HandleOwner { handle }),
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
                log::debug!("recv_from {e:?},{:?}", tunnel_rx.protocol());
                return;
            }
        };
        if let Err(e) = rs {
            log::debug!("recv_data_handle {e:?}");
            continue;
        };
        for x in list.drain(..) {
            if sender.send_async(x).await.is_err() {
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
