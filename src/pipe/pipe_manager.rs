use crate::error::*;
use crate::pipe::pipe_context::NodeAddress;
use crate::pipe::PipeWriter;
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::Builder;
use std::collections::HashSet;
use std::time::Duration;

pub struct PipeManager {
    puncher: rust_p2p_core::punch::Puncher<NodeID>,
    idle_route_manager: rust_p2p_core::idle::IdleRouteManager<NodeID>,
}
pub async fn poll_peer_node(pipe_writer: PipeWriter) -> Result<()> {
    loop {
        if let Err(e) = poll_peer_node0(&pipe_writer).await {
            log::warn!("poll_peer_node, e={e:?}");
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
async fn poll_peer_node0(pipe_writer: &PipeWriter) -> Result<()> {
    let buf = if let Some(buf) = id_route_query_packet(pipe_writer)? {
        buf
    } else {
        return Ok(());
    };
    let direct_nodes = pipe_writer.pipe_context.get_direct_nodes();
    let mut direct_node_set = HashSet::new();
    for (_, peer_id) in &direct_nodes {
        if let Some(peer_id) = peer_id {
            direct_node_set.insert(*peer_id);
        }
    }
    poll_direct_peer_node(direct_nodes, pipe_writer, &buf).await;
    poll_route_table_peer_node(pipe_writer, &buf, direct_node_set).await;
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
    }
}
async fn poll_route_table_peer_node(
    pipe_writer: &PipeWriter,
    buf: &[u8],
    direct_nodes: HashSet<NodeID>,
) {
    let route_table = pipe_writer.pipe_writer.route_table().route_table_one();
    for (peer_id, route) in route_table {
        if !direct_nodes.contains(&peer_id) {
            if let Err(e) = pipe_writer.send_to(buf, &route.route_key()).await {
                log::warn!(
                    "poll_route_table_peer_node, e={e:?},peer_id={peer_id:?},route={route:?}"
                );
            }
        }
    }
}
fn id_route_query_packet(pipe_writer: &PipeWriter) -> Result<Option<Vec<u8>>> {
    if let Some(node_id) = pipe_writer.pipe_context().load_id() {
        let mut buf = vec![0; 4 + node_id.len() * 2 + 4];
        Builder::new(&mut buf, node_id.len() as _)?
            .protocol(ProtocolType::IDRouteQuery)?
            .src_id(node_id)?
            .ttl(1)?;
        Ok(Some(buf))
    } else {
        Ok(None)
    }
}
