use std::io;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use crossbeam_utils::atomic::AtomicCell;
use dashmap::DashMap;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Mutex;

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

#[derive(Clone)]
pub struct ExtensibleWriter {
    write: Arc<Mutex<Box<dyn ExtendWrite>>>,
}

impl ExtensibleReader {
    pub async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read.read(buf).await
    }
}
impl ExtensibleWriter {
    pub async fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut guard = self.write.lock().await;
        guard.write_all(buf).await
    }
    pub async fn write_all_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<()> {
        let mut guard = self.write.lock().await;
        guard.write_all_vectored(bufs).await
    }
}

pub struct ExtensiblePipe {
    connect_receiver: Receiver<(RouteKey, ExtensibleReader, ExtensibleWriter)>,
    write_half_collect: Arc<WriteHalfCollect>,
    extensible_pipe_writer: ExtensiblePipeWriter,
}

impl ExtensiblePipe {
    pub fn new() -> ExtensiblePipe {
        let (connect_sender, connect_receiver) = tokio::sync::mpsc::channel(64);
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
    pub async fn accept(&mut self) -> anyhow::Result<ExtensiblePipeLine> {
        let (route_key, read_half, write_half) = self
            .connect_receiver
            .recv()
            .await
            .context("connect_receiver done")?;
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
    w: ExtensibleWriter,
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
        w: ExtensibleWriter,
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
    pub async fn send_to(&self, buf: &[u8], route_key: &RouteKey) -> crate::error::Result<()> {
        if &self.line_owned.route_key != route_key {
            Err(crate::error::Error::RouteNotFound("mismatch".into()))?
        }
        self.w.write_all(buf).await?;
        Ok(())
    }
}

struct LineOwned {
    route_key: RouteKey,
    write_half_collect: Arc<WriteHalfCollect>,
}

pub struct WriteHalfCollect {
    write_half_map: DashMap<RouteKey, ExtensibleWriter>,
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
}

impl WriteHalfCollect {
    pub fn get(&self, route_key: &RouteKey) -> Option<ExtensibleWriter> {
        self.write_half_map
            .get(route_key)
            .map(|v| v.value().clone())
    }
}

pub struct ExtensiblePipeWriter {
    id: Arc<AtomicCell<usize>>,
    connect_sender: Sender<(RouteKey, ExtensibleReader, ExtensibleWriter)>,
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
        connect_sender: Sender<(RouteKey, ExtensibleReader, ExtensibleWriter)>,
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
        r: ExtensibleReader,
        w: ExtensibleWriter,
    ) -> anyhow::Result<()> {
        let id = self.id.load();
        if id == 0 {
            Err(anyhow!("overflow"))?;
        }
        let index = self.id.fetch_add(1);
        let route_key = RouteKey::new(Index::Extend(index), addr);
        if let Err(e) = self.connect_sender.send((route_key, r, w.clone())).await {
            Err(anyhow!("{e}"))?
        }
        self.write_half_collect.write_half_map.insert(route_key, w);
        Ok(())
    }
}

impl ExtensiblePipeWriter {
    pub async fn send_to(&self, buf: &[u8], route_key: &RouteKey) -> crate::error::Result<()> {
        let w = self
            .write_half_collect
            .get(route_key)
            .ok_or(crate::error::Error::RouteNotFound("".into()))?;
        w.write_all(buf).await?;
        Ok(())
    }
    pub async fn send_vectored_to(
        &self,
        bufs: &[IoSlice<'_>],
        route_key: &RouteKey,
    ) -> crate::error::Result<()> {
        let w = self
            .write_half_collect
            .get(route_key)
            .ok_or(crate::error::Error::RouteNotFound("".into()))?;
        w.write_all_vectored(bufs).await?;
        Ok(())
    }
}

#[derive(Copy, Clone)]
pub struct ExtensiblePipeWriterRef<'a> {
    shadow: &'a ExtensiblePipeWriter,
}

impl<'a> ExtensiblePipeWriterRef<'a> {
    pub fn to_owned(&self) -> ExtensiblePipeWriter {
        self.shadow.clone()
    }
}
