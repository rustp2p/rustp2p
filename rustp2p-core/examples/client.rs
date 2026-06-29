use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use clap::Parser;
use env_logger::Env;
use parking_lot::Mutex;
use rust_p2p_core::endpoint::{Config, EndPoint, Sender};
use rust_p2p_core::idle::IdleRouteManager;
use rust_p2p_core::nat::NatInfo;
use rust_p2p_core::punch::{PunchInfo, PunchModel, Puncher};
use rust_p2p_core::route_table::route_table::RouteTable;
use rust_p2p_core::route_table::RouteKey;

/*Demo Protocol
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                     protocol_type(32)                                       |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          src_id(32)                                         |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          dest_id(32)                                        |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                           data(n)                                           |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
*/
pub const HEAD_LEN: usize = 12;
pub const UP: u32 = 1;
pub const PUSH_PEER_LIST: u32 = 2;
pub const PUNCH_START_1: u32 = 3;
pub const PUNCH_START_2: u32 = 4;
pub const PUNCH_REQ: u32 = 5;
pub const PUNCH_RES: u32 = 6;
pub const PUBLIC_ADDR_REQ: u32 = 7;
pub const PUBLIC_ADDR_RES: u32 = 8;
pub const MY_SERVER_ID: u32 = 0;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    tcp: bool,
    #[arg(short, long)]
    server: SocketAddr,
    #[arg(short, long)]
    id: u32,
}

#[tokio::main]
async fn main() {
    let Args {
        server,
        id: my_id,
        tcp: _,
    } = Args::parse();
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    log::info!("my_id:{my_id},server:{server}");

    let mut ep = EndPoint::bind(Config::new().udp_port(0).tcp_port(0))
        .await
        .unwrap();
    let sender = ep.sender();
    let puncher = ep.puncher();
    let route_table: RouteTable<u32> = RouteTable::default();
    let idle_route_manager = IdleRouteManager::new(Duration::from_secs(12), route_table.clone());

    // Get NAT info using endpoint helpers
    let nat_info = my_nat_info(&ep).await;

    // Register with server
    {
        let mut request = BytesMut::new();
        request.put_u32(UP);
        request.put_u32(my_id);
        request.put_u32(MY_SERVER_ID);
        sender.try_send_via_all(request.freeze().as_ref(), server);
    }

    let peer_list = Arc::new(Mutex::new(Vec::<u32>::new()));
    let peer_list1 = peer_list.clone();
    let sender1 = sender.clone();
    let nat_info1 = nat_info.clone();

    // Idle route cleanup
    tokio::spawn(async move {
        loop {
            let (peer_id, route, time) = idle_route_manager.next_idle().await;
            log::info!(
                "route timeout peer_id={peer_id},route={route:?},time={:?}",
                time.elapsed()
            );
            idle_route_manager.remove_route(&peer_id, &route.route_key());
        }
    });

    // Periodic punch initiation
    let route_table1 = route_table.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            {
                let peer_list = peer_list1.lock().clone();
                for peer_id in peer_list {
                    if peer_id <= my_id {
                        continue;
                    }
                    if !route_table1.need_punch(&peer_id) {
                        continue;
                    }
                    let mut request = BytesMut::new();
                    request.put_u32(PUNCH_START_1);
                    request.put_u32(my_id);
                    request.put_u32(peer_id);
                    let nat_info = nat_info1.lock().clone();
                    let data = serde_json::to_string(&nat_info).unwrap();
                    request.extend_from_slice(data.as_bytes());
                    sender1.try_send_via_all(request.freeze().as_ref(), server);
                }
            }
        }
    });

    // Periodic public address request
    let sender2 = sender.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let mut request = BytesMut::new();
            request.put_u32(PUBLIC_ADDR_REQ);
            request.put_u32(my_id);
            request.put_u32(MY_SERVER_ID);
            sender2.try_send_via_all(request.freeze().as_ref(), server);
        }
    });

    let context_handler = ContextHandler {
        my_id,
        peer_list,
        puncher,
        nat_info,
        route_table: route_table.clone(),
        server,
        sender,
    };

    // Handle incoming messages
    loop {
        let received = match ep.recv().await {
            Some(r) => r,
            None => break,
        };
        let context_handler = context_handler.clone();
        tokio::spawn(async move {
            let _ = context_handler.handle(received).await;
        });
    }
}

#[derive(Clone)]
struct ContextHandler {
    my_id: u32,
    peer_list: Arc<Mutex<Vec<u32>>>,
    puncher: Puncher,
    nat_info: Arc<Mutex<NatInfo>>,
    route_table: RouteTable<u32>,
    #[allow(dead_code)]
    server: SocketAddr,
    sender: Sender,
}

