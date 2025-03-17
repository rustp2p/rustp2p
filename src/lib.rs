pub mod protocol;

pub mod cipher;
pub mod config;
pub mod extend;
pub mod pipe;

use crate::pipe::PeerNodeAddress;
use cipher::Algorithm;
use config::{PipeConfig, TcpPipeConfig, UdpPipeConfig};
use flume::{Receiver, Sender};
use pipe::{Pipe, PipeLine, PipeWriter, RecvUserData};
use protocol::node_id::{GroupCode, NodeID};
use rust_p2p_core::async_compat::JoinHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct EndPoint {
    receiver: Receiver<RecvUserData>,
    sender: PipeWriter,
    _handle: Arc<HandleOwner>,
}
struct HandleOwner {
    handle: JoinHandle<()>,
}

impl EndPoint {
    pub async fn recv(&self) -> std::io::Result<RecvUserData> {
        self.receiver
            .recv_async()
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "shutdown"))
    }
    pub async fn send(&self, data: &[u8], node_id: NodeID) -> std::io::Result<()> {
        let mut send_packet = self.sender.allocate_send_packet();
        send_packet.set_payload(data);
        self.sender.send_packet_to(send_packet, &node_id).await
    }
}

impl Drop for HandleOwner {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub struct Builder {
    udp_ports: Option<Vec<u16>>,
    tcp_port: Option<u16>,
    peers: Option<Vec<PeerNodeAddress>>,
    group_code: Option<GroupCode>,
    encryption: Option<Algorithm>,
    node_id: Option<NodeID>,
}
impl Builder {
    pub fn new() -> Self {
        Self {
            udp_ports: None,
            tcp_port: None,
            peers: None,
            group_code: None,
            encryption: None,
            node_id: None,
        }
    }
    pub fn udp_ports(mut self, ports: Vec<u16>) -> Self {
        self.udp_ports = Some(ports);
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
    pub async fn build(self) -> std::io::Result<EndPoint> {
        let mut config = PipeConfig::empty()
            .set_udp_pipe_config(
                UdpPipeConfig::default().set_udp_ports(self.udp_ports.unwrap_or_default()),
            )
            .set_tcp_pipe_config(TcpPipeConfig::default().set_tcp_port(self.tcp_port.unwrap_or(0)))
            .set_group_code(self.group_code.ok_or(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "group_code is required",
            ))?)
            .set_node_id(self.node_id.ok_or(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "node_id is required",
            ))?);
        if let Some(peers) = self.peers {
            config = config.set_direct_addrs(peers);
        }
        #[cfg(any(feature = "aes-gcm", feature = "chacha20-poly1305"))]
        let config = config.set_encryption(self.encryption.ok_or(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "encryption is required",
        ))?);

        let (sender, receiver) = flume::unbounded();
        let mut pipe = Pipe::new(config).await?;
        let writer = pipe.writer();
        let handle = rust_p2p_core::async_compat::spawn(async move {
            while let Ok(line) = pipe.dispatch().await {
                rust_p2p_core::async_compat::spawn(handle(line, sender.clone()));
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

async fn handle(mut line: PipeLine, sender: Sender<RecvUserData>) {
    let mut list = Vec::with_capacity(16);
    loop {
        let rs = match line.recv_multi(&mut list).await {
            Ok(rs) => rs,
            Err(e) => {
                log::debug!("recv_from {e:?},{:?}", line.protocol());
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
