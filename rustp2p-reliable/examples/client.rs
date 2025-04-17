/*Demo Protocol
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                     protocol_type(32)                                       |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                      json data(n)                                           |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
*/
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use clap::Parser;
use env_logger::Env;
use parking_lot::Mutex;
use rustp2p_reliable::LengthPrefixedInitCodec;
use rustp2p_reliable::NatInfo;
use rustp2p_reliable::{Config, Puncher};
use rustp2p_reliable::{PunchInfo, PunchModelIntersect};
use rustp2p_reliable::{TcpTunnelConfig, TunnelConfig, UdpTunnelConfig};
use serde::Deserialize;
use serde_json::Value;
use tokio::net::UdpSocket;

pub const HEAD_LEN: usize = 1;
//
pub const UP: u32 = 1;
// push peer list
pub const PUSH_PEER_LIST: u32 = 2;
// Initiate NAT penetration towards the target
pub const PUNCH_START_1: u32 = 3;
pub const PUNCH_START_2: u32 = 4;
pub const PUNCH_REQ: u32 = 5;
pub const PUNCH_RES: u32 = 6;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    server: SocketAddr,
}

#[tokio::main]
async fn main() {
    let Args { server } = Args::parse();
    env_logger::Builder::from_env(Env::default().default_filter_or("debug")).init();

    log::info!("server:{server}");
    let udp_config = UdpTunnelConfig::default();
    let tcp_config = TcpTunnelConfig::new(Box::new(LengthPrefixedInitCodec));
    let config = TunnelConfig::empty()
        .set_udp_tunnel_config(udp_config)
        .set_tcp_tunnel_config(tcp_config)
        .set_tcp_multi_count(2);
    let config = Config::new(config);
    let (mut listener, puncher) = rustp2p_reliable::from_config(config).await.unwrap();
    tokio::spawn(async move {
        loop {
            let reliable_tunnel = listener.accept().await.unwrap();
            log::info!(
                "========= accept local:{}, remote:{} {:?}",
                reliable_tunnel.local_addr(),
                reliable_tunnel.remote_addr(),
                reliable_tunnel.tunnel_type()
            );
            let reliable_tunnel = Arc::new(reliable_tunnel);
            let reliable_tunnel1 = reliable_tunnel.clone();
            tokio::spawn(async move {
                while let Ok(buf) = reliable_tunnel1.next().await {
                    log::info!(
                        "========= recv :{buf:?}, local:{}, remote:{} {:?}",
                        reliable_tunnel1.local_addr(),
                        reliable_tunnel1.remote_addr(),
                        reliable_tunnel1.tunnel_type()
                    );
                }
                log::info!(
                    "========= recv end:{}-{} {:?}",
                    reliable_tunnel1.local_addr(),
                    reliable_tunnel1.remote_addr(),
                    reliable_tunnel1.tunnel_type()
                );
            });
            tokio::spawn(async move {
                let mut count = 1;
                while reliable_tunnel
                    .send(format!("hello-{count}").as_bytes().into())
                    .await
                    .is_ok()
                {
                    log::info!(
                        "========= send 'hello-{count}', local:{}, remote:{} {:?}",
                        reliable_tunnel.local_addr(),
                        reliable_tunnel.remote_addr(),
                        reliable_tunnel.tunnel_type()
                    );
                    count += 1;
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                log::info!(
                    "========= send end:{}-{} {:?}",
                    reliable_tunnel.local_addr(),
                    reliable_tunnel.remote_addr(),
                    reliable_tunnel.tunnel_type()
                );
            });
        }
    });
    let udp = Arc::new(UdpSocket::bind("0.0.0.0:0").await.unwrap());
    udp.connect(server).await.unwrap();
    {
        let mut request = BytesMut::new();
        request.put_u32(UP);
        udp.send(&request).await.unwrap();
    }
    let peer_list = Arc::new(Mutex::new(Vec::<SocketAddr>::new()));
    let peer_list1 = peer_list.clone();
    let puncher1 = puncher.clone();
    let udp1 = udp.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            {
                let peer_list = peer_list1.lock().clone();
                log::info!("punch request {peer_list:?}");
                let nat_info = puncher1.nat_info();
                log::info!("nat_info {nat_info:?}");
                for peer_addr in peer_list {
                    // Initiate NAT penetration
                    let mut request = BytesMut::new();
                    request.put_u32(PUNCH_START_1);
                    let nat_info = puncher1.nat_info();
                    let val = serde_json::json!({
                        "to":peer_addr,
                        "nat_info":nat_info
                    });
                    let data = serde_json::to_vec(&val).unwrap();
                    request.extend_from_slice(&data);
                    udp1.send(&request).await.unwrap();
                }
            }
        }
    });

    let handler = ContextHandler::new(peer_list, puncher, udp);
    handler.handle().await.unwrap()
}

struct ContextHandler {
    peer_list: Arc<Mutex<Vec<SocketAddr>>>,
    puncher: Puncher,
    udp: Arc<UdpSocket>,
}

impl ContextHandler {
    fn new(peer_list: Arc<Mutex<Vec<SocketAddr>>>, puncher: Puncher, udp: Arc<UdpSocket>) -> Self {
        Self {
            peer_list,
            puncher,
            udp,
        }
    }
    async fn handle(&self) -> std::io::Result<()> {
        let mut buf = [0; 65536];
        loop {
            let (len, addr) = self.udp.recv_from(&mut buf).await?;
            if len < HEAD_LEN {
                log::warn!("invalid protocol {:?},{addr:?}", &buf[..len]);
            }
            let protocol_type: u32 = u32::from_be_bytes(buf[0..4].try_into().unwrap());
            let body = &buf[4..len];
            log::info!(
                "recv_from {:?},type={protocol_type},addr={addr:?}",
                &buf[..len]
            );
            match protocol_type {
                PUSH_PEER_LIST => {
                    let mut guard = self.peer_list.lock();
                    *guard = serde_json::from_slice(body).unwrap();
                    log::info!("peer_list={guard:?}");
                }
                PUNCH_START_1 | PUNCH_START_2 => {
                    let map: HashMap<String, Value> = serde_json::from_slice(body).unwrap();
                    let peer_nat_info: NatInfo =
                        serde_json::from_value(map.get("nat_info").unwrap().clone()).unwrap();
                    let peer_addr = map.get("from").unwrap();
                    let peer_addr = SocketAddr::deserialize(peer_addr).unwrap();
                    log::info!("peer_addr={peer_addr},peer_nat_info={peer_nat_info:?}");

                    if protocol_type == PUNCH_START_1 {
                        let nat_info = self.puncher.nat_info();
                        let val = serde_json::json!({
                            "to":peer_addr,
                            "nat_info":nat_info
                        });
                        let data = serde_json::to_vec(&val).unwrap();
                        let mut request = BytesMut::new();
                        request.put_u32(PUNCH_START_2);
                        request.extend_from_slice(&data);
                        self.udp.send(&request).await.unwrap();
                    }

                    let puncher = self.puncher.clone();
                    tokio::spawn(async move {
                        let rs = puncher
                            .punch(PunchInfo::new(PunchModelIntersect::all(), peer_nat_info))
                            .await;
                        log::info!("punch peer_addr={peer_addr},{rs:?}")
                    });
                }
                _ => {
                    log::warn!("invalid protocol {:?},addr={addr:?}", body);
                }
            }
        }
    }
}
