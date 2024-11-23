use std::io;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::Arc;

use crate::async_compat::net::udp::UdpSocket;
use anyhow::{anyhow, Context};
use bytes::BytesMut;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tachyonix::{Receiver, Sender, TrySendError};

use crate::pipe::config::UdpPipeConfig;
use crate::pipe::recycle::RecycleBuf;
use crate::pipe::{DEFAULT_ADDRESS_V4, DEFAULT_ADDRESS_V6};
use crate::route::{Index, RouteKey};
use crate::socket::{bind_udp, LocalInterface};

#[cfg(any(target_os = "linux", target_os = "android"))]
const MAX_MESSAGES: usize = 16;
#[cfg(any(target_os = "linux", target_os = "android"))]
use libc::{c_uint, iovec, mmsghdr, sockaddr_storage, socklen_t, timespec};
#[cfg(any(target_os = "linux", target_os = "android"))]
use std::os::fd::AsRawFd;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub enum Model {
    High,
    #[default]
    Low,
}

impl Model {
    pub fn is_low(&self) -> bool {
        self == &Model::Low
    }
    pub fn is_high(&self) -> bool {
        self == &Model::High
    }
}

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub enum UDPIndex {
    MainV4(usize),
    MainV6(usize),
    SubV4(usize),
}

impl UDPIndex {
    pub(crate) fn index(&self) -> usize {
        match self {
            UDPIndex::MainV4(i) => *i,
            UDPIndex::MainV6(i) => *i,
            UDPIndex::SubV4(i) => *i,
        }
    }
}

/// initialize udp pipe by config
pub(crate) fn udp_pipe(config: UdpPipeConfig) -> anyhow::Result<UdpPipe> {
    config.check()?;
    let mut udp_ports = config.udp_ports;
    udp_ports.resize(config.main_pipeline_num, 0);
    let mut main_udp_v4: Vec<Arc<UdpSocket>> = Vec::with_capacity(config.main_pipeline_num);
    let mut main_udp_v6: Vec<Arc<UdpSocket>> = Vec::with_capacity(config.main_pipeline_num);
    // 因为在mac上v4和v6的对绑定网卡的处理不同，所以这里分开监听，并且分开监听更容易处理发送目标为v4的情况，因为双协议栈下发送v4目标需要转换成v6
    for port in &udp_ports {
        loop {
            let mut addr_v4 = DEFAULT_ADDRESS_V4;
            addr_v4.set_port(*port);
            let socket_v4 = bind_udp(addr_v4, config.default_interface.as_ref())?;
            let udp_v4: std::net::UdpSocket = socket_v4.into();
            if config.use_v6 {
                let mut addr_v6 = DEFAULT_ADDRESS_V6;
                let socket_v6 = if *port == 0 {
                    let port = udp_v4.local_addr()?.port();
                    addr_v6.set_port(port);
                    match bind_udp(addr_v6, config.default_interface.as_ref()) {
                        Ok(socket_v6) => socket_v6,
                        Err(_) => continue,
                    }
                } else {
                    addr_v6.set_port(*port);
                    bind_udp(addr_v6, config.default_interface.as_ref())?
                };
                let udp_v6: std::net::UdpSocket = socket_v6.into();
                main_udp_v6.push(Arc::new(UdpSocket::from_std(udp_v6)?))
            }
            main_udp_v4.push(Arc::new(UdpSocket::from_std(udp_v4)?));
            break;
        }
    }
    let (pipe_line_sender, pipe_line_receiver) =
        tachyonix::channel(config.main_pipeline_num + config.sub_pipeline_num);
    let socket_layer = Arc::new(SocketLayer {
        main_udp_v4,
        main_udp_v6,
        sub_udp: RwLock::new(Vec::with_capacity(config.sub_pipeline_num)),
        sub_close_notify: Default::default(),
        pipe_line_sender,
        sub_udp_num: config.sub_pipeline_num,
        default_interface: config.default_interface,
        sender_map: Default::default(),
    });
    let udp_pipe = UdpPipe {
        pipe_line_receiver,
        socket_layer,
        recycle_buf: config.recycle_buf,
    };
    udp_pipe.init()?;
    udp_pipe.socket_layer.switch_model(config.model)?;
    Ok(udp_pipe)
}

pub struct SocketLayer {
    main_udp_v4: Vec<Arc<UdpSocket>>,
    main_udp_v6: Vec<Arc<UdpSocket>>,
    sub_udp: RwLock<Vec<Arc<UdpSocket>>>,
    sub_close_notify: Mutex<Option<async_broadcast::Sender<()>>>,
    pipe_line_sender: Sender<UdpPipeLine>,
    sub_udp_num: usize,
    default_interface: Option<LocalInterface>,
    sender_map: DashMap<Index, Sender<(BytesMut, SocketAddr)>>,
}

