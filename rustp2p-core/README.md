# rustp2p-core

`rustp2p-core` is the low-level transport crate in the `rustp2p` workspace. It
provides UDP/TCP endpoint primitives, route table utilities, STUN helpers, NAT
information types, and hole-punching primitives.

This crate does not provide the high-level PeerId QUIC overlay. For encrypted
application datagrams, reliable streams, discovery, and relay forwarding, use
`rustp2p-quic`.

## Features

- UDP and TCP endpoint with a unified `Transport` send handle.
- Cloneable `Sender` for sending to raw socket addresses.
- TCP framing through configurable codecs.
- Route table utilities with multiple routes per peer id and load balancing.
- STUN-based NAT type and port-range detection when explicitly configured.
- NAT model application for local assistant UDP sockets.
- Hole-punching primitives driven by remote `NatInfo`.

`Config::default()` does not include public STUN servers. Configure STUN
explicitly when NAT detection is needed.

## Quick Start

```toml
[dependencies]
rustp2p-core = "0.1"
```

### Echo Endpoint

```rust
use rustp2p_core::endpoint::{Config, EndPoint};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut endpoint = EndPoint::bind(Config::new().udp_port(3000).tcp_port(3000)).await?;

    while let Some(received) = endpoint.recv().await {
        println!(
            "from={} protocol={:?} bytes={:?}",
            received.transport.remote_addr(),
            received.transport.protocol(),
            received.data
        );
        received.transport.send(b"echo").await?;
    }

    Ok(())
}
```

### Send Through `Sender`

```rust
use rustp2p_core::endpoint::{Config, EndPoint};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let endpoint = EndPoint::bind(Config::new().udp_port(0)).await?;
    let sender = endpoint.sender();

    sender.send_to(b"hello", "127.0.0.1:3000".parse().unwrap()).await?;
    sender
        .try_send_via_all(b"probe", "127.0.0.1:3000".parse().unwrap())
        .await;

    Ok(())
}
```

## NAT And Punching

STUN is explicit:

```rust
use rustp2p_core::endpoint::{Config, EndPoint};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let endpoint = EndPoint::bind(
        Config::new().stun_servers(vec![
            "stun.miwifi.com:3478".to_string(),
            "stun.chat.bilibili.com:3478".to_string(),
            "stun.hitv.com:3478".to_string(),
        ]),
    )
    .await?;

    let nat_info = endpoint.nat_info().await?;
    endpoint.apply_nat_model(nat_info.nat_type).await?;

    Ok(())
}
```

`apply_nat_model` only applies an externally detected local `NatType`:

- `NatType::Symmetric` adds assistant UDP sockets up to
  `Config::max_assistant_sockets`.
- `NatType::Cone` removes assistant UDP sockets.

`Puncher` is separate. It uses the remote peer's `NatInfo` in `PunchInfo` to
execute punching and does not decide the local socket model.

## Core Types

| Type | Purpose |
| ---- | ------- |
| `EndPoint` | Binds UDP/TCP sockets, receives packets, and creates handles. |
| `Received` | Data plus the source `Transport` returned by `EndPoint::recv`. |
| `Sender` | Cloneable handle for raw address sends and socket queries. |
| `Transport` | Send handle tied to a received UDP/TCP route. |
| `RouteKey` | `(Protocol, SocketAddr)` route identity. |
| `RouteTable<T>` | Multi-route table keyed by caller-defined peer id type. |
| `NatInfo` / `NatType` | NAT shape, local/public addresses, and port metadata. |
| `Puncher` / `PunchInfo` | Low-level NAT punching primitive. |

## Route Table

`RouteTable<T>` is generic over your own peer id type. It stores confirmed
routes and can select a route based on the configured `LoadBalance` policy.

```rust
use rustp2p_core::endpoint::LoadBalance;
use rustp2p_core::route_table::{Protocol, RouteKey, RouteTable};

fn main() -> std::io::Result<()> {
    let routes: RouteTable<String> = RouteTable::new(LoadBalance::MinHopLowestLatency);
    let key = RouteKey::new(Protocol::UDP, "127.0.0.1:3000".parse().unwrap());

    routes.add_route("peer-a".to_string(), (key, 0));
    let route = routes.get_route_by_id(&"peer-a".to_string())?;
    assert!(route.is_direct());

    Ok(())
}
```

## Design

See [DESIGN.md](DESIGN.md) for the implemented architecture, socket lifecycle,
route semantics, and NAT traversal boundaries.

## Validation

```bash
cargo test -p rustp2p-core
cargo test -p rustp2p-core --doc
cargo clippy --workspace --all-targets -- -D warnings
```

## License

Apache-2.0
