use std::collections::HashMap;
use std::hash::Hash;
use std::io;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Instant;

use crate::endpoint::LoadBalance;
use crate::route_table::{RouteKey, RouteSortKey, DEFAULT_RTT};
use crossbeam_utils::atomic::AtomicCell;
use dashmap::DashMap;

#[derive(Copy, Clone, Debug)]
pub struct Route {
    route_key: RouteKey,
    metric: u8,
    rtt: u32,
}
impl Route {
    pub fn from(route_key: RouteKey, metric: u8, rtt: u32) -> Self {
        Self {
            route_key,
            metric,
            rtt,
        }
    }
    pub fn from_default_rt(route_key: RouteKey, metric: u8) -> Self {
        Self {
            route_key,
            metric,
            rtt: DEFAULT_RTT,
        }
    }
    pub fn route_key(&self) -> RouteKey {
        self.route_key
    }
    pub fn sort_key(&self) -> RouteSortKey {
        RouteSortKey {
            metric: self.metric,
            rtt: self.rtt,
        }
    }
    pub fn is_direct(&self) -> bool {
        self.metric == 0
    }
    pub fn is_relay(&self) -> bool {
        self.metric > 0
    }
    pub fn rtt(&self) -> u32 {
        self.rtt
    }
    pub fn metric(&self) -> u8 {
        self.metric
    }
}

impl From<(RouteKey, u8)> for Route {
    fn from((key, metric): (RouteKey, u8)) -> Self {
        Route::from_default_rt(key, metric)
    }
}

pub(crate) type RouteTableInner<PeerID> =
    Arc<DashMap<PeerID, (AtomicUsize, Vec<(Route, AtomicCell<Instant>)>)>>;
