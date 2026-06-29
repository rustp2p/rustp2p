use rustp2p_quic::Endpoint;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> rustp2p_quic::Result<()> {
    env_logger::init();

    let addr: SocketAddr = "0.0.0.0:4433".parse().unwrap();
    let endpoint = Endpoint::bind(addr).await?;

    println!("Server listening on: {:?}", endpoint.addr());
    println!("Node ID: {}", hex::encode(endpoint.node_id()));

    while let Some(connection) = endpoint.accept().await {
        let remote = connection.remote_addr();
        println!("New connection from: {remote}");

        tokio::spawn(async move {
            loop {
                match connection.accept_bi().await {
                    Ok((mut send, mut recv)) => {
                        let remote = connection.remote_addr();
                        tokio::spawn(async move {
                            let mut buf = [0u8; 1024];
                            loop {
                                match recv.read(&mut buf).await {
                                    Ok(Some(n)) => {
                                        println!("[{remote}] received {n} bytes");
                                        if let Err(e) = send.write_all(&buf[..n]).await {
                                            eprintln!("send error: {e}");
                                            break;
                                        }
                                    }
                                    Ok(None) => {
                                        println!("[{remote}] stream finished");
                                        break;
                                    }
                                    Err(e) => {
                                        eprintln!("[{remote}] recv error: {e}");
                                        break;
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("accept_bi error: {e}");
                        break;
                    }
                }
            }
        });
    }

    Ok(())
}
