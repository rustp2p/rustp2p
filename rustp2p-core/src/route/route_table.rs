use std::hash::Hash;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_utils::atomic::AtomicCell;
use dashmap::DashMap;

use crate::route::{Route, RouteKey, DEFAULT_RTT};

pub(crate) type RouteTableInner<PeerID> =
    Arc<DashMap<PeerID, (AtomicUsize, Vec<(Route, AtomicCell<Instant>)>)>>;
pub struct RouteTable<PeerID> {
    pub(crate) route_table: RouteTableInner<PeerID>,
    first_latency: bool,
    channel_num: usize,
}
impl<PeerID> Clone for RouteTable<PeerID> {
    fn clone(&self) -> Self {
        Self {
            route_table: self.route_table.clone(),
            first_latency: self.first_latency,
            channel_num: self.channel_num,
        }
    }
}
impl<PeerID: Hash + Eq> RouteTable<PeerID> {
    pub fn new(first_latency: bool, channel_num: usize) -> RouteTable<PeerID> {
        Self {
            route_table: Arc::new(DashMap::with_capacity(64)),
            first_latency,
            channel_num,
        }
    }
}
impl<PeerID: Hash + Eq> RouteTable<PeerID> {
    pub fn get_route_by_id(&self, id: &PeerID) -> io::Result<Route> {
        if let Some(entry) = self.route_table.get(id) {
            let (count, routes) = entry.value();
            if self.first_latency {
                if let Some((route, _)) = routes.first() {
                    return Ok(*route);
                }
            } else {
                let len = routes.len();
                if len != 0 {
                    let index = count.fetch_add(1, Ordering::Relaxed);
                    let route = &routes[index % len].0;
                    // 尝试跳过默认rt的路由(一般是刚加入的)，这有助于提升稳定性
                    if route.rtt != DEFAULT_RTT {
                        return Ok(*route);
                    }
                    for (route, _) in routes {
                        if route.rtt != DEFAULT_RTT {
                            return Ok(*route);
                        }
                    }
                    return Ok(routes[0].0);
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "route not found"))
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
            routes.is_empty()
        });
    }
    pub fn remove_all(&self, id: &PeerID) {
        self.route_table.remove(id);
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
                if i.is_p2p() {
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
                if &route.route_key() == route_key && route.is_p2p() {
                    return Some(id.clone());
                }
            }
        }
        None
    }
    pub fn need_punch(&self, id: &PeerID) -> bool {
        if let Some(entry) = self.route_table.get(id) {
            let (_, routes) = entry.value();
            //p2p的通道数符合要求
            return routes.iter().filter(|(k, _)| k.is_p2p()).count() < self.channel_num;
        }
        true
    }
    pub fn no_need_punch(&self, id: &PeerID) -> bool {
        !self.need_punch(id)
    }
    pub fn p2p_num(&self, id: &PeerID) -> usize {
        if let Some(entry) = self.route_table.get(id) {
            let (_, routes) = entry.value();
            routes.iter().filter(|(k, _)| k.is_p2p()).count()
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
                if route.is_p2p() {
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
        let (_, list) = route_table.value_mut();
        let mut exist = false;
        for (x, time) in list.iter_mut() {
            if x.metric < route.metric && !self.first_latency {
                //非优先延迟的情况下 不能比当前的路径更长
                return false;
            }
            if x.route_key() == key {
                time.store(Instant::now());
                if only_if_absent {
                    return true;
                }
                x.metric = route.metric;
                x.rtt = route.rtt;
                exist = true;
                break;
            }
        }
        if exist {
            list.sort_by_key(|(k, _)| k.rtt);
        } else {
            if !self.first_latency && route.is_p2p() {
                //非优先延迟的情况下 添加了直连的则排除非直连的
                list.retain(|(k, _)| k.is_p2p());
            };
            list.sort_by_key(|(k, _)| k.rtt);
            list.push((route, AtomicCell::new(Instant::now())));
        }
        true
    }
}
