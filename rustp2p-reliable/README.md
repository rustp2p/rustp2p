# rustp2p-reliable

This crate is used for reliable transmission after hole punching, internally using TCP or KCP to achieve reliability.

# Supported Platforms

It's a cross-platform crate

# Usage

Add this dependency to your `cargo.toml`

```toml
rust-reliable = {version = "0.1"}
```

# Example
```rust
use rustp2p_reliable::LengthPrefixedInitCodec;
use rustp2p_reliable::{Config, Puncher};
use rustp2p_reliable::{TcpTunnelConfig, TunnelConfig, UdpTunnelConfig};


#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let udp_config = UdpTunnelConfig::default();
    let tcp_config = TcpTunnelConfig::new(Box::new(LengthPrefixedInitCodec));
    let config = TunnelConfig::empty()
        .set_udp_tunnel_config(udp_config)
        .set_tcp_tunnel_config(tcp_config)
        .set_tcp_multi_count(2);
    let config = Config::new(config);
    let (mut listener, puncher) = rustp2p_reliable::from_config(config).await.unwrap();
    // 1. The listener is used to listen for tunnels established after successful hole punching.
    // 2. Use "puncher" for hole punching. 
    loop {
        let reliable_tunnel = listener.accept().await.unwrap();
        // "reliable_tunnel" is used for reliable transmission after successful direct connection through hole punching.
        // "reliable_tunnel" uses TCP or KCP internally to implement reliable transmission.
    }
}

```
