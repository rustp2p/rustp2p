use crate::KcpMessageHub;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use kcp::Kcp;
use rust_p2p_core::route::RouteKey;
use rust_p2p_core::tunnel::udp::WeakUdpTunnelSender;
use std::collections::HashMap;
use std::io;
use std::io::{Error, Write};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Interval;

pub(crate) struct KcpHandle {
    local_addr: SocketAddr,
    tunnel_sender: WeakUdpTunnelSender,
    kcp_hub_sender: Sender<KcpMessageHub>,
    map: HashMap<SocketAddr, (Sender<BytesMut>, flume::Sender<BytesMut>)>,
}
impl KcpHandle {
    pub fn new(
        local_addr: SocketAddr,
        tunnel_sender: WeakUdpTunnelSender,
        kcp_hub_sender: Sender<KcpMessageHub>,
    ) -> Self {
        Self {
            local_addr,
            tunnel_sender,
            kcp_hub_sender,
            map: Default::default(),
        }
    }
    pub async fn handle(&mut self, buf: &[u8], route_key: RouteKey) {
        let remote_addr = route_key.addr();
        let map = &mut self.map;
        if let Some((kcp_data_sender, data_in_sender)) = map.get(&remote_addr) {
            if buf[0] == 0x03 {
                _ = data_in_sender.send_async(buf[1..].into()).await;
            } else if buf.len() >= 24 && kcp_data_sender.send(buf.into()).await.is_err() {
                map.remove(&remote_addr);
            }
        } else if buf.len() >= 8 && buf[0] == 0x02 {
            let conv = u32::from_le_bytes(buf[0..4].try_into().unwrap());
            // create kcp
            let (input_sender, input_receiver) = tokio::sync::mpsc::channel(128);
            let (data_in_sender, data_in_receiver) = flume::bounded(128);
            if buf.len() >= 24 {
                //kcp client connect
                _ = input_sender.send(buf.into()).await;
            }
            map.insert(remote_addr, (input_sender, data_in_sender.clone()));
            let output_sender = self.tunnel_sender.clone();
            let tunnel_sender = output_sender.clone();
            let (data_out_sender, data_out_receiver) = tokio::sync::mpsc::channel(128);
            let mut kcp = Kcp::new(
                conv,
                KcpOutput {
                    addr: remote_addr,
                    sender: output_sender,
                },
            );
            kcp.set_wndsize(128, 128);
            kcp.set_wndsize(128, 128);
            kcp.set_nodelay(true, 10, 2, true);

            tokio::spawn(async move {
                if let Err(e) = kcp_run(
                    tunnel_sender,
                    remote_addr,
                    input_receiver,
                    kcp,
                    data_out_receiver,
                    data_in_sender,
                )
                .await
                {
                    log::warn!("kcp run: {e:?}");
                }
            });
            _ = self
                .kcp_hub_sender
                .send(KcpMessageHub::new(
                    self.local_addr,
                    remote_addr,
                    data_out_sender,
                    data_in_receiver,
                ))
                .await;
        }
    }
}
#[derive(Debug)]
pub(crate) enum DataType {
    Raw(BytesMut),
    Kcp(BytesMut),
}
async fn kcp_run(
    tunnel_sender: WeakUdpTunnelSender,
    remote_addr: SocketAddr,
    mut input: Receiver<BytesMut>,
    mut kcp: Kcp<KcpOutput>,
    mut data_out_receiver: Receiver<DataType>,
    data_in_sender: flume::Sender<BytesMut>,
) -> io::Result<()> {
    let mut interval = tokio::time::interval(Duration::from_millis(10));
    let mut buf = [0; 65536];
    let mut input_data = Option::<BytesMut>::None;

    loop {
        let event = if kcp.wait_snd() >= kcp.snd_wnd() as usize {
            input_event(&mut input, &mut interval).await?
        } else if input_data.is_some() {
            output_event(&mut data_out_receiver, &mut interval).await?
        } else {
            all_event(&mut input, &mut data_out_receiver, &mut interval).await?
        };
        if let Some(mut buf) = input_data.take() {
            let len = kcp.input(&buf).map_err(|e| Error::other(e))?;
            if len < buf.len() {
                buf.advance(len);
                input_data.replace(buf);
            }
        }
        match event {
            Event::Input(mut buf) => {
                let len = kcp.input(&buf).map_err(|e| Error::other(e))?;
                if len < buf.len() {
                    buf.advance(len);
                    input_data.replace(buf);
                }
            }
            Event::Output(buf) => match buf {
                DataType::Raw(buf) => {
                    let mut new_buf = BytesMut::with_capacity(buf.len() + 1);
                    new_buf.put_u8(0x03);
                    new_buf.extend_from_slice(&buf[..]);
                    tunnel_sender.send_to(new_buf.into(), remote_addr).await?;
                }
                DataType::Kcp(buf) => {
                    kcp.send(&buf).map_err(|e| Error::other(e))?;
                }
            },
            Event::Timeout => {
                let now = std::time::SystemTime::now();
                let millis = now
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                kcp.update(millis.as_millis() as _)
                    .map_err(|e| Error::other(e))?;
            }
        }

        if let Ok(len) = kcp.recv(&mut buf) {
            if data_in_sender.send_async(buf[..len].into()).await.is_err() {
                break;
            }
        }
    }
    Ok(())
}
async fn all_event(
    input: &mut Receiver<BytesMut>,
    data_out_receiver: &mut Receiver<DataType>,
    interval: &mut Interval,
) -> io::Result<Event> {
    tokio::select! {
        rs=input.recv()=>{
            let buf = rs.ok_or(Error::from(io::ErrorKind::Other))?;
            Ok(Event::Input(buf))
        }
        rs=data_out_receiver.recv()=>{
            let buf = rs.ok_or(Error::from(io::ErrorKind::Other))?;
            Ok(Event::Output(buf))
        }
        _=interval.tick()=>{
            Ok(Event::Timeout)
        }
    }
}
async fn input_event(input: &mut Receiver<BytesMut>, interval: &mut Interval) -> io::Result<Event> {
    tokio::select! {
        rs=input.recv()=>{
            let buf = rs.ok_or(Error::from(io::ErrorKind::Other))?;
            Ok(Event::Input(buf))
        }
        _=interval.tick()=>{
            Ok(Event::Timeout)
        }
    }
}
async fn output_event(
    data_out_receiver: &mut Receiver<DataType>,
    interval: &mut Interval,
) -> io::Result<Event> {
    tokio::select! {
        rs=data_out_receiver.recv()=>{
            let buf = rs.ok_or(Error::from(io::ErrorKind::Other))?;
            Ok(Event::Output(buf))
        }
        _=interval.tick()=>{
            Ok(Event::Timeout)
        }
    }
}
#[derive(Debug)]
enum Event {
    Output(DataType),
    Input(BytesMut),
    Timeout,
}

struct KcpOutput {
    addr: SocketAddr,
    sender: WeakUdpTunnelSender,
}
impl Write for KcpOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.sender.try_send_to(Bytes::copy_from_slice(buf), self.addr) {
            Ok(_) => {}
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => Err(e)?,
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
