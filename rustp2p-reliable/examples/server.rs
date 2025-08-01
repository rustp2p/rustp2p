use bytes::{BufMut, BytesMut};
use env_logger::Env;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
/*Demo Protocol
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                     protocol_type(32)                                       |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          json data                                          |
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
#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let udp = UdpSocket::bind("0.0.0.0:23333").await?;

    log::info!("listen 23333");
    let mut set = HashSet::new();
    let mut buf = [0; 65536];
    loop {
        let (len, addr) = udp.recv_from(&mut buf).await?;
        set.insert(addr);
        handle(addr, &set, &udp, &buf[..len]).await;
    }
}
async fn handle(addr: SocketAddr, set: &HashSet<SocketAddr>, udp: &UdpSocket, buf: &[u8]) {
    let protocol_type: u32 = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    log::info!("recv_from {:?},type={protocol_type},addr={addr:?}", buf);
    let body = &buf[4..];
    match protocol_type {
        UP => {
            for key in set.iter() {
                let mut response = BytesMut::new();
                response.put_u32(PUSH_PEER_LIST);
                let json = serde_json::to_vec(
                    &set.iter()
                        .filter(|k| *k != key)
                        .copied()
                        .collect::<Vec<SocketAddr>>(),
                )
                .unwrap();
                response.extend_from_slice(&json);
                _ = udp.send_to(&response, key).await;
            }
        }
        PUNCH_START_1 | PUNCH_START_2 => {
            let mut map: HashMap<String, Value> = serde_json::from_slice(body).unwrap();
            map.insert("from".into(), Value::String(addr.to_string()));
            log::info!("PUNCH_START {map:?}");
            let to = map.get("to").unwrap();
            let to_addr = to.as_str().unwrap();
            let data = serde_json::to_vec(&map).unwrap();
            let mut response = BytesMut::new();
            response.put_u32(protocol_type);
            response.extend_from_slice(&data);
            _ = udp.send_to(&response, to_addr).await;
        }
        _ => {
            log::warn!("invalid protocol {:?},addr={addr:?}", &buf);
        }
    }
}
