use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use env_logger::Env;
use mimalloc_rust::GlobalMiMalloc;
use pnet_packet::icmp::IcmpTypes;
use pnet_packet::ip::IpNextHeaderProtocols;
use pnet_packet::Packet;
use rustp2p::protocol::NetPacket;
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
    let (tx, mut quit) = tokio::sync::mpsc::channel::<()>(1);

    ctrlc2::set_async_handler(async move {
        tx.send(()).await.expect("Signal error");
    })
    .await;
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
    let shutdown_writer = writer.clone();
    let device_r = device.clone();
    let (sender1, mut receiver1) = channel::<(Vec<u8>, usize, usize)>(128);
    let (sender2, mut receiver2) = channel(128);
    tokio::spawn(async move {
        tun_recv(sender2, writer, device_r, self_id).await.unwrap();
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
    log::info!("listen local port: {port}");

    tokio::spawn(async move {
        loop {
            let line = pipe.accept().await?;
            let writer = pipe.writer();
            tokio::spawn(recv(line, sender1.clone(), writer));
        }
        #[allow(unreachable_code)]
        Ok::<(), Error>(())
    });

    quit.recv().await.expect("quit error");
    _ = shutdown_writer.shutdown();
    log::info!("exit!!!!");
    Ok(())
}
async fn recv(
    mut line: PipeLine,
    sender: Sender<(Vec<u8>, usize, usize)>,
    mut _pipe_wirter: PipeWriter,
) {
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
        let net_pkt = NetPacket::unchecked(&buf);
        log::info!(
            "recv from route_key: {:?} is_relay:{}",
            handle_rs.route_key,
            if (net_pkt.max_ttl() - net_pkt.ttl()) != 0 {
                true
            } else {
                false
            }
        );
        let payload = &buf[handle_rs.start..handle_rs.end];
        log::info!(
            "read pkt from peer: {:?}",
            pnet_packet::ipv4::Ipv4Packet::new(payload)
        );
        if is_icmp_request(payload).await {
            if let Err(err) = process_icmp(payload, &mut _pipe_wirter).await {
                log::error!("reply icmp error: {err:?}");
            }
            continue;
        }
        if let Err(e) = sender.send((buf, handle_rs.start, handle_rs.end)).await {
            log::warn!("UserData {e:?},{handle_rs:?}")
        }
    }
}
async fn tun_recv(
    sender: Sender<(SendPacket, NodeID)>,
    pipe_writer: PipeWriter,
    device: Arc<AsyncDevice>,
    _self_id: Ipv4Addr,
) -> Result<()> {
    loop {
        let mut send_packet = pipe_writer.allocate_send_packet()?;
        let payload = send_packet.data_mut();
        let payload_len = device.recv(payload).await?;
        if payload[0] >> 4 != 4 {
            log::warn!("payload[0] >> 4 != 4");
            continue;
        }
        let mut dest_ip = Ipv4Addr::new(payload[16], payload[17], payload[18], payload[19]);
        if dest_ip.is_unspecified() {
            continue;
        }
        if dest_ip.is_broadcast() || dest_ip.is_multicast() || payload[19] == 255 {
            dest_ip = Ipv4Addr::BROADCAST;
        }
        #[cfg(target_os = "macos")]
        {
            if dest_ip == _self_id {
                if let Err(err) = process_myself(payload, &device).await {
                    log::error!("process myself err: {err:?}");
                }
                continue;
            }
        }
        log::info!(
            "read tun pkt: {:?}",
            pnet_packet::ipv4::Ipv4Packet::new(&payload[..payload_len])
        );
        send_packet.set_payload_len(payload_len);
        if let Err(e) = sender.send((send_packet, dest_ip.into())).await {
            log::warn!("{e:?},{dest_ip:?}")
        }
    }
}

