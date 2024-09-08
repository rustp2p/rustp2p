use crate::pipe::pipe_context::PipeContext;
use crate::pipe::PipeWriter;
use rust_p2p_core::socket::LocalInterface;
use std::time::Duration;

pub(crate) async fn nat_test_loop(
    pipe_writer: PipeWriter,
    stun_servers: Vec<String>,
    default_interface: Option<LocalInterface>,
) {
    let pipe_context = pipe_writer.pipe_context();
    loop {
        nat_test(
            pipe_writer.pipe_context(),
            &stun_servers,
            default_interface.as_ref(),
        )
        .await;
        if pipe_context.exists_nat_info() {
            tokio::time::sleep(Duration::from_secs(10 * 60)).await;
        } else {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}

async fn nat_test(
    pipe_context: &PipeContext,
    stun_servers: &Vec<String>,
    default_interface: Option<&LocalInterface>,
) {
    let rs = rust_p2p_core::stun::stun_test_nat(stun_servers.clone(), default_interface).await;
    let local_ipv4 = rust_p2p_core::extend::addr::local_ipv4().await;
    let local_ipv6 = rust_p2p_core::extend::addr::local_ipv6().await;
    let mut guard = pipe_context.punch_info().write();
    match rs {
        Ok((nat_type, ips, port_range)) => {
            guard.nat_type = nat_type;
            guard.set_public_ip(ips);
            guard.public_port_range = port_range;
        }
        Err(e) => {
            log::debug!("stun_test_nat {e:?} {stun_servers:?}")
        }
    }
    match local_ipv4 {
        Ok(local_ipv4) => {
            guard.local_ipv4 = local_ipv4;
        }
        Err(e) => {
            log::debug!("local_ipv4 {e:?}")
        }
    }
    match local_ipv6 {
        Ok(local_ipv6) => {
            if rust_p2p_core::extend::addr::is_ipv6_global(&local_ipv6) {
                guard.ipv6.replace(local_ipv6);
            }
        }
        Err(e) => {
            log::debug!("local_ipv6 {e:?}")
        }
    }
}
