use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::route::route_table::RouteTable;
use crate::route::{Route, RouteKey};

pub struct IdleRouteManager<PeerID> {
    read_idle: Duration,
    route_table: RouteTable<PeerID>,
}

impl<PeerID: Hash + Eq + Clone> IdleRouteManager<PeerID> {
    pub fn new(read_idle: Duration, route_table: RouteTable<PeerID>) -> IdleRouteManager<PeerID> {
        Self {
            read_idle,
            route_table,
        }
    }
    /// Take the timeout routes from the managed route_table
    pub async fn next_idle(&self) -> (PeerID, Route, Instant) {
        loop {
            let time = if let Some((peer_id, route, instant)) = self.route_table.oldest_route() {
                let time = Instant::now() - instant;
                if time > self.read_idle {
                    return (peer_id, route, instant);
                }
                time
            } else {
                self.read_idle
            };
            tokio::time::sleep(time).await;
        }
    }
    pub fn delay(&self, peer_id: &PeerID, route_key: &RouteKey) -> bool {
        self.route_table.update_read_time(peer_id, route_key)
    }
    pub fn remove_route(&self, peer_id: &PeerID, route_key: &RouteKey) {
        self.route_table.remove_route(peer_id, route_key)
    }
}
