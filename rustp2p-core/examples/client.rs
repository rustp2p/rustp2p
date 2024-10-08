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
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use clap::Parser;
use env_logger::Env;
use parking_lot::Mutex;

use rust_p2p_core::nat::NatInfo;
use rust_p2p_core::pipe::config::{PipeConfig, TcpPipeConfig, UdpPipeConfig};
use rust_p2p_core::pipe::tcp_pipe::LengthPrefixedInitCodec;
use rust_p2p_core::pipe::{pipe, PipeLine, PipeWriter};
use rust_p2p_core::punch::{PunchInfo, PunchModelBoxes, Puncher};
use rust_p2p_core::route::route_table::RouteTable;
use rust_p2p_core::route::ConnectProtocol;

pub const HEAD_LEN: usize = 12;
//
pub const UP: u32 = 1;
// push peer list
pub const PUSH_PEER_LIST: u32 = 2;
// Initiate NAT penetration towards the target
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
        tcp,
    } = Args::parse();
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let connect_protocol = if tcp {
        ConnectProtocol::TCP
    } else {
        ConnectProtocol::UDP
    };
    log::info!("my_id:{my_id},server:{server}");
    let udp_config = UdpPipeConfig::default();
    let tcp_config = TcpPipeConfig::new(Box::new(LengthPrefixedInitCodec));
    let config = PipeConfig::empty()
        .set_udp_pipe_config(udp_config)
        .set_tcp_pipe_config(tcp_config)
        .set_main_pipeline_num(2);
    let (mut pipe, puncher, idle_route_manager) = pipe(config).unwrap();
    let pipe_writer = pipe.writer_ref().to_owned();
    let nat_info = my_nat_info(&pipe_writer).await;
    {
        let mut request = BytesMut::new();
        request.put_u32(UP);
        request.put_u32(my_id);
        request.put_u32(MY_SERVER_ID);
        pipe_writer
            .send_to_addr(connect_protocol, request, server)
            .await
            .unwrap();
    }
    let peer_list = Arc::new(Mutex::new(Vec::<u32>::new()));
    let peer_list1 = peer_list.clone();
    let puncher1 = puncher.clone();
    let pipe_writer1 = pipe_writer.clone();
    let nat_info1 = nat_info.clone();
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
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            {
                let peer_list = peer_list1.lock().clone();
                for peer_id in peer_list {
                    if peer_id <= my_id {
                        continue;
                    }
                    if puncher1.need_punch(&peer_id) {
                        // Initiate NAT penetration
                        let mut request = BytesMut::new();
                        request.put_u32(PUNCH_START_1);
                        request.put_u32(my_id);
                        request.put_u32(peer_id);
                        let mut nat_info = nat_info1.lock().clone();
                        nat_info.seq = rand::random();
                        let data = serde_json::to_string(&nat_info).unwrap();
                        request.extend_from_slice(data.as_bytes());
                        pipe_writer1
                            .send_to_addr(connect_protocol, request, server)
                            .await
                            .unwrap();
                    }
                }
            }
        }
    });
    let pipe_writer2 = pipe_writer.clone();
    tokio::spawn(async move {
        // Obtain public network address
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let mut request = BytesMut::new();
            request.put_u32(PUBLIC_ADDR_REQ);
            request.put_u32(my_id);
            request.put_u32(MY_SERVER_ID);
            pipe_writer2
                .send_to_addr(connect_protocol, request, server)
                .await
                .unwrap();
        }
    });
    let context_handler = ContextHandler {
        my_id,
        peer_list,
        puncher,
        nat_info,
        route_table: pipe.route_table().clone(),
        server,
        pipe_writer,
    };
    loop {
        let pipe_line = pipe.accept().await.unwrap();
        let context_handler = context_handler.clone();
        tokio::spawn(async move {
            let _ = context_handler.handle(pipe_line).await;
        });
    }
}

#[derive(Clone)]
struct ContextHandler {
    my_id: u32,
    peer_list: Arc<Mutex<Vec<u32>>>,
    puncher: Puncher<u32>,
    nat_info: Arc<Mutex<NatInfo>>,
    route_table: RouteTable<u32>,
    #[allow(dead_code)]
    server: SocketAddr,
    pipe_writer: PipeWriter<u32>,
}

