use crate::Puncher;
use async_shutdown::ShutdownManager;
use tokio::task::JoinSet;

mod nat_query;
mod query_public_addr;

pub(crate) fn start_task(shutdown_manager: ShutdownManager<()>, puncher: Puncher) {
    let mut join_set = JoinSet::new();
    join_set.spawn(nat_query::nat_test_loop(puncher.clone()));
    join_set.spawn(query_public_addr::query_tcp_public_addr_loop(
        puncher.clone(),
    ));
    join_set.spawn(query_public_addr::query_udp_public_addr_loop(
        puncher.clone(),
    ));
    let mut join_set = join_set;
    let fut =
        shutdown_manager.wrap_cancel(async move { while join_set.join_next().await.is_some() {} });
    tokio::spawn(async move {
        if fut.await.is_err() {
            log::debug!("recv shutdown signal: built-in maintain tasks are shutdown");
        }
    });
}
