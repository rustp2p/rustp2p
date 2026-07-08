# rustp2p-quic

`rustp2p-quic` is the PeerId-based QUIC layer for this workspace. It builds on
`rustp2p-core` as the underlying transport without modifying core APIs.

The high-level API uses `PeerId` for all application traffic. Socket addresses are only used to
bootstrap connectivity to a reachable node.

## Design

Every node is equal. There is no fixed server/client role at the high-level P2P layer.

If `A <-> B <-> C` are reachable but `A` and `C` cannot connect directly, `B` can forward traffic
between `A` and `C`. Reliable traffic is still end-to-end QUIC:

- `A` and `C` establish one QUIC connection.
- `B` forwards QUIC UDP datagrams inside rustp2p overlay relay packets.
- `B` does not terminate QUIC streams.
- `B` cannot read reliable stream payloads.
- High-level user datagrams and streams run over end-to-end QUIC.
- Discovery, route sync, punch negotiation, and QUIC relay packets use the protocol/control plane
  backed by `rustp2p-core` transport I/O.

Internally this is split into three layers:

- `transport` owns `rustp2p-core::endpoint::EndPoint`, real reachable addresses, multiple routes
  per `PeerId`, and raw wire-byte send/receive.
- `protocol` owns the custom rustp2p packet wire format, control payload encoding, discovery,
  relay forwarding, NAT candidate exchange, and punch decisions.
- `quic` owns quinn, certificates, stream/datagram handling, and `PeerId` to synthetic address
  mapping. It sends encrypted QUIC packets to `protocol`, which wraps them as `QuicRelay` packets
  before handing them to `transport`.

There is no group concept in `rustp2p-quic`. A discovered peer can relay for any other reachable
peer.

## Identity And Certificates

`PeerId` is supplied by the application:

```rust
use rustp2p_quic::Identity;

let identity = Identity::new("node-a", "seed-a").unwrap();
```

The first argument is the node id. The second argument is a seed used to deterministically generate
the local QUIC certificate key. The certificate and `PeerId` are intentionally not bound together.

Certificate verification is controlled through `CertificateVerifier`:

```rust
use rustp2p_quic::{Endpoint, Identity, SkipCertificateVerification};
use std::sync::Arc;

#[tokio::main]
async fn main() -> rustp2p_quic::Result<()> {
    let endpoint = Endpoint::builder()
        .identity(Identity::new("node-a", "seed-a")?)
        .certificate_verifier(Arc::new(SkipCertificateVerification))
        .bind("127.0.0.1:7001".parse().unwrap())
        .build()
        .await?;

    Ok(())
}
```

`SkipCertificateVerification` is the default. Applications that need stricter TLS trust can provide
their own verifier.

## Main APIs

### Build an endpoint

```rust
use rustp2p_quic::{Endpoint, Identity};

#[tokio::main]
async fn main() -> rustp2p_quic::Result<()> {
    let endpoint = Endpoint::builder()
        .identity(Identity::new("node-a", "seed-a")?)
        .bind("0.0.0.0:0".parse().unwrap())
        .build()
        .await?;

    println!("peer_id={}", endpoint.peer_id());
    println!("addr={}", endpoint.local_addr().unwrap());

    Ok(())
}
```

### Bootstrap by address

Bootstrap addresses are only entry points. The remote peer id is learned through hello discovery.

```rust
let peer_id = endpoint.add_bootstrap("127.0.0.1:7001".parse().unwrap()).await?;
println!("connected to {peer_id}");
```

After bootstrap, send and connect by `PeerId` only.

### Send an unreliable message

```rust
endpoint.send_to(peer_id, b"hello").await?;

let msg = endpoint.recv().await?;
println!("from={} {:?}", msg.src, msg.payload);
```

`send_to` uses QUIC application datagrams. The payload is encrypted by QUIC/TLS. The rustp2p
protocol layer only sees and forwards QUIC ciphertext packets.

### Open a reliable bidirectional stream

```rust
let (mut send, mut recv) = endpoint.open_bi(peer_id).await?;
send.write_all(b"ping").await?;
send.finish()?;

let mut response = [0u8; 1024];
if let Some(n) = recv.read(&mut response).await? {
    println!("response={:?}", &response[..n]);
}
```

On the receiving side, `accept_bi` returns source information:

```rust
let mut stream = endpoint.accept_bi().await?;
println!("from={}", stream.peer_id);

if let Some(info) = endpoint.link_info(stream.peer_id.clone()) {
    println!("current link={:?} metric={}", info.mode, info.metric);
}

let mut request = [0u8; 1024];
if let Some(n) = stream.recv.read(&mut request).await? {
    stream.send.write_all(b"echo: ").await?;
    stream.send.write_all(&request[..n]).await?;
    stream.send.finish()?;
}
```

