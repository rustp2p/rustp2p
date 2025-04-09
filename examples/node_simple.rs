use std::io;
use std::net::Ipv4Addr;

use clap::Parser;
use env_logger::Env;
use rustp2p::cipher::Algorithm;
use rustp2p::protocol::node_id::{GroupCode, NodeID};
use rustp2p::tunnel::PeerNodeAddress;
use rustp2p::Builder;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Request to specify address
    #[arg(short, long)]
    request: Option<Ipv4Addr>,
    /// Peer node address.
    /// example: --peer tcp://192.168.10.13:23333 --peer udp://192.168.10.23:23333
    #[arg(short, long)]
    peer: Option<Vec<PeerNodeAddress>>,
    /// Local node IP and mask.
    /// example: --local 10.26.0.2
    #[arg(short, long)]
    local: Ipv4Addr,
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
        local,
        group_code,
        port,
    } = Args::parse();
    env_logger::Builder::from_env(Env::default().default_filter_or("debug")).init();
    let addrs = peer.unwrap_or_default();

    let (tx, mut quit) = tokio::sync::mpsc::channel::<()>(1);

    ctrlc::set_handler(move || {
        tx.try_send(()).expect("Could not send signal on channel.");
    })
    .expect("Error setting Ctrl-C handler");

    if let Some(port) = port {
        log::info!("listen local port: {port}");
    }
    let port = port.unwrap_or(23333);

    let endpoint = Builder::new()
        .node_id(local.into())
        .tcp_port(port)
        .udp_port(port)
        .peers(addrs)
        .group_code(string_to_group_code(&group_code))
        .encryption(Algorithm::AesGcm("password".to_string()))
        .build()
        .await?;
    if let Some(request) = request {
        endpoint.send_to(b"hello", NodeID::from(request)).await?;
    } else {
        let (data, metadata) = endpoint.recv_from().await?;
        println!("recv: {:?} {:?}", data.payload(), metadata.src_id())
    }
    quit.recv().await.expect("quit error");
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