impl SocketLayer {
    pub(crate) fn try_sub_send_to_addr_v4(&self, buf: &[u8], addr: SocketAddr) {
        for (i, udp) in self.sub_udp.read().iter().enumerate() {
            if let Err(e) = udp.try_send_to(buf, addr) {
                log::info!("try_sub_send_to_addr_v4: {e:?},{i},{addr}")
            }
        }
    }
    pub(crate) fn try_main_send_to_addr(&self, buf: &[u8], addr: &[SocketAddr]) {
        let len = self.main_pipeline_len();
        for (i, addr) in addr.iter().enumerate() {
            match self.get(*addr, i % len) {
                Ok(writer) => {
                    if let Err(e) = writer.try_send(buf) {
                        log::info!("try_main_send_to_addr: {e:?},{i},{addr}")
                    }
                }
                Err(e) => {
                    log::info!("try_main_send_to_addr: {e:?},{i},{addr}")
                }
            }
        }
    }
    pub(crate) fn generate_route_key_from_addr(
        &self,
        index: usize,
        addr: SocketAddr,
    ) -> crate::error::Result<RouteKey> {
        let route_key = if addr.is_ipv4() {
            let len = self.main_udp_v4.len();
            if index >= len {
                return Err(crate::error::Error::IndexOutOfBounds { len, index });
            }
            RouteKey::new(Index::Udp(UDPIndex::MainV4(index)), addr)
        } else {
            let len = self.main_udp_v6.len();
            if len == 0 {
                return Err(crate::error::Error::NotSupportIPV6);
            }
            if index >= len {
                return Err(crate::error::Error::IndexOutOfBounds { len, index });
            }
            RouteKey::new(Index::Udp(UDPIndex::MainV6(index)), addr)
        };
        Ok(route_key)
    }
    pub(crate) fn switch_low(&self) {
        let mut guard = self.sub_udp.write();
        if guard.is_empty() {
            return;
        }
        guard.clear();
        if let Some(sub_close_notify) = self.sub_close_notify.lock().take() {
            let _ = sub_close_notify.close();
        }
    }
    pub(crate) fn switch_high(&self) -> anyhow::Result<()> {
        let mut guard = self.sub_udp.write();
        if !guard.is_empty() {
            return Ok(());
        }
        let mut sub_close_notify_guard = self.sub_close_notify.lock();
        if let Some(sender) = sub_close_notify_guard.take() {
            let _ = sender.close();
        }
        let (sub_close_notify_sender, sub_close_notify_receiver) = async_broadcast::broadcast(2);
        let mut sub_udp_list = Vec::with_capacity(self.sub_udp_num);
        for _ in 0..self.sub_udp_num {
            let udp = bind_udp(DEFAULT_ADDRESS_V4, self.default_interface.as_ref())?;
            let udp: std::net::UdpSocket = udp.into();
            sub_udp_list.push(Arc::new(UdpSocket::from_std(udp)?));
        }
        for (index, udp) in sub_udp_list.iter().enumerate() {
            let udp = udp.clone();
            let udp_pipe_line = UdpPipeLine::sub_new(
                UDPIndex::SubV4(index),
                udp,
                sub_close_notify_receiver.clone(),
            );
            if self.pipe_line_sender.try_send(udp_pipe_line).is_err() {
                Err(anyhow::anyhow!("pipe channel error"))?
            }
        }
        sub_close_notify_guard.replace(sub_close_notify_sender);
        *guard = sub_udp_list;
        Ok(())
    }

    /// Acquire the underlying `UDP` socket by the index
    #[inline]
    fn get_sub_udp(&self, index: usize) -> crate::error::Result<Arc<UdpSocket>> {
        let guard = self.sub_udp.read();
        let len = guard.len();
        if len <= index {
            Err(crate::error::Error::IndexOutOfBounds { len, index })
        } else {
            Ok(guard[index].clone())
        }
    }
    #[inline]
    fn get_udp(&self, udp_index: UDPIndex) -> crate::error::Result<Arc<UdpSocket>> {
        Ok(match udp_index {
            UDPIndex::MainV4(index) => self
                .main_udp_v4
                .get(index)
                .ok_or(crate::error::Error::IndexOutOfBounds {
                    len: self.v4_pipeline_len(),
                    index,
                })?
                .clone(),
            UDPIndex::MainV6(index) => self
                .main_udp_v6
                .get(index)
                .ok_or(crate::error::Error::IndexOutOfBounds {
                    len: self.v6_pipeline_len(),
                    index,
                })?
                .clone(),
            UDPIndex::SubV4(index) => self.get_sub_udp(index)?,
        })
    }

