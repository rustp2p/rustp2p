use crate::pipe::PipeWriter;
use crate::protocol::node_id::NodeID;

mod heartbeat;
mod id_route;
mod idle;

pub(crate) fn start_task(
    pipe_writer: &PipeWriter,
    idle_route_manager: rust_p2p_core::idle::IdleRouteManager<NodeID>,
) {
    tokio::spawn(heartbeat::heartbeat_loop(pipe_writer.clone()));
    tokio::spawn(idle::idle_check_loop(idle_route_manager));
    tokio::spawn(id_route::id_route_query_loop(pipe_writer.clone()));
}
