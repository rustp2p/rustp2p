use crate::pipe::{NodeAddress, PipeWriter};
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::Builder;
use std::collections::HashSet;
use std::time::Duration;

pub async fn id_route_query_loop(pipe_writer: PipeWriter) {
    loop {
        if let Err(e) = id_route_query(&pipe_writer).await {
            log::warn!("poll_peer_node, e={e:?}");
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
async fn id_route_query(pipe_writer: &PipeWriter) -> crate::error::Result<()> {
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
    poll_route_table_peer_node(pipe_writer, packet.buf_mut(), direct_node_set).await;
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
) {
    let route_table = pipe_writer.pipe_writer.route_table().route_table_one();
    // todo 随机取几个节点拉取，而不是全部节点都遍历
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
