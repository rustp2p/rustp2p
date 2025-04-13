use clap::Parser;
use env_logger::Env;
use rustp2p::protocol::node_id::{GroupCode, NodeID};
use rustp2p::tunnel::PeerNodeAddress;
use rustp2p::Builder;
use std::io;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Request to specify ID
    #[arg(short, long)]
    request: Option<u32>,
    /// Peer node address.
    /// example: --peer tcp://192.168.10.13:23333 --peer udp://192.168.10.23:23333
    #[arg(short, long)]
    peer: Option<Vec<PeerNodeAddress>>,
    /// example: --id 1
    #[arg(short, long)]
    id: u32,
    /// Nodes with the same group_comde can form a network
    #[arg(short, long)]
    group_code: String,
    /// Listen local port
    #[arg(short = 'P', long)]
    port: Option<u16>,
}

#[tokio::main]
pub async fn main() -> io::Result<()> {
    let Args {
        request,
        peer,
        id,
        group_code,
        port,
    } = Args::parse();
    env_logger::Builder::from_env(Env::default().default_filter_or("debug")).init();
    let addrs = peer.unwrap_or_default();

    if let Some(port) = port {
        log::info!("listen local port: {port}");
    }
    let port = port.unwrap_or(23333);

    let endpoint = Builder::new()
        .node_id(id.into())
        .tcp_port(port)
        .udp_port(port)
        .peers(addrs)
        .group_code(string_to_group_code(&group_code))
        .build()
        .await?;
    let manager = endpoint.kcp_stream();

    if let Some(request) = request {
        tokio::time::sleep(Duration::from_secs(3)).await;
        log::info!("=========== send kcp_stream 'hello' to {}", request);
        let mut client_kcp_stream = manager.new_stream(NodeID::from(request), 1)?;
        client_kcp_stream.write_all(b"hello").await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    } else {
        let (mut server_kcp_stream, node_id) = manager.accept().await?;
        log::info!("=========== accept kcp_stream from {:?}", node_id);
        let mut buf = [0; 1024];
        let len = server_kcp_stream.read(&mut buf).await?;
        log::info!(
            "=========== read kcp_stream from {:?},buf = {:?}",
            node_id,
            &buf[..len]
        );
    }
    log::info!("exit!!!!");
    Ok(())
}
fn string_to_group_code(input: &str) -> GroupCode {
    let mut array = [0u8; 16];
    let bytes = input.as_bytes();
    let len = bytes.len().min(16);
    array[..len].copy_from_slice(&bytes[..len]);
    array.into()
}