    #[inline]
    fn get_udp_from_route(&self, route_key: &RouteKey) -> crate::error::Result<Arc<UdpSocket>> {
        Ok(match route_key.index() {
            Index::Udp(index) => self.get_udp(index)?,
            _ => return Err(crate::error::Error::InvalidProtocol),
        })
    }

    #[inline]
    async fn send_to_addr_via_index(
        &self,
        buf: &[u8],
        addr: SocketAddr,
        index: usize,
    ) -> crate::error::Result<()> {
        let key = self.generate_route_key_from_addr(index, addr)?;
        self.send_to(buf, &key).await
    }
    async fn send_buf_to_addr_via_index(
        &self,
        buf: BytesMut,
        addr: SocketAddr,
        index: usize,
    ) -> crate::error::Result<()> {
        let key = self.generate_route_key_from_addr(index, addr)?;
        self.send_buf_to(buf, &key).await
    }
    #[inline]
    fn try_send_to_addr_via_index(
        &self,
        buf: &[u8],
        addr: SocketAddr,
        index: usize,
    ) -> crate::error::Result<()> {
        let key = self.generate_route_key_from_addr(index, addr)?;
        self.try_send_to(buf, &key)
    }
}

impl SocketLayer {
    pub fn model(&self) -> Model {
        if self.sub_udp.read().is_empty() {
            Model::Low
        } else {
            Model::High
        }
    }

    #[inline]
    pub fn main_pipeline_len(&self) -> usize {
        self.v4_pipeline_len()
    }
    #[inline]
    pub fn v4_pipeline_len(&self) -> usize {
        self.main_udp_v4.len()
    }
    #[inline]
    pub fn v6_pipeline_len(&self) -> usize {
        self.main_udp_v6.len()
    }

    pub fn switch_model(&self, model: Model) -> anyhow::Result<()> {
        match model {
            Model::High => self.switch_high(),
            Model::Low => {
                self.switch_low();
                Ok(())
            }
        }
    }
    /// Acquire the local ports `UDP` sockets bind on
    pub fn local_ports(&self) -> anyhow::Result<Vec<u16>> {
        let mut ports = Vec::with_capacity(self.v4_pipeline_len());
        for udp in &self.main_udp_v4 {
            ports.push(udp.local_addr()?.port());
        }
        Ok(ports)
    }
    pub async fn send_buf_to(
        &self,
        buf: BytesMut,
        route_key: &RouteKey,
    ) -> crate::error::Result<()> {
        let sender = if let Some(sender) = self.sender_map.get(&route_key.index()) {
            sender.value().clone()
        } else {
            return Err(crate::error::Error::RouteNotFound("".into()));
        };
        if let Err(_e) = sender.send((buf, route_key.addr())).await {
            Err(io::Error::from(io::ErrorKind::WriteZero))?
        } else {
            Ok(())
        }
    }
    /// Writing `buf` to the target denoted by `route_key`
    pub async fn send_to(&self, buf: &[u8], route_key: &RouteKey) -> crate::error::Result<()> {
        let len = self
            .get_udp_from_route(route_key)?
            .send_to(buf, route_key.addr())
            .await?;
        if len == 0 {
            return Err(crate::error::Error::Io(std::io::Error::from(
                std::io::ErrorKind::WriteZero,
            )));
        }
        Ok(())
    }
    pub async fn send_multiple_to(
        &self,
        bufs: &[IoSlice<'_>],
        route_key: &RouteKey,
    ) -> crate::error::Result<()> {
        let udp = self.get_udp_from_route(route_key)?;
        for buf in bufs {
            let len = udp.send_to(buf, route_key.addr()).await?;
            if len == 0 {
                return Err(crate::error::Error::Io(std::io::Error::from(
                    std::io::ErrorKind::WriteZero,
                )));
            }
        }

        Ok(())
    }
    /// Try to write `buf` to the target denoted by `route_key`
    pub fn try_send_to(&self, buf: &[u8], route_key: &RouteKey) -> crate::error::Result<()> {
        let len = self
            .get_udp_from_route(route_key)?
            .try_send_to(buf, route_key.addr())?;
        if len == 0 {
            return Err(crate::error::Error::Io(std::io::Error::from(
                std::io::ErrorKind::WriteZero,
            )));
        }
        Ok(())
    }
    /// Writing `buf` to the target denoted by SocketAddr
    pub async fn send_to_addr<A: Into<SocketAddr>>(
        &self,
        buf: &[u8],
        addr: A,
    ) -> crate::error::Result<()> {
        self.send_to_addr_via_index(buf, addr.into(), 0).await
    }
    pub async fn send_buf_to_addr<A: Into<SocketAddr>>(
        &self,
        buf: BytesMut,
        addr: A,
    ) -> crate::error::Result<()> {
        self.send_buf_to_addr_via_index(buf, addr.into(), 0).await
    }
    /// Try to write `buf` to the target denoted by SocketAddr
    pub fn try_send_to_addr<A: Into<SocketAddr>>(
        &self,
        buf: &[u8],
        addr: A,
    ) -> crate::error::Result<()> {
        self.try_send_to_addr_via_index(buf, addr.into(), 0)
    }
    /// Acquire the PipeWriter by the index
    pub fn get(&self, addr: SocketAddr, index: usize) -> anyhow::Result<UdpPipeWriterIndex<'_>> {
        if index >= self.main_udp_v4.len() && index >= self.main_udp_v6.len() {
            return Err(anyhow!(
                "neither in the bound of both the udp_v4 set nor the udp_v6 set"
            ));
        }
        Ok(UdpPipeWriterIndex {
            shadow: self,
            addr,
            index,
        })
    }

