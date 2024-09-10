use crate::pipe::config::TcpPipeConfig;
use crate::route::{Index, RouteKey};
use crate::socket::{connect_tcp, create_tcp_listener, LocalInterface};
use anyhow::{anyhow, Context};
use async_lock::Mutex;
use async_trait::async_trait;
use dashmap::DashMap;
use std::io;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use rand::Rng;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{Receiver, Sender};

pub struct TcpPipe {
    route_idle_time: Duration,
    tcp_listener: TcpListener,
    connect_receiver: Receiver<(RouteKey, ReadHalfBox, WriteHalfBox)>,
    tcp_pipe_writer: TcpPipeWriter,
    write_half_collect: Arc<WriteHalfCollect>,
    init_codec: Arc<Box<dyn InitCodec>>,
}

impl TcpPipe {
    /// Construct a `TCP` pipe with the specified configuration
    pub fn new(config: TcpPipeConfig) -> anyhow::Result<TcpPipe> {
        config.check()?;
        let address: SocketAddr = if config.use_v6 {
            format!("[::]:{}", config.tcp_port).parse().unwrap()
        } else {
            format!("0.0.0.0:{}", config.tcp_port).parse().unwrap()
        };

        let tcp_listener = create_tcp_listener(address)?;
        let local_addr = tcp_listener.local_addr()?;
        let tcp_listener = TcpListener::from_std(tcp_listener)?;
        let (connect_sender, connect_receiver) = tokio::sync::mpsc::channel(64);
        let write_half_collect = Arc::new(WriteHalfCollect::new(config.tcp_multiplexing_limit));
        let init_codec = Arc::new(config.init_codec);
        let tcp_pipe_writer = TcpPipeWriter {
            socket_layer: Arc::new(SocketLayer::new(
                local_addr,
                config.tcp_multiplexing_limit,
                write_half_collect.clone(),
                connect_sender,
                config.default_interface,
                init_codec.clone(),
            )),
        };
        Ok(TcpPipe {
            route_idle_time: config.route_idle_time,
            tcp_listener,
            connect_receiver,
            tcp_pipe_writer,
            write_half_collect,
            init_codec,
        })
    }
    #[inline]
    pub fn writer_ref(&self) -> TcpPipeWriterRef<'_> {
        TcpPipeWriterRef {
            shadow: &self.tcp_pipe_writer,
        }
    }
}

impl TcpPipe {
    /// Accept `TCP` pipelines from this kind pipe
    pub async fn accept(&mut self) -> anyhow::Result<TcpPipeLine> {
        tokio::select! {
            rs=self.connect_receiver.recv()=>{
                let (route_key,read_half,write_half) = rs.context("connect_receiver done")?;
                Ok(TcpPipeLine::new(self.route_idle_time,route_key,read_half,write_half,self.write_half_collect.clone()))
            }
            rs=self.tcp_listener.accept()=>{
                let (tcp_stream,addr) = rs?;
                tcp_stream.set_nodelay(true)?;
                let route_key = tcp_stream.route_key()?;
                let (read_half,write_half) = tcp_stream.into_split();
                let (decoder,encoder) = self.init_codec.codec(addr)?;
                let write_half = WriteHalfBox::new(write_half,encoder);
                let read_half = ReadHalfBox::new(read_half,decoder);
                self.write_half_collect.add_write_half(route_key,0, write_half.clone());
                Ok(TcpPipeLine::new(self.route_idle_time,route_key,read_half,write_half,self.write_half_collect.clone()))
            }
        }
    }
}

pub struct TcpPipeLine {
    route_idle_time: Duration,
    tcp_read: OwnedReadHalf,
    tcp_write: WriteHalfBox,
    stream_owned: StreamOwned,
    decoder: Box<dyn Decoder>,
}

impl TcpPipeLine {
    pub(crate) fn new(
        route_idle_time: Duration,
        route_key: RouteKey,
        read: ReadHalfBox,
        tcp_write: WriteHalfBox,
        write_half_collect: Arc<WriteHalfCollect>,
    ) -> Self {
        let stream_owned = StreamOwned {
            route_key,
            write_half_collect,
        };
        let decoder = read.decoder;
        let tcp_read = read.read_half;
        Self {
            route_idle_time,
            tcp_read,
            tcp_write,
            stream_owned,
            decoder,
        }
    }
    #[inline]
    pub fn route_key(&self) -> RouteKey {
        self.stream_owned.route_key
    }
    /// Close this pipeline
    pub async fn shutdown(self) -> anyhow::Result<()> {
        let mut guard = self.tcp_write.lock().await;
        if let Some((write, _)) = guard.as_mut() {
            write.shutdown().await?;
        }
        Ok(())
    }
}

