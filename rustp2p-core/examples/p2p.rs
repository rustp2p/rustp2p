use env_logger::Env;
use rust_p2p_core::endpoint::{Config, EndPoint};
use std::net::SocketAddr;

/// Simple P2P example demonstrating two endpoints communicating.
///
/// Run server first: cargo run --example p2p -- --mode server --port 4000
/// Then run client:  cargo run --example p2p -- --mode client --port 4001 --peer 127.0.0.1:4000

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    let mode = args
        .iter()
        .position(|a| a == "--mode")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("server");
    let port: u16 = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);
    let peer: Option<SocketAddr> = args
        .iter()
        .position(|a| a == "--peer")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    match mode {
        "server" => run_server(port).await,
        "client" => run_client(port, peer.expect("--peer required for client mode")).await,
        _ => eprintln!("Unknown mode: {mode}"),
    }
}

async fn run_server(port: u16) {
    let mut ep = EndPoint::bind(Config::new().udp_port(port).tcp_port(port))
        .await
        .unwrap();

    log::info!("Server listening on {:?}", ep.local_addr().await);

    while let Some(received) = ep.recv().await {
        let data = String::from_utf8_lossy(&received.data);
        log::info!(
            "Server received from {} ({:?}): {}",
            received.transport.remote_addr(),
            received.transport.protocol(),
            data
        );

        // Echo back
        if let Err(e) = received.transport.send(received.data.as_ref()).await {
            log::warn!("Server send error: {e}");
        }
    }
}

async fn run_client(port: u16, server_addr: SocketAddr) {
    let mut ep = EndPoint::bind(Config::new().udp_port(port).tcp_port(port))
        .await
        .unwrap();

    log::info!("Client listening on {:?}", ep.local_addr().await);

    // Send message to server
    let msg = b"Hello from P2P client!";
    log::info!("Sending to server at {server_addr}");
    ep.pool().try_send_via_all(msg, server_addr);

    // Wait for echo response
    tokio::select! {
        Some(received) = ep.recv() => {
            let data = String::from_utf8_lossy(&received.data);
            log::info!(
                "Client received from {} ({:?}): {}",
                received.transport.remote_addr(),
                received.transport.protocol(),
                data
            );
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
            log::warn!("Timeout waiting for server response");
        }
    }

    log::info!("Client done");
}
