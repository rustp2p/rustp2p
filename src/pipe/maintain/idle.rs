use std::time::Duration;

use crate::pipe::pipe_context::PipeContext;
use crate::protocol::node_id::NodeID;

pub async fn idle_check_loop(idle_route_manager: rust_p2p_core::idle::IdleRouteManager<NodeID>) {
    loop {
        let (node_id, route, _) = idle_route_manager.next_idle().await;
        idle_route_manager.remove_route(&node_id, &route.route_key());
        log::info!("idle {node_id:?},{route:?}");
    }
}

pub async fn other_group_idle_check_loop(pipe_context: PipeContext, timeout: Duration) {
    loop {
        for x in pipe_context.other_route_table.iter() {
            if let Some((node_id, route, time)) = x.oldest_route() {
                if time.elapsed() > timeout {
                    x.remove_route(&node_id, &route.route_key());
                }
            }
        }
        pipe_context.other_route_table.retain(|_k, v| !v.is_empty());
        rust_p2p_core::async_compat::time::sleep(timeout).await;
    }
}
