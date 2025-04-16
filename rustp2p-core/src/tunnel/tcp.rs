use crate::route::{Index, RouteKey};
use crate::socket::{connect_tcp, create_tcp_listener, LocalInterface};
use crate::tunnel::config::TcpTunnelConfig;
use crate::tunnel::recycle::RecycleBuf;
use async_lock::Mutex;
use async_trait::async_trait;
use bytes::BytesMut;
use dashmap::DashMap;
use dyn_clone::DynClone;
use rand::Rng;
use std::io;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tachyonix::{Receiver, Sender, TrySendError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

pub struct TcpTunnelFactory {
    route_idle_time: Duration,
    tcp_listener: TcpListener,
    connect_receiver: Receiver<(RouteKey, ReadHalfBox, Sender<BytesMut>)>,
    #[allow(dead_code)]
    pub(crate) socket_manager: Arc<SocketManager>,
    write_half_collect: WriteHalfCollect,
    init_codec: Arc<Box<dyn InitCodec>>,
}

impl TcpTunnelFactory {
    /// Construct a `TCP` tunnel with the specified configuration
    pub fn new(config: TcpTunnelConfig) -> io::Result<TcpTunnelFactory> {
        config.check()?;
        let address: SocketAddr = if config.use_v6 {
            format!("[::]:{}", config.tcp_port).parse().unwrap()
        } else {
            format!("0.0.0.0:{}", config.tcp_port).parse().unwrap()
        };

        let tcp_listener = create_tcp_listener(address)?;
        let local_addr = tcp_listener.local_addr()?;
        let tcp_listener = TcpListener::from_std(tcp_listener)?;
        let (connect_sender, connect_receiver) = tachyonix::channel(128);
        let write_half_collect =
            WriteHalfCollect::new(config.tcp_multiplexing_limit, config.recycle_buf);
        let init_codec = Arc::new(config.init_codec);
        let socket_manager = Arc::new(SocketManager::new(
            local_addr,
            config.tcp_multiplexing_limit,
            write_half_collect.clone(),
            connect_sender,
            config.default_interface,
            init_codec.clone(),
        ));
        Ok(TcpTunnelFactory {
            route_idle_time: config.route_idle_time,
            tcp_listener,
            connect_receiver,
            socket_manager,
            write_half_collect,
            init_codec,
        })
    }
}

impl TcpTunnelFactory {
    /// Accept `TCP` tunnel from this kind factory
    pub async fn accept(&mut self) -> io::Result<TcpTunnel> {
        tokio::select! {
            rs=self.connect_receiver.recv()=>{
                let (route_key,read_half,sender) = rs.
                    map_err(|_| io::Error::new(io::ErrorKind::Other,"connect_receiver done"))?;
                let local_addr = read_half.read_half.local_addr()?;
                Ok(TcpTunnel::new(local_addr,self.route_idle_time,route_key,read_half,sender))
            },
            rs=self.tcp_listener.accept()=>{
                let (tcp_stream,addr) = rs?;
                tcp_stream.set_nodelay(true)?;
                let route_key = tcp_stream.route_key()?;
                let (read_half,write_half) = tcp_stream.into_split();
                let (decoder,encoder) = self.init_codec.codec(addr)?;
                let read_half = ReadHalfBox::new(read_half,decoder);
                let sender = self.write_half_collect.add_write_half(route_key,0, write_half,encoder);
                let local_addr = read_half.read_half.local_addr()?;
                Ok(TcpTunnel::new(local_addr,self.route_idle_time,route_key,read_half,sender))
            }
        }
    }
    pub fn manager(&self) -> &Arc<SocketManager> {
        &self.socket_manager
    }
}

pub struct TcpTunnel {
    local_addr: SocketAddr,
    route_key: RouteKey,
    route_idle_time: Duration,
    tcp_read: OwnedReadHalf,
    decoder: Box<dyn Decoder>,
    sender: Sender<BytesMut>,
}

impl TcpTunnel {
    pub(crate) fn new(
        local_addr: SocketAddr,
        route_idle_time: Duration,
        route_key: RouteKey,
        read: ReadHalfBox,
        sender: Sender<BytesMut>,
    ) -> Self {
        let decoder = read.decoder;
        let tcp_read = read.read_half;
        Self {
            local_addr,
            route_key,
            route_idle_time,
            tcp_read,
            decoder,
            sender,
        }
    }
    #[inline]
    pub fn route_key(&self) -> RouteKey {
        self.route_key
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
    pub fn done(&mut self) {
        self.sender.close();
    }
    pub fn sender(&self) -> io::Result<WeakTcpTunnelSender> {
        Ok(WeakTcpTunnelSender::new(self.sender.clone()))
    }
}
#[derive(Clone)]
pub struct WeakTcpTunnelSender {
    sender: Sender<BytesMut>,
}
impl WeakTcpTunnelSender {
    fn new(sender: Sender<BytesMut>) -> Self {
        Self { sender }
    }
    pub async fn send(&self, buf: BytesMut) -> io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        self.sender
            .send(buf)
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::WriteZero))
    }
}

