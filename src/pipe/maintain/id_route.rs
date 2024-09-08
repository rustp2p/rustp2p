use crate::pipe::{NodeAddress, PipeWriter};
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::time::Duration;

pub async fn id_route_query_loop(
    pipe_writer: PipeWriter,
    query_id_interval: Duration,
    query_id_max_num: usize,
) {
    loop {
        if let Err(e) = id_route_query(&pipe_writer, query_id_max_num).await {
            log::warn!("poll_peer_node, e={e:?}");
        }
        tokio::time::sleep(query_id_interval).await;
        pipe_writer.pipe_context().clear_timeout_reachable_nodes();
    }
}

async fn id_route_query(
    pipe_writer: &PipeWriter,
    query_id_max_num: usize,
) -> crate::error::Result<()> {
    let mut packet =
        if let Ok(packet) = pipe_writer.allocate_send_packet_proto(ProtocolType::IDRouteQuery, 4) {
            packet
        } else {
            return Ok(());
        };
    packet.set_payload_len(4);
    packet.set_ttl(1);
    let direct_nodes = pipe_writer.pipe_context.get_direct_nodes();
    let mut direct_node_set = HashSet::new();
    for (_, peer_id) in &direct_nodes {
        if let Some(peer_id) = peer_id {
            direct_node_set.insert(*peer_id);
        }
    }
    poll_direct_peer_node(direct_nodes, pipe_writer, packet.buf_mut()).await;
    poll_route_table_peer_node(
        pipe_writer,
        packet.buf_mut(),
        direct_node_set,
        query_id_max_num,
    )
    .await;
    Ok(())
}

async fn poll_direct_peer_node(
    direct_nodes: Vec<(NodeAddress, Option<NodeID>)>,
    pipe_writer: &PipeWriter,
    buf: &[u8],
) {
    for (addr, _) in direct_nodes {
        match addr {
            NodeAddress::Tcp(addr) => match pipe_writer.pipe_writer.tcp_pipe_writer() {
                None => {}
                Some(tcp) => {
                    if let Err(e) = tcp.send_to_addr(buf, addr).await {
                        log::warn!("poll_direct_peer_node tcp, e={e:?},addr={addr:?}");
                    }
                }
            },
            NodeAddress::Udp(addr) => match pipe_writer.pipe_writer.udp_pipe_writer() {
                None => {}
                Some(udp) => {
                    if let Err(e) = udp.send_to_addr(buf, addr).await {
                        log::warn!("poll_direct_peer_node udp, e={e:?},addr={addr:?}");
                    }
                }
            },
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn poll_route_table_peer_node(
    pipe_writer: &PipeWriter,
    buf: &[u8],
    direct_nodes: HashSet<NodeID>,
    query_id_max_num: usize,
) {
    let mut route_table = pipe_writer.pipe_writer.route_table().route_table_one();
    if route_table.is_empty() {
        return;
    }
    if route_table.len() > query_id_max_num {
        let mut rng = rand::thread_rng();
        route_table.shuffle(&mut rng);
        route_table.truncate(query_id_max_num);
    }
    for (peer_id, route) in route_table {
        if !direct_nodes.contains(&peer_id) {
            if let Err(e) = pipe_writer.send_to_route(buf, &route.route_key()).await {
                log::warn!(
                    "poll_route_table_peer_node, e={e:?},peer_id={peer_id:?},route={route:?}"
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
