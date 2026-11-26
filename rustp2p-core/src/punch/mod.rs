use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use parking_lot::Mutex;
use rand::seq::SliceRandom;
use rand::RngExt;

use crate::endpoint::pool::SocketPool;
use crate::nat::{NatInfo, NatType};

pub use config::*;
pub mod config;

fn symmetric_prediction_candidates(
    peer_nat_info: &NatInfo,
    predict_range: u16,
) -> (Vec<u16>, Vec<u16>) {
    let mut known_ports = peer_nat_info.public_udp_ports.clone();
    known_ports.extend(peer_nat_info.stun_mapped_ports.iter().copied());
    known_ports.retain(|port| *port != 0);
    known_ports.sort_unstable();
    known_ports.dedup();

    let mut predicted_ports = Vec::new();
    for &base_port in &known_ports {
        let min_port = base_port.saturating_sub(predict_range).max(1);
        let max_port = base_port.saturating_add(predict_range);
        predicted_ports.extend(min_port..=max_port);
    }
    predicted_ports.sort_unstable();
    predicted_ports.dedup();

    (known_ports, predicted_ports)
}

#[derive(Default, Clone)]
struct PunchStats {
    batch_count: usize,
    last_time: u64,
}

fn should_punch(stats: &PunchStats, current_time: u64) -> bool {
    if stats.batch_count <= 8 {
        return true;
    }

    let min_interval = (stats.batch_count / 8).min(360) as u64;
    current_time.saturating_sub(stats.last_time) >= min_interval
}

/// A cloneable handle for UDP socket access, NAT discovery, and hole punching.
///
/// `Puncher` owns the operations that do not belong to an accepted
/// [`Tunnel`](crate::endpoint::Tunnel): connectionless UDP sends, socket
/// inspection, NAT discovery, and punch scheduling.
#[derive(Clone)]
pub struct Puncher {
    shuffled_ports: Arc<Vec<u16>>,
    port_cursor: Arc<Mutex<HashMap<SocketAddr, usize>>>,
    punch_stats: Arc<Mutex<HashMap<SocketAddr, PunchStats>>>,
    pool: Arc<SocketPool>,
    nat_config: Arc<NatQueryConfig>,
}

struct NatQueryConfig {
    stun_servers: Vec<String>,
    mapping_tcp_addr: Vec<SocketAddr>,
    mapping_udp_addr: Vec<SocketAddr>,
    default_interface: Option<crate::socket::LocalInterface>,
    bind_ipv4: Option<Ipv4Addr>,
    bind_ipv6: Option<Ipv6Addr>,
    enable_ipv6: bool,
    local_tcp_port: u16,
    max_assistant_sockets: usize,
}

/// Upper bound for the per-peer punch bookkeeping maps (`punch_stats` and
/// `port_cursor`). When a map is full, it is cleared wholesale: entries are
/// small and the occasional backoff/cursor reset is preferable to unbounded
/// growth on long-lived nodes.
const MAX_PUNCH_ENTRIES: usize = 4096;

impl Puncher {
    pub(crate) fn new(
        pool: Arc<SocketPool>,
        config: &crate::endpoint::Config,
        local_tcp_port: u16,
    ) -> Puncher {
        let mut shuffled_ports: Vec<u16> = (1..=65535).collect();
        shuffled_ports.shuffle(&mut rand::rng());
        Self {
            shuffled_ports: Arc::new(shuffled_ports),
            port_cursor: Arc::new(Mutex::new(HashMap::new())),
            punch_stats: Arc::new(Mutex::new(HashMap::new())),
            pool,
            nat_config: Arc::new(NatQueryConfig {
                stun_servers: config.stun_servers.clone(),
                mapping_tcp_addr: config.mapping_tcp_addr.clone(),
                mapping_udp_addr: config.mapping_udp_addr.clone(),
                default_interface: config.default_interface.clone(),
                bind_ipv4: config.bind_ipv4,
                bind_ipv6: config.bind_ipv6,
                enable_ipv6: config.enable_ipv6,
                local_tcp_port,
                max_assistant_sockets: config.max_assistant_sockets,
            }),
        }
    }

    /// Sends through all UDP sockets matching the target's address family.
    pub fn try_send_via_all(&self, buf: &[u8], addr: SocketAddr) {
        self.pool.try_send_via_all(buf, addr);
    }