impl TcpPipeLine {
    pub async fn into_raw(self) -> anyhow::Result<(OwnedWriteHalf, OwnedReadHalf)> {
        let option = self.tcp_write.lock().await.take();
        if let Some((write_half, _)) = option {
            Ok((write_half, self.tcp_read))
        } else {
            Err(anyhow!("miss"))
        }
    }
}

impl TcpPipeLine {
    /// Writing `buf` to the target denoted by `route_key` via this pipeline
    pub async fn send_to(&self, buf: &[u8], route_key: &RouteKey) -> crate::error::Result<usize> {
        if &self.stream_owned.route_key != route_key {
            Err(crate::error::Error::RouteNotFound("mismatch".into()))?
        }
        let mut guard = self.tcp_write.lock().await;
        if let Some((write, encoder)) = guard.as_mut() {
            let len = encoder.encode(write, buf).await?;
            Ok(len)
        } else {
            Err(crate::error::Error::RouteNotFound("miss".into()))
        }
    }

    pub(crate) async fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match tokio::time::timeout(
            self.route_idle_time,
            self.decoder.decode(&mut self.tcp_read, buf),
        )
        .await
        {
            Ok(rs) => rs,
            Err(_) => Err(io::Error::from(io::ErrorKind::TimedOut)),
        }
    }
    /// Receive bytes from this pipeline, which the configured Decoder pre-processes
    /// `usize` in the `Ok` branch indicates how many bytes are received
    /// `RouteKey` in the `Ok` branch denotes the source where these bytes are received from
    pub async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, RouteKey)> {
        match self.recv(buf).await {
            Ok(len) => Ok((len, self.route_key())),
            Err(e) => Err(e),
        }
    }
}

struct StreamOwned {
    route_key: RouteKey,
    write_half_collect: Arc<WriteHalfCollect>,
}

impl Drop for StreamOwned {
    fn drop(&mut self) {
        self.write_half_collect.remove(&self.route_key);
    }
}

pub struct WriteHalfCollect {
    tcp_multiplexing_limit: usize,
    addr_mapping: DashMap<SocketAddr, Vec<usize>>,
    write_half_map: DashMap<usize, WriteHalfBox>,
}

impl WriteHalfCollect {
    fn new(tcp_multiplexing_limit: usize) -> Self {
        Self {
            tcp_multiplexing_limit,
            addr_mapping: Default::default(),
            write_half_map: Default::default(),
        }
    }
}

pub(crate) struct ReadHalfBox {
    read_half: OwnedReadHalf,
    decoder: Box<dyn Decoder>,
}

impl ReadHalfBox {
    pub(crate) fn new(read_half: OwnedReadHalf, decoder: Box<dyn Decoder>) -> Self {
        Self { read_half, decoder }
    }
}

type W = Mutex<Option<(OwnedWriteHalf, Box<dyn Encoder>)>>;

#[derive(Clone)]
pub(crate) struct WriteHalfBox {
    write_half: Arc<W>,
}

impl Deref for WriteHalfBox {
    type Target = W;

    fn deref(&self) -> &Self::Target {
        &self.write_half
    }
}

impl WriteHalfBox {
    pub(crate) fn new(write_half: OwnedWriteHalf, encoder: Box<dyn Encoder>) -> WriteHalfBox {
        Self {
            write_half: Arc::new(Mutex::new(Some((write_half, encoder)))),
        }
    }
}

impl WriteHalfCollect {
    pub(crate) fn add_write_half(
        &self,
        route_key: RouteKey,
        index_offset: usize,
        write_half: WriteHalfBox,
    ) {
        assert!(index_offset < self.tcp_multiplexing_limit);
        let index = route_key.index_usize();
        let _ref = self
            .addr_mapping
            .entry(route_key.addr())
            .and_modify(|v| {
                v[index_offset] = index;
            })
            .or_insert_with(|| {
                let mut v = vec![0; self.tcp_multiplexing_limit];
                v[index_offset] = index;
                v
            });
        self.write_half_map.insert(index, write_half);
    }
    pub(crate) fn remove(&self, route_key: &RouteKey) {
        let index_usize = route_key.index_usize();
        self.addr_mapping
            .remove_if_mut(&route_key.addr(), |_k, index_vec| {
                let mut remove = true;
                for v in index_vec {
                    if *v == index_usize {
                        *v = 0;
                    }
                    if *v != 0 {
                        remove = false;
                    }
                }
                remove
            });

        self.write_half_map.remove(&index_usize);
    }
    pub(crate) fn get(&self, index: &usize) -> Option<WriteHalfBox> {
        self.write_half_map.get(index).map(|v| v.value().clone())
    }