pub struct RouteTable<PeerID> {
    pub(crate) route_table: RouteTableInner<PeerID>,
    route_key_table: Arc<DashMap<RouteKey, PeerID>>,
    load_balance: LoadBalance,
}
impl<PeerID: Hash + Eq> Default for RouteTable<PeerID> {
    fn default() -> Self {
        Self {
            route_table: Default::default(),
            route_key_table: Default::default(),
            load_balance: Default::default(),
        }
    }
}
impl<PeerID> Clone for RouteTable<PeerID> {
    fn clone(&self) -> Self {
        Self {
            route_table: self.route_table.clone(),
            route_key_table: self.route_key_table.clone(),
            load_balance: self.load_balance,
        }
    }
}
impl<PeerID: Hash + Eq> RouteTable<PeerID> {
    pub fn new(load_balance: LoadBalance) -> RouteTable<PeerID> {
        Self {
            route_table: Arc::new(DashMap::with_capacity(64)),
            route_key_table: Arc::new(DashMap::with_capacity(64)),
            load_balance,
        }
    }
}
impl<PeerID: Hash + Eq> RouteTable<PeerID> {
    pub fn is_empty(&self) -> bool {
        self.route_table.is_empty()
    }
    pub fn is_route_of_peer_id(&self, id: &PeerID, route_key: &RouteKey) -> bool {
        if let Some(src) = self.route_key_table.get(route_key) {
            return src.value() == id;
        }
        false
    }
    pub fn get_route_by_id(&self, id: &PeerID) -> io::Result<Route> {
        if let Some(entry) = self.route_table.get(id) {
            let (count, routes) = entry.value();
            // Always prefer direct routes (metric == 0) over relay routes.
            // QUIC and other latency-sensitive traffic must use the direct
            // path when available; relay routes are fallback only. This
            // prevents QUIC handshake timeouts when a direct route exists
            // alongside a stale relay route.
            if let Some((route, _)) = routes.iter().find(|(r, _)| r.is_direct()) {
                return Ok(*route);
            }
            if LoadBalance::RoundRobin != self.load_balance {
                if let Some((route, _)) = routes.first() {
                    return Ok(*route);
                }
            } else {
                let len = routes.len();
                if len != 0 {
                    let index = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % len;
                    return Ok(routes[index].0);
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "route not found"))
    }
    pub fn exists(&self, id: &PeerID) -> bool {
        self.route_table.get(id).is_some()
    }
}
impl<PeerID: Hash + Eq + Clone> RouteTable<PeerID> {
    pub fn add_route_if_absent(&self, id: PeerID, route: Route) -> bool {
        self.add_route_(id, route, true)
    }
    pub fn add_route<R: Into<Route>>(&self, id: PeerID, route: R) -> bool {
        self.add_route_(id, route.into(), false)
    }
    /// Update the usage time of the route,
    /// routes that have not received data for a long time will be excluded
    pub fn update_read_time(&self, id: &PeerID, route_key: &RouteKey) -> bool {
        if let Some(entry) = self.route_table.get(id) {
            let (_, routes) = entry.value();
            for (route, time) in routes {
                if &route.route_key() == route_key {
                    time.store(Instant::now());
                    return true;
                }
            }
        }
        false
    }
    /// Remove specified route
    pub fn remove_route(&self, id: &PeerID, route_key: &RouteKey) {
        self.route_table.remove_if_mut(id, |_, (_, routes)| {
            routes.retain(|(x, _)| &x.route_key() != route_key);
            self.route_key_table.remove_if(route_key, |_, v| v == id);
            routes.is_empty()
        });
    }
    pub fn remove_all(&self, id: &PeerID) {
        self.route_table.remove_if(id, |_, (_, routes)| {
            for (route, _) in routes {
                if route.is_direct() {
                    self.route_key_table
                        .remove_if(&route.route_key(), |_, v| v == id);
                }
            }
            true
        });
    }
    pub fn get_id_by_route_key(&self, route_key: &RouteKey) -> Option<PeerID> {
        self.route_key_table
            .get(route_key)
            .map(|v| v.value().clone())
    }
    pub fn route(&self, id: &PeerID) -> Option<Vec<Route>> {
        if let Some(entry) = self.route_table.get(id) {
            let (_, routes) = entry.value();
            Some(routes.iter().map(|(i, _)| *i).collect())
        } else {
            None
        }
    }
    pub fn route_one(&self, id: &PeerID) -> Option<Route> {
        if let Some(entry) = self.route_table.get(id) {
            let (_, routes) = entry.value();
            routes.first().map(|(i, _)| *i)
        } else {
            None
        }
    }
    pub fn route_one_p2p(&self, id: &PeerID) -> Option<Route> {
        if let Some(entry) = self.route_table.get(id) {
            let (_, routes) = entry.value();
            for (i, _) in routes {
                if i.is_direct() {
                    return Some(*i);
                }
            }
        }
        None
    }
    pub fn route_to_id(&self, route_key: &RouteKey) -> Option<PeerID> {
        let table = self.route_table.iter();
        for entry in table {
            let (id, (_, routes)) = (entry.key(), entry.value());
            for (route, _) in routes {
                if &route.route_key() == route_key && route.is_direct() {
                    return Some(id.clone());
                }
            }
        }
        None
    }
    pub fn need_punch(&self, id: &PeerID) -> bool {
        if let Some(entry) = self.route_table.get(id) {
            let (_, routes) = entry.value();
            // Check if peer has sufficient direct channels
            return !routes.iter().any(|(k, _)| k.is_direct());
        }
        true
    }
    pub fn no_need_punch(&self, id: &PeerID) -> bool {
        !self.need_punch(id)
    }
    pub fn p2p_num(&self, id: &PeerID) -> usize {
        if let Some(entry) = self.route_table.get(id) {
            let (_, routes) = entry.value();
            routes.iter().filter(|(k, _)| k.is_direct()).count()
        } else {
            0
        }
    }
    pub fn relay_num(&self, id: &PeerID) -> usize {
        if let Some(entry) = self.route_table.get(id) {
            let (_, routes) = entry.value();
            routes.iter().filter(|(k, _)| k.is_relay()).count()
        } else {
            0
        }
    }
    /// Return all routes
    pub fn route_table(&self) -> Vec<(PeerID, Vec<Route>)> {
        let table = self.route_table.iter();

        table
            .map(|entry| {
                (
                    entry.key().clone(),
                    entry.value().1.iter().map(|(i, _)| *i).collect(),
                )
            })
            .collect()
    }
    /// Return all P2P routes
    pub fn route_table_p2p(&self) -> Vec<(PeerID, Route)> {
        let table = self.route_table.iter();
        let mut list = Vec::with_capacity(8);
        for entry in table {
            let (id, (_, routes)) = (entry.key(), entry.value());
            for (route, _) in routes.iter() {
                if route.is_direct() {
                    list.push((id.clone(), *route));
                    break;
                }
            }
        }
        list
    }
    /// Return to the first route
    pub fn route_table_one(&self) -> Vec<(PeerID, Route)> {
        let mut list = Vec::with_capacity(8);
        let table = self.route_table.iter();
        for entry in table {
            let (id, (_, routes)) = (entry.key(), entry.value());
            if let Some((route, _)) = routes.first() {
                list.push((id.clone(), *route));
            }
        }
        list
    }
    /// Return to `route_key` -> `Vec<PeerID>`,
    /// where `vec[0]` is the owner of the route
    pub fn route_key_table(&self) -> HashMap<RouteKey, Vec<PeerID>> {
        let mut map: HashMap<RouteKey, Vec<PeerID>> = HashMap::new();
        let table = self.route_table.iter();
        for entry in table {
            let (id, (_, routes)) = (entry.key(), entry.value());
            for (route, _) in routes {
                let is_p2p = route.is_direct();
                map.entry(route.route_key())
                    .and_modify(|list| {
                        list.push(id.clone());
                        if is_p2p {
                            let last_index = list.len() - 1;
                            list.swap(0, last_index);
                        }
                    })
                    .or_insert_with(|| vec![id.clone()]);
            }
        }
        map
    }
    pub fn route_table_ids(&self) -> Vec<PeerID> {
        self.route_table.iter().map(|v| v.key().clone()).collect()
    }
    pub fn route_table_min_metric(&self) -> Vec<(PeerID, Route)> {
        let mut list = Vec::with_capacity(8);
        let table = self.route_table.iter();
        for entry in table {
            let (id, (_, routes)) = (entry.key(), entry.value());
            if let Some((route, _)) = routes.iter().min_by_key(|(v, _)| v.metric) {
                list.push((id.clone(), *route));
            }
        }
        list
    }
    pub fn route_table_min_rtt(&self) -> Vec<(PeerID, Route)> {
        let mut list = Vec::with_capacity(8);
        let table = self.route_table.iter();
        for entry in table {
            let (id, (_, routes)) = (entry.key(), entry.value());
            if let Some((route, _)) = routes.iter().min_by_key(|(v, _)| v.rtt) {
                list.push((id.clone(), *route));
            }
        }
        list
    }

