use std::io;
use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use env_logger::Env;
use pnet_packet::icmp::IcmpTypes;
use pnet_packet::ip::IpNextHeaderProtocols;
use pnet_packet::Packet;
use rustp2p::cipher::Algorithm;
use rustp2p::protocol::node_id::{GroupCode, NodeID};
use rustp2p::tunnel::PeerNodeAddress;
use rustp2p::{Builder, EndPoint};
use tun_rs::AsyncDevice;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Peer node address.
    /// example: --peer tcp://192.168.10.13:23333 --peer udp://192.168.10.23:23333
    #[arg(short, long)]
    peer: Option<Vec<PeerNodeAddress>>,
    /// Local node IP and mask.
    /// example: --local 10.26.0.2/24
    #[arg(short, long)]
    local: String,
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
        peer,
        local,
        group_code,
        port,
    } = Args::parse();
    env_logger::Builder::from_env(Env::default().default_filter_or("debug")).init();
    let mut split = local.split('/');
    let self_id = Ipv4Addr::from_str(split.next().expect("--local error")).expect("--local error");
    let mask = u8::from_str(split.next().expect("--local error")).expect("--local error");
    let addrs = peer.unwrap_or_default();

    let (tx, mut quit) = tokio::sync::mpsc::channel::<()>(1);

    ctrlc::set_handler(move || {
        tx.try_send(()).expect("Could not send signal on channel.");
    })
    .expect("Error setting Ctrl-C handler");

    let dev_builder = tun_rs::DeviceBuilder::new()
        .ipv4(self_id, mask, None)
        .mtu(1400);

    let device = Arc::new(dev_builder.build_async()?);

    let port = port.unwrap_or(23333);

    let endpoint = Arc::new(
        Builder::new()
            .node_id(self_id.into())
            .tcp_port(port)
            .udp_port(port)
            .peers(addrs)
            .group_code(string_to_group_code(&group_code))
            .encryption(Algorithm::AesGcm("password".to_string()))
            .build()
            .await?,
    );

    log::info!("listen local port: {port}");

    let endpoint_clone = endpoint.clone();
    let device_clone = device.clone();
    tokio::spawn(async move {
        while let Ok((data, metadata)) = endpoint_clone.recv_from().await {
            log::debug!(
                "recv from peer from addr: {:?}, {:?} ->{:?} is_relay:{}\n{:?}",
                metadata.route_key().addr(),
                metadata.src_id(),
                metadata.dest_id(),
                metadata.is_relay(),
                pnet_packet::ipv4::Ipv4Packet::new(data.payload())
            );
            if let Err(e) = device_clone.send(data.payload()).await {
                log::warn!("device.send {e:?}")
            }
        }
    });

    tokio::spawn(async move {
        tun_recv(endpoint.clone(), device, self_id).await.unwrap();
    });

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

async fn tun_recv(
    endpoint: Arc<EndPoint>,
    device: Arc<AsyncDevice>,
    _self_id: Ipv4Addr,
) -> io::Result<()> {
    let mut buf = [0; 2048];
    loop {
        let payload_len = device.recv(&mut buf).await?;
        if buf[0] >> 4 != 4 {
            // log::warn!("payload[0] >> 4 != 4");
            continue;
        }
        let mut dest_ip = Ipv4Addr::new(buf[16], buf[17], buf[18], buf[19]);
        if dest_ip.is_unspecified() {
            continue;
        }
        if dest_ip.is_broadcast() || dest_ip.is_multicast() || buf[19] == 255 {
            dest_ip = Ipv4Addr::BROADCAST;
        }
        #[cfg(target_os = "macos")]
        {
            if dest_ip == _self_id {
                if let Err(err) = process_myself(&buf[..payload_len], &device).await {
                    log::error!("process myself err: {err:?}");
                }
                continue;
            }
        }

        if let Err(e) = endpoint
            .send_to(&buf[..payload_len], NodeID::from(dest_ip))
            .await
        {
            log::warn!("{e:?},{dest_ip:?}");
        }
    }
}

#[allow(dead_code)]
async fn process_myself(payload: &[u8], device: &Arc<AsyncDevice>) -> io::Result<()> {
    if let Some(ip_packet) = pnet_packet::ipv4::Ipv4Packet::new(payload) {
        match ip_packet.get_next_level_protocol() {
            IpNextHeaderProtocols::Icmp => {
                let icmp_pkt = pnet_packet::icmp::IcmpPacket::new(ip_packet.payload()).ok_or(
                    std::io::Error::new(std::io::ErrorKind::Other, "invalid icmp packet"),
                )?;
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