    /// Sends through the matching-family main UDP socket.
    pub fn send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        self.pool.send_to(buf, addr)
    }

    /// Sends through every assistant UDP socket. IPv6 targets are ignored.
    pub fn try_send_via_assistants(&self, buf: &[u8], addr: SocketAddr) -> io::Result<()> {
        self.pool.try_send_via_assistants(buf, addr)
    }

    /// Returns the local address of the main IPv4 UDP socket.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.pool.local_addr()
    }

    /// Returns the current number of assistant UDP sockets.
    pub fn assistant_count(&self) -> usize {
        self.pool.assistant_count()
    }

    /// Returns all UDP sockets: main IPv4, optional main IPv6, then assistants.
    ///
    /// These are owning socket handles. Drop them (and any underlying socket
    /// extracted or duplicated from them) before calling
    /// [`TunnelIncoming::close`](crate::endpoint::TunnelIncoming::close)
    /// if the ports need to be reused. The endpoint cannot track or close
    /// sockets whose ownership has been taken out of the returned Arcs.
    pub fn udp_sockets(&self) -> Vec<Arc<tokio::net::UdpSocket>> {
        self.pool.udp_sockets()
    }

    /// Connects to `addr` and publishes the resulting TCP connection as an
    /// incoming tunnel.
    ///
    /// The connection is established through the configured default
    /// interface. `initial_data`, when present, is sent to the peer before
    /// the tunnel is published, replaying leading bytes that were consumed
    /// while the connection was established. Dropping the future before
    /// publication prevents the tunnel from appearing in
    /// [`TunnelIncoming::next`](crate::endpoint::TunnelIncoming::next).
    pub async fn connect_tcp(
        &self,
        addr: SocketAddr,
        initial_data: Option<Bytes>,
    ) -> io::Result<()> {
        self.connect_tcp_inner(addr, initial_data, None).await
    }

    async fn connect_tcp_inner(
        &self,
        addr: SocketAddr,
        initial_data: Option<Bytes>,
        ttl: Option<u8>,
    ) -> io::Result<()> {
        let puncher = self.clone();
        self.pool
            .connect_outgoing_tcp(addr, initial_data, async move {
                crate::socket::connect_tcp(
                    addr,
                    Some(puncher.tcp_bind_addr(addr, 0)?),
                    puncher.nat_config.default_interface.as_ref(),
                    ttl,
                )
                .await
            })
            .await
    }

    /// Gets NAT information using the STUN and mapping configuration captured
    /// when this puncher was created.
    pub async fn nat_info(&self) -> io::Result<NatInfo> {
        self.nat_info_with_servers(&self.nat_config.stun_servers)
            .await
    }

    /// Gets NAT information using custom STUN servers.
    ///
    /// `stun_servers` override the servers captured when this puncher was
    /// created; the mapping-address and interface configurations are still
    /// taken from it. An empty slice falls back to the configured servers.
    pub async fn nat_info_with_servers(&self, stun_servers: &[String]) -> io::Result<NatInfo> {
        let stun_servers = if stun_servers.is_empty() {
            &self.nat_config.stun_servers
        } else {
            stun_servers
        };
        let stun_result = crate::stun::stun_test_nat_bound(
            stun_servers.to_vec(),
            self.nat_config.default_interface.as_ref(),
            self.nat_config.bind_ipv4,
            self.nat_config.bind_ipv6,
            self.nat_config.enable_ipv6,
        )
        .await?;

        log::debug!(
            "nat_type:{:?},public_ipv4:{:?},public_ipv6:{:?},public_udp_ports:{:?},port_range:{}",
            stun_result.nat_type,
            stun_result.public_ipv4,
            stun_result.public_ipv6,
            stun_result.public_udp_ports,
            stun_result.port_range
        );

        // Enumerate the local IP addresses — only those of the bound NIC when
        // one is configured. Probe-based resolution is the fallback for an
        // address family that came up empty.
        let interface = self.nat_config.default_interface.as_ref();
        let scanned = crate::util::addr::local_ips(interface);
        let mut local_ipv4s: Vec<Ipv4Addr> = self.nat_config.bind_ipv4.into_iter().collect();
        if local_ipv4s.is_empty() {
            local_ipv4s = scanned.ipv4s;
        }
        if local_ipv4s.is_empty() {
            if let Some(ip) = crate::util::addr::local_ipv4(interface, stun_servers).await {
                local_ipv4s.push(ip);
            }
        }
        let mut ipv6 = if self.nat_config.enable_ipv6 {
            self.nat_config
                .bind_ipv6
                .or_else(|| scanned.ipv6s.into_iter().next())
        } else {
            None
        };
        if ipv6.is_none() && self.nat_config.enable_ipv6 {
            ipv6 = crate::util::addr::local_ipv6(interface, stun_servers).await;
        }
        let local_udp_ports = self.pool.local_udp_ports();

        // STUN uses a temporary UDP socket, so its mapped ports do not belong
        // to the actual tunnel sockets. Real mapped ports are learned later by
        // the higher-level NAT observation protocol.
        Ok(NatInfo {
            nat_type: stun_result.nat_type,
            public_ips: stun_result.public_ipv4,
            public_udp_ports: Vec::new(),
            mapping_tcp_addr: self.nat_config.mapping_tcp_addr.clone(),
            mapping_udp_addr: self.nat_config.mapping_udp_addr.clone(),
            public_port_range: stun_result.port_range,
            local_ipv4: local_ipv4s
                .first()
                .copied()
                .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED),
            local_ipv4s,
            ipv6,
            local_udp_ports,
            local_tcp_port: self.nat_config.local_tcp_port,
            public_tcp_port: 0,
            stun_mapped_ports: Vec::new(),
        })
    }

    /// Applies the socket model for an externally detected local NAT type.
    ///
    /// This method does not run STUN or otherwise detect the NAT type. Call
    /// [`Self::nat_info`] or another detector first, then pass its result here.
    ///
    /// - [`NatType::Symmetric`] adds assistant sockets up to the configured
    ///   `max_assistant_sockets` value.
    /// - [`NatType::Cone`] removes all assistant sockets.
    pub fn apply_nat_model(&self, nat_type: NatType) -> io::Result<()> {
        let _model_guard = self.pool.lock_assistant_model();
        self.pool.ensure_open()?;
        match nat_type {
            NatType::Symmetric => {
                let current = self.pool.assistant_count();
                let target = self.nat_config.max_assistant_sockets;
                if target > current {
                    log::debug!(
                        "Symmetric NAT model selected, adding {} assistant sockets",
                        target - current
                    );
                    for _ in current..target {
                        let socket = crate::socket::bind_udp(
                            SocketAddr::from((
                                self.nat_config.bind_ipv4.unwrap_or(Ipv4Addr::UNSPECIFIED),
                                0,
                            )),
                            self.nat_config.default_interface.as_ref(),
                        )?;
                        let std_socket: std::net::UdpSocket = socket.into();
                        let tokio_socket = tokio::net::UdpSocket::from_std(std_socket)?;
                        self.pool.add_assistant_udp(tokio_socket)?;
                    }
                }
            }
            NatType::Cone => {
                let count = self.pool.assistant_count();
                if count > 0 {
                    log::debug!("Cone NAT model selected, cleaning {count} assistant sockets");
                    self.pool.clean_assistant_udp();
                }
            }
        }

        Ok(())
    }

    /// Returns whether a new punch round should be started for the peer,
    /// based on the per-peer backoff schedule.
    pub fn need_punch(&self, punch_info: &PunchInfo) -> bool {
        let Some(id) = punch_info.peer_nat_info.flag() else {
            return false;
        };
        // Read-only query: do not create an entry here, otherwise mere
        // lookups would grow the map without bound.
        let stats = self
            .punch_stats
            .lock()
            .get(&id)
            .cloned()
            .unwrap_or_default();
        should_punch(&stats, now())
    }

    /// Punches only if [`Puncher::need_punch`] allows it.
    ///
    /// Note: like [`Puncher::punch_now`], the returned future is
    /// long-running; spawn it instead of awaiting it inline.
    pub async fn punch(&self, buf: Bytes, punch_info: PunchInfo) -> io::Result<()> {
        self.pool.ensure_open()?;
        if !self.need_punch(&punch_info) {
            return Ok(());
        }
        self.punch_now(Some(buf.clone()), buf, punch_info).await
    }

    /// Runs one full punch round, ignoring the backoff schedule.
    ///
    /// Note: the returned future is long-running. Symmetric-NAT punching
    /// sends up to ~1500 UDP packets paced at 2 ms intervals, so a single
    /// round can take several seconds. Callers must `tokio::spawn` this
    /// (or otherwise run it on a dedicated task) rather than awaiting it
    /// inline in a packet-dispatch or receive loop, which would stall all
    /// other protocol processing for the duration of the round.
    pub async fn punch_now(
        &self,
        tcp_buf: Option<Bytes>,
        udp_buf: Bytes,
        punch_info: PunchInfo,
    ) -> io::Result<()> {
        let mut shutdown = self.pool.subscribe_shutdown()?;
        tokio::select! {
            biased;
            _ = shutdown.recv() => self.pool.ensure_open(),
            () = self.punch_round(tcp_buf, udp_buf, punch_info) => self.pool.ensure_open(),
        }
    }

    async fn punch_round(&self, tcp_buf: Option<Bytes>, udp_buf: Bytes, punch_info: PunchInfo) {
        let peer = punch_info
            .peer_nat_info
            .flag()
            .unwrap_or(SocketAddr::V4(SocketAddrV4::new(
                std::net::Ipv4Addr::UNSPECIFIED,
                0,
            )));
        log::debug!(
            "punch_now -> {} (nat={:?}, model={:?})",
            peer,
            punch_info.peer_nat_info.nat_type,
            punch_info.punch_model
        );
        {
            let mut stats = self.punch_stats.lock();
            if !stats.contains_key(&peer) && stats.len() >= MAX_PUNCH_ENTRIES {
                stats.clear();
            }
            let entry = stats.entry(peer).or_default();
            entry.batch_count += 1;
            entry.last_time = now();
        }
        let stats = self
            .punch_stats
            .lock()
            .get(&peer)
            .cloned()
            .unwrap_or_default();
        let count = stats.batch_count;
        let ttl = if count < 255 { Some(count as u8) } else { None };
        let peer_nat_info = punch_info.peer_nat_info;
        let punch_model = punch_info.punch_model;

        // UDP punch
        self.punch_udp(count, &udp_buf, &peer_nat_info, &punch_model)
            .await;

        // TCP punch
        let mut tcp_tasks = tokio::task::JoinSet::new();
        let tcp_buf_owned = tcp_buf;
        if !peer_nat_info.mapping_tcp_addr.is_empty() {
            for addr in &peer_nat_info.mapping_tcp_addr {
                let buf = tcp_buf_owned.clone();
                let a = *addr;
                let puncher = self.clone();
                tcp_tasks.spawn(async move {
                    puncher
                        .connect_tcp_punch(buf, a, ttl, Duration::from_secs(3))
                        .await;
                });
            }
        }
        if punch_model.is_match(PunchPolicy::IPv4Tcp) {
            if let Some(addr) = peer_nat_info.local_ipv4_tcp() {
                let buf = tcp_buf_owned.clone();
                let puncher = self.clone();
                tcp_tasks.spawn(async move {
                    puncher
                        .connect_tcp_punch(buf, addr, ttl, Duration::from_millis(100))
                        .await;
                });
            }
            for addr in peer_nat_info.public_ipv4_tcp() {
                let buf = tcp_buf_owned.clone();
                let puncher = self.clone();
                tcp_tasks.spawn(async move {
                    puncher
                        .connect_tcp_punch(buf, addr, ttl, Duration::from_secs(3))
                        .await;
                });
            }
        }
        if punch_model.is_match(PunchPolicy::IPv6Tcp) {
            if let Some(addr) = peer_nat_info.ipv6_tcp_addr() {
                let buf = tcp_buf_owned.clone();
                let puncher = self.clone();
                tcp_tasks.spawn(async move {
                    puncher
                        .connect_tcp_punch(buf, addr, ttl, Duration::from_secs(3))
                        .await;
                });
            }
        }
        while tcp_tasks.join_next().await.is_some() {}
    }

    async fn connect_tcp_punch(
        &self,
        buf: Option<Bytes>,
        addr: SocketAddr,
        ttl: Option<u8>,
        timeout: Duration,
    ) {
        match tokio::time::timeout(timeout, self.connect_tcp_inner(addr, buf, ttl)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if self.pool.ensure_open().is_ok() => {
                log::warn!("tcp punch error: {error}");
            }
            Err(_) if self.pool.ensure_open().is_ok() => log::warn!("tcp punch timeout"),
            _ => {}
        }
    }

    fn tcp_bind_addr(&self, remote: SocketAddr, port: u16) -> io::Result<SocketAddr> {
        match remote {
            SocketAddr::V4(_) => Ok(SocketAddr::from((
                self.nat_config.bind_ipv4.unwrap_or(Ipv4Addr::UNSPECIFIED),
                port,
            ))),
            SocketAddr::V6(_) if self.nat_config.enable_ipv6 => Ok(SocketAddr::from((
                self.nat_config.bind_ipv6.unwrap_or(Ipv6Addr::UNSPECIFIED),
                port,
            ))),
            SocketAddr::V6(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "IPv6 TCP is disabled by Config::enable_ipv6",
            )),
        }
    }

    async fn punch_udp(
        &self,
        count: usize,
        buf: &[u8],
        peer_nat_info: &NatInfo,
        punch_model: &PunchModel,
    ) {
        log::debug!(
            "punch_udp count={} nat={:?} public_ips={:?} public_ports={:?} stun_ports={:?} port_range={}",
            count,
            peer_nat_info.nat_type,
            peer_nat_info.public_ips,
            peer_nat_info.public_udp_ports,
            peer_nat_info.stun_mapped_ports,
            peer_nat_info.public_port_range
        );
        let allow_v4 = punch_model.is_match(PunchPolicy::IPv4Udp);
        let allow_v6 = punch_model.is_match(PunchPolicy::IPv6Udp);
        // Send to manually configured mapping addresses
        for addr in &peer_nat_info.mapping_udp_addr {
            let allowed = if addr.is_ipv4() { allow_v4 } else { allow_v6 };
            if allowed {
                let _ = self.pool.send_to(buf, *addr);
            }
        }
        if allow_v4 {
            // Send to local addresses (same LAN)
            if !peer_nat_info.local_ipv4_addrs().is_empty() {
                let addrs = peer_nat_info.local_ipv4_addrs();
                for addr in &addrs {
                    let _ = self.pool.send_to(buf, *addr);
                }
            }

            match peer_nat_info.nat_type {
                NatType::Symmetric => {
                    let max_k1: usize = 60;
                    // Keep Phase 2 batch size constant. The need_punch() backoff
                    // already reduces punch frequency over time, so shrinking the
                    // batch as well causes a compounding slowdown that makes later
                    // rounds nearly useless (e.g., 300 ports/round at count=30).
                    let max_k2: usize = rand::rng().random_range(1200..1500);

                    let pub_ips: Vec<std::net::Ipv4Addr> = peer_nat_info.public_ips.clone();
                    if pub_ips.is_empty() {
                        log::warn!(
                            "punch_udp: Symmetric NAT peer has no public IPs — \
                             NatObserve may not have completed, cannot punch"
                        );
                        return;
                    }

                    // Phase 1: Predicted range punching.
                    //
                    // For each known port, generate a prediction window and send to
                    // random ports within that window.
                    //
                    // We combine two sources of known ports:
                    //   - `public_udp_ports`: observed by the relay (NatObserve).
                    //     This is the port the peer's main socket uses to talk to
                    //     the relay.
                    //   - `stun_mapped_ports`: discovered by STUN testing.  These
                    //     belong to a temp socket but reveal the NAT's port
                    //     allocation range.  For Symmetric NAT, the actual port
                    //     assigned for communication with us could be near either
                    //     set of ports.
                    //
                    // The port_range from STUN estimates how much the NAT varies
                    // port allocation between destinations. Keep one local window
                    // around every known port; the distance between unrelated
                    // relay and STUN mappings must not widen either window.
                    let predict_range = (peer_nat_info.public_port_range as usize * 10)
                        .max(100)
                        .min(max_k1 * 3 - 1) as u16;
                    let (all_known_ports, mut predicted_ports) =
                        symmetric_prediction_candidates(peer_nat_info, predict_range);
                    predicted_ports.shuffle(&mut rand::rng());

                    let k = max_k1.min(predicted_ports.len());
                    if k > 0 {
                        log::debug!(
                            "punch_symmetric phase 1: sending to {} predicted ports \
                             (known_ports={:?}, relay_ports={:?}, stun_ports={:?}, range=±{}, ips={:?})",
                            k,
                            all_known_ports,
                            peer_nat_info.public_udp_ports,
                            peer_nat_info.stun_mapped_ports,
                            predict_range,
                            pub_ips
                        );
                        self.punch_symmetric(&predicted_ports[..k], buf, &pub_ips, k)
                            .await;
                    }

                    // Phase 2: Global random scan — send to random ports across
                    // the full 1-65535 range.  The cursor persists across punch
                    // attempts so we don't re-scan the same ports every time.
                    // Key the scan cursor by the peer's first public IP (the
                    // port part is irrelevant). public_ipv4_addr() cannot be
                    // used as the key: it is empty while no public UDP port is
                    // known and changes as ports are observed, which lost the
                    // cursor and restarted the global scan from the beginning
                    // every round. pub_ips is guaranteed non-empty here.
                    let cursor_key = SocketAddr::V4(SocketAddrV4::new(pub_ips[0], 0));
                    let start = self
                        .port_cursor
                        .lock()
                        .get(&cursor_key)
                        .copied()
                        .unwrap_or(0);
                    let end = (start + max_k2).min(self.shuffled_ports.len());
                    log::debug!(
                        "punch_symmetric phase 2: global scan {} ports (range [{}, {}))",
                        end - start,
                        start,
                        end
                    );
                    let mut index = start
                        + self
                            .punch_symmetric(
                                &self.shuffled_ports[start..end],
                                buf,
                                &pub_ips,
                                max_k2,
                            )
                            .await;
                    if index >= self.shuffled_ports.len() {
                        index = 0;
                    }
                    let mut cursor = self.port_cursor.lock();
                    if !cursor.contains_key(&cursor_key) && cursor.len() >= MAX_PUNCH_ENTRIES {
                        cursor.clear();
                    }
                    cursor.insert(cursor_key, index);
                }
                NatType::Cone => {
                    // Send to ALL known public addresses, not just the first
                    let addrs = peer_nat_info.public_ipv4_addr();
                    if addrs.is_empty() {
                        log::warn!(
                            "punch_udp: Cone NAT peer has no public addresses — \
                             NatObserve may not have completed, cannot punch"
                        );
                    }
                    for addr in &addrs {
                        log::debug!("punch_cone: sending to {addr}");
                        self.pool.try_send_via_all(buf, *addr);
                    }
                }
            }
        }
        // IPv6 needs no NAT traversal for global addresses; probe the
        // peer's known v6 UDP addresses directly.
        if allow_v6 {
            for addr in peer_nat_info.ipv6_udp_addr() {
                log::debug!("punch_udp ipv6: sending to {addr}");
                self.pool.try_send_via_all(buf, addr);
            }
        }
    }

    async fn punch_symmetric(
        &self,
        ports: &[u16],
        buf: &[u8],
        ips: &[std::net::Ipv4Addr],
        max: usize,
    ) -> usize {
        let mut count = 0;
        for (index, port) in ports.iter().enumerate() {
            for pub_ip in ips {
                count += 1;
                if count == max {
                    return index;
                }
                let addr = SocketAddr::V4(SocketAddrV4::new(*pub_ip, *port));
                self.pool.try_send_via_all(buf, addr);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
        ports.len()
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{should_punch, symmetric_prediction_candidates, PunchStats};
    use crate::nat::NatInfo;

    #[test]
    fn punch_backoff_allows_first_nine_executed_batches() {
        for batch_count in 0..=8 {
            let stats = PunchStats {
                batch_count,
                last_time: 100,
            };
            assert!(should_punch(&stats, 100));
        }
    }

    #[test]
    fn punch_backoff_uses_elapsed_time_after_initial_batches() {
        let stats = PunchStats {
            batch_count: 16,
            last_time: 100,
        };

        assert!(!should_punch(&stats, 101));
        assert!(should_punch(&stats, 102));
        assert!(should_punch(&stats, 200));
    }

    #[test]
    fn punch_backoff_interval_is_capped() {
        let stats = PunchStats {
            batch_count: usize::MAX,
            last_time: 100,
        };

        assert!(!should_punch(&stats, 459));
        assert!(should_punch(&stats, 460));
    }

    #[test]
    fn symmetric_prediction_uses_relay_and_stun_ports_as_local_bases() {
        let info = NatInfo {
            public_udp_ports: vec![12_065],
            stun_mapped_ports: vec![22_000, 22_010],
            ..Default::default()
        };

        let (known_ports, candidates) = symmetric_prediction_candidates(&info, 100);

        assert_eq!(known_ports, vec![12_065, 22_000, 22_010]);
        assert!(candidates.contains(&11_965));
        assert!(candidates.contains(&12_165));
        assert!(candidates.contains(&21_900));
        assert!(candidates.contains(&22_110));
        assert!(!candidates.contains(&17_000));
    }

    #[test]
    fn symmetric_prediction_filters_zero_and_clamps_port_bounds() {
        let info = NatInfo {
            public_udp_ports: vec![0, 20],
            stun_mapped_ports: vec![65_530],
            ..Default::default()
        };

        let (known_ports, candidates) = symmetric_prediction_candidates(&info, 100);

        assert_eq!(known_ports, vec![20, 65_530]);
        assert_eq!(candidates.first(), Some(&1));
        assert_eq!(candidates.last(), Some(&65_535));
        assert!(!candidates.contains(&0));
    }
}