    /// Send bytes to the target denoted by SocketAddr with every main underlying socket
    pub async fn detect_pub_addrs<A: Into<SocketAddr>>(
        &self,
        buf: &[u8],
        addr: A,
    ) -> anyhow::Result<()> {
        let addr: SocketAddr = addr.into();
        for index in 0..self.main_pipeline_len() {
            self.send_to_addr_via_index(buf, addr, index).await?;
        }
        Ok(())
    }
}

pub struct UdpPipe {
    pipe_line_receiver: Receiver<UdpPipeLine>,
    socket_layer: Arc<SocketLayer>,
    recycle_buf: Option<RecycleBuf>,
}

impl UdpPipe {
    pub(crate) fn init(&self) -> anyhow::Result<()> {
        for (index, udp) in self.socket_layer.main_udp_v4.iter().enumerate() {
            let udp = udp.clone();
            let udp_pipe_line = UdpPipeLine::main_new(
                UDPIndex::MainV4(index),
                udp,
                self.socket_layer.pipe_line_sender.clone(),
            );
            if self
                .socket_layer
                .pipe_line_sender
                .try_send(udp_pipe_line)
                .is_err()
            {
                Err(anyhow::anyhow!("pipe channel error"))?
            }
        }
        for (index, udp) in self.socket_layer.main_udp_v6.iter().enumerate() {
            let udp = udp.clone();
            let udp_pipe_line = UdpPipeLine::main_new(
                UDPIndex::MainV6(index),
                udp,
                self.socket_layer.pipe_line_sender.clone(),
            );
            if self
                .socket_layer
                .pipe_line_sender
                .try_send(udp_pipe_line)
                .is_err()
            {
                Err(anyhow::anyhow!("pipe channel error"))?
            }
        }
        Ok(())
    }
}

