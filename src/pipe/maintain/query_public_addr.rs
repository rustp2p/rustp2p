use crate::pipe::PipeWriter;
use std::net::ToSocketAddrs;
use std::time::Duration;

pub(crate) async fn query_public_addr_loop(pipe_writer: PipeWriter, stun_servers: Vec<String>) {
    let mut count = 0;
    let len = stun_servers.len();
    let stun_request = rust_p2p_core::stun::send_stun_request();
    loop {
        match stun_servers[count % len].to_socket_addrs() {
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
                log::debug!(
                    "query_public_addr to_socket_addrs {e:?} {:?}",
                    stun_servers[count % len]
                );
            }
        }
        count += 1;
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
