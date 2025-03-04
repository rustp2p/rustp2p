use std::io;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use crossbeam_utils::atomic::AtomicCell;
use dashmap::DashMap;
use tachyonix::{Receiver, Sender};

use crate::route::{Index, RouteKey};

#[async_trait]
pub trait ExtendRead: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

#[async_trait]
pub trait ExtendWrite: Send + Sync {
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    async fn write_all_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<()> {
        for buf in bufs {
            self.write_all(buf.as_ref()).await?;
        }
        Ok(())
    }
}

pub struct ExtensibleReader {
    read: Box<dyn ExtendRead>,
}

impl ExtensibleReader {
    pub async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read.read(buf).await
    }
}

pub struct ExtensiblePipe {
    connect_receiver: Receiver<(RouteKey, ExtensibleReader, Sender<BytesMut>)>,
    write_half_collect: Arc<WriteHalfCollect>,
    extensible_pipe_writer: ExtensiblePipeWriter,
}

impl ExtensiblePipe {
    pub fn new() -> ExtensiblePipe {
        let (connect_sender, connect_receiver) = tachyonix::channel(64);
        let write_half_collect = Arc::new(WriteHalfCollect::default());
        Self {
            connect_receiver,
            write_half_collect: write_half_collect.clone(),
            extensible_pipe_writer: ExtensiblePipeWriter::new(connect_sender, write_half_collect),
        }
    }
}
impl Default for ExtensiblePipe {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensiblePipe {
    pub async fn accept(&mut self) -> io::Result<ExtensiblePipeLine> {
        let (route_key, read_half, write_half) = self
            .connect_receiver
            .recv()
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "connect_receiver done"))?;
        Ok(ExtensiblePipeLine::new(
            route_key,
            read_half,
            write_half,
            self.write_half_collect.clone(),
        ))
    }
    pub fn writer_ref(&self) -> ExtensiblePipeWriterRef {
        ExtensiblePipeWriterRef {
            shadow: &self.extensible_pipe_writer,
        }
    }
}

pub struct ExtensiblePipeLine {
    r: ExtensibleReader,
    w: Sender<BytesMut>,
    line_owned: LineOwned,
}

impl Drop for LineOwned {
    fn drop(&mut self) {
        self.write_half_collect.remove(&self.route_key)
    }
}

impl ExtensiblePipeLine {
    pub(crate) fn new(
        route_key: RouteKey,
        r: ExtensibleReader,
        w: Sender<BytesMut>,
        write_half_collect: Arc<WriteHalfCollect>,
    ) -> ExtensiblePipeLine {
        let line_owned = LineOwned {
            route_key,
            write_half_collect,
        };
        Self { r, w, line_owned }
    }
    #[inline]
    pub fn route_key(&self) -> RouteKey {
        self.line_owned.route_key
    }
    pub async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, RouteKey)> {
        match self.r.read(buf).await {
            Ok(len) => Ok((len, self.route_key())),
            Err(e) => Err(e),
        }
    }
    pub async fn send_to(&self, buf: BytesMut, route_key: &RouteKey) -> io::Result<()> {
        if &self.line_owned.route_key != route_key {
            Err(io::Error::new(io::ErrorKind::Other, "mismatch"))?
        }
        if let Err(_e) = self.w.send(buf).await {
            Err(io::Error::from(io::ErrorKind::WriteZero))?
        }
        Ok(())
    }
    pub fn done(&mut self) {}
}

struct LineOwned {
    route_key: RouteKey,
    write_half_collect: Arc<WriteHalfCollect>,
}

pub struct WriteHalfCollect {
    write_half_map: DashMap<RouteKey, Sender<BytesMut>>,
}

impl Default for WriteHalfCollect {
    fn default() -> Self {
        Self {
            write_half_map: DashMap::new(),
        }
    }
}

impl WriteHalfCollect {
    pub fn remove(&self, route_key: &RouteKey) {
        self.write_half_map.remove(route_key);
    }
    pub fn insert(&self, route_key: RouteKey, sender: Sender<BytesMut>) {
        self.write_half_map.insert(route_key, sender);
    }
}

impl WriteHalfCollect {
    pub fn get(&self, route_key: &RouteKey) -> Option<Sender<BytesMut>> {
        self.write_half_map
            .get(route_key)
            .map(|v| v.value().clone())
    }
}

pub struct ExtensiblePipeWriter {
    id: Arc<AtomicCell<usize>>,
    connect_sender: Sender<(RouteKey, ExtensibleReader, Sender<BytesMut>)>,
    write_half_collect: Arc<WriteHalfCollect>,
}

impl Clone for ExtensiblePipeWriter {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            connect_sender: self.connect_sender.clone(),
            write_half_collect: self.write_half_collect.clone(),
        }
    }
}

impl ExtensiblePipeWriter {
    pub(crate) fn new(
        connect_sender: Sender<(RouteKey, ExtensibleReader, Sender<BytesMut>)>,
        write_half_collect: Arc<WriteHalfCollect>,
    ) -> Self {
        Self {
            id: Arc::new(AtomicCell::new(1)),
            connect_sender,
            write_half_collect,
        }
    }
    pub async fn add_pipe(
        &self,
        addr: SocketAddr,
        r: Box<dyn ExtendRead>,
        mut w: Box<dyn ExtendWrite>,
    ) -> io::Result<()> {
        let id = self.id.load();
        if id == 0 {
            Err(io::Error::new(io::ErrorKind::Other, "overflow"))?;
        }
        let index = self.id.fetch_add(1);
        let route_key = RouteKey::new(Index::Extend(index), addr);
        let reader = ExtensibleReader { read: r };
        let (sender, mut receiver) = tachyonix::channel::<BytesMut>(32);
        let collect = self.write_half_collect.clone();
        collect.insert(route_key, sender.clone());
        crate::async_compat::spawn(async move {
            while let Ok(data) = receiver.recv().await {
                if let Err(e) = w.write_all(&data).await {
                    log::debug!("ExtendWrite {route_key:?},{e:?}");
                    break;
                }
            }
            collect.remove(&route_key);
        });
        if let Err(_e) = self.connect_sender.send((route_key, reader, sender)).await {
            Err(io::Error::new(io::ErrorKind::Other, "connect close"))?
        }
        Ok(())
    }
}

impl ExtensiblePipeWriter {
    pub async fn send_to(&self, buf: BytesMut, route_key: &RouteKey) -> io::Result<()> {
        let w = self
            .write_half_collect
            .get(route_key)
            .ok_or(io::Error::new(io::ErrorKind::NotFound, "route not found"))?;
        if let Err(_e) = w.send(buf).await {
            Err(io::Error::from(io::ErrorKind::WriteZero))?
        }
        Ok(())
    }
}

#[derive(Copy, Clone)]
pub struct ExtensiblePipeWriterRef<'a> {
    shadow: &'a ExtensiblePipeWriter,
}

impl ExtensiblePipeWriterRef<'_> {
    pub fn to_owned(&self) -> ExtensiblePipeWriter {
        self.shadow.clone()
    }
}
