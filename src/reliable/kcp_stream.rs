use crate::protocol::node_id::NodeID;
use bytes::{Buf, BufMut, BytesMut};
use kcp::Kcp;
use std::io;
use std::io::{Error, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Interval;
use tokio_util::sync::PollSender;

pub struct KcpStream {
    last_buf: Option<BytesMut>,
    receiver: Receiver<BytesMut>,
    sender: PollSender<BytesMut>,
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
        conv: u32,
        node_id: NodeID,
        input: Receiver<BytesMut>,
        sender: Sender<(NodeID, BytesMut)>,
    ) -> Self {
        let kcp = Kcp::new_stream(conv, KcpOutput { node_id, sender });
        let (data_in_sender, data_in_receiver) = tokio::sync::mpsc::channel(128);
        let (data_out_sender, data_out_receiver) = tokio::sync::mpsc::channel(128);
        tokio::spawn(async move {
            let _ = kcp_run(input, kcp, data_out_receiver, data_in_sender).await;
        });
        KcpStream {
            last_buf: None,
            receiver: data_in_receiver,
            sender: PollSender::new(data_out_sender),
        }
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
        let event = if kcp.wait_snd() > kcp.snd_wnd() as usize {
            input_event(&mut input, &mut interval).await?
        } else if input_data.is_some() {
            output_event(&mut data_out_receiver, &mut interval).await?
        } else {
            all_event(&mut input, &mut data_out_receiver, &mut interval).await?
        };
        match event {
            Event::Output(mut buf) => {
                let len = kcp
                    .input(&buf)
                    .map_err(|e| Error::new(io::ErrorKind::Other, e))?;
                if len < buf.len() {
                    buf.advance(len);
                    input_data.replace(buf);
                }
            }
            Event::Input(buf) => {
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
async fn input_tick(
    input: &mut Receiver<BytesMut>,
    input_data: &Option<BytesMut>,
) -> io::Result<BytesMut> {
    if input_data.is_some() {
        return futures::future::pending().await;
    }
    let buf = input
        .recv()
        .await
        .ok_or(Error::from(io::ErrorKind::Other))?;
    Ok(buf)
}

struct KcpOutput {
    node_id: NodeID,
    sender: Sender<(NodeID, BytesMut)>,
}
impl Write for KcpOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sender
            .try_send((self.node_id, buf.into()))
            .map_err(|_| Error::from(io::ErrorKind::WriteZero))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
