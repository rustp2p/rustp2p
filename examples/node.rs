use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use env_logger::Env;
use mimalloc_rust::GlobalMiMalloc;
use tokio::sync::mpsc::{channel, Sender};
use tun_rs::AsyncDevice;

use rustp2p::config::{PipeConfig, TcpPipeConfig, UdpPipeConfig};
use rustp2p::error::*;
use rustp2p::pipe::{PeerNodeAddress, Pipe, PipeLine, PipeWriter, SendPacket};
use rustp2p::protocol::node_id::NodeID;

#[global_allocator]
static GLOBAL_MI_MALLOC: GlobalMiMalloc = GlobalMiMalloc;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Peer node address.
    /// example: --peer tcp://192.168.10.13:23333 --peer udp://192.168.10.23:23333
    #[arg(short, long)]
    peer: Option<Vec<String>>,
    /// Local node IP and mask.
    /// example: --local 10.26.0.2/24
    #[arg(short, long)]
    local: String,
    /// Listen local port
    #[arg(short, long)]
    port: Option<u16>,
}

#[tokio::main]
pub async fn main() -> Result<()> {
    let Args { peer, local, port } = Args::parse();
    env_logger::Builder::from_env(Env::default().default_filter_or("debug")).init();
    let mut split = local.split("/");
    let self_id = Ipv4Addr::from_str(split.next().expect("--local error")).expect("--local error");
    let mask = u8::from_str(split.next().expect("--local error")).expect("--local error");
    let mut addrs = Vec::new();
    if let Some(peers) = peer {
        for addr in peers {
            addrs.push(addr.parse::<PeerNodeAddress>().expect("--peer"))
        }
    }
    let device = tun_rs::create_as_async(
        tun_rs::Configuration::default()
            .address_with_prefix(self_id, mask)
            .platform_config(|_v| {
                #[cfg(windows)]
                _v.ring_capacity(4 * 1024 * 1024);
                #[cfg(target_os = "linux")]
                _v.tx_queue_len(1000);
            })
            .mtu(1400)
            .up(),
    )
    .unwrap();
    #[cfg(target_os = "macos")]
    {
        use tun_rs::AbstractDevice;
        device.set_ignore_packet_info(true);
    }
    let device = Arc::new(device);
    let port = port.unwrap_or(23333);
    let udp_config = UdpPipeConfig::default().set_udp_ports(vec![port]);
    let tcp_config = TcpPipeConfig::default().set_tcp_port(port);
    let config = PipeConfig::empty()
        .set_udp_pipe_config(udp_config)
        .set_tcp_pipe_config(tcp_config)
        .set_direct_addrs(addrs)
        .set_node_id(self_id.into());

    let mut pipe = Pipe::new(config).await?;
    let writer = pipe.writer();
    //let shutdown_writer = writer.clone();
    let device_r = device.clone();
    let (sender1, mut receiver1) = channel::<(Vec<u8>, usize, usize)>(128);
    let (sender2, mut receiver2) = channel(128);
    tokio::spawn(async move {
        tun_recv(sender2, writer, device_r).await.unwrap();
    });
    let writer = pipe.writer();
    tokio::spawn(async move {
        while let Some((mut packet, dest)) = receiver2.recv().await {
            if let Err(e) = writer.send_to_packet(&mut packet, &dest).await {
                log::warn!("writer.send {e:?}")
            }
        }
    });
    tokio::spawn(async move {
        while let Some((buf, start, end)) = receiver1.recv().await {
            if let Err(e) = device.send(&buf[start..end]).await {
                log::warn!("device.send {e:?}")
            }
        }
    });
    log::info!("listen {port}");
    // tokio::spawn(async move{
    // 	tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    // 	_ = shutdown_writer.shutdown();
    // });
    loop {
        let line = pipe.accept().await?;
        tokio::spawn(recv(line, sender1.clone()));
    }
}
async fn recv(mut line: PipeLine, sender: Sender<(Vec<u8>, usize, usize)>) {
    loop {
        let mut buf = Vec::with_capacity(2048);
        unsafe {
            buf.set_len(2048);
        }
        let rs = match line.recv_from(&mut buf).await {
            Ok(rs) => rs,
            Err(e) => {
                log::warn!("recv_from {e:?}");
                return;
            }
        };
        let handle_rs = match rs {
            Ok(handle_rs) => handle_rs,
            Err(e) => {
                log::warn!("recv_data_handle {e:?}");
                continue;
            }
        };
        if let Err(e) = sender.send((buf, handle_rs.start, handle_rs.end)).await {
            log::warn!("UserData {e:?},{handle_rs:?}")
        }
    }
}
async fn tun_recv(
    sender: Sender<(SendPacket, NodeID)>,
    pipe_writer: PipeWriter,
    device: Arc<AsyncDevice>,
) -> Result<()> {
    loop {
        let mut send_packet = pipe_writer.allocate_send_packet()?;
        let payload = send_packet.data_mut();
        let payload_len = device.recv(payload).await?;
        if payload[0] >> 4 != 4 {
            continue;
        }
        let mut dest_ip = Ipv4Addr::new(payload[16], payload[17], payload[18], payload[19]);
        if dest_ip.is_unspecified() {
            continue;
        }
        if dest_ip.is_broadcast() || dest_ip.is_multicast() || payload[19] == 255 {
            dest_ip = Ipv4Addr::BROADCAST;
        }
        send_packet.set_payload_len(payload_len);
        if let Err(e) = sender.send((send_packet, dest_ip.into())).await {
            log::warn!("{e:?},{dest_ip:?}")
        }
    }
}
