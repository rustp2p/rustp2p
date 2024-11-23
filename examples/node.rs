use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use env_logger::Env;
use pnet_packet::icmp::IcmpTypes;
use pnet_packet::ip::IpNextHeaderProtocols;
use pnet_packet::Packet;
use rustp2p::cipher::Algorithm;
use rustp2p::config::{PipeConfig, TcpPipeConfig, UdpPipeConfig};
use rustp2p::error::*;
use rustp2p::pipe::{PeerNodeAddress, Pipe, PipeLine, PipeWriter, RecvUserData};
use rustp2p::protocol::node_id::GroupCode;
use tachyonix::{channel, Sender};
use tun_rs::AsyncDevice;

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
    /// Nodes with the same group_comde can form a network
    #[arg(short, long)]
    group_code: String,
    /// Listen local port
    #[arg(short = 'P', long)]
    port: Option<u16>,
}

#[cfg(feature = "use-tokio")]
#[tokio::main]
async fn main() -> Result<()> {
    main0().await
}

#[cfg(feature = "use-async-std")]
#[::async_std::main]
async fn main() -> Result<()> {
    use futures_util::FutureExt;
    main0().boxed().await
}

#[allow(unused_mut)]
pub async fn main0() -> Result<()> {
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
    let mut addrs = Vec::new();
    if let Some(peers) = peer {
        for addr in peers {
            addrs.push(addr.parse::<PeerNodeAddress>().expect("--peer"))
        }
    }

    let (tx, mut quit) = rust_p2p_core::async_compat::mpsc::channel::<()>(1);

    ctrlc::set_handler(move || {
        tx.try_send(()).expect("Could not send signal on channel.");
    })
    .expect("Error setting Ctrl-C handler");

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
        .set_group_code(string_to_group_code(&group_code))
        .set_encryption(Algorithm::AesGcm("password".to_string()))
        .set_node_id(self_id.into());

    let mut pipe = Pipe::new(config).await?;
    let writer = pipe.writer();
    let shutdown_writer = writer.clone();
    let device_r = device.clone();
    let (sender1, mut receiver1) = channel::<RecvUserData>(128);
    rust_p2p_core::async_compat::spawn(async move {
        tun_recv(writer, device_r, self_id).await.unwrap();
    });

    rust_p2p_core::async_compat::spawn(async move {
        while let Ok(data) = receiver1.recv().await {
            if let Err(e) = device.send(data.payload()).await {
                log::warn!("device.send {e:?}")
            }
        }
    });
    log::info!("listen local port: {port}");

    rust_p2p_core::async_compat::spawn(async move {
        loop {
            let line = pipe.accept().await?;
            let writer = pipe.writer();
            rust_p2p_core::async_compat::spawn(recv(line, sender1.clone(), writer));
        }
        #[allow(unreachable_code)]
        Ok::<(), Error>(())
    });

    quit.recv().await.expect("quit error");
    _ = shutdown_writer.shutdown();
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
async fn recv(mut line: PipeLine, sender: Sender<RecvUserData>, _pipe_wirter: PipeWriter) {
    let mut list = Vec::with_capacity(16);
    loop {
        let rs = match line.recv_multi(&mut list).await {
            Ok(rs) => rs,
            Err(e) => {
                log::warn!("recv_from {e:?},{:?}", line.protocol());
                return;
            }
        };
        if let Err(e) = rs {
            log::warn!("recv_data_handle {e:?}");
            continue;
        };
        // log::info!(
        //     "recv from peer from addr: {:?}, {:?} ->{:?} is_relay:{}\n{:?}",
        //     handle_rs.route_key().addr(),
        //     handle_rs.src_id(),
        //     handle_rs.dest_id(),
        //     handle_rs.is_relay(),
        //     pnet_packet::ipv4::Ipv4Packet::new(handle_rs.payload())
        // );

        // if is_icmp_request(payload).await {
        //     if let Err(err) = process_icmp(payload, &mut _pipe_wirter).await {
        //         log::error!("reply icmp error: {err:?}");
        //     }
        //     continue;
        // }
        for x in list.drain(..) {
            if sender.send(x).await.is_err() {
                break;
            }
        }
    }
}
async fn tun_recv(
    pipe_writer: PipeWriter,
    device: Arc<AsyncDevice>,
    _self_id: Ipv4Addr,
) -> Result<()> {
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
        // log::info!(
        //     "read tun pkt: {:?}",
        //     pnet_packet::ipv4::Ipv4Packet::new(&buf[..payload_len])
        // );
        let mut send_packet = pipe_writer.allocate_send_packet();
        send_packet.set_payload(&buf[..payload_len]);

        if let Err(e) = pipe_writer
            .send_packet_to(send_packet, &dest_ip.into())
            .await
        {
            log::warn!("{e:?},{dest_ip:?}")
        }
    }
}

#[allow(dead_code)]
async fn process_myself(payload: &[u8], device: &Arc<AsyncDevice>) -> Result<()> {
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