impl Drop for TcpTunnel {
    fn drop(&mut self) {
        self.done();
    }
}

impl TcpTunnel {
    /// Writing `buf` to the target denoted by `route_key` via this tunnel
    pub async fn send(&self, buf: BytesMut) -> io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        self.sender
            .send(buf)
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::WriteZero))
    }

    pub async fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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
    pub async fn batch_recv<B: AsMut<[u8]>>(
        &mut self,
        bufs: &mut [B],
        sizes: &mut [usize],
    ) -> io::Result<usize> {
        if bufs.is_empty() || bufs.len() != sizes.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bufs error"));
        }
        match tokio::time::timeout(
            self.route_idle_time,
            self.decoder.decode(&mut self.tcp_read, bufs[0].as_mut()),
        )
        .await
        {
            Ok(rs) => {
                let len = rs?;
                sizes[0] = len;
                let mut num = 1;
                while num < bufs.len() {
                    let rs = self
                        .decoder
                        .try_decode(&mut self.tcp_read, bufs[num].as_mut());
                    match rs {
                        Ok(len) => {
                            sizes[num] = len;
                            num += 1;
                        }
                        Err(_e) => break,
                    }
                }

                Ok(num)
            }
            Err(_) => Err(io::Error::from(io::ErrorKind::TimedOut)),
        }
    }
    /// Receive bytes from this tunnel, which the configured Decoder pre-processes
    /// `usize` in the `Ok` branch indicates how many bytes are received
    /// `RouteKey` in the `Ok` branch denotes the source where these bytes are received from
    pub async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, RouteKey)> {
        match self.recv(buf).await {
            Ok(len) => Ok((len, self.route_key())),
            Err(e) => Err(e),
        }
    }
    pub async fn batch_recv_from<B: AsMut<[u8]>>(
        &mut self,
        bufs: &mut [B],
        sizes: &mut [usize],
    ) -> io::Result<(usize, RouteKey)> {
        match self.batch_recv(bufs, sizes).await {
            Ok(len) => Ok((len, self.route_key())),
            Err(e) => Err(e),
        }
    }
}

#[derive(Clone)]
pub struct WriteHalfCollect {
    tcp_multiplexing_limit: usize,
    addr_mapping: Arc<DashMap<SocketAddr, Vec<usize>>>,
    write_half_map: Arc<DashMap<usize, Sender<BytesMut>>>,
    recycle_buf: Option<RecycleBuf>,
}