`read_to_end(max_size)` is still available, but it waits until the peer finishes the send stream.
For request/response protocols, prefer explicit framing such as a length-prefixed message. The
`node` example uses length-prefixed frames for this reason.

### Discovery

Nodes exchange known peers through transport control packets (`Hello`, `RouteQuery`, and
`RouteReply`). These control packets use `rustp2p-core` transport I/O, not user QUIC streams.

For a chain like:

```text
A <-> B <-> C <-> D
```

A bootstraps to B, B bootstraps to C, and C bootstraps to D. Discovery then propagates known peers,
so A eventually learns D's `PeerId` and a relay route to D. A can then call:

```rust
endpoint.send_to("node-d".into(), b"hello d").await?;
```

## Transport Handle

Real socket addresses are only transport/bootstrap concerns. `Endpoint::transport()` is a low-level
escape hatch for already encoded protocol wire bytes; it does not encrypt user payloads by itself.
Application data should use `send_to` or `open_bi`.

- `TransportHandle::local_addr()`
- `TransportHandle::send_to_peer(peer_id, wire_bytes)`

Bootstrap uses `Endpoint::add_bootstrap(addr)` so the protocol layer can perform hello discovery and
learn the remote `PeerId`.

## Hole Punching

Punching is controlled by a dynamic whitelist. A node only starts punch negotiation with peers that
are currently allowed:

```rust
endpoint.allow_punch("node-b".into());
endpoint.punch("node-b".into()).await?;
endpoint.deny_punch("node-b".into());
```

Punch negotiation is handled by the protocol layer. The transport layer only supplies core route
updates and the `rustp2p-core` punch primitive.

## NAT Observation

`Endpoint::nat_info()` combines local transport data, STUN-derived NAT shape, and direct peer
observations:

- `Config::default()` does not include STUN servers; STUN maintenance is opt-in through
  `Builder::stun_servers(...)`.
- STUN is used for NAT type and symmetric-port range hints when configured.
- Public UDP/TCP ports and public IPv6 are learned from direct peers through protocol
  `NatObserve` control packets.
- A long-running public node can be used as a bootstrap peer and as a direct observer, but it is
  still just another peer in the protocol.

Use `Builder::nat_observers(Vec<PeerId>)` to restrict which direct peers can update observed public
addresses. If the list is empty, any direct known peer may act as an observer.

## Example: Peer Node

The example is a single peer program. It is not split into server and client.
The example enables these STUN servers by default:

```text
stun.miwifi.com:3478
stun.chat.bilibili.com:3478
stun.hitv.com:3478
```

Use `--stun <server>` to provide an explicit list, or `--no-stun` to run the example without STUN.

Run node A:

```bash
cargo run -p rustp2p-quic --example node -- --id node-a --seed seed-a --bind 127.0.0.1:7101
```

Run node B and bootstrap it to A:

```bash
cargo run -p rustp2p-quic --example node -- --id node-b --seed seed-b --bind 127.0.0.1:7102 --bootstrap 127.0.0.1:7101
```

In node A, connect back to B if you want an explicit direct route both ways:

```text
connect 127.0.0.1:7102
```

Interactive commands:

```text
connect <addr>
send <peer_id> <message>
stream <peer_id> <message>
broadcast <message>
peers
quit
```

Send an unreliable message:

```text
send node-b hello over datagram
```

Open a reliable bidirectional stream:

```text
stream node-b hello over quic stream
```

### Relay Example

Start B:

```bash
cargo run -p rustp2p-quic --example node -- --id node-b --seed seed-b --bind 127.0.0.1:7202
```

Start A connected to B:

```bash
cargo run -p rustp2p-quic --example node -- --id node-a --seed seed-a --bind 127.0.0.1:7201 --bootstrap 127.0.0.1:7202
```

Start C connected to B:

```bash
cargo run -p rustp2p-quic --example node -- --id node-c --seed seed-c --bind 127.0.0.1:7203 --bootstrap 127.0.0.1:7202
```

After discovery, A can send to C using only C's peer id:

```text
send node-c hello through B
stream node-c reliable hello through B
```

B forwards the traffic but does not receive the reliable stream through `accept_bi`.

### Four-Node Discovery Example

Start a chain:

```text
node-a -> node-b -> node-c -> node-d
```

Use `--bootstrap` so each node connects to the next reachable node. After discovery propagation,
`node-a` can run:

```text
send node-d hello d
stream node-d reliable hello d
```

No address for `node-d` is needed by node A.

## Validation

Useful commands:

```bash
cargo check --workspace
cargo test -p rustp2p-quic
cargo check -p rustp2p-quic --examples
cargo clippy --workspace --all-targets -- -D warnings
```
