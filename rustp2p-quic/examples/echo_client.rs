use rustp2p_quic::{Endpoint, NodeAddr};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> rustp2p_quic::Result<()> {
    env_logger::init();

    // Bind on random port
    let endpoint = Endpoint::bind("0.0.0.0:0".parse().unwrap()).await?;
    println!("Client node ID: {}", hex::encode(endpoint.node_id()));

    // Connect to server
    let server_addr: SocketAddr = "127.0.0.1:4433".parse().unwrap();
    let node_addr = NodeAddr::new([0u8; 32], vec![server_addr]);
    let connection = endpoint.connect(node_addr).await?;
    println!("Connected to: {}", connection.remote_addr());

    // Open a bidirectional stream
    let (mut send, mut recv) = connection.open_bi().await?;

    // Send some data
    let messages = vec![b"Hello, QUIC!".as_ref(), b"P2P is cool!", b"Goodbye!"];
    for msg in messages {
        println!("Sending: {}", String::from_utf8_lossy(msg));
        send.write_all(msg).await?;

        // Read echo response
        let mut buf = [0u8; 1024];
        match recv.read(&mut buf).await? {
            Some(n) => {
                let response = String::from_utf8_lossy(&buf[..n]);
                println!("Received: {response}");
            }
            None => {
                println!("Server closed stream");
                break;
            }
        }
    }

    // Finish the stream
    send.finish()?;
    println!("Done!");

    Ok(())
}
