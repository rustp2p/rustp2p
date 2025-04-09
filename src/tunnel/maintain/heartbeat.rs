use crate::protocol::node_id::{GroupCode, NodeID};
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::NetPacket;
use crate::tunnel::node_context::NodeAddress;
use crate::tunnel::TunnelTransmitHub;
use std::collections::HashSet;
use std::io;
use std::time::Duration;

pub async fn heartbeat_loop(tunnel_tx: TunnelTransmitHub, heartbeat_interval: Duration) {
    let mut count = 0;
    loop {
        if count % 3 == 2 {
            if let Err(e) = timestamp_request(&tunnel_tx).await {
                log::warn!("timestamp_request e={e:?}");
            }
        } else if let Err(e) = heartbeat_request(&tunnel_tx).await {
            log::warn!("heartbeat_request e={e:?}");
        }

        tokio::time::sleep(heartbeat_interval).await;
        count += 1;
    }
}

async fn heartbeat_request(tunnel_tx: &TunnelTransmitHub) -> io::Result<()> {
    let mut packet =
        if let Ok(packet) = tunnel_tx.allocate_send_packet_proto(ProtocolType::EchoRequest, 0) {
            packet
        } else {
            return Ok(());
        };
    let mut packet = NetPacket::new(packet.buf_mut())?;
    let direct_nodes = tunnel_tx.node_context.get_direct_nodes();
    let (mut sent_ids, sent_relay_ids) =
        route_table_heartbeat_request(tunnel_tx, &mut packet).await;
    direct_heartbeat_request(direct_nodes, &sent_ids, tunnel_tx, packet.buffer()).await;
    sent_ids.extend(sent_relay_ids);
    relay_heartbeat_request(sent_ids, tunnel_tx, &mut packet).await;
    Ok(())
}

async fn timestamp_request(tunnel_tx: &TunnelTransmitHub) -> io::Result<()> {
    let mut packet = if let Ok(packet) =
        tunnel_tx.allocate_send_packet_proto(ProtocolType::TimestampRequest, 4)
    {
        packet
    } else {
        return Ok(());
    };
    unsafe {
        packet.set_payload_len(4);
    }
    let mut packet = NetPacket::new(packet.buf_mut())?;
    let now = crate::tunnel::now()?;
    packet.payload_mut().copy_from_slice(&now.to_be_bytes());
    let direct_nodes = tunnel_tx.node_context.get_direct_nodes();
    let (sent_ids, _) = route_table_heartbeat_request(tunnel_tx, &mut packet).await;
    packet.set_dest_id(&NodeID::unspecified());
    direct_heartbeat_request(direct_nodes, &sent_ids, tunnel_tx, packet.buffer()).await;
    Ok(())
}

async fn direct_heartbeat_request(
    direct_nodes: Vec<(NodeAddress, Option<(GroupCode, NodeID)>)>,
    sent_ids: &HashSet<NodeID>,
    tunnel_tx: &TunnelTransmitHub,
    buf: &[u8],
) {
    let self_group_code = tunnel_tx.node_context().load_group_code();
    for (addr, node_id) in direct_nodes {
        if let Some((group_code, node_id)) = node_id {
            if self_group_code == group_code && sent_ids.contains(&node_id) {
                continue;
            }
        }
        match addr {
            NodeAddress::Tcp(addr) => match tunnel_tx.socket_manager.tcp_socket_manager_as_ref() {
                None => {}
                Some(tcp) => {
                    if let Err(e) = tcp.send_to(buf.into(), addr).await {
                        log::warn!("direct_heartbeat_request tcp, e={e:?},addr={addr:?}");
                    }
                }
            },
            NodeAddress::Udp(addr) => match tunnel_tx.socket_manager.udp_socket_manager_as_ref() {
                None => {}
                Some(udp) => {
                    if let Err(e) = udp.send_to(buf, addr).await {
                        log::warn!("direct_heartbeat_request udp, e={e:?},addr={addr:?}");
                    }
                }
            },
        }
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
}

async fn route_table_heartbeat_request(
    tunnel_tx: &TunnelTransmitHub,
    packet: &mut NetPacket<&mut [u8]>,
) -> (HashSet<NodeID>, HashSet<NodeID>) {
    let table = tunnel_tx.route_table.route_table();
    let mut sent_p2p_ids = HashSet::with_capacity(table.len());
    let mut sent_relay_ids = HashSet::with_capacity(table.len());
    for (node_id, routes) in table {
        packet.set_dest_id(&node_id);
        for (i, route) in routes.into_iter().enumerate() {
            if i >= tunnel_tx.node_context.major_tunnel_count {
                break;
            }
            if let Err(e) = tunnel_tx
                .send_to_route(packet.buffer(), &route.route_key())
                .await
            {
                log::warn!("route_table_heartbeat_request e={e:?},node_id={node_id:?}");
            } else if route.is_direct() {
                sent_p2p_ids.insert(node_id);
            } else {
                sent_relay_ids.insert(node_id);
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
    }
    (sent_p2p_ids, sent_relay_ids)
}
async fn relay_heartbeat_request(
    sent_ids: HashSet<NodeID>,
    tunnel_tx: &TunnelTransmitHub,
    packet: &mut NetPacket<&mut [u8]>,
) {
    let group_code = tunnel_tx.node_context().load_group_code();
    let mut dest_list = Vec::new();
    if let Some(x) = tunnel_tx.node_context().reachable_nodes.get(&group_code) {
        for x in x.value() {
            if sent_ids.contains(x.key()) {
                continue;
            }
            dest_list.push((*x.key(), x.value().0, x.value().1));
        }
    }
    for (dest_id, relay_group_code, relay_id) in dest_list {
        packet.set_dest_id(&dest_id);
        if let Err(e) = tunnel_tx
            .send_to_id_by_code(packet, &relay_group_code, &relay_id)
            .await
        {
            log::warn!("relay_heartbeat_request e={e:?},dest_id={dest_id:?},relay_group_code={relay_group_code:?},node_id={relay_id:?}");
        }
    }
}