    pub(crate) fn get_one_route_key(&self, addr: &SocketAddr) -> Option<RouteKey> {
        if let Some(v) = self.addr_mapping.get(addr) {
            for index_usize in v.value() {
                if *index_usize != 0 {
                    return Some(RouteKey::new(Index::Tcp(*index_usize), *addr));
                }
            }
        }
        None
    }
    pub(crate) fn get_limit_route_key(&self, index: usize, addr: &SocketAddr) -> Option<RouteKey> {
        if let Some(v) = self.addr_mapping.get(addr) {
            assert_eq!(v.len(), self.tcp_multiplexing_limit);
            let index_usize = v[index];
            if index_usize == 0 {
                return None;
            }
            return Some(RouteKey::new(Index::Tcp(index_usize), *addr));
        }
        None
    }
}

pub struct SocketLayer {
    lock: Mutex<()>,
    local_addr: SocketAddr,
    tcp_multiplexing_limit: usize,
    write_half_collect: Arc<WriteHalfCollect>,
    connect_sender: Sender<(RouteKey, ReadHalfBox, WriteHalfBox)>,
    default_interface: Option<LocalInterface>,
    init_codec: Arc<Box<dyn InitCodec>>,
}

impl SocketLayer {
    pub(crate) fn new(
        local_addr: SocketAddr,
        tcp_multiplexing_limit: usize,
        write_half_collect: Arc<WriteHalfCollect>,
        connect_sender: Sender<(RouteKey, ReadHalfBox, WriteHalfBox)>,
        default_interface: Option<LocalInterface>,
        init_codec: Arc<Box<dyn InitCodec>>,
    ) -> Self {
        Self {
            local_addr,
            lock: Default::default(),
            tcp_multiplexing_limit,
            write_half_collect,
            connect_sender,
            default_interface,
            init_codec,
        }
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl SocketLayer {
    /// Multiple connections can be initiated to the target address.
    pub async fn multi_connect(
        &self,
        addr: SocketAddr,
        index_offset: usize,
    ) -> crate::error::Result<RouteKey> {
        self.multi_connect0(addr, index_offset, None).await
    }
    pub(crate) async fn multi_connect0(
        &self,
        addr: SocketAddr,
        index_offset: usize,
        ttl: Option<u32>,
    ) -> crate::error::Result<RouteKey> {
        let len = self.tcp_multiplexing_limit;
        if index_offset >= len {
            return Err(crate::error::Error::IndexOutOfBounds {
                len,
                index: index_offset,
            });
        }
        let _guard = self.lock.lock().await;
        if let Some(route_key) = self
            .write_half_collect
            .get_limit_route_key(index_offset, &addr)
        {
            return Ok(route_key);
        }
        self.connect0(0, addr, index_offset, ttl).await
    }
    /// Initiate a connection.
    pub async fn connect(&self, addr: SocketAddr) -> crate::error::Result<RouteKey> {
        let _guard = self.lock.lock().await;
        if let Some(route_key) = self.write_half_collect.get_one_route_key(&addr) {
            return Ok(route_key);
        }
        self.connect0(0, addr, 0, None).await
    }
    /// Reuse the bound port to initiate a connection, which can be used to penetrate NAT1 network type.
    pub async fn connect_reuse_port(&self, addr: SocketAddr) -> crate::error::Result<RouteKey> {
        let _guard = self.lock.lock().await;
        if let Some(route_key) = self.write_half_collect.get_one_route_key(&addr) {
            return Ok(route_key);
        }
        self.connect0(self.local_addr.port(), addr, 0, None).await
    }
    pub async fn connect_reuse_port_raw(
        &self,
        addr: SocketAddr,
    ) -> crate::error::Result<TcpStream> {
        let stream = connect_tcp(
            addr,
            self.local_addr.port(),
            self.default_interface.as_ref(),
            None,
        )
        .await?;
        Ok(stream)
    }
    async fn connect0(
        &self,
        bind_port: u16,
        addr: SocketAddr,
        index_offset: usize,
        ttl: Option<u32>,
    ) -> crate::error::Result<RouteKey> {
        let stream = connect_tcp(addr, bind_port, self.default_interface.as_ref(), ttl).await?;
        let route_key = stream.route_key()?;
        let (read_half, write_half) = stream.into_split();
        let (decoder, encoder) = self.init_codec.codec(addr)?;
        let write_half = WriteHalfBox::new(write_half, encoder);
        let read_half = ReadHalfBox::new(read_half, decoder);

        if let Err(_e) = self
            .connect_sender
            .send((route_key, read_half, write_half.clone()))
            .await
        {
            Err(crate::error::Error::Eof)?
        }
        self.write_half_collect
            .add_write_half(route_key, index_offset, write_half);
        Ok(route_key)
    }
}

impl TcpPipeWriter {
    pub async fn send_to_addr_multi<A: Into<SocketAddr>>(
        &self,
        buf: &[u8],
        addr: A,
    ) -> crate::error::Result<usize> {
        self.send_to_addr_multi0(buf, addr, None).await
    }
    pub(crate) async fn send_to_addr_multi0<A: Into<SocketAddr>>(
        &self,
        buf: &[u8],
        addr: A,
        ttl: Option<u32>,
    ) -> crate::error::Result<usize> {
        let index_offset = rand::thread_rng().gen_range(0..self.tcp_multiplexing_limit);
        let route_key = self.multi_connect0(addr.into(), index_offset, ttl).await?;
        self.send_to(buf, &route_key).await
    }
    /// Reuse the bound port to initiate a connection, which can be used to penetrate NAT1 network type.
    pub async fn send_to_addr_reuse_port<A: Into<SocketAddr>>(
        &self,
        buf: &[u8],
        addr: A,
    ) -> crate::error::Result<usize> {
        let route_key = self.connect_reuse_port(addr.into()).await?;
        self.send_to(buf, &route_key).await
    }
    pub async fn send_to_addr<A: Into<SocketAddr>>(
        &self,
        buf: &[u8],
        addr: A,
    ) -> crate::error::Result<usize> {
        let route_key = self.connect(addr.into()).await?;
        self.send_to(buf, &route_key).await
    }
    /// Writing `buf` to the target denoted by `route_key`
    pub async fn send_to(&self, buf: &[u8], route_key: &RouteKey) -> crate::error::Result<usize> {
        match route_key.index() {
            Index::Tcp(index) => {
                let write_half = self.write_half_collect.get(&index).ok_or_else(|| {
                    crate::error::Error::RouteNotFound(format!("not found {route_key:?}"))
                })?;
                let mut guard = write_half.lock().await;
                if let Some((write_half, encoder)) = guard.as_mut() {
                    let len = encoder.encode(write_half, buf).await?;
                    Ok(len)
                } else {
                    Err(crate::error::Error::RouteNotFound("miss".to_string()))
                }
            }
            _ => Err(crate::error::Error::InvalidProtocol),
        }
    }
    pub async fn get(
        &self,
        addr: SocketAddr,
        index: usize,
    ) -> anyhow::Result<TcpPipeWriterIndex<'_>> {
        let route_key = self.multi_connect(addr, index).await?;
        let write_half = self
            .write_half_collect
            .get(&route_key.index_usize())
            .with_context(|| format!("not found with index={index}"))?;

        Ok(TcpPipeWriterIndex {
            shadow: write_half,
            marker: Default::default(),
        })
    }
}

#[derive(Clone)]
pub struct TcpPipeWriter {
    socket_layer: Arc<SocketLayer>,
}

impl Deref for TcpPipeWriter {
    type Target = Arc<SocketLayer>;