impl WriteHalfCollect {
    fn new(tcp_multiplexing_limit: usize, recycle_buf: Option<RecycleBuf>) -> Self {
        Self {
            tcp_multiplexing_limit,
            addr_mapping: Default::default(),
            write_half_map: Default::default(),
            recycle_buf,
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

impl WriteHalfCollect {
    pub(crate) fn add_write_half(
        &self,
        route_key: RouteKey,
        index_offset: usize,
        mut writer: OwnedWriteHalf,
        mut decoder: Box<dyn Encoder>,
    ) -> Sender<BytesMut> {
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
        let (s, mut r) = tachyonix::channel(128);
        let sender = s.clone();
        self.write_half_map.insert(index, s);
        let collect = self.clone();
        let recycle_buf = self.recycle_buf.clone();
        tokio::spawn(async move {
            let mut vec_buf = Vec::with_capacity(16);
            const IO_SLICE_CAPACITY: usize = 16;
            let mut io_buffer: Vec<IoSlice> = Vec::with_capacity(IO_SLICE_CAPACITY);
            let io_slice_storage = io_buffer.as_mut_slice();
            while let Ok(v) = r.recv().await {
                if let Ok(buf) = r.try_recv() {
                    vec_buf.push(v);
                    vec_buf.push(buf);
                    while let Ok(buf) = r.try_recv() {
                        vec_buf.push(buf);
                        if vec_buf.len() == 16 {
                            break;
                        }
                    }

                    // Safety
                    // reuse the storage of `io_buffer` via `vec` that only lives in this block and manually clear the content
                    // within the storage when exiting the block
                    // leak the memory storage after using `vec` since the storage is managed by `io_buffer`
                    let rs = {
                        let mut vec = unsafe {
                            Vec::from_raw_parts(io_slice_storage.as_mut_ptr(), 0, IO_SLICE_CAPACITY)
                        };
                        for x in &vec_buf {
                            vec.push(IoSlice::new(x));
                        }
                        let rs = decoder.encode_multiple(&mut writer, &vec).await;
                        vec.clear();
                        std::mem::forget(vec);
                        rs
                    };

                    if let Some(recycle_buf) = recycle_buf.as_ref() {
                        while let Some(buf) = vec_buf.pop() {
                            recycle_buf.push(buf);
                        }
                    } else {
                        vec_buf.clear()
                    }
                    if let Err(e) = rs {
                        log::debug!("{route_key:?},{e:?}");
                        break;
                    }
                } else {
                    let rs = decoder.encode(&mut writer, &v).await;
                    if let Some(recycle_buf) = recycle_buf.as_ref() {
                        recycle_buf.push(v);
                    }
                    if let Err(e) = rs {
                        log::debug!("{route_key:?},{e:?}");
                        break;
                    }
                }
            }
            collect.remove(&route_key);
        });
        sender
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
    pub(crate) fn get_write_half(&self, index: &usize) -> Option<Sender<BytesMut>> {
        self.write_half_map.get(index).map(|v| v.value().clone())
    }
    pub(crate) fn get_write_half_by_key(
        &self,
        route_key: &RouteKey,
    ) -> io::Result<Sender<BytesMut>> {
        match route_key.index() {
            Index::Tcp(index) => {
                let sender = self.get_write_half(&index).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Other, format!("not found {route_key:?}"))
                })?;
                Ok(sender)
            }
            _ => Err(io::Error::from(io::ErrorKind::InvalidInput)),
        }
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
    pub async fn send_to(&self, buf: BytesMut, route_key: &RouteKey) -> io::Result<()> {
        let write_half = self.get_write_half_by_key(route_key)?;
        if buf.is_empty() {
            return Ok(());
        }
        if let Err(_e) = write_half.send(buf).await {
            Err(io::Error::from(io::ErrorKind::WriteZero))
        } else {
            Ok(())
        }
    }
    pub fn try_send_to(&self, buf: BytesMut, route_key: &RouteKey) -> io::Result<()> {
        let write_half = self.get_write_half_by_key(route_key)?;
        if buf.is_empty() {
            return Ok(());
        }
        if let Err(e) = write_half.try_send(buf) {
            match e {
                TrySendError::Full(_) => Err(io::Error::from(io::ErrorKind::WouldBlock)),
                TrySendError::Closed(_) => Err(io::Error::from(io::ErrorKind::WriteZero)),
            }
        } else {
            Ok(())
        }
    }
}

pub struct SocketManager {
    lock: Mutex<()>,
    local_addr: SocketAddr,
    tcp_multiplexing_limit: usize,
    write_half_collect: WriteHalfCollect,
    connect_sender: Sender<(RouteKey, ReadHalfBox, Sender<BytesMut>)>,
    default_interface: Option<LocalInterface>,
    init_codec: Arc<Box<dyn InitCodec>>,
}

impl SocketManager {
    pub(crate) fn new(
        local_addr: SocketAddr,
        tcp_multiplexing_limit: usize,
        write_half_collect: WriteHalfCollect,
        connect_sender: Sender<(RouteKey, ReadHalfBox, Sender<BytesMut>)>,
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

impl SocketManager {
    /// Multiple connections can be initiated to the target address.
    pub async fn multi_connect(
        &self,
        addr: SocketAddr,
        index_offset: usize,
    ) -> io::Result<RouteKey> {
        self.multi_connect_impl(addr, index_offset, None).await
    }
    pub(crate) async fn multi_connect_impl(
        &self,
        addr: SocketAddr,
        index_offset: usize,
        ttl: Option<u8>,
    ) -> io::Result<RouteKey> {
        let len = self.tcp_multiplexing_limit;
        if index_offset >= len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "index out of bounds",
            ));
        }
        let _guard = self.lock.lock().await;
        if let Some(route_key) = self
            .write_half_collect
            .get_limit_route_key(index_offset, &addr)
        {
            return Ok(route_key);
        }
        self.connect_impl(0, addr, index_offset, ttl).await
    }
    /// Initiate a connection.
    pub async fn connect(&self, addr: SocketAddr) -> io::Result<RouteKey> {
        let _guard = self.lock.lock().await;
        if let Some(route_key) = self.write_half_collect.get_one_route_key(&addr) {
            return Ok(route_key);
        }
        self.connect_impl(0, addr, 0, None).await
    }
    pub async fn connect_ttl(&self, addr: SocketAddr, ttl: Option<u8>) -> io::Result<RouteKey> {
        let _guard = self.lock.lock().await;
        if let Some(route_key) = self.write_half_collect.get_one_route_key(&addr) {
            return Ok(route_key);
        }
        self.connect_impl(0, addr, 0, ttl).await
    }
    /// Reuse the bound port to initiate a connection, which can be used to penetrate NAT1 network type.
    pub async fn connect_reuse_port(&self, addr: SocketAddr) -> io::Result<RouteKey> {
        let _guard = self.lock.lock().await;
        if let Some(route_key) = self.write_half_collect.get_one_route_key(&addr) {
            return Ok(route_key);
        }
        self.connect_impl(self.local_addr.port(), addr, 0, None)
            .await
    }
    pub async fn connect_reuse_port_raw(&self, addr: SocketAddr) -> io::Result<TcpStream> {
        let stream = connect_tcp(
            addr,
            self.local_addr.port(),
            self.default_interface.as_ref(),
            None,
        )
        .await?;
        Ok(stream)
    }
    async fn connect_impl(
        &self,
        bind_port: u16,
        addr: SocketAddr,
        index_offset: usize,
        ttl: Option<u8>,
    ) -> io::Result<RouteKey> {
        let stream = connect_tcp(addr, bind_port, self.default_interface.as_ref(), ttl).await?;
        let route_key = stream.route_key()?;
        let (read_half, write_half) = stream.into_split();
        let (decoder, encoder) = self.init_codec.codec(addr)?;
        let read_half = ReadHalfBox::new(read_half, decoder);
        let sender =
            self.write_half_collect
                .add_write_half(route_key, index_offset, write_half, encoder);
        if let Err(_e) = self
            .connect_sender
            .send((route_key, read_half, sender))
            .await
        {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connect close",
            ))?
        }
        Ok(route_key)
    }
}

