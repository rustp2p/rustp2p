# rust-p2p-core Architecture

## Overview

```
┌─────────────────────────────────────────────────────────────┐
│                       User Application                       │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│    ┌──────────┐    ┌──────────┐    ┌──────────────────┐     │
│    │ EndPoint │───▶│  Sender  │    │     Puncher      │     │
│    │          │───▶│          │    │                  │     │
│    │ recv()   │    │ send()   │    │  punch()         │     │
│    │ sender() │    │ query()  │    │  need_punch()    │     │
│    │puncher() │    └────┬─────┘    └────────┬─────────┘     │
│    └────┬─────┘         │                   │               │
│         │               │                   │               │
├─────────┼───────────────┼───────────────────┼───────────────┤
│         │          SocketPool (pub(crate))   │               │
│         │               │                   │               │
│    ┌────▼────────────────▼───────────────────▼────┐         │
│    │              SocketPool                       │         │
│    │  ┌─────────────────────────────────────────┐ │         │
│    │  │  UDP Sockets (Main + Assistant)         │ │         │
│    │  │  - Main: Primary communication          │ │         │
│    │  │  - Assistant: Symmetric NAT probing     │ │         │
│    │  └─────────────────────────────────────────┘ │         │
│    │  ┌─────────────────────────────────────────┐ │         │
│    │  │  TCP Connections                        │ │         │
│    │  │  - Length-prefixed framing              │ │         │
│    │  │  - Async read/write loops               │ │         │
│    │  └─────────────────────────────────────────┘ │         │
│    └──────────────────────────────────────────────┘         │
│                                                              │
├─────────────────────────────────────────────────────────────┤
│                    Transport Layer                           │
│    ┌──────────────────────────────────────────────────┐     │
│    │  Transport (Weak<UdpSocket> / Weak<TcpConn>)    │     │
│    │  - Holds reference to socket                     │     │
│    │  - Fails gracefully if socket dropped            │     │
│    └──────────────────────────────────────────────────┘     │
│                                                              │
├─────────────────────────────────────────────────────────────┤
│                    Route Table                               │
│    ┌──────────────────────────────────────────────────┐     │
│    │  RouteTable<PeerID>                              │     │
│    │  - Maps PeerID → Vec<Route>                      │     │
│    │  - Maps RouteKey → PeerID                        │     │
│    │  - Load balancing (RoundRobin, Latency, etc.)    │     │
│    └──────────────────────────────────────────────────┘     │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

## Core Components

### 1. EndPoint

The main entry point for P2P networking.

```rust
pub struct EndPoint {
    pool: Arc<SocketPool>,
    data_rx: mpsc::Receiver<(Transport, Bytes)>,
    config: Config,
}
```

**Responsibilities:**
- Bind UDP/TCP sockets
- Accept TCP connections
- Receive messages from peers
- Create Sender and Puncher handles

**Key Methods:**
- `bind(config)` - Create endpoint with configuration
- `recv()` - Receive next message (returns `Received`)
- `sender()` - Get a Sender handle for sending
- `puncher()` - Get a Puncher for NAT traversal

### 2. Sender

Lightweight, cloneable handle for sending data.

```rust
pub struct Sender(Arc<SocketPool>);
```

**Send Methods:**
- `try_send_via_all(buf, addr)` - Send via all UDP sockets
- `try_send_via_main(buf, addrs)` - Send via main sockets (round-robin)
- `send_via_assistants(buf, addr)` - Send via assistant sockets

**Query Methods:**
- `local_addr()` - Get local address
- `assistant_count()` - Count of assistant sockets
- `all_udp_sockets()` - Get all UDP sockets
- `last_tcp()` - Get last TCP connection

**Design Principle:**
Sender exposes only what users need. Internal socket management (add/remove/clean) is hidden.

### 3. Transport

Send handle to a specific peer.

```rust
pub struct Transport {
    inner: TransportInner,  // Udp(Weak<UdpSocket>) or Tcp(Weak<TcpConnection>)
    addr: SocketAddr,
}
```

**Key Features:**
- Holds `Weak` reference to socket
- Gracefully fails if socket is dropped
- Can send to peer or different address

**Methods:**
- `send(data)` - Send to peer
- `send_to(data, addr)` - Send to different address
- `protocol()` - Get Protocol (UDP/TCP)
- `remote_addr()` - Get peer address

### 4. Received

Message received from a peer.

```rust
pub struct Received {
    pub data: Bytes,
    pub transport: Transport,
}
```

**Usage:**
```rust
while let Some(received) = ep.recv().await {
    // Access data
    let data = &received.data;
    
    // Send response back via same transport
    received.transport.send(b"echo").await?;
    
    // Get route key for routing table
    let route_key = RouteKey::from_transport(&received.transport);
}
```

### 5. RouteKey

Identifies a route by protocol and address.

```rust
pub struct RouteKey {
    protocol: Protocol,  // UDP or TCP
    addr: SocketAddr,
}
```

**Construction:**
```rust
// From Transport (preferred)
let key = RouteKey::from_transport(&received.transport);

