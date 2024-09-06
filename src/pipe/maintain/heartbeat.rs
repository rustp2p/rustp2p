use crate::error::*;
use crate::pipe::pipe_context::NodeAddress;
use crate::pipe::PipeWriter;
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::{Builder, NetPacket};
use std::collections::HashSet;
use std::time::{Duration, UNIX_EPOCH};

pub async fn heartbeat_loop(pipe_writer: PipeWriter) {
    let mut count = 0;
    loop {
        if count % 3 == 2 {
            if let Err(e) = timestamp_request(&pipe_writer).await {
                log::warn!("timestamp_request e={e:?}");
            }
        } else {
            if let Err(e) = heartbeat_request(&pipe_writer).await {
                log::warn!("heartbeat_request e={e:?}");
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
        count += 1;
    }
}

async fn heartbeat_request(pipe_writer: &PipeWriter) -> Result<()> {
    let mut packet =
        if let Ok(packet) = pipe_writer.allocate_send_packet_proto(ProtocolType::EchoRequest, 0) {
            packet
        } else {
            return Ok(());
        };
    let mut packet = NetPacket::new(packet.buf_mut())?;
    let direct_nodes = pipe_writer.pipe_context.get_direct_nodes();
    direct_heartbeat_request(direct_nodes, pipe_writer, packet.buffer()).await;
    route_table_heartbeat_request(pipe_writer, &mut packet).await;
    Ok(())
}

async fn timestamp_request(pipe_writer: &PipeWriter) -> Result<()> {
    let mut packet = if let Ok(packet) =
        pipe_writer.allocate_send_packet_proto(ProtocolType::TimestampRequest, 4)
    {
        packet
    } else {
        return Ok(());
    };
    packet.set_payload_len(4);
    let mut packet = NetPacket::new(packet.buf_mut())?;
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis() as u32;
    packet.payload_mut().copy_from_slice(&now.to_be_bytes());
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

async fn route_table_heartbeat_request(
    pipe_writer: &PipeWriter,
    packet: &mut NetPacket<&mut [u8]>,
) {
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