impl SocketManager {
    pub async fn multi_send_to<A: Into<SocketAddr>>(
        &self,
        buf: BytesMut,
        addr: A,
    ) -> io::Result<()> {
        self.multi_send_to_impl(buf, addr, None).await
    }
    pub(crate) async fn multi_send_to_impl<A: Into<SocketAddr>>(
        &self,
        buf: BytesMut,
        addr: A,
        ttl: Option<u8>,
    ) -> io::Result<()> {
        let index_offset = rand::rng().random_range(0..self.tcp_multiplexing_limit);
        let route_key = self
            .multi_connect_impl(addr.into(), index_offset, ttl)
            .await?;
        self.send_to(buf, &route_key).await
    }
    /// Reuse the bound port to initiate a connection, which can be used to penetrate NAT1 network type.
    pub async fn reuse_port_send_to<A: Into<SocketAddr>>(
        &self,
        buf: BytesMut,
        addr: A,
    ) -> io::Result<()> {
        let route_key = self.connect_reuse_port(addr.into()).await?;
        self.send_to(buf, &route_key).await
    }

    /// Writing `buf` to the target denoted by `route_key`
    pub async fn send_to<D: ToRouteKeyForTcp<()>>(&self, buf: BytesMut, dest: D) -> io::Result<()> {
        let route_key = ToRouteKeyForTcp::route_key(self, dest)?;
        self.write_half_collect.send_to(buf, &route_key).await
    }
    pub async fn send_to_addr<D: Into<SocketAddr>>(
        &self,
        buf: BytesMut,
        dest: D,
    ) -> io::Result<()> {
        let route_key = self.connect(dest.into()).await?;
        self.write_half_collect.send_to(buf, &route_key).await
    }
    pub fn try_send_to<D: ToRouteKeyForTcp<()>>(&self, buf: BytesMut, dest: D) -> io::Result<()> {
        let route_key = ToRouteKeyForTcp::route_key(self, dest)?;
        self.write_half_collect.try_send_to(buf, &route_key)
    }
}
pub trait ToRouteKeyForTcp<T> {
    fn route_key(_: &SocketManager, _: Self) -> io::Result<RouteKey>;
}

