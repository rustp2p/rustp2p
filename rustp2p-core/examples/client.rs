use env_logger::Env;
use rust_p2p_core::endpoint::{Config, EndPoint};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let ep = EndPoint::bind(Config::default()).await.unwrap();
    log::info!("Client listening on {:?}", ep.local_addr().await);

    let server_addr: SocketAddr = "127.0.0.1:3000".parse().unwrap();

    // Send a message to server via the pool
    let msg = b"Hello from client!";
    log::info!("Sending to {server_addr}");
    ep.pool().try_send_via_all(msg, server_addr);

    // Wait for echo response
    let mut ep = ep;
    tokio::select! {
        Some(received) = ep.recv() => {
            let data = String::from_utf8_lossy(&received.data);
            log::info!(
                "Received from {} ({:?}): {}",
                received.transport.remote_addr(),
                received.transport.protocol(),
                data
            );
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {
            log::warn!("Timeout waiting for response");
        }
    }

    log::info!("Done");
}
