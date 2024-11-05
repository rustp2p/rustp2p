use crate::pipe::PipeWriter;
use crate::protocol::node_id::NodeID;
use rust_p2p_core::punch::{PunchConsultInfo, Puncher};
use rust_p2p_core::socket::LocalInterface;
use std::time::Duration;
#[cfg(feature = "use-tokio")]
use tokio::task::JoinSet;
#[cfg(feature = "use-tokio")]
use tokio::sync::mpsc::Receiver;

mod heartbeat;
mod id_route;
mod idle;
mod nat_query;
mod punch_consult;
mod query_public_addr;

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "use-tokio")]
pub(crate) fn start_task(
    pipe_writer: &PipeWriter,
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
        pipe_writer.clone(),
        heartbeat_interval,
    ));
    join_set.spawn(idle::idle_check_loop(idle_route_manager));
    join_set.spawn(idle::other_group_idle_check_loop(
        pipe_writer.pipe_context.clone(),
        route_idle_time,
    ));
    join_set.spawn(id_route::id_route_query_loop(
        pipe_writer.clone(),
        query_id_interval,
        query_id_max_num,
    ));
    join_set.spawn(nat_query::nat_test_loop(
        pipe_writer.clone(),
        udp_stun_servers.clone(),
        default_interface,
    ));
    join_set.spawn(query_public_addr::query_tcp_public_addr_loop(
        pipe_writer.clone(),
        tcp_stun_servers,
    ));
    join_set.spawn(query_public_addr::query_udp_public_addr_loop(
        pipe_writer.clone(),
        udp_stun_servers,
    ));
    join_set.spawn(punch_consult::punch_consult_loop(
        pipe_writer.clone(),
        puncher.clone(),
    ));
    join_set.spawn(punch_consult::punch_loop(
        true,
        active_receiver,
        pipe_writer.clone(),
        puncher.clone(),
    ));
    join_set.spawn(punch_consult::punch_loop(
        false,
        passive_receiver,
        pipe_writer.clone(),
        puncher,
    ));
    join_set
}
#[cfg(feature = "use-async-std")]
use async_std::channel::Receiver;
#[cfg(feature = "use-async-std")]
use futures_util::future::BoxFuture;
#[cfg(feature = "use-async-std")]
use futures_util::FutureExt;
#[cfg(feature = "use-async-std")]
use rust_p2p_core::async_compat::futures;

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "use-async-std")]
pub(crate) fn start_task(
    pipe_writer: &PipeWriter,
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
) -> futures::future::JoinAll<BoxFuture<'static,()>> {
    let mut join_set = Vec::new();
    join_set.push(heartbeat::heartbeat_loop(
        pipe_writer.clone(),
        heartbeat_interval,
    ).boxed());
    join_set.push(idle::idle_check_loop(idle_route_manager).boxed());
    join_set.push(idle::other_group_idle_check_loop(
        pipe_writer.pipe_context.clone(),
        route_idle_time,
    ).boxed());
    join_set.push(id_route::id_route_query_loop(
        pipe_writer.clone(),
        query_id_interval,
        query_id_max_num,
    ).boxed());
    join_set.push(nat_query::nat_test_loop(
        pipe_writer.clone(),
        udp_stun_servers.clone(),
        default_interface,
    ).boxed());
    join_set.push(query_public_addr::query_tcp_public_addr_loop(
        pipe_writer.clone(),
        tcp_stun_servers,
    ).boxed());
    join_set.push(query_public_addr::query_udp_public_addr_loop(
        pipe_writer.clone(),
        udp_stun_servers,
    ).boxed());
    join_set.push(punch_consult::punch_consult_loop(
        pipe_writer.clone(),
        puncher.clone(),
    ).boxed());
    join_set.push(punch_consult::punch_loop(
        true,
        active_receiver,
        pipe_writer.clone(),
        puncher.clone(),
    ).boxed());
    join_set.push(punch_consult::punch_loop(
        false,
        passive_receiver,
        pipe_writer.clone(),
        puncher,
    ).boxed());
    futures::future::join_all(join_set)
}
