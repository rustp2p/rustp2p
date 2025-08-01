# rust-p2p

NAT traversal for p2p communication, this is implemented in terms of a hole-punching technique.

[![Crates.io](https://img.shields.io/crates/v/rust-p2p-core.svg)](https://crates.io/crates/rust-p2p-core)
![rust-p2p-core](https://docs.rs/rust-p2p-core/badge.svg)

This crate provides a convenient way to create connections between multiple remote peers that may be behind Nats, these tunnel that are spawned from the TunnelFactory can be used to read/write bytes from/to a peer to another.

The underlying transport protocols are `TCP`, `UDP` in the tunnel.

This crate is built on the async ecosystem tokio

# Supported Platforms

It's a cross-platform crate

# Usage

Add this dependency to your `cargo.toml`

```toml
rust-p2p-core = {version = "0.3"}
```

# Example
```rust
use rust_p2p_core::tunnel::config::{TcpTunnelConfig, TunnelConfig, UdpTunnelConfig};
use rust_p2p_core::tunnel::tcp::LengthPrefixedInitCodec;
use rust_p2p_core::tunnel::new_tunnel_component;


#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let udp_config = UdpTunnelConfig::default();
    let tcp_config = TcpTunnelConfig::new(Box::new(LengthPrefixedInitCodec));
    let config = TunnelConfig::empty()
        .set_udp_tunnel_config(udp_config)
        .set_tcp_tunnel_config(tcp_config)
        .set_tcp_multi_count(2);
    let (mut tunnel_dispatcher, puncher) = new_tunnel_component(config).unwrap();
    let socket_manager = tunnel_dispatcher.socket_manager();
    // 1. Use "puncher" for hole punching. 
    // 2. "tunnel_dispatcher" distributes encapsulated sockets
    // 3. Use "socket_manager" to send messages to either the direct connection address or the post-punching routed address.
    loop {
        let tunnel = tunnel_dispatcher.dispatch().await.unwrap();
        // Each tunnel corresponds to a TCP or UDP socket.
        // 1. A TCP-type tunnel only appears after successful hole punching
        // 2. UDP-type tunnels are dispatched right from the start.
    }
}

```

It is recommended to use `rustp2p` directly, which is ergonomic and easy to use.