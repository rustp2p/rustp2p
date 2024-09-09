use crate::protocol::node_id::NodeID;

pub async fn idle_check_loop(idle_route_manager: rust_p2p_core::idle::IdleRouteManager<NodeID>) {
    loop {
        let (node_id, route, _) = idle_route_manager.next_idle().await;
        idle_route_manager.remove_route(&node_id, &route.route_key());
        log::info!("idle {node_id:?},{route:?}");
    }
}
