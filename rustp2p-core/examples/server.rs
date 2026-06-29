use env_logger::Env;
use rust_p2p_core::endpoint::{EndPoint, EndPointConfig};

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    // Create endpoint with UDP + TCP on port 3000
    let mut ep = EndPoint::bind(EndPointConfig {
        udp_port: Some(3000),
        tcp_port: Some(3000),
        ..Default::default()
    })
    .await
    .unwrap();

    log::info!("Server listening on port 3000");

    // Receive and echo back
    while let Some(received) = ep.recv().await {
        let data = String::from_utf8_lossy(&received.data);
        log::info!(
            "Received from {} ({:?}): {}",
            received.route.remote_addr(),
            received.route.protocol(),
            data
        );

        // Echo back
        if let Err(e) = received.route.send(received.data.as_ref()).await {
            log::warn!("Send error: {e}");
        }
    }
}