impl ContextHandler {
    async fn handle(&self, received: rust_p2p_core::endpoint::Received) -> std::io::Result<()> {
        let data = &received.data;
        let addr = received.transport.remote_addr();

        if data.len() < HEAD_LEN {
            log::warn!("invalid protocol {:?},addr={addr:?}", &data[..]);
            return Ok(());
        }

        let protocol_type: u32 = u32::from_be_bytes(data[0..4].try_into().unwrap());
        let src_id: u32 = u32::from_be_bytes(data[4..8].try_into().unwrap());
        let dest_id: u32 = u32::from_be_bytes(data[8..12].try_into().unwrap());
        log::info!(
            "recv_from {:?},type={protocol_type},src_id={src_id},peer_id={dest_id},addr={addr:?}",
            &data[..]
        );

        match protocol_type {
            PUSH_PEER_LIST => {
                let mut guard = self.peer_list.lock();
                *guard = serde_json::from_str(core::str::from_utf8(&data[12..]).unwrap()).unwrap();
                log::info!("peer_list={guard:?}");
            }
            PUNCH_START_1 => {
                let peer_nat_info: NatInfo =
                    serde_json::from_str(core::str::from_utf8(&data[12..]).unwrap()).unwrap();
                log::info!("peer_id={src_id},peer_nat_info={peer_nat_info:?}");

                // Reply to server with our NAT info (server will relay to peer)
                let mut request = BytesMut::new();
                request.put_u32(PUNCH_START_2);
                request.put_u32(self.my_id);
                request.put_u32(src_id);
                let nat_info = self.nat_info.lock().clone();
                let nat_data = serde_json::to_string(&nat_info).unwrap();
                request.extend_from_slice(nat_data.as_bytes());
                received
                    .transport
                    .send(request.freeze().as_ref())
                    .await
                    .ok();

                // Start punching to the peer
                {
                    let mut request = BytesMut::new();
                    request.put_u32(PUNCH_REQ);
                    request.put_u32(self.my_id);
                    request.put_u32(src_id);
                    let puncher = self.puncher.clone();
                    tokio::spawn(async move {
                        let rs = puncher
                            .punch(&request, PunchInfo::new(PunchModel::all(), peer_nat_info))
                            .await;
                        log::info!("punch peer_id={src_id},{rs:?}")
                    });
                }
            }
            PUNCH_START_2 => {
                let peer_nat_info: NatInfo =
                    serde_json::from_str(core::str::from_utf8(&data[12..]).unwrap()).unwrap();
                log::info!("peer_id={src_id},peer_nat_info={peer_nat_info:?}");
                let mut request = BytesMut::new();
                request.put_u32(PUNCH_REQ);
                request.put_u32(self.my_id);
                request.put_u32(src_id);
                let puncher = self.puncher.clone();
                tokio::spawn(async move {
                    let rs = puncher
                        .punch(&request, PunchInfo::new(PunchModel::all(), peer_nat_info))
                        .await;
                    log::info!("punch peer_id={src_id},{rs:?}")
                });
            }
            PUNCH_REQ => {
                log::info!("======================== PUNCH_REQ ========================");
                // Reply to peer with PUNCH_RES
                let mut request = BytesMut::new();
                request.put_u32(PUNCH_RES);
                request.put_u32(self.my_id);
                request.put_u32(src_id);
                received
                    .transport
                    .send(request.freeze().as_ref())
                    .await
                    .ok();
                // Add direct route to peer
                self.route_table
                    .add_route(src_id, (RouteKey::from_transport(&received.transport), 0));
            }
            PUNCH_RES => {
                log::info!("======================== PUNCH_RES ========================");
                // Punch succeeded, add direct route (metric=0)
                self.route_table
                    .add_route(src_id, (RouteKey::from_transport(&received.transport), 0));
            }
            PUBLIC_ADDR_RES => {
                let public_addr =
                    SocketAddr::from_str(core::str::from_utf8(&data[12..]).unwrap()).unwrap();
                log::info!("public_addr={public_addr}");
                let mut guard = self.nat_info.lock();
                if let Some(port) = guard.public_udp_ports.get_mut(0) {
                    *port = public_addr.port();
                }
            }
            _ => {
                log::warn!(
                    "invalid protocol {:?},src_id={src_id},peer_id={dest_id},addr={addr:?}",
                    &data[..]
                );
            }
        }
        Ok(())
    }
}

async fn my_nat_info(ep: &EndPoint) -> Arc<Mutex<NatInfo>> {
    let stun_server = vec![
        "stun.miwifi.com:3478".to_string(),
        "stun.chat.bilibili.com:3478".to_string(),
        "stun.hitv.com:3478".to_string(),
    ];
    let (nat_type, public_ips, port_range) = rust_p2p_core::stun::stun_test_nat(stun_server, None)
        .await
        .unwrap();
    log::info!("nat_type:{nat_type:?},public_ips:{public_ips:?},port_range={port_range}");

    let local_ipv4 = rust_p2p_core::util::addr::local_ipv4()
        .await
        .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED);

    let local_udp_ports = ep.local_udp_ports().await;
    let local_tcp_port = ep.local_tcp_port().await;

    let mut public_ports = local_udp_ports.clone();
    public_ports.fill(0);

    let nat_info = NatInfo {
        nat_type,
        public_ips,
        public_udp_ports: public_ports,
        mapping_tcp_addr: vec![],
        mapping_udp_addr: vec![],
        public_port_range: port_range,
        local_ipv4,
        local_ipv4s: vec![],
        ipv6: None,
        local_udp_ports,
        local_tcp_port,
        public_tcp_port: 0,
    };
    Arc::new(Mutex::new(nat_info))
}
