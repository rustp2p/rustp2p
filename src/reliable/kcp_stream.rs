use crate::protocol::node_id::NodeID;
use crate::tunnel::TunnelTransmitHub;
use bytes::{Buf, BytesMut};
use kcp::Kcp;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::io;
use std::io::{Error, Write};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Interval;
use tokio_util::sync::PollSender;

pub struct KcpStream {
    map: Map,
    node_id: NodeID,
    conv: u32,
    last_buf: Option<BytesMut>,
    receiver: Receiver<BytesMut>,
    sender: PollSender<BytesMut>,
}
type Map = Arc<RwLock<HashMap<(NodeID, u32), Sender<BytesMut>>>>;
pub struct KcpStreamManager {
    counter: Arc<AtomicUsize>,
    input_receiver: flume::Receiver<(NodeID, BytesMut)>,
    map: Map,
    output: Arc<TunnelTransmitHub>,
}
impl Clone for KcpStreamManager {
    fn clone(&self) -> Self {
        self.counter.fetch_add(1, Ordering::Release);
        self.clone0()
    }
}
impl Drop for KcpStreamManager {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Release);
    }
}
impl KcpStreamManager {
    pub(crate) fn clone0(&self) -> Self {
        Self {
            counter: self.counter.clone(),
            input_receiver: self.input_receiver.clone(),
            map: self.map.clone(),
            output: self.output.clone(),
        }
    }
}
#[derive(Clone)]
pub(crate) struct KcpDataInput {
    counter: Arc<AtomicUsize>,
    map: Map,
    sender: flume::Sender<(NodeID, BytesMut)>,
}
impl KcpDataInput {
    pub(crate) async fn input(&self, buf: &[u8], node_id: NodeID) {
        if self.counter.load(Ordering::Acquire) <= 1 {
            return;
        }
        if buf.is_empty() {
            return;
        }
        let conv = kcp::get_conv(buf);
        if let Some(v) = self.get_stream_sender(node_id, conv) {
            if v.send(buf.into()).await.is_ok() {
                return;
            }
        }
        _ = self.sender.send_async((node_id, buf.into())).await;
    }
    fn get_stream_sender(&self, node_id: NodeID, conv: u32) -> Option<Sender<BytesMut>> {
        self.map.read().get(&(node_id, conv)).cloned()
    }
}
pub(crate) async fn create_kcp_stream_manager(
    sender: Arc<TunnelTransmitHub>,
) -> (KcpStreamManager, KcpDataInput) {
    let (input_sender, input_receiver) = flume::bounded(128);
    let map = Map::default();
    let counter = Arc::new(AtomicUsize::new(1));
    let manager = KcpStreamManager {
        counter: counter.clone(),
        input_receiver,
        map: map.clone(),
        output: sender,
    };
    let input = KcpDataInput {
        counter,
        map,
        sender: input_sender,
    };
    (manager, input)
}
impl KcpStreamManager {
    pub async fn accept(&self) -> io::Result<(KcpStream, NodeID)> {
        loop {
            let (node_id, bytes) = self
                .input_receiver
                .recv_async()
                .await
                .map_err(|_| io::Error::from(io::ErrorKind::Other))?;
            if bytes.is_empty() {
                continue;
            }
            let conv = kcp::get_conv(&bytes);
            match self.new_stream(node_id, conv) {
                Ok(stream) => {
                    self.send_data_to_kcp(node_id, conv, bytes).await?;
                    return Ok((stream, node_id));
                }
                Err(e) => {
                    if e.kind() == io::ErrorKind::AlreadyExists {
                        self.send_data_to_kcp(node_id, conv, bytes).await?;
                    }
                }
            }
        }
    }
    async fn send_data_to_kcp(
        &self,
        node_id: NodeID,
        conv: u32,
        bytes_mut: BytesMut,
    ) -> io::Result<()> {
        let sender = self.get_stream_sender(node_id, conv)?;
        sender
            .send(bytes_mut)
            .await
            .map_err(|_| Error::new(io::ErrorKind::NotFound, "not found stream"))
    }
    fn get_stream_sender(&self, node_id: NodeID, conv: u32) -> io::Result<Sender<BytesMut>> {
        if let Some(v) = self.map.read().get(&(node_id, conv)) {
            Ok(v.clone())
        } else {
            Err(Error::new(io::ErrorKind::NotFound, "not found stream"))
        }
    }
    pub fn new_stream(&self, node_id: NodeID, conv: u32) -> io::Result<KcpStream> {
        KcpStream::new(node_id, conv, self.map.clone(), self.output.clone())
    }
}
impl AsyncWrite for KcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        match self.sender.poll_reserve(cx) {
            Poll::Ready(Ok(_)) => {
                match self.sender.send_item(buf.into()) {
                    Ok(_) => {}
                    Err(_) => return Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero))),
                };
                Poll::Ready(Ok(buf.len()))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.sender.close();
        Poll::Ready(Ok(()))
    }
}
impl AsyncRead for KcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Some(p) = self.last_buf.as_mut() {
            let len = buf.remaining().min(p.len());
            buf.put_slice(&p[..len]);
            p.advance(len);
            if p.is_empty() {
                self.last_buf.take();
            }
            return Poll::Ready(Ok(()));
        }
        let poll = self.receiver.poll_recv(cx);
        match poll {
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Ready(Some(mut p)) => {
                if p.is_empty() {
                    self.receiver.close();
                    return Poll::Ready(Ok(()));
                }
                let len = buf.remaining().min(p.len());
                buf.put_slice(&p[..len]);
                p.advance(len);
                if !p.is_empty() {
                    self.last_buf.replace(p);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
impl KcpStream {
    pub(crate) fn new(
        node_id: NodeID,
        conv: u32,
        map: Map,
        output_sender: Arc<TunnelTransmitHub>,
    ) -> io::Result<Self> {
        let mut guard = map.write();
        if guard.contains_key(&(node_id, conv)) {
            return Err(Error::new(
                io::ErrorKind::AlreadyExists,
                "stream already exists",
            ));
        }
        let (input_sender, input_receiver) = tokio::sync::mpsc::channel(128);
        guard.insert((node_id, conv), input_sender);
        drop(guard);
        let stream = KcpStream::new_stream(node_id, conv, map, input_receiver, output_sender);
        Ok(stream)
    }
    fn new_stream(
        node_id: NodeID,
        conv: u32,
        map: Map,
        input: Receiver<BytesMut>,
        output_sender: Arc<TunnelTransmitHub>,
    ) -> Self {
        let mut kcp = Kcp::new_stream(
            conv,
            KcpOutput {
                node_id,
                sender: output_sender,
            },
        );
        kcp.set_wndsize(128, 128);
        kcp.set_wndsize(128, 128);
        kcp.set_nodelay(true, 10, 2, true);
        let (data_in_sender, data_in_receiver) = tokio::sync::mpsc::channel(128);
        let (data_out_sender, data_out_receiver) = tokio::sync::mpsc::channel(128);
        tokio::spawn(async move {
            let _ = kcp_run(input, kcp, data_out_receiver, data_in_sender).await;
        });
        KcpStream {
            map,
            node_id,
            conv,
            last_buf: None,
            receiver: data_in_receiver,
            sender: PollSender::new(data_out_sender),
        }
    }
}
impl Drop for KcpStream {
    fn drop(&mut self) {
        let mut guard = self.map.write();
        let _ = guard.remove(&(self.node_id, self.conv));
    }
}
async fn kcp_run(
    mut input: Receiver<BytesMut>,
    mut kcp: Kcp<KcpOutput>,
    mut data_out_receiver: Receiver<BytesMut>,
    data_in_sender: Sender<BytesMut>,
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
        match event {
            Event::Input(mut buf) => {
                let len = kcp
                    .input(&buf)
                    .map_err(|e| Error::new(io::ErrorKind::Other, e))?;
                if len < buf.len() {
                    buf.advance(len);
                    input_data.replace(buf);
                }
            }
            Event::Output(buf) => {
                kcp.send(&buf)
                    .map_err(|e| Error::new(io::ErrorKind::Other, e))?;
            }
            Event::Timeout => {
                kcp.update(tokio::time::Instant::now().elapsed().as_millis() as u32)
                    .map_err(|e| Error::new(io::ErrorKind::Other, e))?;
            }
        }

        if let Ok(len) = kcp.recv(&mut buf) {
            if data_in_sender.send(buf[..len].into()).await.is_err() {
                break;
            }
        }
    }
    Ok(())
}
async fn all_event(
    input: &mut Receiver<BytesMut>,
    data_out_receiver: &mut Receiver<BytesMut>,
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
    data_out_receiver: &mut Receiver<BytesMut>,
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
enum Event {
    Output(BytesMut),
    Input(BytesMut),
    Timeout,
}

struct KcpOutput {
    node_id: NodeID,
    sender: Arc<TunnelTransmitHub>,
}
impl Write for KcpOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        println!("---------------------- write");
        self.sender
            .try_kcp_send_to(buf, self.node_id)
            .map_err(|_| Error::from(io::ErrorKind::WriteZero))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
