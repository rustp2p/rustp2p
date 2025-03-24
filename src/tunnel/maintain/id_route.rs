use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::NetPacket;
use crate::tunnel::node_context::DirectNodes;
use crate::tunnel::{NodeAddress, TunnelTransmitHub};
use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::io;
use std::time::Duration;

pub async fn id_route_query_loop(
    tunnel_tx: TunnelTransmitHub,
    query_id_interval: Duration,
    query_id_max_num: usize,
) {
    loop {
        if let Err(e) = id_route_query(&tunnel_tx, query_id_max_num).await {
            log::warn!("poll_peer_node, e={e:?}");
        }
        tokio::time::sleep(query_id_interval).await;
        tunnel_tx
            .node_context()
            .clear_timeout_reachable_nodes(query_id_interval);
    }
}

async fn id_route_query(tunnel_tx: &TunnelTransmitHub, query_id_max_num: usize) -> io::Result<()> {
    let mut packet =
        if let Ok(packet) = tunnel_tx.allocate_send_packet_proto(ProtocolType::IDRouteQuery, 4) {
            packet
        } else {
            return Ok(());
        };
    unsafe {
        packet.set_payload_len(4);
    }
    packet.set_ttl(1);
    let direct_nodes = tunnel_tx.node_context.get_direct_nodes_and_id();

    let sent_ids = poll_route_table_peer_node(tunnel_tx, packet.buf_mut(), query_id_max_num).await;
    poll_direct_peer_node(direct_nodes, sent_ids, tunnel_tx, packet.buf_mut()).await;
    Ok(())
}

async fn poll_direct_peer_node(
    direct_nodes: DirectNodes,
    sent_ids: HashSet<NodeID>,
    tunnel_tx: &TunnelTransmitHub,
    buf: &mut [u8],
) {
    let self_group_code = tunnel_tx.node_context().load_group_code();
    let mut packet = NetPacket::unchecked(buf);
    for (addr, id, node_id) in direct_nodes {
        if let Some((group_code, node_id)) = node_id {
            if self_group_code == group_code && sent_ids.contains(&node_id) {
                continue;
            }
        }
        packet.payload_mut()[2..4].copy_from_slice(&id.to_be_bytes());
        match addr {
            NodeAddress::Tcp(addr) => match tunnel_tx.socket_manager.tcp_socket_manager_as_ref() {
                None => {}
                Some(tcp) => {
                    if let Err(e) = tcp.send_to(packet.buffer().into(), addr).await {
                        log::warn!("poll_direct_peer_node tcp, e={e:?},addr={addr:?}");
                    }
                }
            },
            NodeAddress::Udp(addr) => match tunnel_tx.socket_manager.udp_socket_manager_as_ref() {
                None => {}
                Some(udp) => {
                    if let Err(e) = udp.send_to(packet.buffer(), addr).await {
                        log::warn!("poll_direct_peer_node udp, e={e:?},addr={addr:?}");
                    }
                }
            },
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn poll_route_table_peer_node(
    tunnel_tx: &TunnelTransmitHub,
    buf: &[u8],
    query_id_max_num: usize,
) -> HashSet<NodeID> {
    let mut route_table = tunnel_tx.socket_manager.route_table().route_table_p2p();
    if route_table.is_empty() {
        return HashSet::new();
    }
    let mut sent_ids = HashSet::with_capacity(route_table.len());

    if route_table.len() > query_id_max_num {
        let mut rng = rand::rng();
        route_table.shuffle(&mut rng);
        route_table.truncate(query_id_max_num);
    }
    for (peer_id, route) in route_table {
        if let Err(e) = tunnel_tx.send_to_route(buf, &route.route_key()).await {
            log::warn!("poll_route_table_peer_node, e={e:?},peer_id={peer_id:?},route={route:?}");
        } else {
            sent_ids.insert(peer_id);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    sent_ids
}
