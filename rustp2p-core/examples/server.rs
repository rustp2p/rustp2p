use bytes::{BufMut, BytesMut};
use env_logger::Env;

use rust_p2p_core::route::route_table::RouteTable;
use rust_p2p_core::tunnel::config::{TcpTunnelConfig, TunnelConfig, UdpTunnelConfig};
use rust_p2p_core::tunnel::tcp::LengthPrefixedInitCodec;
use rust_p2p_core::tunnel::{new_tunnel_component, UnifiedSocketManager, UnifiedTunnel};

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
#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let udp_config = UdpTunnelConfig::default().set_simple_udp_port(3000);
    let tcp_config = TcpTunnelConfig::new(Box::new(LengthPrefixedInitCodec)).set_tcp_port(3000);
    let config = TunnelConfig::empty()
        .set_tcp_multi_count(1)
        .set_tcp_tunnel_config(tcp_config)
        .set_udp_tunnel_config(udp_config);
    let (mut tunnel_factory, _puncher) = new_tunnel_component(config).unwrap();
    let writer = tunnel_factory.socket_manager();
    let route_table = RouteTable::default();

    log::info!("listen 3000");
    loop {
        let tunnel = tunnel_factory.dispatch().await.unwrap();
        let table = route_table.clone();
        let writer = writer.clone();
        tokio::spawn(async move {
            handler(table, tunnel, writer).await;
        });
    }
}
async fn handler(
    route_table: RouteTable<u32>,
    mut tunnel: UnifiedTunnel,
    writer: UnifiedSocketManager,
) {
    let mut buf = [0; 65536];
    while let Some(rs) = tunnel.recv_from(&mut buf).await {
        let (len, route_key) = match rs {
            Ok(rs) => rs,
            Err(e) => {
                log::error!("err {e:?}");
                if tunnel.protocol().is_udp() {
                    continue;
                }
                break;
            }
        };
        if len < HEAD_LEN {
            log::warn!("invalid protocol {:?},{route_key:?}", &buf[..len]);
            continue;
        }
        let protocol_type: u32 = u32::from_be_bytes(buf[0..4].try_into().unwrap());
        let src_id: u32 = u32::from_be_bytes(buf[4..8].try_into().unwrap());
        let dest_id: u32 = u32::from_be_bytes(buf[8..12].try_into().unwrap());
        log::info!(
            "recv_from {:?},type={protocol_type},src_id={src_id},peer_id={dest_id},addr={route_key:?}",
            &buf[..len]
        );
        match protocol_type {
            UP => {
                route_table.remove_all(&src_id);
                route_table.add_route(src_id, (route_key, 1));
                let vec = route_table.route_table_one();
                let peer_ids: Vec<u32> = vec.iter().map(|(k, _)| *k).collect();
                for (peer_id, peer_route) in route_table.route_table_one() {
                    let mut response = BytesMut::new();
                    response.put_u32(PUSH_PEER_LIST);
                    response.put_u32(MY_SERVER_ID);
                    response.put_u32(peer_id);
                    let json = serde_json::to_string(
                        &peer_ids
                            .iter()
                            .filter(|k| **k != peer_id)
                            .copied()
                            .collect::<Vec<u32>>(),
                    )
                    .unwrap();
                    response.extend_from_slice(json.as_bytes());
                    writer
                        .send_to(response, &peer_route.route_key())
                        .await
                        .unwrap();
                }
            }
            PUNCH_START_1 | PUNCH_START_2 => match route_table.get_route_by_id(&dest_id) {
                Ok(route) => {
                    if let Err(e) = writer
                        .send_to((&buf[..len]).into(), &route.route_key())
                        .await
                    {
                        log::warn!(
                            "{:?},src_id={src_id},peer_id={dest_id},addr={route_key:?},{e:?}",
                            &buf[..len]
                        );
                    }
                }
                Err(e) => {
                    log::warn!(
                        "{:?},src_id={src_id},peer_id={dest_id},addr={route_key:?},{e:?}",
                        &buf[..len]
                    );
                }
            },
            PUBLIC_ADDR_REQ => {
                let mut response = BytesMut::new();
                response.put_u32(PUBLIC_ADDR_RES);
                response.put_u32(MY_SERVER_ID);
                response.put_u32(src_id);
                response.extend_from_slice(route_key.addr().to_string().as_bytes());
                if let Err(e) = writer.send_to(response, &route_key).await {
                    log::warn!(
                        "{:?},src_id={src_id},peer_id={dest_id},addr={route_key:?},{e:?}",
                        &buf[..len]
                    );
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
}