    fn deref(&self) -> &Self::Target {
        &self.socket_layer
    }
}

pub struct TcpPipeWriterRef<'a> {
    shadow: &'a Arc<SocketLayer>,
}

impl<'a> Clone for TcpPipeWriterRef<'a> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a> Copy for TcpPipeWriterRef<'a> {}

impl<'a> TcpPipeWriterRef<'a> {
    pub fn to_owned(&self) -> TcpPipeWriter {
        TcpPipeWriter {
            socket_layer: self.shadow.clone(),
        }
    }
}

impl<'a> Deref for TcpPipeWriterRef<'a> {
    type Target = Arc<SocketLayer>;

    fn deref(&self) -> &Self::Target {
        self.shadow
    }
}

pub struct TcpPipeWriterIndex<'a> {
    shadow: WriteHalfBox,
    marker: PhantomData<&'a ()>,
}

impl<'a> TcpPipeWriterIndex<'a> {
    pub async fn send(&self, buf: &[u8]) -> anyhow::Result<usize> {
        let mut guard = self.shadow.lock().await;
        if let Some((write_half, encoder)) = guard.as_mut() {
            let len = encoder.encode(write_half, buf).await?;
            Ok(len)
        } else {
            Err(anyhow!("miss"))?
        }
    }
}

pub trait TcpStreamIndex {
    fn route_key(&self) -> crate::error::Result<RouteKey>;
    fn index(&self) -> Index;
}

