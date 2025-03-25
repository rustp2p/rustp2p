use crate::tunnel::node_context::NodeContext;
use crate::tunnel::TunnelTransmitHub;
use rand::seq::SliceRandom;
use rust_p2p_core::socket::LocalInterface;
use std::time::Duration;

pub(crate) async fn nat_test_loop(
    tunnel_tx: TunnelTransmitHub,
    mut udp_stun_servers: Vec<String>,
    default_interface: Option<LocalInterface>,
) {
    let node_context = tunnel_tx.node_context();
    loop {
        udp_stun_servers.shuffle(&mut rand::rng());
        let udp_stun_servers = if udp_stun_servers.len() > 3 {
            &udp_stun_servers[..3]
        } else {
            &udp_stun_servers
        };
        nat_test(
            &tunnel_tx,
            tunnel_tx.node_context(),
            udp_stun_servers,
            default_interface.as_ref(),
        )
        .await;
        if node_context.exists_nat_info() {
            tokio::time::sleep(Duration::from_secs(10 * 60)).await;
        } else {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}

async fn nat_test(
    tunnel_tx: &TunnelTransmitHub,
    node_context: &NodeContext,
    udp_stun_servers: &[String],
    default_interface: Option<&LocalInterface>,
) {
    let local_ipv4 = rust_p2p_core::extend::addr::local_ipv4().await;
    let local_ipv6 = rust_p2p_core::extend::addr::local_ipv6().await;
    {
        let mut guard = node_context.punch_info().write();
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
    let rs = rust_p2p_core::stun::stun_test_nat(udp_stun_servers.to_vec(), default_interface).await;

    match rs {
        Ok((nat_type, ips, port_range)) => {
            if let Err(err) = tunnel_tx.switch_model(nat_type) {
                log::error!("switch to {nat_type:?} model error:{err:?}");
            }
            let mut guard = node_context.punch_info().write();
            guard.nat_type = nat_type;
            guard.set_public_ip(ips);
            guard.public_port_range = port_range;
        }
        Err(e) => {
            log::debug!("stun_test_nat {e:?} {udp_stun_servers:?}")
        }
    }
}
