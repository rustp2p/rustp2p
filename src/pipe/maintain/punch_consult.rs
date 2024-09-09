use crate::pipe::PipeWriter;
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use rand::seq::SliceRandom;
use rust_p2p_core::punch::{PunchConsultInfo, Puncher};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

pub async fn punch_consult_loop(pipe_writer: PipeWriter, puncher: Puncher<NodeID>) {
    let mut seq = 0;
    let route_table = pipe_writer.pipe_writer.route_table();
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        seq += 1;
        let self_id = if let Some(self_id) = pipe_writer.pipe_context.load_id() {
            self_id
        } else {
            continue;
        };
        let consult_info = pipe_writer.pipe_context().gen_punch_info(seq);
        let data = match rmp_serde::to_vec(&consult_info) {
            Ok(data) => data,
            Err(e) => {
                log::warn!("punch_consult_loop rmp_serde {e:?}");
                continue;
            }
        };
        let mut send_packet = match pipe_writer
            .allocate_send_packet_proto(ProtocolType::PunchConsultRequest, data.len())
        {
            Ok(send_packet) => send_packet,
            Err(e) => {
                log::warn!("punch_consult_loop send_packet{e:?}");
                continue;
            }
        };
        send_packet.data_mut()[..data.len()].copy_from_slice(&data);
        send_packet.set_payload_len(data.len());

        let mut node_ids = route_table.route_table_ids();
        node_ids.shuffle(&mut rand::thread_rng());
        let mut count = 0;
        for node_id in node_ids {
            if node_id <= self_id {
                continue;
            }
            if !puncher.need_punch(&node_id) {
                continue;
            }
            if pipe_writer
                .send_to_packet(&mut send_packet, &node_id)
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
pub async fn punch_loop(receiver: Receiver<PunchConsultInfo>, puncher: Puncher<NodeID>) {}