impl ContextHandler {
    async fn handle(&self, mut pipe_line: PipeLine) -> anyhow::Result<()> {
        let mut buf = [0; 65536];
        while let Some(rs) = pipe_line.recv_from(&mut buf).await {
            let (len, route_key) = match rs {
                Ok(rs) => rs,
                Err(e) => {
                    log::warn!("{e:?}");
                    if pipe_line.protocol().is_udp() {
                        continue;
                    }
                    break;
                }
            };

            if len < HEAD_LEN {
                log::warn!("invalid protocol {:?},{route_key:?}", &buf[..len]);
            }
            let protocol_type: u32 = u32::from_be_bytes(buf[0..4].try_into().unwrap());
            let src_id: u32 = u32::from_be_bytes(buf[4..8].try_into().unwrap());
            let dest_id: u32 = u32::from_be_bytes(buf[8..12].try_into().unwrap());
            log::info!("recv_from {:?},type={protocol_type},src_id={src_id},peer_id={dest_id},addr={route_key:?}",&buf[..len]);
            match protocol_type {
                PUSH_PEER_LIST => {
                    let mut guard = self.peer_list.lock();
                    *guard =
                        serde_json::from_str(&String::from_utf8(buf[12..len].to_vec()).unwrap())
                            .unwrap();
                    log::info!("peer_list={guard:?}");
                }
                PUNCH_START_1 => {
                    let mut request = BytesMut::new();
                    request.put_u32(PUNCH_START_2);
                    request.put_u32(self.my_id);
                    request.put_u32(src_id);
                    let peer_nat_info: NatInfo =
                        serde_json::from_str(&String::from_utf8(buf[12..len].to_vec()).unwrap())
                            .unwrap();
                    log::info!("peer_id={src_id},peer_nat_info={peer_nat_info:?}");
                    let mut nat_info = self.nat_info.lock().clone();
                    nat_info.seq = peer_nat_info.seq;
                    let data = serde_json::to_string(&nat_info).unwrap();
                    request.extend_from_slice(data.as_bytes());
                    self.pipe_writer.send_to(request, &route_key).await.unwrap();

                    {
                        let mut request = BytesMut::new();
                        request.put_u32(PUNCH_REQ);
                        request.put_u32(self.my_id);
                        request.put_u32(src_id);
                        let puncher = self.puncher.clone();
                        tokio::spawn(async move {
                            let rs = puncher
                                .punch(
                                    src_id,
                                    &request,
                                    PunchInfo::new_by_other(PunchModelBoxes::all(), peer_nat_info),
                                )
                                .await;
                            log::info!("punch peer_id={src_id},{rs:?}")
                        });
                    }
                }
                PUNCH_START_2 => {
                    let peer_nat_info: NatInfo =
                        serde_json::from_str(&String::from_utf8(buf[12..len].to_vec()).unwrap())
                            .unwrap();
                    log::info!("peer_id={src_id},peer_nat_info={peer_nat_info:?}");
                    let mut request = BytesMut::new();
                    request.put_u32(PUNCH_REQ);
                    request.put_u32(self.my_id);
                    request.put_u32(src_id);
                    let puncher = self.puncher.clone();
                    tokio::spawn(async move {
                        let rs = puncher
                            .punch(
                                src_id,
                                &request,
                                PunchInfo::new_by_oneself(PunchModelBoxes::all(), peer_nat_info),
                            )
                            .await;
                        log::info!("punch peer_id={src_id},{rs:?}")
                    });
                }
                PUNCH_REQ => {
                    let protocol = route_key.protocol();
                    log::info!(
                        "======================== PUNCH_REQ ({protocol:?}) ========================"
                    );
                    let mut request = BytesMut::new();
                    request.put_u32(PUNCH_RES);
                    request.put_u32(self.my_id);
                    request.put_u32(src_id);
                    self.pipe_writer.send_to(request, &route_key).await.unwrap();
                    self.route_table.add_route(src_id, (route_key, 1));
                }
                PUNCH_RES => {
                    let protocol = route_key.protocol();
                    log::info!(
                        "======================== PUNCH_RES ({protocol:?}) ========================"
                    );
                    self.route_table.add_route(src_id, (route_key, 1));
                }
                PUBLIC_ADDR_RES => {
                    let public_addr =
                        SocketAddr::from_str(&String::from_utf8(buf[12..len].to_vec()).unwrap())
                            .unwrap();
                    log::info!("public_addr={public_addr}");
                    let mut guard = self.nat_info.lock();
                    if let Some(port) = guard.public_ports.get_mut(route_key.index_usize()) {
                        *port = public_addr.port();
                    }
                }
                _ => {
                    log::warn!(
					"invalid protocol {:?},src_id={src_id},peer_id={dest_id},addr={route_key:?}",
					&buf[..len]
				);
                }
            }
        }
        log::info!("pipe_line done");
        Ok(())
    }
}

async fn my_nat_info(pipe_writer: &PipeWriter<u32>) -> Arc<Mutex<NatInfo>> {
    let stun_server = vec![
        "stun.miwifi.com:3478".to_string(),
        "stun.chat.bilibili.com:3478".to_string(),
        "stun.hitv.com:3478".to_string(),
    ];
    let (nat_type, public_ips, port_range) = rust_p2p_core::stun::stun_test_nat(stun_server, None)
        .await
        .unwrap();
    log::info!("nat_type:{nat_type:?},public_ips:{public_ips:?},port_range={port_range}");
    let local_ipv4 = rust_p2p_core::extend::addr::local_ipv4().await.unwrap();
    let local_udp_ports = pipe_writer
        .udp_pipe_writer()
        .unwrap()
        .local_ports()
        .unwrap();
    let local_tcp_port = pipe_writer.tcp_pipe_writer().unwrap().local_addr().port();
    let mut public_ports = local_udp_ports.clone();
    public_ports.fill(0);
    let nat_info = NatInfo {
        nat_type,
        public_ips,
        public_ports,
        mapping_tcp_addr: vec![],
        mapping_udp_addr: vec![],
        public_port_range: port_range,
        local_ipv4,
        ipv6: None,
        local_udp_ports,
        local_tcp_port,
        public_tcp_port: 0,
        seq: 0,
    };
    Arc::new(Mutex::new(nat_info))
}