// Manual (when no Transport available)
let key = RouteKey::new(Protocol::UDP, addr);
```

**Properties:**
- `Copy + Hash + Eq` - Can be used as HashMap key
- Lightweight - Just protocol + address

### 6. Route

Route with quality metrics.

```rust
pub struct Route {
    route_key: RouteKey,
    metric: u8,   // 0 = direct, >0 = relay hops
    rtt: u32,     // Round-trip time
}
```

**Usage with RouteTable:**
```rust
// Add route
route_table.add_route(peer_id, (route_key, metric));

// Get best route
if let Some(route) = route_table.get_route_by_id(&peer_id) {
    let key = route.route_key();
    sender.try_send_via_all(data, key.addr());
}
```

### 7. Puncher

NAT hole-punching logic.

```rust
pub struct Puncher {
    pool: Arc<SocketPool>,
    // ... internal state
}
```

**Construction:**
```rust
// From EndPoint (preferred)
let puncher = ep.puncher();

// Manual
let puncher = Puncher::new(pool);
```

**Methods:**
- `punch(buf, punch_info)` - Execute hole punching
- `need_punch(punch_info)` - Check if punch needed

## Data Flow

### Receiving Messages

```
UDP Socket ──┐
             ├──▶ SocketPool reader tasks ──▶ mpsc channel ──▶ EndPoint::recv()
TCP Socket ──┘                                                    │
                                                                  ▼
                                                          Received { data, transport }
```

### Sending Messages

```
User Code ──▶ Sender::try_send_via_all() ──▶ SocketPool ──▶ UDP Socket
                               │
                               └──▶ Transport::send() ──▶ Weak<Socket> upgrade ──▶ Socket
```

### NAT Hole Punching

```
Client A                    Server                    Client B
    │                          │                          │
    │──── PUNCH_START ────────▶│                          │
    │                          │◀──── PUNCH_START ────────│
    │                          │                          │
    │◀─── PUNCH_START (relay) ─│──── PUNCH_START (relay) ─▶│
    │                          │                          │
    │═══════════ UDP Punch Packets ═══════════════════════▶│
    │◀══════════ UDP Punch Packets ═══════════════════════│
    │                          │                          │
    │──── PUNCH_REQ ──────────────────────────────────────▶│
    │◀─── PUNCH_RES ──────────────────────────────────────│
    │                          │                          │
    │═══════════ Direct P2P Connection Established ═══════│
```

## Socket Management

### Socket Roles

| Role | Purpose | Count |
|------|---------|-------|
| Main | Primary communication | 1-10 |
| Assistant | Symmetric NAT probing | 0-200 |

### Socket Lifecycle

```
Create ──▶ Add to Pool ──▶ Reader Task Spawned ──▶ Ready
                                    │
                                    ▼
                              Socket Dropped ──▶ Reader Exits ──▶ Removed
```

## Configuration

```rust
Config::new()
    .udp_port(3000)           // UDP listen port
    .tcp_port(3000)           // TCP listen port
    .stun_servers(vec![...])  // STUN servers for NAT detection
    .load_balance(LoadBalance::MinHopLowestLatency)
```

## Error Handling

- **Transport::send()** - Returns `Err` if socket dropped
- **Sender::try_send_via_all()** - Non-blocking, ignores individual failures
- **RouteTable** - Returns `Err` if route not found

## Thread Safety

- `SocketPool` - Protected by `RwLock`
- `RouteTable` - Uses `DashMap` for concurrent access
- `Transport` - `Clone + Send + Sync`
- `Sender` - `Clone` (wraps `Arc<SocketPool>`)
