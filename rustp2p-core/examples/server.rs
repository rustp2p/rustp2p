use env_logger::Env;
use rust_p2p_core::endpoint::{Config, EndPoint};

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let mut ep = EndPoint::bind(Config::new().udp_port(3000).tcp_port(3000))
        .await
        .unwrap();

    log::info!("Server listening on {:?}", ep.local_addr().await);

    while let Some(received) = ep.recv().await {
        let data = String::from_utf8_lossy(&received.data);
        log::info!(
            "Received from {} ({:?}): {}",
            received.transport.remote_addr(),
            received.transport.protocol(),
            data
        );

        if let Err(e) = received.transport.send(received.data.as_ref()).await {
            log::warn!("Send error: {e}");
        }
    }
}
