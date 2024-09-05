use crate::error::*;
use crate::pipe::pipe_context::NodeAddress;
use crate::pipe::PipeWriter;
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::{Builder, NetPacket};
use std::collections::HashSet;
use std::time::Duration;

pub async fn heartbeat_loop(pipe_writer: PipeWriter) {
    loop {
        if let Err(e) = heartbeat_request(&pipe_writer).await {
            log::warn!("heartbeat_request e={e:?}");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn heartbeat_request(pipe_writer: &PipeWriter) -> Result<()> {
    let mut packet = if let Some(packet) = heartbeat_packet(pipe_writer)? {
        NetPacket::new(packet)?
    } else {
        return Ok(());
    };
    let direct_nodes = pipe_writer.pipe_context.get_direct_nodes();
    direct_heartbeat_request(direct_nodes, pipe_writer, packet.buffer()).await;
    route_table_heartbeat_request(pipe_writer, &mut packet).await;
    Ok(())
}

async fn direct_heartbeat_request(
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
                        log::warn!("direct_heartbeat_request tcp, e={e:?},addr={addr:?}");
                    }
                }
            },
            NodeAddress::Udp(addr) => match pipe_writer.pipe_writer.udp_pipe_writer() {
                None => {}
                Some(udp) => {
                    if let Err(e) = udp.send_to_addr(buf, addr).await {
                        log::warn!("direct_heartbeat_request udp, e={e:?},addr={addr:?}");
                    }
                }
            },
        }
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
}

async fn route_table_heartbeat_request(pipe_writer: &PipeWriter, packet: &mut NetPacket<Vec<u8>>) {
    let table = pipe_writer.pipe_writer.route_table().route_table();
    for (node_id, routes) in table {
        if let Err(e) = packet.set_dest_id(&node_id) {
            log::warn!("route_table_heartbeat_request e={e:?},node_id={node_id:?}");
            continue;
        }
        for route in routes {
            if let Err(e) = pipe_writer
                .send_to_route(packet.buffer(), &route.route_key())
                .await
            {
                log::warn!("route_table_heartbeat_request e={e:?},node_id={node_id:?}");
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
    }
}

fn heartbeat_packet(pipe_writer: &PipeWriter) -> Result<Option<Vec<u8>>> {
    if let Some(node_id) = pipe_writer.pipe_context().load_id() {
        let mut buf = vec![0; 4 + node_id.len() * 2 + 4];
        Builder::new(&mut buf, node_id.len() as _)?
            .protocol(ProtocolType::EchoRequest)?
            .src_id(node_id)?
            .ttl(15)?;
        Ok(Some(buf))
    } else {
        Ok(None)
    }
}
