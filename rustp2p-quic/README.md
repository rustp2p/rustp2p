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
- High-level user messages and streams run over end-to-end QUIC.
- Discovery, route sync, direct transport messages, and QUIC relay packets use the core-backed
  transport layer.

Internally this is split into two layers:

- `CoreTransportLayer` owns `rustp2p-core::endpoint::EndPoint`, real reachable addresses, route
  updates, discovery, and relay forwarding.
- The QUIC adapter only maps `PeerId` to synthetic addresses required by quinn. It does not keep a
  real next-hop address; each outgoing QUIC packet is sent through the transport layer by `PeerId`.

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

`send_to` uses QUIC application datagrams. The implementation sends a small number of duplicate
datagram envelopes with a message id and deduplicates on receive, so normal local and LAN usage is
stable while preserving datagram semantics at the transport layer.

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

Real socket addresses are only transport/bootstrap concerns. Use `Endpoint::transport()` when you
need direct transport access:

- `TransportHandle::add_bootstrap(addr)`
- `TransportHandle::local_addr()`
- `TransportHandle::send_to_peer(peer_id, bytes)`
- `TransportHandle::recv_from_peer()`

## Example: Peer Node

The example is a single peer program. It is not split into server and client.

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
