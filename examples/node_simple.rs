use clap::Parser;
use env_logger::Env;
use rustp2p::node_id::NodeID;
use rustp2p::Builder;
use rustp2p::PeerNodeAddress;
use std::io;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Request to specify address
    #[arg(short, long)]
    request: Option<u32>,
    /// Peer node address.
    /// example: --peer tcp://192.168.10.13:23333 --peer udp://192.168.10.23:23333
    #[arg(short, long)]
    peer: Option<Vec<PeerNodeAddress>>,
    /// example: --id 1
    #[arg(short, long)]
    id: u32,
    /// Nodes with the same group_code can form a network
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
        .group_code(group_code.try_into().unwrap())
        .build()
        .await?;
    if let Some(request) = request {
        tokio::time::sleep(Duration::from_secs(3)).await;
        log::info!("=========== send 'hello' to {request}");
        endpoint.send_to(b"hello", NodeID::from(request)).await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
    } else {
        let (data, metadata) = endpoint.recv_from().await?;
        log::info!(
            "=========== recv: {:?} {:?}",
            String::from_utf8(data.payload().into()),
            metadata.src_id()
        )
    }
    log::info!("exit!!!!");
    Ok(())
}
