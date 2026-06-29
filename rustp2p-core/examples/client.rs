use env_logger::Env;
use rust_p2p_core::endpoint::{EndPoint, EndPointConfig};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    // Create endpoint on random port
    let mut ep = EndPoint::bind(EndPointConfig::default()).await.unwrap();
    log::info!("Client listening on {:?}", ep.local_addr().await);

    // Connect to server (simplified - just send to server address)
    let server_addr: SocketAddr = "127.0.0.1:3000".parse().unwrap();

    // Send a message
    let msg = b"Hello from client!";
    log::info!("Sending to {server_addr}");

    // For UDP, we need a Route to send. In real usage, we'd get a Route from recv().
    // Here we demonstrate the API by receiving and echoing.

    // Send via any available UDP socket in the pool
    // (In real P2P, you'd use Route obtained from a previous recv)

    log::info!("Client started. Waiting for server responses...");

    // Simple echo client: send, then wait for response
    while let Some(received) = ep.recv().await {
        let data = String::from_utf8_lossy(&received.data);
        log::info!(
            "Received from {} ({:?}): {}",
            received.route.remote_addr(),
            received.route.protocol(),
            data
        );
    }
}
