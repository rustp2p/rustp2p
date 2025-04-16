use crate::Puncher;
use rust_p2p_core::nat::NatType;
use rust_p2p_core::tunnel::udp::Model;
use std::time::Duration;

pub(crate) async fn nat_test_loop(puncher: Puncher) {
    loop {
        nat_test(&puncher).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

async fn nat_test(puncher: &Puncher) {
    match puncher.punch_context.update_info().await {
        Ok(nat_info) => match nat_info.nat_type {
            NatType::Cone => {
                if let Some(socket_manager) = puncher.socket_manager.udp_socket_manager_as_ref() {
                    _ = socket_manager.switch_model(Model::Low);
                }
            }
            NatType::Symmetric => {
                if let Some(socket_manager) = puncher.socket_manager.udp_socket_manager_as_ref() {
                    _ = socket_manager.switch_model(Model::High);
                }
            }
        },
        Err(e) => {
            log::debug!("stun_test_nat {e:?} ")
        }
    }
}
