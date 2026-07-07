# rustp2p

This workspace now contains the core transport primitives and the QUIC transport:

- `rustp2p-core`: shared endpoint, NAT/STUN, socket, punch, and route-table APIs.
- `rustp2p-quic`: QUIC-based reliable connections, direct UDP datagrams, and a high-level P2P API.

## Build

```bash
cargo check --workspace
```

## QUIC Transport

`rustp2p-quic` supports:

- reliable bidirectional and unidirectional QUIC streams;
- QUIC application datagrams through `Connection::send_datagram` and `Connection::recv_datagram`;
- direct non-QUIC UDP datagrams through `Endpoint::send_direct_datagram` and `Endpoint::recv_direct_datagram`;
- rustp2p-style packet demuxing that keeps `0x80..0xbf` protocol packets out of the QUIC path;
- high-level `PeerId`, `GroupCode`, route discovery, message relay, broadcast, and reliable streams;
- end-to-end encrypted reliable streams using Noise over direct or relayed QUIC streams.

## High-Level API

```rust
use rustp2p_quic::{Endpoint, GroupCode, Identity, PeerAddr};

# #[tokio::main]
# async fn main() -> std::io::Result<()> {
let identity = Identity::generate()?;
let endpoint = Endpoint::builder()
    .identity(identity)
    .group(GroupCode::try_from("my-group")?)
    .bind("0.0.0.0:0".parse().unwrap())
    .build()
    .await?;

// Add peers with `endpoint.add_peer(PeerAddr::new(peer_id, addrs)).await?`.
// Then send unreliable messages:
// endpoint.send_to(peer_id, b"hello").await?;
// Or open an end-to-end encrypted reliable stream:
// let mut stream = endpoint.open_stream_to(peer_id).await?;
# Ok(())
# }
```
