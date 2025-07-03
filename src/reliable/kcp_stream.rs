use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::tunnel::TunnelRouter;
use bytes::{Buf, BytesMut};
use kcp::Kcp;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::io::Error;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Interval;
use tokio_util::sync::PollSender;

pub struct KcpStream {
    read: KcpStreamRead,
    write: KcpStreamWrite,
}
pub struct KcpListener {
    inner: Arc<KcpListenerInner>,
}
impl KcpListener {
    pub async fn accept(&self) -> io::Result<(KcpStream, NodeID)> {
        self.inner.accept().await
    }
    fn downgrade(&self) -> Weak<KcpListenerInner> {
        Arc::downgrade(&self.inner)
    }
}
struct OwnedKcp {
    map: Map,
    node_id: NodeID,
    conv: u32,
}

type Map = Arc<RwLock<HashMap<(NodeID, u32), Sender<BytesMut>>>>;
struct KcpListenerInner {
    input_receiver: flume::Receiver<(NodeID, BytesMut)>,
    map: Map,
    output: TunnelRouter,
}

#[derive(Clone)]
pub(crate) struct KcpContext {
    conv: Counter,
    map: Map,
    #[allow(clippy::type_complexity)]
    channel: Arc<Mutex<Option<(flume::Sender<(NodeID, BytesMut)>, Weak<KcpListenerInner>)>>>,
}
impl Default for KcpContext {
    fn default() -> Self {
        Self {
            conv: Counter::new(rand::random()),
            map: Arc::new(Default::default()),
            channel: Arc::new(Default::default()),
        }
    }
}
#[derive(Clone)]
struct Counter {
    counter: Arc<AtomicU32>,
}
impl Counter {
    fn new(v: u32) -> Self {
        Self {
            counter: Arc::new(AtomicU32::new(v)),
        }
    }
    fn add(&self) -> u32 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }
}
impl KcpContext {
    pub(crate) async fn input(&self, buf: &[u8], node_id: NodeID) {
        if buf.is_empty() {
            return;
        }
        let conv = kcp::get_conv(buf);
        if let Some(v) = self.get_stream_sender(node_id, conv) {
            if v.send(buf.into()).await.is_ok() {
                return;
            }
        }
        let sender = {
            let mut guard = self.channel.lock();
            if let Some((sender, hub_weak)) = guard.as_ref() {
                if hub_weak.strong_count() == 0 {
                    guard.take();
                    return;
                }
                sender.clone()
            } else {
                return;
            }
        };
        if sender.send_async((node_id, buf.into())).await.is_err() {
            log::warn!("input error");
        }
    }
    fn get_stream_sender(&self, node_id: NodeID, conv: u32) -> Option<Sender<BytesMut>> {
        self.map.read().get(&(node_id, conv)).cloned()
    }
    pub(crate) fn create_kcp_hub(&self, sender: TunnelRouter) -> KcpListener {
        let mut guard = self.channel.lock();
        if let Some((_, hub)) = guard.as_ref() {
            if let Some(inner) = hub.upgrade() {
                return KcpListener { inner };
            }
        }
        let (input_sender, input_receiver) = flume::bounded(128);
        let inner = KcpListenerInner {
            input_receiver,
            map: self.map.clone(),
            output: sender,
        };
        let hub = KcpListener {
            inner: Arc::new(inner),
        };
        guard.replace((input_sender, hub.downgrade()));
        hub
    }
    pub(crate) fn new_stream(
        &self,
        output: TunnelRouter,
        node_id: NodeID,
    ) -> io::Result<KcpStream> {
        KcpStream::new(node_id, self.conv.add(), self.map.clone(), output)
    }
}

