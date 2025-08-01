use clap::Parser;
use env_logger::Env;
use futures::{SinkExt, StreamExt};
use rustp2p::node_id::NodeID;
use rustp2p::Builder;
use rustp2p::PeerNodeAddress;
use std::io;
use std::time::Duration;
use tokio_util::codec::{Framed, FramedRead, FramedWrite, LengthDelimitedCodec};

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
        .group_code(group_code.try_into().unwrap())
        .build()
        .await?;
    let kcp_listener = endpoint.kcp_listener();

    if let Some(request) = request {
        let client_kcp_stream = endpoint.open_kcp_stream(NodeID::from(request))?;
        let (write, read) = client_kcp_stream.split();
        let mut framed_write = FramedWrite::new(write, LengthDelimitedCodec::new());

        let mut framed_read = FramedRead::new(read, LengthDelimitedCodec::new());

        tokio::spawn(async move {
            loop {
                let buf = framed_read.next().await.unwrap().unwrap();
                log::info!("Echo,message={:?}", std::str::from_utf8(&buf));
            }
        });
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut reader = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            println!("input: {}", line);
            if line.trim() == "exit" {
                break;
            }
            framed_write.send(line.as_bytes().to_vec().into()).await?;
        }
    } else {
        tokio::spawn(async move {
            while let Ok((stream, remote_id)) = kcp_listener.accept().await {
                let remote_id: u32 = remote_id.into();
                let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
                log::info!("=========== accept kcp_stream from {:?}", remote_id);
                tokio::spawn(async move {
                    loop {
                        let result =
                            tokio::time::timeout(Duration::from_secs(100), framed.next()).await;
                        match result {
                            Ok(Some(Ok(buf))) => {
                                log::info!(
                                    "read remote_id={remote_id:?},message={:?}",
                                    std::str::from_utf8(&buf)
                                );
                                framed.send(buf.freeze()).await.unwrap();
                            }
                            Ok(e) => {
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
