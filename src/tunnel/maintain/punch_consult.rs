use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::tunnel::TunnelTransmitHub;
use rand::seq::SliceRandom;
use rust_p2p_core::punch::{PunchConsultInfo, PunchInfo, Puncher};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

pub async fn punch_consult_loop(tunnel_tx: TunnelTransmitHub, puncher: Puncher<NodeID>) {
    let mut seq = 0;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let route_table = tunnel_tx.socket_manager.route_table();
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        seq += 1;
        let self_id = if let Some(self_id) = tunnel_tx.node_context.load_id() {
            self_id
        } else {
            continue;
        };
        let consult_info = tunnel_tx.node_context().gen_punch_info(seq);
        let data = match rmp_serde::to_vec(&consult_info) {
            Ok(data) => data,
            Err(e) => {
                log::warn!("punch_consult_loop rmp_serde {e:?}");
                continue;
            }
        };
        let mut send_packet = match tunnel_tx
            .allocate_send_packet_proto(ProtocolType::PunchConsultRequest, data.len())
        {
            Ok(send_packet) => send_packet,
            Err(e) => {
                log::warn!("punch_consult_loop send_packet{e:?}");
                continue;
            }
        };
        send_packet.set_payload(&data);

        let mut node_ids = route_table.route_table_ids();
        node_ids.shuffle(&mut rand::rng());
        let mut count = 0;
        for node_id in node_ids {
            if node_id <= self_id {
                continue;
            }
            if !puncher.need_punch(&node_id) {
                continue;
            }
            if tunnel_tx
                .send_packet_to(send_packet.clone(), &node_id)
                .await
                .is_ok()
            {
                log::debug!("punch_consult {:?}", node_id);
                count += 1;
            }
            if count > 3 {
                break;
            }
        }
    }
}
#[allow(unused_mut)]
pub async fn punch_loop(
    active: bool,
    mut receiver: Receiver<(NodeID, PunchConsultInfo)>,
    tunnel_tx: TunnelTransmitHub,
    puncher: Puncher<NodeID>,
) {
    while let Some((node_id, info)) = receiver.recv().await {
        let punch_info = PunchInfo::new(
            active,
            info.peer_punch_model & tunnel_tx.node_context().punch_model_box(),
            info.peer_nat_info,
        );
        if let Ok(packet) = tunnel_tx.allocate_send_packet_proto(ProtocolType::PunchRequest, 0) {
            if let Err(e) = puncher.punch(node_id, packet.buf(), punch_info).await {
                log::warn!("punch {e:?} {node_id:?}");
            }
        }
    }
}