impl KcpListenerInner {
    async fn accept(&self) -> io::Result<(KcpStream, NodeID)> {
        loop {
            let (node_id, mut bytes) = self
                .input_receiver
                .recv_async()
                .await
                .map_err(|_| io::Error::from(io::ErrorKind::Other))?;
            if bytes.len() < 24 {
                continue;
            }
            let conv = kcp::get_conv(&bytes);
            let sn = kcp::get_sn(&bytes);
            if sn != 0 {
                //reset
                bytes.truncate(24);
                bytes[5] = 1;
                _ = self.output.try_kcp_send_to(&bytes, node_id);
                continue;
            }
            match self.new_stream_impl(node_id, conv) {
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

    fn new_stream_impl(&self, node_id: NodeID, conv: u32) -> io::Result<KcpStream> {
        KcpStream::new(node_id, conv, self.map.clone(), self.output.clone())
    }
}
impl AsyncWrite for KcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        Pin::new(&mut self.write).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.write).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.write).poll_shutdown(cx)
    }
}
impl AsyncRead for KcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.read).poll_read(cx, buf)
    }
}
pub struct KcpStreamRead {
    #[allow(dead_code)]
    owned_kcp: Arc<OwnedKcp>,
    last_buf: Option<BytesMut>,
    receiver: Receiver<BytesMut>,
}
pub struct KcpStreamWrite {
    #[allow(dead_code)]
    owned_kcp: Arc<OwnedKcp>,
    sender: PollSender<BytesMut>,
}
impl AsyncRead for KcpStreamRead {
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
impl AsyncWrite for KcpStreamWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        match self.sender.poll_reserve(cx) {
            Poll::Ready(Ok(_)) => {
                let len = buf.len().min(10240);
                match self.sender.send_item(buf[..len].into()) {
                    Ok(_) => Poll::Ready(Ok(len)),
                    Err(_) => Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero))),
                }
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
impl KcpStream {
    pub fn split(self) -> (KcpStreamWrite, KcpStreamRead) {
        (self.write, self.read)
    }
    pub fn conv(&self) -> u32 {
        self.read.owned_kcp.conv
    }
}
impl KcpStream {
    pub(crate) fn new(
        node_id: NodeID,
        conv: u32,
        map: Map,
        output_sender: TunnelRouter,
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
        output_sender: TunnelRouter,
    ) -> Self {
        let mut kcp = Kcp::new_stream(
            conv,
            KcpOutput {
                node_id,
                sender: output_sender,
                send_fut: None,
            },
        );
        kcp.set_wndsize(1024, 1024);
        kcp.set_nodelay(true, 10, 2, true);
        let (data_in_sender, data_in_receiver) = tokio::sync::mpsc::channel(128);
        let (data_out_sender, data_out_receiver) = tokio::sync::mpsc::channel(128);
        tokio::spawn(async move {
            if let Err(e) = kcp_run(input, kcp, data_out_receiver, data_in_sender).await {
                log::warn!("kcp run: {e:?}");
            }
        });
        let owned_kcp = OwnedKcp { map, node_id, conv };
        let owned_kcp = Arc::new(owned_kcp);
        let read = KcpStreamRead {
            owned_kcp: owned_kcp.clone(),
            last_buf: None,
            receiver: data_in_receiver,
        };
        let write = KcpStreamWrite {
            owned_kcp,
            sender: PollSender::new(data_out_sender),
        };
        KcpStream { read, write }
    }
}
impl Drop for OwnedKcp {
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
    let mut buf = vec![0; 65536];
    let mut input_data = Option::<BytesMut>::None;
    let mut output_data = Option::<BytesMut>::None;
    'out: loop {
        if kcp.is_dead_link() {
            break;
        }
        let event = if output_data.is_some() {
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
        if let Some(mut buf) = output_data.take() {
            let len = kcp.send(&buf).map_err(|e| Error::other(e))?;
            if len < buf.len() {
                buf.advance(len);
                output_data.replace(buf);
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
            Event::Output(mut buf) => {
                let len = kcp.send(&buf).map_err(|e| Error::other(e))?;
                if len < buf.len() {
                    buf.advance(len);
                    output_data.replace(buf);
                }
                _ = kcp.async_flush().await;
            }
            Event::Timeout => {
                let now = std::time::SystemTime::now();
                let millis = now
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                kcp.async_update(millis.as_millis() as _)
                    .await
                    .map_err(|e| Error::other(e))?;
            }
        }

        while let Ok(len) = kcp.recv(&mut buf) {
            if data_in_sender.send(buf[..len].into()).await.is_err() {
                break 'out;
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
            let buf = rs.ok_or(Error::other( "input close"))?;
            Ok(Event::Input(buf))
        }
        rs=data_out_receiver.recv()=>{
            let buf = rs.ok_or(Error::other("output close"))?;
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
            let buf = rs.ok_or(Error::other("input close"))?;
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
            let buf = rs.ok_or(Error::other( "output close"))?;
            Ok(Event::Output(buf))
        }
        _=interval.tick()=>{
            Ok(Event::Timeout)
        }
    }
}
#[derive(Debug)]
enum Event {
    Output(BytesMut),
    Input(BytesMut),
    Timeout,
}

struct KcpOutput {
    node_id: NodeID,
    sender: TunnelRouter,
    send_fut: Option<futures::future::BoxFuture<'static, io::Result<usize>>>,
}

impl AsyncWrite for KcpOutput {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        if self.send_fut.is_none() {
            match self.sender.try_kcp_send_to(buf, self.node_id) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                _ => {
                    return Poll::Ready(Ok(buf.len()));
                }
            }
            let sender = self.sender.clone();
            let node_id = self.node_id;
            let mut send_packet = sender.allocate_send_packet();
            send_packet.set_payload(buf);
            send_packet.set_protocol(ProtocolType::KcpData);
            let len = buf.len();
            self.send_fut = Some(Box::pin(async move {
                _ = sender.send_packet_to(send_packet, &node_id).await;
                Ok(len)
            }));
        }

        let fut = self.send_fut.as_mut().unwrap();
        match Pin::new(fut).poll(cx) {
            Poll::Ready(res) => {
                self.send_fut = None;
                match res {
                    Ok(n) => Poll::Ready(Ok(n)),
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }
}