#[allow(dead_code)]
async fn process_myself(payload: &[u8], device: &Arc<AsyncDevice>) -> Result<()> {
    if let Some(ip_packet) = pnet_packet::ipv4::Ipv4Packet::new(payload) {
        match ip_packet.get_next_level_protocol() {
            IpNextHeaderProtocols::Icmp => {
                let icmp_pkt = pnet_packet::icmp::IcmpPacket::new(ip_packet.payload())
                    .ok_or(std::io::Error::other("invalid icmp packet"))?;
                if IcmpTypes::EchoRequest == icmp_pkt.get_icmp_type() {
                    let mut v = ip_packet.payload().to_owned();
                    let mut icmp_new =
                        pnet_packet::icmp::MutableIcmpPacket::new(&mut v[..]).unwrap();
                    icmp_new.set_icmp_type(IcmpTypes::EchoReply);
                    icmp_new.set_checksum(pnet_packet::icmp::checksum(&icmp_new.to_immutable()));
                    let len = ip_packet.packet().len();
                    let mut buf = vec![0u8; len];
                    let mut res = pnet_packet::ipv4::MutableIpv4Packet::new(&mut buf).unwrap();
                    res.set_total_length(ip_packet.get_total_length());
                    res.set_header_length(ip_packet.get_header_length());
                    res.set_destination(ip_packet.get_source());
                    res.set_source(ip_packet.get_destination());
                    res.set_identification(0x42);
                    res.set_next_level_protocol(IpNextHeaderProtocols::Icmp);
                    res.set_payload(&v);
                    res.set_ttl(64);
                    res.set_version(ip_packet.get_version());
                    res.set_checksum(pnet_packet::ipv4::checksum(&res.to_immutable()));
                    device.send(&buf).await?;
                }
            }
            IpNextHeaderProtocols::Tcp => {
                device.send(payload).await?;
            }
            IpNextHeaderProtocols::Udp => {
                device.send(payload).await?;
            }
            other => {
                log::warn!("{other:?} is not processed by myself");
            }
        }
    };
    Ok(())
}

#[allow(dead_code)]
async fn process_icmp(payload: &[u8], writer: &mut PipeWriter) -> Result<()> {
    if let Some(ip_packet) = pnet_packet::ipv4::Ipv4Packet::new(payload) {
        match ip_packet.get_next_level_protocol() {
            IpNextHeaderProtocols::Icmp => {
                let icmp_pkt = pnet_packet::icmp::IcmpPacket::new(ip_packet.payload())
                    .ok_or(std::io::Error::other("invalid icmp packet"))?;
                if IcmpTypes::EchoRequest == icmp_pkt.get_icmp_type() {
                    let dest_id = NodeID::from(ip_packet.get_source());
                    let mut v = ip_packet.payload().to_owned();
                    let mut icmp_new =
                        pnet_packet::icmp::MutableIcmpPacket::new(&mut v[..]).unwrap();
                    icmp_new.set_icmp_type(IcmpTypes::EchoReply);
                    icmp_new.set_checksum(pnet_packet::icmp::checksum(&icmp_new.to_immutable()));
                    let len = ip_packet.packet().len();
                    let mut buf = vec![0u8; len];
                    let mut res = pnet_packet::ipv4::MutableIpv4Packet::new(&mut buf).unwrap();
                    res.set_total_length(ip_packet.get_total_length());
                    res.set_header_length(ip_packet.get_header_length());
                    res.set_destination(ip_packet.get_source());
                    res.set_source(ip_packet.get_destination());
                    res.set_identification(0x42);
                    res.set_next_level_protocol(IpNextHeaderProtocols::Icmp);
                    res.set_payload(&v);
                    res.set_ttl(64);
                    res.set_version(ip_packet.get_version());
                    res.set_checksum(pnet_packet::ipv4::checksum(&res.to_immutable()));
                    let mut send_packet = writer.allocate_send_packet()?;
                    send_packet.set_payload(&buf)?;
                    writer.send_to_packet(&mut send_packet, &dest_id).await?;
                }
            }
            other => {
                log::warn!("{other:?} is not processed by this");
            }
        }
    };
    Ok(())
}

async fn is_icmp_request(payload: &[u8]) -> bool {
    if let Some(ip_packet) = pnet_packet::ipv4::Ipv4Packet::new(payload) {
        if ip_packet.get_next_level_protocol() == IpNextHeaderProtocols::Icmp {
            if let Some(icmp_pkt) = pnet_packet::icmp::IcmpPacket::new(ip_packet.payload()) {
                return icmp_pkt.get_icmp_type() == IcmpTypes::EchoRequest;
            }
        }
    }
    false
}