    pub fn oldest_route(&self) -> Option<(PeerID, Route, Instant)> {
        if self.route_table.is_empty() {
            return None;
        }
        let mut option: Option<(PeerID, Route, Instant)> = None;
        for entry in self.route_table.iter() {
            let (peer_id, (_, routes)) = (entry.key(), entry.value());
            for (route, time) in routes {
                let instant = time.load();
                if let Some((t_peer_id, t_route, t_instant)) = &mut option {
                    if *t_instant > instant {
                        *t_peer_id = peer_id.clone();
                        *t_route = *route;
                        *t_instant = instant;
                    }
                } else {
                    option.replace((peer_id.clone(), *route, instant));
                }
            }
        }
        option
    }
}
impl<PeerID: Hash + Eq + Clone> RouteTable<PeerID> {
    fn add_route_(&self, id: PeerID, route: Route, only_if_absent: bool) -> bool {
        let key = route.route_key();
        if only_if_absent {
            if let Some(entry) = self.route_table.get(&id) {
                let (_, routes) = entry.value();
                for (x, time) in routes {
                    if x.route_key() == key {
                        time.store(Instant::now());
                        return true;
                    }
                }
            }
        }
        let mut route_table = self
            .route_table
            .entry(id)
            .or_insert_with(|| (AtomicUsize::new(0), Vec::with_capacity(4)));
        let (peer_id, (_, list)) = route_table.pair_mut();
        let mut exist_index = None;
        for (index, (x, time)) in list.iter_mut().enumerate() {
            if x.metric < route.metric
                && self.load_balance != LoadBalance::LowestLatency
                && x.is_direct() == route.is_direct()
            {
                // When not preferring latency, reject a new route only if a
                // shorter route of the *same type* (direct vs relay) already
                // exists. This allows direct and relay routes to coexist.
                return false;
            }
            if x.route_key() == key {
                time.store(Instant::now());
                if only_if_absent {
                    return true;
                }
                // Don't downgrade a direct route (metric 0) to a relay route
                // (metric > 0) when the same physical path is already known
                // to be direct. Route discovery responses can arrive via the
                // same route_key but carry a higher metric (the advertised
                // peer's own metric + 1), which does not change the physical
                // reachability of this path.
                if x.is_direct() && !route.is_direct() {
                    return true;
                }
                x.metric = route.metric;
                x.rtt = route.rtt;
                exist_index = Some(index);
                break;
            }
        }
        if let Some(index) = exist_index {
            if self.load_balance != LoadBalance::MostRecent {
                if self.load_balance == LoadBalance::LowestLatency {
                    list.sort_by_key(|(k, _)| k.rtt);
                } else {
                    list.sort_by_key(|(k, _)| k.sort_key());
                }
            } else {
                list.swap(0, index);
            }
        } else {
            // Direct and relay routes coexist. When a direct route is added,
            // existing relay routes are retained as fallback so that
            // communication can continue if the direct path becomes stale
            // (e.g. NAT mapping expires).
            if route.is_direct() {
                self.route_key_table
                    .insert(route.route_key(), peer_id.clone());
            }
            list.push((route, AtomicCell::new(Instant::now())));
            // Sort after push so the newly added route is included in the
            // ordering. For MinHopLowestLatency this places direct routes
            // (metric 0) ahead of relay routes.
            match self.load_balance {
                LoadBalance::MostRecent => {
                    let last = list.len() - 1;
                    list.swap(0, last);
                }
                LoadBalance::LowestLatency => {
                    list.sort_by_key(|(k, _)| k.rtt);
                }
                _ => {
                    list.sort_by_key(|(k, _)| k.sort_key());
                }
            }
        }
        true
    }
}