impl UdpPipe {
    /// Construct a `UDP` pipe with the specified configuration
    pub fn new(config: UdpPipeConfig) -> anyhow::Result<UdpPipe> {
        udp_pipe(config)
    }
    /// Accept `UDP` pipelines from this kind pipe
    pub async fn accept(&mut self) -> anyhow::Result<UdpPipeLine> {
        let mut line = self
            .pipe_line_receiver
            .recv()
            .await
            .context("UdpPipe close")?;
        line.active = true;
        if line.socket_layer.is_some() {
            return Ok(line);
        }
        let (s, mut r) = tachyonix::channel(32);
        let index = line.index;
        self.socket_layer.sender_map.insert(index, s);
        line.socket_layer.replace(self.socket_layer.clone());
        let socket_layer = self.socket_layer.clone();
        let udp = line.udp.clone().unwrap();
        let recycle_buf = self.recycle_buf.clone();
        crate::async_compat::spawn(async move {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            let mut vec_buf = Vec::with_capacity(16);

            #[cfg(any(target_os = "linux", target_os = "android"))]
            use std::os::fd::AsRawFd;

            while let Ok((buf, addr)) = r.recv().await {
                #[cfg(any(target_os = "linux", target_os = "android"))]
                {
                    vec_buf.push((buf, addr));
                    while let Ok(tup) = r.try_recv() {
                        vec_buf.push(tup);
                        if vec_buf.len() == MAX_MESSAGES {
                            break;
                        }
                    }
                    if vec_buf.len() == 1 {
                        let (buf, addr) = unsafe { vec_buf.get_unchecked(0) };
                        if let Err(e) = udp.send_to(buf, *addr).await {
                            log::debug!("{addr:?},{e:?}")
                        }
                    } else if let Err(e) = loop {
                        match sendmmsg(udp.as_raw_fd(), &mut vec_buf) {
                            Ok(_) => break Ok(()),
                            Err(e) => {
                                if e.kind() == io::ErrorKind::WouldBlock {
                                    if let Err(e) = udp.readable().await {
                                        break Err(e);
                                    }
                                    continue;
                                } else {
                                    break Err(e);
                                }
                            }
                        };
                    } {
                        log::debug!("sendmmsg {e:?}");
                    };
                    if let Some(recycle_buf) = recycle_buf.as_ref() {
                        while let Some((buf, _)) = vec_buf.pop() {
                            recycle_buf.push(buf);
                        }
                    } else {
                        vec_buf.clear();
                    }
                }
                #[cfg(not(any(target_os = "linux", target_os = "android")))]
                {
                    let rs = udp.send_to(&buf, addr).await;
                    if let Some(recycle_buf) = recycle_buf.as_ref() {
                        recycle_buf.push(buf);
                    }
                    if let Err(e) = rs {
                        log::debug!("{addr:?},{e:?}")
                    }
                }
            }
            socket_layer.sender_map.remove(&index);
        });
        Ok(line)
    }
    /// The number of pipelines established by main `UDP` sockets(IPv4)
    #[inline]
    pub fn main_pipeline_len(&self) -> usize {
        self.socket_layer.main_pipeline_len()
    }
    /// The number of pipelines established by main `UDP` sockets(IPv4)
    #[inline]
    pub fn v4_pipeline_len(&self) -> usize {
        self.socket_layer.v4_pipeline_len()
    }
    /// The number of pipelines established by main `UDP` sockets(IPv6)
    #[inline]
    pub fn v6_pipeline_len(&self) -> usize {
        self.socket_layer.v6_pipeline_len()
    }
    /// Acquire a shared reference for writing to the pipe
    pub fn writer_ref(&self) -> UdpPipeWriterRef<'_> {
        UdpPipeWriterRef {
            socket_layer: &self.socket_layer,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn sendmmsg(fd: std::os::fd::RawFd, buf: &mut [(BytesMut, SocketAddr)]) -> io::Result<()> {
    assert!(buf.len() <= MAX_MESSAGES);
    let mut iov: [iovec; MAX_MESSAGES] = unsafe { std::mem::zeroed() };
    let mut msgs: [mmsghdr; MAX_MESSAGES] = unsafe { std::mem::zeroed() };
    let mut addrs: [sockaddr_storage; MAX_MESSAGES] = unsafe { std::mem::zeroed() };
    for (i, (buf, addr)) in buf.iter_mut().enumerate() {
        addrs[i] = socket_addr_to_sockaddr(addr);
        iov[i].iov_base = buf.as_mut_ptr() as *mut libc::c_void;
        iov[i].iov_len = buf.len();
        msgs[i].msg_hdr.msg_iov = &mut iov[i];
        msgs[i].msg_hdr.msg_iovlen = 1;

        msgs[i].msg_hdr.msg_name = &mut addrs[i] as *mut _ as *mut libc::c_void;
        msgs[i].msg_hdr.msg_namelen = std::mem::size_of::<sockaddr_storage>() as socklen_t;
    }

    unsafe {
        let res = libc::sendmmsg(fd, msgs.as_mut_ptr(), buf.len() as _, 0);
        if res == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn socket_addr_to_sockaddr(addr: &SocketAddr) -> sockaddr_storage {
    let mut storage: sockaddr_storage = unsafe { std::mem::zeroed() };

    match addr {
        SocketAddr::V4(v4_addr) => {
            let sin = libc::sockaddr_in {
                sin_family: libc::AF_INET as _,
                sin_port: v4_addr.port().to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(v4_addr.ip().octets()), // IP 地址
                },
                sin_zero: [0; 8],
            };

            unsafe {
                let sin_ptr = &sin as *const libc::sockaddr_in as *const u8;
                let storage_ptr = &mut storage as *mut sockaddr_storage as *mut u8;
                std::ptr::copy_nonoverlapping(
                    sin_ptr,
                    storage_ptr,
                    std::mem::size_of::<libc::sockaddr>(),
                );
            }
        }
        SocketAddr::V6(v6_addr) => {
            let sin6 = libc::sockaddr_in6 {
                sin6_family: libc::AF_INET6 as _,
                sin6_port: v6_addr.port().to_be(),
                sin6_flowinfo: v6_addr.flowinfo(),
                sin6_addr: libc::in6_addr {
                    s6_addr: v6_addr.ip().octets(),
                },
                sin6_scope_id: v6_addr.scope_id(),
            };

            unsafe {
                let sin6_ptr = &sin6 as *const libc::sockaddr_in6 as *const u8;
                let storage_ptr = &mut storage as *mut sockaddr_storage as *mut u8;
                std::ptr::copy_nonoverlapping(
                    sin6_ptr,
                    storage_ptr,
                    std::mem::size_of::<libc::sockaddr>(),
                );
            }
        }
    }
    storage
}

#[derive(Clone)]
pub struct UdpPipeWriter {
    socket_layer: Arc<SocketLayer>,
}

impl Deref for UdpPipeWriter {
    type Target = SocketLayer;

    fn deref(&self) -> &Self::Target {
        &self.socket_layer
    }
}

pub struct UdpPipeWriterIndex<'a> {
    shadow: &'a SocketLayer,
    addr: SocketAddr,
    index: usize,
}

impl<'a> UdpPipeWriterIndex<'a> {
    pub async fn send(&self, buf: &[u8]) -> crate::error::Result<()> {
        self.shadow
            .send_to_addr_via_index(buf, self.addr, self.index)
            .await
    }
    pub fn try_send(&self, buf: &[u8]) -> crate::error::Result<()> {
        self.shadow
            .try_send_to_addr_via_index(buf, self.addr, self.index)
    }
}

#[derive(Clone, Copy)]
pub struct UdpPipeWriterRef<'a> {
    socket_layer: &'a Arc<SocketLayer>,
}

impl<'a> UdpPipeWriterRef<'a> {
    pub fn to_owned(&self) -> UdpPipeWriter {
        UdpPipeWriter {
            socket_layer: self.socket_layer.clone(),
        }
    }
}

impl<'a> Deref for UdpPipeWriterRef<'a> {
    type Target = Arc<SocketLayer>;

