use crate::pipe::PipeWriter;
use crate::protocol::node_id::NodeID;
use std::time::Duration;

mod heartbeat;
mod id_route;
mod idle;

pub(crate) fn start_task(
    pipe_writer: &PipeWriter,
    idle_route_manager: rust_p2p_core::idle::IdleRouteManager<NodeID>,
    query_id_interval: Duration,
    query_id_max_num: usize,
    heartbeat_interval: Duration,
) {
    tokio::spawn(heartbeat::heartbeat_loop(
        pipe_writer.clone(),
        heartbeat_interval,
    ));
    tokio::spawn(idle::idle_check_loop(idle_route_manager));
    tokio::spawn(id_route::id_route_query_loop(
        pipe_writer.clone(),
        query_id_interval,
        query_id_max_num,
    ));
}