impl TcpStreamIndex for TcpStream {
    fn route_key(&self) -> crate::error::Result<RouteKey> {
        let addr = self.peer_addr()?;

        Ok(RouteKey::new(self.index(), addr))
    }

    fn index(&self) -> Index {
        #[cfg(windows)]
        use std::os::windows::io::AsRawSocket;
        #[cfg(windows)]
        let index = self.as_raw_socket() as usize;
        #[cfg(unix)]
        use std::os::fd::{AsFd, AsRawFd};
        #[cfg(unix)]
        let index = self.as_fd().as_raw_fd() as usize;
        Index::Tcp(index)
    }
}

/// The default byte encoder/decoder; using this is no different from directly using a TCP stream.
pub struct BytesCodec;

/// Fixed-length prefix encoder/decoder.
pub struct LengthPrefixedCodec;

#[async_trait]
impl Decoder for BytesCodec {
    async fn decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize> {
        let len = read.read(src).await?;
        Ok(len)
    }
}

#[async_trait]
impl Encoder for BytesCodec {
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<usize> {
        write.write_all(data).await?;
        Ok(data.len())
    }
}

#[async_trait]
impl Decoder for LengthPrefixedCodec {
    async fn decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize> {
        let mut head = [0; 4];
        read.read_exact(&mut head).await?;
        let len = u32::from_be_bytes(head) as usize;
        read.read_exact(&mut src[..len]).await?;
        Ok(len)
    }
}

#[async_trait]
impl Encoder for LengthPrefixedCodec {
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<usize> {
        let head: [u8; 4] = (data.len() as u32).to_be_bytes();
        write.write_all(&head).await?;
        write.write_all(data).await?;
        Ok(data.len())
    }
}

pub struct BytesInitCodec;

impl InitCodec for BytesInitCodec {
    fn codec(&self, _addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
        Ok((Box::new(BytesCodec), Box::new(BytesCodec)))
    }
}

pub struct LengthPrefixedInitCodec;

impl InitCodec for LengthPrefixedInitCodec {
    fn codec(&self, _addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
        Ok((Box::new(LengthPrefixedCodec), Box::new(LengthPrefixedCodec)))
    }
}

pub trait InitCodec: Send + Sync {
    fn codec(&self, addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)>;
}

#[async_trait]
pub trait Decoder: Send + Sync {
    async fn decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize>;
}

#[async_trait]
pub trait Encoder: Send + Sync {
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<usize>;
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use std::io;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

    use crate::pipe::config::TcpPipeConfig;
    use crate::pipe::tcp_pipe::{Decoder, Encoder, InitCodec, TcpPipe};

    #[tokio::test]
    pub async fn create_tcp_pipe() {
        let config: TcpPipeConfig = TcpPipeConfig::default();
        let tcp_pipe = TcpPipe::new(config).unwrap();
        drop(tcp_pipe)
    }

    #[tokio::test]
    pub async fn create_codec_tcp_pipe() {
        let config = TcpPipeConfig::new(Box::new(MyInitCodeC));
        let tcp_pipe = TcpPipe::new(config).unwrap();
        drop(tcp_pipe)
    }

    struct MyInitCodeC;

    impl InitCodec for MyInitCodeC {
        fn codec(&self, _addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
            Ok((Box::new(MyCodeC), Box::new(MyCodeC)))
        }
    }

    struct MyCodeC;

    #[async_trait]
    impl Decoder for MyCodeC {
        async fn decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize> {
            let mut head = [0; 2];
            read.read_exact(&mut head).await?;
            let len = u16::from_be_bytes(head) as usize;
            read.read_exact(&mut src[..len]).await?;
            Ok(len)
        }
    }

    #[async_trait]
    impl Encoder for MyCodeC {
        async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<usize> {
            let head: [u8; 2] = (data.len() as u16).to_be_bytes();
            write.write_all(&head).await?;
            write.write_all(data).await?;
            Ok(data.len())
        }
    }
}