    fn deref(&self) -> &Self::Target {
        self.socket_layer
    }
}

pub struct UdpPipeLine {
    active: bool,
    index: Index,
    udp: Option<Arc<UdpSocket>>,
    close_notify: Option<async_broadcast::Receiver<()>>,
    re_sender: Option<Sender<UdpPipeLine>>,
    socket_layer: Option<Arc<SocketLayer>>,
}

impl Drop for UdpPipeLine {
    fn drop(&mut self) {
        if self.udp.is_none() || !self.active {
            if let Some(socket_layer) = self.socket_layer.take() {
                socket_layer.sender_map.remove(&self.index);
            }
            return;
        }
        if let Some(re_sender) = &self.re_sender {
            let rs = re_sender.try_send(UdpPipeLine {
                active: false,
                index: self.index,
                udp: self.udp.take(),
                close_notify: self.close_notify.take(),
                re_sender: self.re_sender.clone(),
                socket_layer: self.socket_layer.take(),
            });
            if let Err(TrySendError::Full(_)) = rs {
                log::warn!("UdpPipeLine TrySendError full");
            }
        }
    }
}

impl UdpPipeLine {
    pub(crate) fn sub_new(
        index: UDPIndex,
        udp: Arc<UdpSocket>,
        close_notify: async_broadcast::Receiver<()>,
    ) -> Self {
        Self {
            active: false,
            index: Index::Udp(index),
            udp: Some(udp),
            close_notify: Some(close_notify),
            re_sender: None,
            socket_layer: None,
        }
    }
    pub(crate) fn main_new(
        index: UDPIndex,
        udp: Arc<UdpSocket>,
        re_sender: Sender<UdpPipeLine>,
    ) -> Self {
        Self {
            active: false,
            index: Index::Udp(index),
            udp: Some(udp),
            close_notify: None,
            re_sender: Some(re_sender),
            socket_layer: None,
        }
    }
    pub fn done(&mut self) {
        let _ = self.udp.take();
        let _ = self.close_notify.take();
    }
}

impl UdpPipeLine {
    /// Writing `buf` to the target denoted by SocketAddr via this pipeline
    pub async fn send_to_addr<A: Into<SocketAddr>>(
        &self,
        buf: &[u8],
        addr: A,
    ) -> anyhow::Result<()> {
        if let Some(udp) = &self.udp {
            udp.send_to(buf, addr.into()).await?;
            Ok(())
        } else {
            Err(anyhow!("closed"))
        }
    }
    /// Try to write `buf` to the target denoted by SocketAddr via this pipeline
    pub fn try_send_to_addr<A: Into<SocketAddr>>(&self, buf: &[u8], addr: A) -> anyhow::Result<()> {
        if let Some(udp) = &self.udp {
            udp.try_send_to(buf, addr.into())?;
            Ok(())
        } else {
            Err(anyhow!("closed"))
        }
    }

