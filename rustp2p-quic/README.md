# rustp2p-quic

`rustp2p-quic` is the QUIC transport and high-level P2P layer for this workspace.
It builds on `rustp2p-core` without modifying core APIs.

The crate exposes two layers:

- a low-level QUIC API around `quinn`;
- a high-level peer API with `PeerId`, `GroupCode`, route discovery, unreliable messages, broadcast, and reliable bidirectional streams.

## Design

Every node is equal. There is no fixed server/client role at the high-level P2P layer.

Any reachable node can forward traffic for any two other reachable nodes. For example, if
`A <-> B <-> C` are reachable but `A` and `C` cannot connect directly, `B` can forward traffic
between `A` and `C`.

Reliable traffic is still end-to-end QUIC:

- `A` and `C` establish one QUIC connection.
- `B` forwards QUIC UDP datagrams inside rustp2p overlay relay packets.
- `B` does not terminate QUIC streams.
- `B` cannot read reliable stream payloads.

`GroupCode` is not a forwarding ACL. A node from another group may still relay packets. Group
checks are applied when a packet is delivered locally to the final receiver.

## Main APIs

### Build an endpoint

```rust
use rustp2p_quic::{Endpoint, GroupCode, Identity};

# #[tokio::main]
# async fn main() -> rustp2p_quic::Result<()> {
let identity = Identity::generate()?;
let endpoint = Endpoint::builder()
    .identity(identity)
    .group(GroupCode::try_from("demo")?)
    .bind("0.0.0.0:0".parse().unwrap())
    .build()
    .await?;

println!("peer_id={}", endpoint.peer_id());
println!("addr={}", endpoint.local_addr().unwrap());
# Ok(())
# }
```

### Add a peer

```rust
use rustp2p_quic::PeerAddr;

# async fn example(endpoint: rustp2p_quic::Endpoint, peer_id: rustp2p_quic::PeerId) -> rustp2p_quic::Result<()> {
let peer = PeerAddr::new(peer_id, vec!["127.0.0.1:7001".parse().unwrap()]);
endpoint.add_peer(peer).await?;
# Ok(())
# }
```

### Send an unreliable message

`send_to` sends a rustp2p overlay datagram. It is unreliable and may be forwarded by relay nodes.

```rust
# async fn example(endpoint: rustp2p_quic::Endpoint, peer_id: rustp2p_quic::PeerId) -> rustp2p_quic::Result<()> {
endpoint.send_to(peer_id, b"hello").await?;
let msg = endpoint.recv().await?;
println!("from={} {:?}", msg.src, msg.payload);
# Ok(())
# }
```

### Open a reliable bidirectional stream

Use `open_bi` / `accept_bi` for high-level reliable communication.

```rust
# async fn example(endpoint: rustp2p_quic::Endpoint, peer_id: rustp2p_quic::PeerId) -> rustp2p_quic::Result<()> {
let (mut send, mut recv) = endpoint.open_bi(peer_id).await?;
send.write_all(b"ping").await?;
send.finish()?;

let response = recv.read_to_end(1024 * 1024).await?;
println!("response={:?}", response);
# Ok(())
# }
```

On the receiving side:

```rust
# async fn example(endpoint: rustp2p_quic::Endpoint) -> rustp2p_quic::Result<()> {
let (mut send, mut recv) = endpoint.accept_bi().await?;
let request = recv.read_to_end(1024 * 1024).await?;
send.write_all(b"echo: ").await?;
send.write_all(&request).await?;
send.finish()?;
# Ok(())
# }
```

`open_stream_to` and `accept_stream` remain available as compatibility wrappers around the same
end-to-end QUIC stream behavior.

### Low-level escape hatches

The low-level QUIC API is still available:

- `Endpoint::connect(NodeAddr)`
- `Endpoint::accept()`
- `Connection::open_bi()`
- `Connection::accept_bi()`
- `Connection::send_datagram()`
- `Connection::recv_datagram()`

