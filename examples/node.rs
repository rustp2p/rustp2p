use clap::Parser;
use rustp2p::config::{PipeConfig, TcpPipeConfig, UdpPipeConfig};
use rustp2p::error::*;
use rustp2p::pipe::{HandleResult, Pipe, PipeLine, RecvResult};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Peer node address.
    /// example: --peer tcp://192.168.10.13:23333
    #[arg(short, long)]
    peer: Option<String>,
    /// Local node IP and mask.
    /// example: --local 10.26.0.2/24
    #[arg(short, long)]
    local: String,
}

#[tokio::main]
pub async fn main() -> Result<()> {
    let Args { peer, local } = Args::parse();
    let udp_config = UdpPipeConfig::default().set_udp_ports(vec![23333, 23334]);
    let tcp_config = TcpPipeConfig::default().set_tcp_port(23333);
    let config = PipeConfig::empty()
        .set_udp_pipe_config(udp_config)
        .set_tcp_pipe_config(tcp_config);
    let mut pipe = Pipe::new(config)?;
    loop {
        let line = pipe.accept().await?;
        tokio::spawn(async move {
            if let Err(e) = recv(line).await {
                log::warn!("recv {e:?}")
            }
        });
    }
}
async fn recv(mut line: PipeLine) -> Result<()> {
    let mut buf = [0; 65535];
    while let Ok(rs) = line.recv_from(&mut buf).await {
        let handle_rs = match rs {
            Ok(handle_rs) => handle_rs,
            Err(e) => {
                log::warn!("recv_data_handle {e:?}");
                continue;
            }
        };
        match handle_rs {
            HandleResult::Turn(buf, dest_id, _) => {
                line.send_to(buf.buffer(), &dest_id).await?;
            }
            HandleResult::UserData(buf, src_id, route_key) => {
                todo!()
            }
        }
    }
    Ok(())
}
