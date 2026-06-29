use bytes::{BufMut, BytesMut};
use env_logger::Env;
use rust_p2p_core::endpoint::{Config, EndPoint, SocketPool};
use rust_p2p_core::route_table::route_table::RouteTable;
use rust_p2p_core::route_table::{Index, RouteKey, UDPIndex};
use std::sync::Arc;

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

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let mut ep = EndPoint::bind(Config::new().udp_port(3000).tcp_port(3000))
        .await
        .unwrap();

    let route_table: RouteTable<u32> = RouteTable::default();
    let pool = ep.pool().clone();

    log::info!("Server listening on {:?}", ep.local_addr().await);

    while let Some(received) = ep.recv().await {
        let route_table = route_table.clone();
        let pool = pool.clone();
        tokio::spawn(async move {
            handler(route_table, received, pool).await;
        });
    }
}

async fn handler(
    route_table: RouteTable<u32>,
    received: rust_p2p_core::endpoint::Received,
    pool: Arc<SocketPool>,
) {
    let data = &received.data;
    let addr = received.transport.remote_addr();
    let route_key = RouteKey::new(Index::Udp(UDPIndex::MainV4(0)), addr);

    if data.len() < HEAD_LEN {
        log::warn!("invalid protocol {:?}", &data[..]);
        return;
    }

    let protocol_type: u32 = u32::from_be_bytes(data[0..4].try_into().unwrap());
    let src_id: u32 = u32::from_be_bytes(data[4..8].try_into().unwrap());
    let dest_id: u32 = u32::from_be_bytes(data[8..12].try_into().unwrap());
    log::info!(
        "recv_from {:?},type={protocol_type},src_id={src_id},peer_id={dest_id},addr={route_key:?}",
        &data[..]
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
                let peer_addr = peer_route.route_key().addr();
                pool.try_send_via_all(response.freeze().as_ref(), peer_addr);
            }
        }
        PUNCH_START_1 | PUNCH_START_2 => match route_table.get_route_by_id(&dest_id) {
            Ok(route) => {
                let peer_addr = route.route_key().addr();
                if let Err(e) = received.transport.send_to(data.as_ref(), peer_addr).await {
                    log::warn!("PUNCH_START send error: {e:?},src_id={src_id},peer_id={dest_id}");
                }
            }
            Err(e) => {
                log::warn!(
                    "PUNCH_START error: {e:?},src_id={src_id},peer_id={dest_id},addr={route_key:?}"
                );
            }
        },
        PUBLIC_ADDR_REQ => {
            let mut response = BytesMut::new();
            response.put_u32(PUBLIC_ADDR_RES);
            response.put_u32(MY_SERVER_ID);
            response.put_u32(src_id);
            response.extend_from_slice(addr.to_string().as_bytes());
            pool.try_send_via_all(response.freeze().as_ref(), addr);
        }
        _ => {
            log::warn!(
                "invalid protocol {:?},src_id={src_id},peer_id={dest_id},addr={route_key:?}",
                &data[..]
            );
        }
    }
}
