use crate::protocol::node_id::NodeID;
use crate::tunnel::TunnelTransmit;
use rust_p2p_core::punch::{PunchConsultInfo, Puncher};
use rust_p2p_core::socket::LocalInterface;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinSet;

mod heartbeat;
mod id_route;
mod idle;
mod nat_query;
mod punch_consult;
mod query_public_addr;

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_task(
    tunnel_tx: &TunnelTransmit,
    idle_route_manager: rust_p2p_core::idle::IdleRouteManager<NodeID>,
    puncher: Puncher<NodeID>,
    query_id_interval: Duration,
    query_id_max_num: usize,
    heartbeat_interval: Duration,
    route_idle_time: Duration,
    tcp_stun_servers: Vec<String>,
    udp_stun_servers: Vec<String>,
    default_interface: Option<LocalInterface>,
    active_receiver: Receiver<(NodeID, PunchConsultInfo)>,
    passive_receiver: Receiver<(NodeID, PunchConsultInfo)>,
) -> JoinSet<()> {
    let mut join_set = JoinSet::new();
    join_set.spawn(heartbeat::heartbeat_loop(
        tunnel_tx.clone(),
        heartbeat_interval,
    ));
    join_set.spawn(idle::idle_check_loop(idle_route_manager));
    join_set.spawn(idle::other_group_idle_check_loop(
        tunnel_tx.node_context.clone(),
        route_idle_time,
    ));
    join_set.spawn(id_route::id_route_query_loop(
        tunnel_tx.clone(),
        query_id_interval,
        query_id_max_num,
    ));
    join_set.spawn(nat_query::nat_test_loop(
        tunnel_tx.clone(),
        udp_stun_servers.clone(),
        default_interface,
    ));
    join_set.spawn(query_public_addr::query_tcp_public_addr_loop(
        tunnel_tx.clone(),
        tcp_stun_servers,
    ));
    join_set.spawn(query_public_addr::query_udp_public_addr_loop(
        tunnel_tx.clone(),
        udp_stun_servers,
    ));
    join_set.spawn(punch_consult::punch_consult_loop(
        tunnel_tx.clone(),
        puncher.clone(),
    ));
    join_set.spawn(punch_consult::punch_loop(
        true,
        active_receiver,
        tunnel_tx.clone(),
        puncher.clone(),
    ));
    join_set.spawn(punch_consult::punch_loop(
        false,
        passive_receiver,
        tunnel_tx.clone(),
        puncher,
    ));
    join_set
}
