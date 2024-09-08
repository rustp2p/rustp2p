# rust-p2p

NAT traversal for p2p communication, this is implemented in terms of a hole-punching technique.

[![Crates.io](https://img.shields.io/crates/v/rust-p2p.svg)](https://crates.io/crates/rust-p2p)
![rust-p2p](https://docs.rs/rust-p2p/badge.svg)

This crate provides a convenient way to create a pipe between multiple remote peers that may be behind Nats, these pipelines that are spawned from the pipe can be used to read/write bytes from/to a peer to another.

The underlying transport protocols are `TCP`, `UDP` in the pipelines, users can even extend the protocol for the pipeline by using the powerful trait.

This crate is built on the async ecosystem tokio

# Supported Platforms

It's a cross-platform crate

# Usage

Add this dependency to your `cargo.toml`

```toml
rust-p2p = {version = "0.1"}
```

# Example

````rust
use rust_p2p::nat::{NatInfo, NatType};
use rust_p2p::pipe::{
    config::{PipeConfig, TcpPipeConfig, UdpPipeConfig},
    pipe, PipeLine, PipeWriter,
};
#[tokio::main]
async fn main() {
    let udp_config = UdpPipeConfig::default();
    let tcp_config = TcpPipeConfig::default();
    let config = PipeConfig::empty()
        .set_udp_pipe_config(udp_config)
        .set_tcp_pipe_config(tcp_config)
        .set_main_pipeline_num(2);
    let (mut pipe, puncher, idle_route_manager) = pipe(config).unwrap();
    // Handle the idle route
    tokio::spawn(async move {
        loop {
            let (peer_id, route, time) = idle_route_manager.next_idle().await;
            log::info!(
                "route timeout peer_id={peer_id},route={route:?},time={:?}",
                time.elapsed()
            );
            // delete the mapping associated with the route and peer_id
            idle_route_manager.remove_route(&peer_id, &route.route_key());
        }
    });
    let pipe_writer = pipe.writer_ref().to_owned();
    let peer_id = "peer_id".to_owned(); // The remote peer is said to be named "peer_id"
                                        // The Nat information of the peer for which we prepare to punch the hole between local and it
    let peer_nat_info = NatInfo {
        nat_type: NatType::Cone,                     //Nat type
        public_ips: vec![Ipv4Addr::new(1, 1, 1, 1)], // The public IP mapped by Nat
        public_ports: vec![8080],                    // The public port mapped by Nat
        mapping_tcp_addr: vec![],
        mapping_udp_addr: vec![],
        public_port_range: 0,
        local_ipv4: Ipv4Addr::new(1, 1, 1, 1),
        ipv6: None,
        local_udp_ports: vec![0],
        local_tcp_port: 0,
        public_tcp_port: 0,
    };
    tokio::spawn(async move {
        // We may need to keep calling this method until the peer receives "hello"
        let rs = puncher
            .punch(
                peer_id,
                b"hello",
                PunchInfo::new(PunchModelBoxes::all(), peer_nat_info),
            )
            .await;
    });
    loop {
        let pipe_line = pipe.accept().await.unwrap();
        let pipe_writer_ = pipe_writer.clone();
        tokio::spawn(async move {
            let mut buf = [0; 65536];
            loop {
                let (len, route_key) = match pipe_line.recv_from(&mut buf).await {
                    Ok(rs) => rs,
                    Err(e) => break,
                };
                // route_key denotes the source from which the buf is sent from in the pipeline
                // pipe_writer_.send_to(b"hello", &route_key).await.unwrap();
            }
        });
    }
}

````
