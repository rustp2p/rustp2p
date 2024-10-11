use crate::pipe::PipeWriter;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(crate) async fn query_public_addr_loop(
    pipe_writer: PipeWriter,
    tcp_stun_servers: Vec<String>,
    udp_stun_servers: Vec<String>,
) {
    let mut udp_count = 0;
    let mut tcp_count = 0;
    let udp_len = udp_stun_servers.len();
    let tcp_len = tcp_stun_servers.len();
    if udp_len == 0 && tcp_len == 0 {
        return;
    }
    let stun_request = rust_p2p_core::stun::send_stun_request();
    let mut tcp_stream_owner: Option<TcpStream> = None;
    loop {
        if udp_len != 0 {
            let stun = &udp_stun_servers[udp_count % udp_len];
            udp_count += 1;
            match stun.to_socket_addrs() {
                Ok(mut addr) => {
                    if let Some(addr) = addr.next() {
                        if let Some(w) = pipe_writer.pipe_writer.udp_pipe_writer() {
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
        if tcp_len != 0 {
            let stun = &tcp_stun_servers[tcp_count % tcp_len];
            if tcp_stream_owner.is_none() {
                tcp_count += 1;
                match stun.to_socket_addrs() {
                    Ok(mut addr) => {
                        if let Some(addr) = addr.next() {
                            if let Some(w) = pipe_writer.pipe_writer.tcp_pipe_writer() {
                                match w.connect_reuse_port_raw(addr).await {
                                    Ok(tcp_stream) => {
                                        tcp_stream_owner.replace(tcp_stream);
                                    }
                                    Err(e) => {
                                        log::debug!("connect_reuse_port_raw {e:?} {addr:?}");
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
            if let Some(tcp_stream) = tcp_stream_owner.as_mut() {
                match tokio::time::timeout(
                    Duration::from_secs(5),
                    tcp_stream.write_all(&stun_request),
                )
                .await
                {
                    Ok(rs) => {
                        if rs.is_err() {
                            log::debug!("is_err {rs:?},server={stun:?}",);
                            if let Some(mut tcp) = tcp_stream_owner.take() {
                                let _ = tcp.shutdown().await;
                            }
                        }
                    }
                    Err(_) => {
                        if let Some(mut tcp) = tcp_stream_owner.take() {
                            let _ = tcp.shutdown().await;
                        }
                    }
                }
            }
            if let Some(tcp_stream) = tcp_stream_owner.as_mut() {
                match stun_tcp_read(tcp_stream).await {
                    Ok(addr) => {
                        pipe_writer.pipe_context().update_tcp_public_addr(addr);
                    }
                    Err(e) => {
                        log::debug!("stun_tcp_read {e:?},server={stun:?}",);
                        if let Some(mut tcp) = tcp_stream_owner.take() {
                            let _ = tcp.shutdown().await;
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(12)).await;
    }
}
async fn stun_tcp_read(tcp_stream: &mut TcpStream) -> crate::error::Result<SocketAddr> {
    let mut head = [0; 20];
    match tokio::time::timeout(Duration::from_secs(5), tcp_stream.read_exact(&mut head)).await {
        Ok(rs) => rs?,
        Err(_) => Err(crate::error::Error::Timeout)?,
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
        Err(_) => Err(crate::error::Error::Timeout)?,
    };
    if let Some(addr) = rust_p2p_core::stun::recv_stun_response(&buf) {
        Ok(addr)
    } else {
        log::debug!("stun_tcp_read {buf:?}");
        Err(crate::error::Error::InvalidArgument("".into()))
    }
}
