pub mod protocol;

pub mod cipher;
pub mod config;
pub mod extend;
pub mod pipe;

use std::net::Ipv4Addr;

use cipher::Algorithm;
use config::{PipeConfig, TcpPipeConfig, UdpPipeConfig};
use pipe::{Pipe, PipeLine, PipeWriter, RecvUserData};
use protocol::node_id::{GroupCode, NodeID};
use rust_p2p_core::async_compat::mpsc::{self, Receiver, Sender};
use rust_p2p_core::async_compat::JoinHandle;
pub struct EndPoint {
    rx: EndPointReceiver,
    tx: EndPointSender,
}

impl EndPoint {
    pub async fn close(&mut self) {
        self.rx.handler.abort();
    }
    pub async fn recv(&mut self) -> Option<RecvUserData> {
        self.rx.receiver.recv().await
    }
    pub async fn send(&mut self, data: &[u8], node_id: NodeID) {
        let mut send_packet = self.tx.sender.allocate_send_packet();
        send_packet.set_payload(data);
        if let Err(e) = self.tx.sender.send_packet_to(send_packet, &node_id).await {
            log::warn!("{e:?},{node_id:?}");
        }
    }
    pub fn split(self) -> (EndPointReceiver, EndPointSender) {
        (self.rx, self.tx)
    }
}

pub struct EndPointReceiver {
    receiver: Receiver<RecvUserData>,
    handler: JoinHandle<()>,
}

impl EndPointReceiver {
    pub async fn recv(&mut self) -> Option<RecvUserData> {
        self.receiver.recv().await
    }
}

impl Drop for EndPointReceiver {
    fn drop(&mut self) {
        self.handler.abort();
    }
}

#[repr(transparent)]
#[derive(Clone)]
pub struct EndPointSender {
    sender: PipeWriter,
}

impl EndPointSender {
    pub async fn send(&self, data: &[u8], node_id: NodeID) {
        let mut send_packet = self.sender.allocate_send_packet();
        send_packet.set_payload(data);
        if let Err(e) = self.sender.send_packet_to(send_packet, &node_id).await {
            log::warn!("{e:?},{node_id:?}");
        }
    }
}

pub struct Builder {
    udp_ports: Option<Vec<u16>>,
    tcp_port: Option<u16>,
    local_ip: Option<Ipv4Addr>,
    group_code: Option<GroupCode>,
    encryption: Option<Algorithm>,
    node_id: Option<NodeID>,
}
impl Builder {
    pub fn new() -> Self {
        Self {
            udp_ports: None,
            tcp_port: None,
            local_ip: None,
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
    pub fn local_ip(mut self, ip: Ipv4Addr) -> Self {
        self.local_ip = Some(ip);
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
        let config = PipeConfig::empty()
            .set_udp_pipe_config(
                UdpPipeConfig::default().set_udp_ports(self.udp_ports.unwrap_or(vec![])),
            )
            .set_tcp_pipe_config(TcpPipeConfig::default().set_tcp_port(self.tcp_port.unwrap_or(0)))
            .set_group_code(self.group_code.ok_or(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "group_code is required",
            ))?)
            .set_encryption(self.encryption.ok_or(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "encryption is required",
            ))?)
            .set_node_id(self.node_id.ok_or(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "node_id is required",
            ))?);
        let (sender, receiver) = mpsc::channel(1024);
        let mut pipe = Pipe::new(config).await?;
        let writer = pipe.writer();
        let handler = rust_p2p_core::async_compat::spawn(async move {
            while let Ok(line) = pipe.accept().await {
                rust_p2p_core::async_compat::spawn(handle(line, sender.clone()));
            }
        });
        Ok(EndPoint {
            tx: EndPointSender { sender: writer },
            rx: EndPointReceiver { receiver, handler },
        })
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
            if sender.send(x).await.is_err() {
                break;
            }
        }
    }
}
