# rustp2p

`rustp2p` is a Rust workspace for peer-to-peer networking. It currently contains
two crates:

- `rustp2p-core`: low-level UDP/TCP endpoint primitives, route tables,
  NAT/STUN helpers, and hole-punching primitives.
- `rustp2p-quic`: a PeerId-based QUIC overlay with high-level peer discovery,
  relay forwarding, encrypted QUIC DATAGRAM messages, and reliable QUIC streams.

The high-level API lives in `rustp2p-quic`. Application traffic is addressed by
`PeerId`; real socket addresses are used only for bind/bootstrap and internal
transport routing.

## Architecture

```text
Application
  -> rustp2p-quic Endpoint
  -> quic      (quinn, TLS, stream/datagram, synthetic PeerId addresses)
  -> protocol  (packet format, discovery, relay, NAT observe, punch control)
  -> transport (PeerId routes over rustp2p-core)
  -> rustp2p-core endpoint
  -> UDP/TCP network
```

Every node is equal. A reachable node can relay traffic for other peers, but it
does not terminate their QUIC streams. Relay nodes forward encrypted QUIC packets
inside rustp2p protocol packets; user payloads remain protected by end-to-end
QUIC/TLS.

## High-Level QUIC API

```rust
use rustp2p_quic::{Endpoint, Identity, PeerId};
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> rustp2p_quic::Result<()> {
    let endpoint = Endpoint::builder()
        .identity(Identity::new("node-a", "seed-a")?)
        .bind("0.0.0.0:7001".parse().unwrap())
        .bootstrap(vec!["127.0.0.1:7002".parse().unwrap()])
        .build()
        .await?;

    let peer = PeerId::from("node-b");

    // Unreliable but encrypted: QUIC DATAGRAM.
    endpoint.send_to(peer.clone(), b"hello datagram").await?;

    // Reliable and encrypted: end-to-end QUIC bidirectional stream.
    let (mut send, _recv) = endpoint.open_bi(peer).await?;
    send.write_all(b"hello stream").await?;
    send.finish()?;

    endpoint.close().await;
    Ok(())
}
```

Bootstrap addresses are only entry points. `add_bootstrap(addr)` connects to a
reachable node, learns its `PeerId`, and lets discovery propagate additional
peers and routes. After that, high-level send/connect APIs use `PeerId` only.

## Defaults And Security

- `Config::default()` does not include public STUN servers. Configure STUN
  explicitly when NAT type maintenance is needed.
- The `rustp2p-quic` node example uses these explicit STUN servers by default:
  `stun.miwifi.com:3478`, `stun.chat.bilibili.com:3478`, and
  `stun.hitv.com:3478`; pass `--no-stun` to disable STUN in the example.
- `SkipCertificateVerification` is the default certificate verifier for easy
  local testing. Production applications should install a custom
  `CertificateVerifier`. The QUIC setup supports both server-certificate and
  client-certificate verification.

## Example Node

Start node A:

```bash
cargo run -p rustp2p-quic --example node -- --id node-a --seed seed-a --bind 127.0.0.1:7101
```

Start node B and bootstrap it to A:

```bash
cargo run -p rustp2p-quic --example node -- --id node-b --seed seed-b --bind 127.0.0.1:7102 --bootstrap 127.0.0.1:7101
```

Inside the example, use commands such as:

```text
connect <addr>
send <peer_id> <message>
stream <peer_id> <message>
broadcast <message>
peers
quit
```

## Crate Documentation

- `rustp2p-core/README.md` covers the low-level endpoint, route table, STUN,
  and punch primitives.
- `rustp2p-core/DESIGN.md` describes the core endpoint, socket pool, route
  table, NAT, and punch architecture.
- `rustp2p-quic/README.md` covers the PeerId QUIC API, discovery, relay,
  NAT observation, punching, and the interactive node example.
- `rustp2p-quic/DESIGN.md` describes the transport/protocol/quic layering and
  protobuf wire format.

## Validation

```bash
cargo check --workspace
cargo test -p rustp2p-core
cargo test -p rustp2p-quic
cargo check -p rustp2p-quic --examples
cargo clippy --workspace --all-targets -- -D warnings
```
