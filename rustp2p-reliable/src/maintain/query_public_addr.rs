use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;
use tokio::net::TcpStream;

use crate::Puncher;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) async fn query_tcp_public_addr_loop(puncher: Puncher) {
    let tcp_stun_servers = puncher.punch_context.tcp_stun_servers.clone();
    log::debug!("tcp_stun_servers = {tcp_stun_servers:?}");
    let stun_num = tcp_stun_servers.len();
    if stun_num == 0 {
        return;
    }
    let stun_request = rust_p2p_core::stun::send_stun_request();
    let mut tcp_stream_owner: HashMap<usize, TcpStream> = HashMap::new();
    let mut tcp_count = 0;
    loop {
        tcp_count += 1;
        for (index, stun) in tcp_stun_servers.iter().enumerate() {
            if tcp_stream_owner.contains_key(&index) {
                continue;
            }
            match stun.to_socket_addrs() {
                Ok(mut addr) => {
                    if let Some(addr) = addr.next() {
                        if let Some(w) = puncher.socket_manager.tcp_socket_manager_as_ref() {
                            match tokio::time::timeout(
                                Duration::from_secs(5),
                                w.connect_reuse_port_raw(addr),
                            )
                            .await
                            {
                                Ok(rs) => match rs {
                                    Ok(tcp_stream) => {
                                        tcp_stream_owner.insert(index, tcp_stream);
                                    }
                                    Err(e) => {
                                        log::debug!("connect_reuse_port_raw {e:?} {addr:?}");
                                    }
                                },
                                Err(_) => {
                                    log::debug!("connect_reuse_port_raw timeout {addr:?}");
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::debug!("query_public_addr to_socket_addrs {e:?} {stun:?}",);
                }
            }
        }
        let cur_index = tcp_count % stun_num;
        let stun = &tcp_stun_servers[cur_index];
        if let Some(mut tcp_stream) = tcp_stream_owner.remove(&cur_index) {
            match tokio::time::timeout(Duration::from_secs(5), tcp_stream.write_all(&stun_request))
                .await
            {
                Ok(rs) => {
                    if let Err(e) = rs {
                        log::debug!("query_tcp_public_addr_loop write_all {e:?},server={stun:?}");
                    } else {
                        match stun_tcp_read(&mut tcp_stream).await {
                            Ok(addr) => {
                                log::debug!("update_tcp_public_addr {cur_index},{stun} {addr}");
                                puncher.punch_context.update_tcp_public_addr(addr);
                                let mut buf = [0; 1024];
                                loop {
                                    tokio::time::sleep(Duration::from_secs(10)).await;
                                    if let Err(e) = tcp_stream.try_write(b"1") {
                                        if std::io::ErrorKind::WouldBlock != e.kind() {
                                            log::debug!(
                                                "stun tcp w close {cur_index},{stun} {addr} {e}"
                                            );
                                            break;
                                        }
                                    }
                                    match tcp_stream.try_read(&mut buf) {
                                        Ok(len) => {
                                            if len == 0 {
                                                log::debug!("stun tcp r close {cur_index},{stun} {addr} EOF");
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            if std::io::ErrorKind::WouldBlock != e.kind() {
                                                log::debug!(
                                                    "stun tcp r close {cur_index},{stun} {addr} {e}"
                                                );
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                log::debug!(
                                    "query_tcp_public_addr_loop stun_tcp_read {e:?},server={stun:?}",
                                );
                            }
                        }
                    }
                }
                Err(_) => {
                    log::debug!("query_tcp_public_addr_loop write_all timeout,server={stun:?}");
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

pub(crate) async fn query_udp_public_addr_loop(puncher: Puncher) {
    let udp_stun_servers = puncher.punch_context.udp_stun_servers.clone();
    log::debug!("udp_stun_servers = {udp_stun_servers:?}");
    let udp_len = udp_stun_servers.len();
    if udp_len == 0 {
        return;
    }
    let mut udp_count = 0;
    let stun_request = rust_p2p_core::stun::send_stun_request();
    loop {
        if udp_len != 0 {
            let stun = &udp_stun_servers[udp_count % udp_len];
            udp_count += 1;
            match stun.to_socket_addrs() {
                Ok(mut addr) => {
                    if let Some(addr) = addr.next() {
                        if let Some(w) = puncher.socket_manager.udp_socket_manager_as_ref() {
                            if let Err(e) = w.detect_pub_addrs(&stun_request, addr).await {
                                log::debug!("detect_pub_addrs {e:?} {addr:?}");
                            }
                        }
                    }
                }
                Err(e) => {
                    log::debug!("query_public_addr to_socket_addrs {e:?} {stun:?}",);
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(12)).await;
    }
}

async fn stun_tcp_read(tcp_stream: &mut TcpStream) -> io::Result<SocketAddr> {
    let mut head = [0; 20];
    match tokio::time::timeout(Duration::from_secs(5), tcp_stream.read_exact(&mut head)).await {
        Ok(rs) => rs?,
        Err(_) => Err(io::Error::from(io::ErrorKind::TimedOut))?,
    };
    let len = u16::from_be_bytes([head[2], head[3]]) as usize;
    let mut buf = vec![0; len + 20];
    buf[..20].copy_from_slice(&head);
    match tokio::time::timeout(
        Duration::from_secs(5),
        tcp_stream.read_exact(&mut buf[20..]),
    )
    .await
    {
        Ok(rs) => rs?,
        Err(_) => Err(io::Error::from(io::ErrorKind::TimedOut))?,
    };
    if let Some(addr) = rust_p2p_core::stun::recv_stun_response(&buf) {
        Ok(addr)
    } else {
        log::debug!("stun_tcp_read {buf:?}");
        Err(io::Error::from(io::ErrorKind::InvalidData))
    }
}
