# rust-p2p-core

NAT traversal for P2P communication via hole-punching.

[![Crates.io](https://img.shields.io/crates/v/rust-p2p-core.svg)](https://crates.io/crates/rust-p2p-core)
![rust-p2p-core](https://docs.rs/rust-p2p-core/badge.svg)

## Overview

`rust-p2p-core` provides the core networking layer for P2P applications behind NATs. It handles:

- **UDP/TCP hole-punching** for NAT traversal
- **Symmetric NAT** probing with assistant sockets
- **Route management** with load balancing
- **Codec abstraction** for TCP framing

## Quick Start

```toml
[dependencies]
rust-p2p-core = "0.4"
```

## Example: Echo Server

```rust
use rust_p2p_core::endpoint::{EndPoint, Config};

#[tokio::main]
async fn main() {
    let mut ep = EndPoint::bind(Config::new().udp_port(3000).tcp_port(3000))
        .await
        .unwrap();

    while let Some(received) = ep.recv().await {
        println!("From {}: {:?}", received.transport.remote_addr(), received.data);
        received.transport.send(b"echo").await.unwrap();
    }
}
```

## Example: Client with NAT Punching

```rust
use rust_p2p_core::endpoint::{EndPoint, Config};
use rust_p2p_core::route_table::{RouteKey, Protocol};

#[tokio::main]
async fn main() {
    let ep = EndPoint::bind(Config::default()).await.unwrap();
    let sender = ep.sender();
    let puncher = ep.puncher();

    // Send to server
    let addr = "127.0.0.1:3000".parse().unwrap();
    sender.send_to(b"hello", addr);

    // Receive responses
    while let Some(received) = ep.recv().await {
        let route_key = RouteKey::from_transport(&received.transport);
        println!("Route: {route_key}");
    }
}
```

## Core Types

| Type | Description |
|------|-------------|
| `EndPoint` | Main P2P endpoint, entry point for send/recv |
| `Sender` | Lightweight handle for sending data and querying state |
| `Transport` | Send handle to a peer (holds socket reference) |
| `Received` | Received message with data + source Transport |
| `RouteKey` | Identifies a route by (Protocol, SocketAddr) |
| `Puncher` | NAT hole-punching logic |

## API Design

- **`EndPoint`** - Main API entry point
- **`Sender`** - Cloneable send handle (no socket management)
- **`RouteKey::from_transport()`** - Easy route construction

See [design.md](design.md) for architecture details.

## Supported Platforms

Cross-platform (Windows, Linux, macOS).

## License

MIT OR Apache-2.0
