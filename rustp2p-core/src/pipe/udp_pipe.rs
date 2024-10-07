use std::io;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use bytes::BytesMut;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::Sender;

use crate::pipe::config::UdpPipeConfig;
use crate::pipe::{DEFAULT_ADDRESS_V4, DEFAULT_ADDRESS_V6};
use crate::route::{Index, RouteKey};
use crate::socket::{bind_udp, LocalInterface};

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
    let (pipe_line_sender, pipe_line_receiver) = tokio::sync::mpsc::unbounded_channel();
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
    };
    udp_pipe.init()?;
    udp_pipe.socket_layer.switch_model(config.model)?;
    Ok(udp_pipe)
}

pub struct SocketLayer {
    main_udp_v4: Vec<Arc<UdpSocket>>,
    main_udp_v6: Vec<Arc<UdpSocket>>,
    sub_udp: RwLock<Vec<Arc<UdpSocket>>>,
    sub_close_notify: Mutex<Option<tokio::sync::broadcast::Sender<()>>>,
    pipe_line_sender: tokio::sync::mpsc::UnboundedSender<UdpPipeLine>,
    sub_udp_num: usize,
    default_interface: Option<LocalInterface>,
    sender_map: DashMap<usize, Sender<(BytesMut, SocketAddr)>>,
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
            let _ = sub_close_notify.send(());
        }
    }
    pub(crate) fn switch_high(&self) -> anyhow::Result<()> {
        let mut guard = self.sub_udp.write();
        if !guard.is_empty() {
            return Ok(());
        }
        let mut sub_close_notify_guard = self.sub_close_notify.lock();
        if let Some(sender) = sub_close_notify_guard.take() {
            let _ = sender.send(());
        }
        let (sub_close_notify_sender, _sub_close_notify_receiver) =
            tokio::sync::broadcast::channel(2);
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
                sub_close_notify_sender.subscribe(),
            );
            self.pipe_line_sender.send(udp_pipe_line)?;
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
    pub async fn send_to_buf(
        &self,
        buf: BytesMut,
        route_key: &RouteKey,
    ) -> crate::error::Result<()> {
        let sender = if let Some(sender) = self.sender_map.get(&route_key.index_usize()) {
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
    pipe_line_receiver: tokio::sync::mpsc::UnboundedReceiver<UdpPipeLine>,
    socket_layer: Arc<SocketLayer>,
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
            self.socket_layer.pipe_line_sender.send(udp_pipe_line)?;
        }
        for (index, udp) in self.socket_layer.main_udp_v6.iter().enumerate() {
            let udp = udp.clone();
            let udp_pipe_line = UdpPipeLine::main_new(
                UDPIndex::MainV6(index),
                udp,
                self.socket_layer.pipe_line_sender.clone(),
            );
            self.socket_layer.pipe_line_sender.send(udp_pipe_line)?;
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
        let (s, mut r) = tokio::sync::mpsc::channel(32);
        self.socket_layer
            .sender_map
            .insert(line.index.index(), s.clone());
        line.sender.replace(s);
        let udp = line.udp.clone().unwrap();
        tokio::spawn(async move {
            while let Some((buf, addr)) = r.recv().await {
                if let Err(e) = udp.send_to(&buf, addr).await {
                    log::debug!("{addr:?},{e:?}")
                }
            }
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
    close_notify: Option<tokio::sync::broadcast::Receiver<()>>,
    re_sender: Option<tokio::sync::mpsc::UnboundedSender<UdpPipeLine>>,
    sender: Option<Sender<(BytesMut, SocketAddr)>>,
}
impl Drop for UdpPipeLine {
    fn drop(&mut self) {
        if self.udp.is_none() || !self.active {
            return;
        }
        if let Some(re_sender) = &self.re_sender {
            let _ = re_sender.send(UdpPipeLine {
                active: false,
                index: self.index,
                udp: self.udp.take(),
                close_notify: self.close_notify.take(),
                re_sender: self.re_sender.clone(),
                sender: None,
            });
        }
    }
}

impl UdpPipeLine {
    pub(crate) fn sub_new(
        index: UDPIndex,
        udp: Arc<UdpSocket>,
        close_notify: tokio::sync::broadcast::Receiver<()>,
    ) -> Self {
        Self {
            active: false,
            index: Index::Udp(index),
            udp: Some(udp),
            close_notify: Some(close_notify),
            re_sender: None,
            sender: None,
        }
    }
    pub(crate) fn main_new(
        index: UDPIndex,
        udp: Arc<UdpSocket>,
        re_sender: tokio::sync::mpsc::UnboundedSender<UdpPipeLine>,
    ) -> Self {
        Self {
            active: false,
            index: Index::Udp(index),
            udp: Some(udp),
            close_notify: None,
            re_sender: Some(re_sender),
            sender: None,
        }
    }
    pub(crate) fn done(&mut self) {
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
    /// Writing `buf` to the target denoted by `route_key` via this pipeline
    pub async fn send_to(&self, buf: &[u8], route_key: &RouteKey) -> crate::error::Result<()> {
        if self.index != route_key.index() {
            Err(crate::error::Error::RouteNotFound("mismatch".into()))?
        }
        if let Some(udp) = &self.udp {
            let len = udp.send_to(buf, route_key.addr()).await?;
            if len == 0 {
                return Err(crate::error::Error::Io(std::io::Error::from(
                    std::io::ErrorKind::WriteZero,
                )));
            }
            Ok(())
        } else {
            Err(crate::error::Error::RouteNotFound("miss".into()))
        }
    }
    pub async fn readable(&self) -> crate::error::Result<()> {
        if let Some(udp) = &self.udp {
            udp.readable().await?;
            Ok(())
        } else {
            Err(crate::error::Error::RouteNotFound("miss".into()))
        }
    }
    /// Receving buf from this PipeLine
    /// `usize` in the `Ok` branch indicates how many bytes are received
    /// `RouteKey` in the `Ok` branch denotes the source where these bytes are received from
    pub async fn recv_from(
        &mut self,
        buf: &mut [u8],
    ) -> Option<std::io::Result<(usize, RouteKey)>> {
        let udp = if let Some(udp) = &self.udp {
            udp
        } else {
            return None;
        };
        loop {
            if let Some(close_notify) = &mut self.close_notify {
                tokio::select! {
                    _=close_notify.recv()=>{
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
