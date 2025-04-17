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
        .group_code(string_to_group_code(&group_code))
        .build()
        .await?;
    let manager = endpoint.kcp_stream();

    if let Some(request) = request {
        let client_kcp_stream = manager.new_stream(NodeID::from(request))?;
        let (mut write, mut read) = client_kcp_stream.split();
        tokio::spawn(async move {
            let mut buf = [0; 1024];
            loop {
                let len = read.read(&mut buf).await.unwrap();
                log::info!("Echo,message={:?}", String::from_utf8(buf[..len].into()));
            }
        });
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut reader = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            println!("input: {}", line);
            if line.trim() == "exit" {
                break;
            }
            write.write_all(line.as_bytes()).await?;
        }
    } else {
        tokio::spawn(async move {
            while let Ok((mut stream, remote_id)) = manager.accept().await {
                let remote_id: u32 = remote_id.into();
                log::info!("=========== accept kcp_stream from {:?}", remote_id);
                tokio::spawn(async move {
                    let mut buf = [0; 1024];
                    loop {
                        let result =
                            tokio::time::timeout(Duration::from_secs(100), stream.read(&mut buf))
                                .await;
                        match result {
                            Ok(Ok(len)) => {
                                if len == 0 {
                                    break;
                                }
                                log::info!(
                                    "read remote_id={remote_id:?},message={:?}",
                                    String::from_utf8(buf[..len].into())
                                );
                                stream.write_all(&buf[..len]).await.unwrap();
                            }
                            Ok(Err(e)) => {
                                log::info!("read remote_id={remote_id:?},error={e:?}",);
                                break;
                            }
                            Err(_) => {
                                log::info!("read remote_id={remote_id:?} timeout");
                                break;
                            }
                        }
                    }
                });
            }
            log::info!("=========== accept kcp_stream end====");
        });
        tokio::signal::ctrl_c().await?;
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