Direct raw datagrams on the shared UDP socket are also available:

- `Endpoint::send_direct_datagram(...)`
- `Endpoint::recv_direct_datagram()`

## Example: peer node

The example is a single peer program. It is not split into server and client. Every instance can:

- receive unreliable datagrams;
- accept reliable bidirectional streams;
- send unreliable datagrams;
- open reliable bidirectional streams;
- add peers at startup or interactively;
- broadcast to known peers.

Run it with:

```bash
cargo run -p rustp2p-quic --example node -- --bind 127.0.0.1:7001
```

The node prints:

```text
peer_id=<64 hex chars>
group=GroupCode(...)
addr=127.0.0.1:7001
commands:
  add <peer_id>@<addr>
  send <peer_id> <message>
  stream <peer_id> <message>
  broadcast <message>
  peers
  quit
```

Copy the printed `peer_id`; it is needed by other nodes.

### Two-node direct test

Terminal 1:

```bash
cargo run -p rustp2p-quic --example node -- --bind 127.0.0.1:7001
```

Copy node A's printed `peer_id`.

Terminal 2:

```bash
cargo run -p rustp2p-quic --example node -- --bind 127.0.0.1:7002 --peer <A_PEER_ID>@127.0.0.1:7001
```

Copy node B's printed `peer_id`.

In terminal 1, add node B:

```text
add <B_PEER_ID>@127.0.0.1:7002
```

Send an unreliable message from A to B:

```text
send <B_PEER_ID> hello over datagram
```

Open a reliable bidirectional stream from A to B:

```text
stream <B_PEER_ID> hello over quic stream
```

B prints the incoming stream payload. A prints the echo response.

### Three-node relay test

This verifies the peer-to-peer relay model:

```text
A <-> B <-> C
```

Start B first:

```bash
cargo run -p rustp2p-quic --example node -- --bind 127.0.0.1:7102
```

Copy `B_PEER_ID`.

Start A and connect it to B:

```bash
cargo run -p rustp2p-quic --example node -- --bind 127.0.0.1:7101 --peer <B_PEER_ID>@127.0.0.1:7102
```

Copy `A_PEER_ID`.

Start C and connect it to B:

```bash
cargo run -p rustp2p-quic --example node -- --bind 127.0.0.1:7103 --peer <B_PEER_ID>@127.0.0.1:7102
```

Copy `C_PEER_ID`.

In B's terminal, add A and C if they were not already learned:

```text
add <A_PEER_ID>@127.0.0.1:7101
add <C_PEER_ID>@127.0.0.1:7103
```

In A's terminal, refresh route discovery through B:

```text
add <B_PEER_ID>@127.0.0.1:7102
peers
```

After route discovery, A should know C. Then send from A to C:

```text
send <C_PEER_ID> hello through B
stream <C_PEER_ID> reliable hello through B
```

C receives the datagram and stream. B forwards the traffic but does not receive the reliable stream
through `accept_bi`, because the QUIC connection is end-to-end between A and C.

### Cross-group relay test

Relay nodes do not need to share the final receiver's group.

Start B in a different group:

```bash
cargo run -p rustp2p-quic --example node -- --group relay --bind 127.0.0.1:7202
```

Start A and C in the same group:

```bash
cargo run -p rustp2p-quic --example node -- --group edge --bind 127.0.0.1:7201 --peer <B_PEER_ID>@127.0.0.1:7202
cargo run -p rustp2p-quic --example node -- --group edge --bind 127.0.0.1:7203 --peer <B_PEER_ID>@127.0.0.1:7202
```

Add A and C on B, then refresh route discovery on A as shown in the relay test. A can send to C
through B even though B is in another group.

If A and C use different groups, B may still forward the packet, but the final receiver drops
group-scoped unreliable messages instead of delivering them to `recv`.

## Validation

Useful commands:

```bash
cargo check --workspace
cargo test -p rustp2p-quic
cargo clippy --workspace --all-targets -- -D warnings
```