impl ToRouteKeyForTcp<()> for RouteKey {
    fn route_key(_: &SocketManager, dest: RouteKey) -> io::Result<RouteKey> {
        Ok(dest)
    }
}

impl ToRouteKeyForTcp<()> for &RouteKey {
    fn route_key(_: &SocketManager, dest: &RouteKey) -> io::Result<RouteKey> {
        Ok(*dest)
    }
}

impl ToRouteKeyForTcp<()> for &mut RouteKey {
    fn route_key(_: &SocketManager, dest: &mut RouteKey) -> io::Result<RouteKey> {
        Ok(*dest)
    }
}
// impl<S: Into<SocketAddr>> ToRouteKeyForTcp<()> for S {
//     async fn route_key(socket_manager: &SocketManager, dest: Self) -> io::Result<RouteKey> {
//         socket_manager.connect(dest.into()).await
//     }
// }

pub trait TcpStreamIndex {
    fn route_key(&self) -> io::Result<RouteKey>;
    fn index(&self) -> Index;
}

impl TcpStreamIndex for TcpStream {
    fn route_key(&self) -> io::Result<RouteKey> {
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

/// The default byte encoder/decoder; using this is no different from directly using a TCP reliable.
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
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<()> {
        write.write_all(data).await?;
        Ok(())
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
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<()> {
        let head: [u8; 4] = (data.len() as u32).to_be_bytes();
        write.write_all(&head).await?;
        write.write_all(data).await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct BytesInitCodec;

impl InitCodec for BytesInitCodec {
    fn codec(&self, _addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
        Ok((Box::new(BytesCodec), Box::new(BytesCodec)))
    }
}

#[derive(Clone)]
pub struct LengthPrefixedInitCodec;

impl InitCodec for LengthPrefixedInitCodec {
    fn codec(&self, _addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)> {
        Ok((Box::new(LengthPrefixedCodec), Box::new(LengthPrefixedCodec)))
    }
}

pub trait InitCodec: Send + Sync + DynClone {
    fn codec(&self, addr: SocketAddr) -> io::Result<(Box<dyn Decoder>, Box<dyn Encoder>)>;
}
dyn_clone::clone_trait_object!(InitCodec);

#[async_trait]
pub trait Decoder: Send + Sync {
    async fn decode(&mut self, read: &mut OwnedReadHalf, src: &mut [u8]) -> io::Result<usize>;
    fn try_decode(&mut self, _read: &mut OwnedReadHalf, _src: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::WouldBlock))
    }
}

#[async_trait]
pub trait Encoder: Send + Sync {
    async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<()>;
    async fn encode_multiple(
        &mut self,
        write: &mut OwnedWriteHalf,
        bufs: &[IoSlice<'_>],
    ) -> io::Result<()> {
        for buf in bufs {
            self.encode(write, buf).await?
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use std::io;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

    use crate::tunnel::config::TcpTunnelConfig;
    use crate::tunnel::tcp::{Decoder, Encoder, InitCodec, TcpTunnelFactory};

    #[tokio::test]
    pub async fn create_tcp_tunnel() {
        let config: TcpTunnelConfig = TcpTunnelConfig::default();
        let tcp_tunnel_factory = TcpTunnelFactory::new(config).unwrap();
        drop(tcp_tunnel_factory)
    }

    #[tokio::test]
    pub async fn create_codec_tcp_tunnel() {
        let config = TcpTunnelConfig::new(Box::new(MyInitCodeC));
        let tcp_tunnel_factory = TcpTunnelFactory::new(config).unwrap();
        drop(tcp_tunnel_factory)
    }

    #[derive(Clone)]
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
        async fn encode(&mut self, write: &mut OwnedWriteHalf, data: &[u8]) -> io::Result<()> {
            let head: [u8; 2] = (data.len() as u16).to_be_bytes();
            write.write_all(&head).await?;
            write.write_all(data).await?;
            Ok(())
        }
    }
}