    pub async fn send_buf_to(
        &self,
        buf: BytesMut,
        route_key: &RouteKey,
    ) -> crate::error::Result<()> {
        if self.index != route_key.index() {
            Err(crate::error::Error::RouteNotFound("mismatch".into()))?
        }
        if let Some(s) = &self.socket_layer {
            s.send_buf_to(buf, route_key).await
        } else {
            Err(crate::error::Error::RouteNotFound("miss".into()))
        }
    }

    /// Receving buf from this PipeLine
    /// `usize` in the `Ok` branch indicates how many bytes are received
    /// `RouteKey` in the `Ok` branch denotes the source where these bytes are received from
    pub async fn recv_from(&mut self, buf: &mut [u8]) -> Option<io::Result<(usize, RouteKey)>> {
        let udp = if let Some(udp) = &self.udp {
            udp
        } else {
            return None;
        };
        loop {
            if let Some(close_notify) = &mut self.close_notify {
                crate::select! {
                    _rs=close_notify.recv()=>{
                         self.done();
                         return None
                    }
                    result=udp.recv_from(buf)=>{
                         let (len, addr) = match result {
                            Ok(rs) => rs,
                            Err(e) => {
                                if should_ignore_error(&e) {
                                    continue;
                                }
                                return Some(Err(e))
                            }
                         };
                         return Some(Ok((len, RouteKey::new(self.index, addr))))
                    }
                }
            } else {
                let (len, addr) = match udp.recv_from(buf).await {
                    Ok(rs) => rs,
                    Err(e) => {
                        if should_ignore_error(&e) {
                            continue;
                        }
                        return Some(Err(e));
                    }
                };
                return Some(Ok((len, RouteKey::new(self.index, addr))));
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub async fn recv_multi_from<B: AsMut<[u8]>>(
        &mut self,
        bufs: &mut [B],
        sizes: &mut [usize],
        addrs: &mut [RouteKey],
    ) -> Option<io::Result<usize>> {
        if bufs.is_empty() || bufs.len() != sizes.len() || bufs.len() != addrs.len() {
            return Some(Err(io::Error::new(io::ErrorKind::Other, "bufs error")));
        }
        let rs = self.recv_from(bufs[0].as_mut()).await?;
        match rs {
            Ok((len, addr)) => {
                let udp = self.udp.as_ref().unwrap();
                sizes[0] = len;
                addrs[0] = addr;
                let mut num = 1;
                while num < bufs.len() {
                    match udp.try_recv_from(bufs[num].as_mut()) {
                        Ok((len, addr)) => {
                            sizes[num] = len;
                            addrs[num] = RouteKey::new(self.index, addr);
                            num += 1;
                        }
                        Err(_) => break,
                    }
                }
                Some(Ok(num))
            }
            Err(e) => Some(Err(e)),
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub async fn recv_multi_from<B: AsMut<[u8]>>(
        &mut self,
        bufs: &mut [B],
        sizes: &mut [usize],
        addrs: &mut [RouteKey],
    ) -> Option<io::Result<usize>> {
        if bufs.is_empty() || bufs.len() != sizes.len() || bufs.len() != addrs.len() {
            return Some(Err(io::Error::new(
                io::ErrorKind::Other,
                "bufs/sizes/addrs error",
            )));
        }
        let udp = if let Some(udp) = &self.udp {
            udp
        } else {
            return None;
        };
        loop {
            let rs = if let Some(close_notify) = &mut self.close_notify {
                crate::select! {
                    _rs=close_notify.recv()=>{
                        self.done();
                        return None
                    }
                    rs=udp.readable()=>{
                        rs
                    }
                }
            } else {
                udp.readable().await
            };
            if let Err(e) = rs {
                if should_ignore_error(&e) {
                    continue;
                }
                return Some(Err(e));
            }
            return match recvmmsg(self.index, udp.as_raw_fd(), bufs, sizes, addrs) {
                Ok(rs) => Some(Ok(rs)),
                Err(e) => Some(Err(e)),
            };
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn recvmmsg<B: AsMut<[u8]>>(
    index: Index,
    fd: std::os::fd::RawFd,
    bufs: &mut [B],
    sizes: &mut [usize],
    route_keys: &mut [RouteKey],
) -> io::Result<usize> {
    let mut iov: [iovec; MAX_MESSAGES] = unsafe { std::mem::zeroed() };
    let mut msgs: [mmsghdr; MAX_MESSAGES] = unsafe { std::mem::zeroed() };
    let mut addrs: [sockaddr_storage; MAX_MESSAGES] = unsafe { std::mem::zeroed() };
    let max_num = bufs.len().min(MAX_MESSAGES);
    for i in 0..max_num {
        iov[i].iov_base = bufs[i].as_mut().as_mut_ptr() as *mut libc::c_void;
        iov[i].iov_len = bufs[i].as_mut().len();
        msgs[i].msg_hdr.msg_iov = &mut iov[i];
        msgs[i].msg_hdr.msg_iovlen = 1;
        msgs[i].msg_hdr.msg_name = &mut addrs[i] as *const _ as *mut libc::c_void;
        msgs[i].msg_hdr.msg_namelen = std::mem::size_of::<sockaddr_storage>() as socklen_t;
    }
    let mut time: timespec = unsafe { std::mem::zeroed() };
    let res = unsafe { libc::recvmmsg(fd, msgs.as_mut_ptr(), max_num as c_uint, 0, &mut time) };
    if res == -1 {
        return Err(io::Error::last_os_error());
    }
    let nmsgs = res as usize;
    for i in 0..nmsgs {
        let addr = sockaddr_to_socket_addr(&addrs[i], msgs[i].msg_hdr.msg_namelen);
        sizes[i] = msgs[i].msg_len as usize;
        route_keys[i] = RouteKey::new(index, addr);
    }
    Ok(nmsgs)
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn sockaddr_to_socket_addr(addr: &sockaddr_storage, _len: socklen_t) -> SocketAddr {
    match addr.ss_family as libc::c_int {
        libc::AF_INET => {
            let addr_in = unsafe { *(addr as *const _ as *const libc::sockaddr_in) };
            let ip = u32::from_be(addr_in.sin_addr.s_addr);
            let port = u16::from_be(addr_in.sin_port);
            SocketAddr::V4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::from(ip),
                port,
            ))
        }
        libc::AF_INET6 => {
            let addr_in6 = unsafe { *(addr as *const _ as *const libc::sockaddr_in6) };
            let ip = std::net::Ipv6Addr::from(addr_in6.sin6_addr.s6_addr);
            let port = u16::from_be(addr_in6.sin6_port);
            SocketAddr::V6(std::net::SocketAddrV6::new(ip, port, 0, 0))
        }
        _ => panic!("Unsupported address family"),
    }
}

fn should_ignore_error(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        // 检查错误码是否为 WSAECONNRESET
        if let Some(os_error) = e.raw_os_error() {
            return os_error == windows_sys::Win32::Networking::WinSock::WSAECONNRESET;
        }
    }
    _ = e;
    false
}

#[cfg(test)]
#[cfg(feature = "use-tokio")]
mod tests {
    use std::time::Duration;

    use crate::pipe::udp_pipe::{Model, UdpPipeLine};

    #[tokio::test]
    pub async fn create_udp_pipe() {
        let config = crate::pipe::config::UdpPipeConfig::default()
            .set_main_pipeline_num(2)
            .set_sub_pipeline_num(10)
            .set_model(Model::Low)
            .set_use_v6(false);
        let mut udp_pipe = crate::pipe::udp_pipe::udp_pipe(config).unwrap();
        let mut count = 0;
        let mut join = Vec::new();
        while let Ok(rs) = tokio::time::timeout(Duration::from_secs(1), udp_pipe.accept()).await {
            join.push(tokio::spawn(pipe_line_recv(rs.unwrap())));
            count += 1;
        }
        assert_eq!(count, 2)
    }

    #[tokio::test]
    pub async fn create_sub_udp_pipe() {
        let config = crate::pipe::config::UdpPipeConfig::default()
            .set_main_pipeline_num(2)
            .set_sub_pipeline_num(10)
            .set_use_v6(false)
            .set_model(Model::High);
        let mut udp_pipe = crate::pipe::udp_pipe::udp_pipe(config).unwrap();
        let mut count = 0;
        let mut join = Vec::new();
        while let Ok(rs) = tokio::time::timeout(Duration::from_secs(1), udp_pipe.accept()).await {
            join.push(tokio::spawn(pipe_line_recv(rs.unwrap())));
            count += 1;
        }
        udp_pipe.writer_ref().switch_low();

        let mut close_pipe_line_count = 0;
        for x in join {
            let rs = tokio::time::timeout(Duration::from_secs(1), x).await;
            match rs {
                Ok(rs) => {
                    if rs.unwrap() {
                        // pipe_line_recv task done
                        close_pipe_line_count += 1;
                    }
                }
                Err(_e) => {
                    _ = _e;
                }
            }
        }
        assert_eq!(count, 12);
        assert_eq!(close_pipe_line_count, 10);
    }

    async fn pipe_line_recv(mut udp_pipe_line: UdpPipeLine) -> bool {
        let mut buf = [0; 1400];
        udp_pipe_line.recv_from(&mut buf).await.is_none()
    }
}
