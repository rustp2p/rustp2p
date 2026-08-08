# P2P NAT Traversal: Principles and Implementation

> This document describes the complete principles, workflow, and strategies for NAT traversal
> (UDP hole punching) in the rustp2p project, and analyzes the health-check mechanisms for
> direct connections established after successful hole punching.

---

## Table of Contents

1. [NAT Type Overview](#1-nat-type-overview)
2. [Core Principle: Why Hole Punching Works](#2-core-principle-why-hole-punching-works)
3. [NAT Type Combinations and Punching Strategies](#3-nat-type-combinations-and-punching-strategies)
4. [Complete rustp2p Hole Punching Workflow](#4-complete-rustp2p-hole-punching-workflow)
5. [Key Mechanisms in Detail](#5-key-mechanisms-in-detail)
6. [Complete Example Scenario](#6-complete-example-scenario)
7. [Direct Connection Health Check Analysis](#7-direct-connection-health-check-analysis)

---

## 1. NAT Type Overview

rustp2p simplifies NAT into two categories: **Cone NAT** and **Symmetric NAT**.

### 1.1 Cone NAT

**Characteristic**: The same internal socket (IP:Port) uses the **same mapped port** for **all external destinations**.

```
+----------------------------------------------+
|              Cone NAT Mapping Table           |
|                                              |
|  Internal 10.0.0.5:51820 -> Public 203.0.113.10:40000
|                                              |
|  To STUN Server A  -> source port 40000      |
|  To STUN Server B  -> source port 40000      |
|  To Peer X         -> source port 40000      |
|  To Peer Y         -> source port 40000      |
|  (All destinations share the same mapped port)|
+----------------------------------------------+
```

**Traversal advantage**: The mapped port is fixed and predictable. The peer only needs to know one public address to connect directly.

Cone NAT can be further classified into three subtypes (rustp2p does not differentiate; all are treated as Cone):

| Subtype | Inbound packet admission rule | Traversal difficulty |
|---------|------------------------------|---------------------|
| Full Cone | Any source IP allowed | Easiest |
| Restricted Cone | Source IP must match a previously contacted IP | Easy |
| Port Restricted Cone | Source IP + source port must both match | Harder |

### 1.2 Symmetric NAT

**Characteristic**: The same internal socket assigns **different mapped ports** for **different external destinations**.

```
+--------------------------------------------------+
|             Symmetric NAT Mapping Table           |
|                                                  |
|  Internal 10.0.0.5:51820                         |
|    +-- To STUN Server A  -> source port 40001    |
|    +-- To STUN Server B  -> source port 40005    |
|    +-- To Peer X         -> source port 40012    |
|    +-- To Peer Y         -> source port 40018    |
|    (Each destination gets a different port,      |
|     ports follow a sequential increment pattern)  |
+--------------------------------------------------+
```

**Traversal challenge**: The peer cannot predict which port the Symmetric NAT will assign for its connection. Port prediction ("guessing") is required.

---

## 2. Core Principle: Why Hole Punching Works

### 2.1 The Essence of Hole Punching

The core principle of UDP hole punching: **Both peers simultaneously send packets to each other, each creating an outbound mapping on their own NAT. This mapping allows the other peer's inbound packets to pass through.**

```
              Public Internet
    +----------------------------------+
    |                                  |
    |   NAT-C          NAT-B           |
    |  (Cone)        (Symmetric)       |
    |  203.0.113.10  198.51.100.20     |
    +-----+--------------+-------------+
          |              |
     +----+----+   +----+----+
     | node-c  |   | node-b  |
     | 10.0.0.5|   | 10.1.0.5|
     +---------+   +---------+

Step 1: node-c sends packets to node-b's public IP (port prediction)
  -> node-c's NAT creates mapping: allow inbound from 198.51.100.20

Step 2: node-b sends packets to node-c's public address (Cone port is fixed)
  -> node-b's NAT assigns a new port for this destination
  -> Packet arrives at node-c's NAT, source IP matches -> admitted!

Step 3: Direct connection established between both peers
```

### 2.2 Why Simultaneous Bidirectional Punching Is Required

Unidirectional packet sending is not enough. If only node-b sends to node-c, and node-c has never sent a packet to node-b:

- If node-c is behind **Restricted Cone NAT**: node-c's NAT will reject the packet (no outbound record for 198.51.100.20)
- If node-c is behind **Port Restricted Cone NAT**: Even stricter — the port must also match

Only when node-c **also sends packets to node-b's IP** will node-c's NAT create an admission rule allowing inbound packets from that IP.

**This is why rustp2p performs hole punching bidirectionally and simultaneously** — each peer opens a "hole" in the other's NAT.

---

## 3. NAT Type Combinations and Punching Strategies

Different NAT type combinations have different success rates:

### 3.1 Cone <-> Cone (Easiest)

```
node-c (Cone, 203.0.113.10:40000)
node-b (Cone, 198.51.100.20:50000)

Both peers have fixed ports, just send directly to each other:
  node-c -> 198.51.100.20:50000  (direct send)
  node-b -> 203.0.113.10:40000   (direct send)

Success rate: ~100%
```

### 3.2 Cone <-> Symmetric (Common Scenario)

```
node-c (Cone, 203.0.113.10:40000)        <- port is fixed
node-b (Symmetric, 198.51.100.20:???)    <- port is NOT fixed

Strategy:
  node-c -> node-b: Port prediction (Phase 1 range prediction + Phase 2 global scan)
  node-b -> node-c: Direct send to 203.0.113.10:40000 (Cone port is fixed)

Success rate: Depends on port prediction hit rate, typically 60-90%
```

### 3.3 Symmetric <-> Symmetric (Hardest)

```
node-c (Symmetric, 203.0.113.10:???)
node-b (Symmetric, 198.51.100.20:???)

Both peers have non-fixed ports, mutual port prediction:
  node-c -> node-b: Predict ports (based on known port range)
  node-b -> node-c: Predict ports (based on known port range)

Success rate: Lower, requires extensive port prediction and assistant sockets
```

---

## 4. Complete rustp2p Hole Punching Workflow

### 4.1 Overall Architecture

```
+-------------------------------------------------------------+
|                     rustp2p Architecture                     |
|                                                             |
|  +-----------------------------------------------------+   |
|  |              rustp2p-quic (Protocol Layer)           |   |
|  |                                                     |   |
|  |  ProtocolLayer                                      |   |
|  |  +- PunchRequest/PunchReply handling                |   |
|  |  +- NatObserve address discovery                    |   |
|  |  +- Route confirmation (confirm_direct_and_promote) |   |
|  |  +- Rate limiting (try_execute_punch)               |   |
|  |  +- Maintenance loop (start_maintenance_loop)       |   |
|  +---------------------------+-------------------------+   |
|                              |                             |
|  +---------------------------+-------------------------+   |
|  |              rustp2p-core (Transport Layer)          |   |
|  |                                                     |   |
|  |  +----------+  +----------+  +------------------+  |   |
|  |  |  STUN    |  | Puncher  |  |  Endpoint        |  |   |
|  |  | NAT detect| | Punching |  |  SocketPool      |  |   |
|  |  |          |  | executor |  |  (main+assistant) |  |   |
|  |  +----------+  +----------+  +------------------+  |   |
|  +-----------------------------------------------------+   |
+-------------------------------------------------------------+
```

### 4.2 Phase 1: NAT Type Detection

```
+---------------------------------------------------------+
|                   STUN NAT Detection Flow                |
|                                                         |
|  stun_test_nat()                                        |
|  +- Create temporary UDP socket (0.0.0.0:0)             |
|  +- Send BindingRequest to STUN Server A                |
|  |   -> Get mapped address 203.0.113.10:40001           |
|  +- Send BindingRequest to STUN Server B                |
|  |   -> Get mapped address 203.0.113.10:40005           |
|  |                                                      |
|  +- Decision:                                           |
|  |   Same mapped address -> Cone NAT                    |
|  |   Different mapped addresses -> Symmetric NAT        |
|  |                                                      |
|  +- port_range calculation: max_port - min_port = 4    |
|  |                                                      |
|  +- apply_stun_result_to_nat_info:                      |
|      Only save nat_type and port_range                  |
|      (temp socket port != main socket port, unusable)   |
+---------------------------------------------------------+
```

> **Key Design**: STUN detection uses a temporary socket (`0.0.0.0:0`), whose mapped port
> differs from the main QUIC socket's port. Therefore, STUN results are **only used for NAT
> type classification and port range estimation**. Real public IP/port is discovered via NatObserve.

### 4.3 Phase 2: Public Address Discovery (NatObserve)

NatObserve leverages existing QUIC connections (via relay) to let the peer observe your actual source address:

```
+-------------+                    +-------------+
|   node-c    |                    |   node-b    |
|  (Cone NAT) |                    |(Symmetric)  |
+------+------+                    +------+------+
       |                                  |
       |  ---- NatObserveRequest ---->    |  (via relay)
       |                                  |
       |  <---- NatObserveReply ------    |  "The source address I observed for you is
       |                                  |   198.51.100.20:40012"
       |                                  |
       |  apply_observation_to_nat_info:  |
       |  public_ips = [198.51.100.20]    |
       |  public_udp_ports = [40012]      |
       +----------------------------------+
```

### 4.4 Phase 3: Endpoint Configuration (Assistant Sockets)

Dynamically adjust the socket pool based on NAT type:

```
apply_nat_model(NatType::Symmetric):
  +--------------------------------------------+
  |  Symmetric NAT -> Create Assistant Sockets |
  |                                            |
  |  Main Socket:     10.0.0.5:51820           |
  |  Assistant Socket 1: 10.0.0.5:41322       |
  |  Assistant Socket 2: 10.0.0.5:51901       |
  |  ...                                       |
  |                                            |
  |  Each socket has a different mapped port   |
  |  under Symmetric NAT, increasing the       |
  |  probability of hitting the peer's NAT     |
  +--------------------------------------------+

apply_nat_model(NatType::Cone):
  +--------------------------------------------+
  |  Cone NAT -> Remove Assistant Sockets      |
  |                                            |
  |  Keep only Main Socket: 10.0.0.5:51820     |
  |  (Cone NAT port is fixed, no need for      |
  |   multiple source ports)                   |
  +--------------------------------------------+
```

### 4.5 Phase 4: Hole Punching Execution

#### 4.5.1 Punching Triggers

Hole punching is triggered in three ways:

```
Trigger 1: Auto-punch loop (start_maintenance_loop, every 10s/peer)
  -> Automatically initiates punching for peers without direct routes

Trigger 2: Triggered upon receiving PunchRequest/PunchReply
  -> try_execute_punch (with rate limiting and address injection)

Trigger 3: Application-layer explicit call
  -> protocol.punch(peer_id)
```

#### 4.5.2 UDP Punching Strategy

```
punch_udp() selects strategy based on peer's NAT type:

+-------------------------------------------------------------+
|                    Common Pre-steps                          |
|  1. Send to mapping_udp_addr (manual port mapping)          |
|  2. Send to local_ipv4_addrs (same LAN)                     |
+---------------------------+----------------------------------+
                            |
              +-------------+-------------+
              v                           v
+---------------------+   +---------------------------------+
|   Peer is Cone NAT  |   |    Peer is Symmetric NAT         |
|                     |   |                                 |
|  Strategy: Direct   |   |  Phase 1: Range Prediction      |
|  Send               |   |  (max 60 ports)                 |
|                     |   |  +- predict_range =              |
|  for addr in        |   |  |   max(port_range x 10, 100)  |
|    public_addrs:    |   |  +- Generate [base +/- range]   |
|    try_send_via_all |   |  |   candidates                  |
|      (buf, addr)    |   |  +- Deduplicate + shuffle        |
|                     |   |  +- Send first 60                |
|  (Cone port fixed,  |   |                                 |
|   just send direct) |   |  Phase 2: Global Random Scan     |
|                     |   |  (1200-1500 ports/round)        |
|                     |   |  +- Use pre-generated            |
|                     |   |  |   shuffled_ports (1-65535)   |
|                     |   |  +- port_cursor persistent       |
|                     |   |  |   cursor, no repeat           |
|                     |   |  +- 2ms interval per packet      |
|                     |   |                                 |
|                     |   |  Send method: try_send_via_all   |
|                     |   |  (main + assistant sockets)      |
+---------------------+   +---------------------------------+
```

#### 4.5.3 Symmetric NAT Port Prediction in Detail

```
Assume peer (Symmetric NAT) known info:
  public_ips = [198.51.100.20]
  public_udp_ports = [40012]  (one mapped port obtained via NatObserve)
  port_range = 4  (port variation range detected by STUN)

Phase 1 Prediction:
  predict_range = max(4 x 10, 100) = 100
  Candidate port range: [40012 - 100, 40012 + 100] = [39912, 40112]
  -> Generate 201 candidate ports
  -> Deduplicate + random shuffle
  -> Send to first 60
  -> Each port sent via main + assistant sockets

Phase 2 Global Scan:
  Start from shuffled_ports[0], take 1200-1500 random ports
  Next round continues from where the last round ended (port_cursor)
  Wrap around to 0 when reaching the end
```

> **Why multiply predict_range by 10?** Because the peer has multiple sockets (main + assistant),
> each with independent port allocation under Symmetric NAT. The port difference between different
> sockets may be much larger than the port difference for a single socket across different destinations.

### 4.6 Phase 5: Route Confirmation

```
+-------------------------------------------------------------+
|                    Route Confirmation Flow                   |
|                                                             |
|  PunchRequest arrives (metric=0):                           |
|  +-----------------------------------------------------+   |
|  |  metric=0 means the packet arrived directly          |   |
|  |  (not relayed)                                       |   |
|  |  -> Received = bidirectional reachable = direct OK!  |   |
|  |  -> Immediately confirm_direct_and_promote           |   |
|  |     (no need to wait for Reply)                      |   |
|  |  -> Store in route_candidates                        |   |
|  |  -> try_execute_punch (reverse punch, help peer)     |   |
|  |  -> Send PunchReply                                  |   |
|  +-----------------------------------------------------+   |
|                                                             |
|  PunchReply arrives (metric=0, request_id matches):        |
|  +-----------------------------------------------------+   |
|  |  Remove request_id from pending_punch                |   |
|  |  -> First time: confirm_direct_and_promote + log     |   |
|  |  -> Already confirmed: debug log only (dedup)        |   |
|  |  -> try_execute_punch (reverse punch)                |   |
|  +-----------------------------------------------------+   |
|                                                             |
|  confirm_direct_and_promote:                                |
|  -> transport.confirm_peer_route(peer, route_key, metric=0)|
|  -> route_candidates.remove(peer_id)                        |
|  -> Subsequent QUIC handshake packets prefer metric=0 route |
+-------------------------------------------------------------+
```

#### Why Can PunchRequest (metric=0) Directly Confirm Direct Connection?

This is a reliable inference based on NAT mapping principles:

```
node-b sends PunchRequest -> node-c

  1. Packet leaves node-b's socket, passes through node-b's NAT
  2. Packet traverses the public internet to node-c's NAT
  3. node-c's NAT admits it (because node-c also sent packets to node-b,
     so the mapping already exists)
  4. Packet arrives at node-c's socket

  Receiving a metric=0 packet = node-b can send to node-c = node-c can reply to node-b
  (because node-c's NAT mapping already allows traffic in this direction)

  Therefore: Received = bidirectional reachable = direct connection successful
```

---

## 5. Key Mechanisms in Detail

### 5.1 Assistant Socket

```
+--------------------------------------------------------+
|              Purpose of Assistant Sockets              |
|                                                        |
|  Only enabled under Symmetric NAT.                     |
|                                                        |
|  Scenario: node-b (Symmetric) punching to node-c (Cone)|
|                                                        |
|  node-b has 3 sockets:                                 |
|    Socket 0 (main): -> NAT mapping 198.51.100.20:40012|
|    Socket 1 (asst): -> NAT mapping 198.51.100.20:40045|
|    Socket 2 (asst): -> NAT mapping 198.51.100.20:40078|
|                                                        |
|  When try_send_via_all sends to node-c:                |
|    Socket 0 -> 198.51.100.20:40012 -> 203.0.113.10:40000
|    Socket 1 -> 198.51.100.20:40045 -> 203.0.113.10:40000
|    Socket 2 -> 198.51.100.20:40078 -> 203.0.113.10:40000
|                                                        |
|  node-c receives and observes 3 source ports from node-b:
|    40012, 40045, 40078                                 |
|                                                        |
|  When node-c reverse-punches to node-b:                |
|    Phase 1 prediction covers: [40012+/-100, 40045+/-100, 40078+/-100]
|    -> Hit probability ~3x higher                       |
|                                                        |
|  Also, all 3 of node-b's sockets are listening:        |
|    If node-c's prediction hits Socket 1's mapped port, |
|    Socket 1 also receives it -> assistant socket can   |
|    also establish direct connection                    |
+--------------------------------------------------------+
```

### 5.2 Direct Address Injection

When a PunchRequest arrives via a direct route (metric=0), the source address in route_key is more accurate than the public address in the peer's NatInfo:

```
Problem scenario:
  node-c learned node-b's address via NatObserve: 198.51.100.20:40012
  But 40012 is the mapped port for node-b's connection to the NatObserve relay node

  When node-b directly sends a PunchRequest to node-c:
    node-b's NAT assigns a new port for the node-c destination: 40035
    -> route_key address is 198.51.100.20:40035 (more accurate!)

  try_execute_punch injects:
    nat_info.public_ips = [198.51.100.20]
    nat_info.public_udp_ports = [40012, 40035]  <- inject route_key port

  -> Phase 1 prediction now covers ranges around both 40012 and 40035
  -> Higher hit probability
```

### 5.3 Rate Limiting and Backoff

```
Multi-layer rate limiting:

ProtocolLayer:
  +----------------------------------------------+
  | try_execute_punch rate limit:                |
  | +- Already has direct route -> skip          |
  | |   (has_direct_route)                       |
  | +- metric > 0 (relay arrival) -> 5s limit    |
  | +- metric = 0 (direct arrival) -> no limit   |
  |   (direct arrival is the best hit            |
  |    opportunity, should not be limited)       |
  +----------------------------------------------+

  +----------------------------------------------+
  | auto-punch loop rate limit:                  |
  | +- Max once per 10s per peer                 |
  +----------------------------------------------+

Puncher:
  +----------------------------------------------+
  | need_punch backoff:                          |
  | +- First 8 times: punch every time          |
  | +- After 8 times: exponential backoff       |
  |    interval = total_count / 8 (cap 360)     |
  |    Only punch when total_count % interval == 0|
  +----------------------------------------------+
```

### 5.4 NatObserve Address vs Actual Punching Address

```
Address observed by node-b via node-a (relay):
  198.51.100.20:40012  (this is the mapped port for node-b -> node-a direction)

Address assigned when node-b directly punches to node-c:
  198.51.100.20:40035  (this is the mapped port for node-b -> node-c direction)

Why are they different?
  Symmetric NAT assigns different ports for different destinations
  node-a and node-c are different destinations -> different ports

  40012 != 40035, but usually within a similar range (determined by port_range)
  Phase 1 prediction covers port ranges around both -> can still hit
```

---

## 6. Complete Example Scenario

### Scenario: Three-Node Network (node-a relay + node-b/node-c punching)

```
Network topology:
  node-a: Public server 203.0.113.1 (no NAT, acts as relay)
  node-b: Symmetric NAT, public IP 198.51.100.20
  node-c: Cone NAT, public IP 203.0.113.10
```

#### Step 1: Initial Connection (via relay)

```
node-b ---- connect ----> node-a (relay) <---- connect ---- node-c
         (QUIC via relay)                    (QUIC via relay)

node-c observes node-b's source address via NatObserve: 198.51.100.20:40012
node-b observes node-c's source address via NatObserve: 203.0.113.10:40000
```

#### Step 2: Auto-Punch Trigger

```
node-c's maintenance_loop detects:
  - peer node-b has no direct route (has_direct_route = false)
  - Has node-b's NatInfo: { ips: [198.51.100.20], ports: [40012], type: Symmetric, port_range: 4 }

-> Trigger execute_punch(node-b, nat_info)
-> Build PunchRequest, call Puncher::punch_now
```

#### Step 3: node-c Executes Punching (sends to node-b)

```
node-c's Puncher executes punch_udp:

  Peer is Symmetric NAT -> Two-phase strategy

  Phase 1 (Range Prediction, max 60 ports):
    predict_range = max(4 x 10, 100) = 100
    Candidate range: [40012-100, 40012+100] = [39912, 40112]
    -> 201 candidates, deduplicate + shuffle, take first 60
    -> Send via main socket to 198.51.100.20:candidate_port

    * Each outbound packet passes through node-c's Cone NAT
    * Cone NAT records: "Allow inbound traffic from 198.51.100.20"

  Phase 2 (Global Random Scan, 1200-1500 ports):
    Take 1350 random ports from shuffled_ports cursor position
    -> Send via main socket to 198.51.100.20:random_port
```

#### Step 4: node-b Executes Punching (sends to node-c)

```
node-b receives node-c's PunchRequest via relay (metric > 0)
-> try_execute_punch triggers reverse punch
-> Rate limit check: not punched in last 5s -> allowed

  Peer is Cone NAT -> Direct send strategy

  for addr in node-c's public addresses:
    try_send_via_all(PunchRequest, 203.0.113.10:40000)
    -> Main socket send:     198.51.100.20:40035 -> 203.0.113.10:40000
    -> Assistant socket 1:   198.51.100.20:40058 -> 203.0.113.10:40000
    -> Assistant socket 2:   198.51.100.20:40081 -> 203.0.113.10:40000
```

#### Step 5: Direct Connection Success

```
Case A: node-b's packet arrives at node-c first (Cone NAT admits it)
  -> node-c receives PunchRequest, metric=0
  -> Confirm direct! confirm_direct_and_promote
  -> Send PunchReply (via direct route)

Case B: node-c's prediction packet hits one of node-b's sockets first
  -> node-b receives PunchRequest, metric=0
  -> Confirm direct! confirm_direct_and_promote
  -> Send PunchReply (via direct route)

Both cases result in:
  node-c route table: node-b -> direct (metric=0, via 198.51.100.20:40035)
  node-b route table: node-c -> direct (metric=0, via 203.0.113.10:40000)
  -> QUIC handshake packets prefer direct route
  -> Direct QUIC connection established!
```

---

## 7. Direct Connection Health Check Analysis

### 7.1 Current Status Summary

After a comprehensive review of the codebase, **the current code lacks a complete direct connection health check mechanism.** Detailed analysis below:

#### 7.1.1 Existing Related Mechanisms

| Mechanism | Status | Description |
|-----------|--------|-------------|
| EchoRequest/EchoReply | **Responder only** | Protocol type is defined (protocol.rs); replies on receive, but **no code actively sends it** |
| TimestampRequest/Reply | **Responder only** | Same as above; defined but not actively used |
| Route.rtt field | **Exists but unused** | Default value `DEFAULT_RTT = 9999`; no code actually measures or updates it |
| IdleRouteManager | **Passive detection** | Detects route idle timeout (default 12s), but does not proactively send heartbeats |
| NatObserveRequest | **Indirect keepalive** | Sent every 10s to direct peers; may prolong NAT mapping as a side effect |
| Quinn default keepalive | **Relies on defaults** | Quinn has internal PING frame keepalive, but `keep_alive_interval` is not customized |
| STUN periodic detection | **Present** | Detects NAT type changes every 10s, but does not involve direct connection keepalive |

#### 7.1.2 Missing Key Mechanisms

```
+-------------------------------------------------------------+
|               Missing Health Check Mechanisms                |
|                                                             |
|  1. Direct Connection Heartbeat / Keepalive                 |
|     Problem: After hole punching succeeds, if there is no   |
|     application-layer data transmission, the NAT mapping    |
|     may age and be deleted (typically 30-120 seconds),      |
|     causing the direct connection to break.                 |
|                                                             |
|     Current state:                                          |
|     - Maintenance loop skips HelloRequest for confirmed     |
|       direct routes (metric=0)                              |
|     - Auto-punch skips peers that already have direct route |
|     - Only NatObserveRequest (every 10s) provides indirect  |
|       keepalive                                             |
|     - If NatObserve fails for any reason, NAT mapping may   |
|       expire                                                |
|                                                             |
|  2. Latency Measurement (RTT)                               |
|     Problem: Route.rtt is always 9999, cannot be used for   |
|     route selection                                         |
|                                                             |
|     Current state:                                          |
|     - EchoRequest/TimestampRequest defined but never sent   |
|     - LowestLatency routing strategy cannot function        |
|       properly (all rtts are identical)                     |
|                                                             |
|  3. Connection Failure Detection                            |
|     Problem: When QUIC connection drops, the direct route   |
|     in the route table is not automatically removed         |
|                                                             |
|     Current state:                                          |
|     - Quinn default max_idle_timeout ~30s                   |
|     - On connection drop, cleanup_connection cleans up the  |
|       connection object                                     |
|     - But the direct route in route_table may linger        |
|     - Lingering direct routes cause packets to be sent to   |
|       a dead connection                                     |
|                                                             |
|  4. NAT Mapping Keepalive                                   |
|     Problem: The underlying UDP socket's NAT mapping needs  |
|     periodic refresh                                        |
|                                                             |
|     Current state:                                          |
|     - NatObserveRequest sent every 10s, but only for peers  |
|       with has_direct_route, and via QUIC virtual socket    |
|     - If QUIC-layer packets don't go through the underlying |
|       UDP socket (e.g., established QUIC stream), the       |
|       underlying NAT mapping may not get refreshed          |
|     - No dedicated keepalive packet for the underlying UDP  |
|       NAT mapping                                           |
+-------------------------------------------------------------+
```

### 7.2 Proposed Improvements

#### 7.2.1 Direct Connection Heartbeat Keepalive

```
Proposed: Reuse existing EchoRequest for heartbeat

+---------------------------------------------------------+
|  Add heartbeat logic in start_maintenance_loop:         |
|                                                         |
|  for peer in known_peers:                               |
|    if has_direct_route(peer):                           |
|      if last_heartbeat(peer) > 15s:                     |
|        send EchoRequest(peer)  <- reuse existing protocol|
|        record send_time(peer)                           |
|                                                         |
|  On receiving EchoReply:                                |
|    rtt = now - send_time(peer)                          |
|    update Route.rtt = rtt                               |
|    update last_heartbeat(peer) = now                    |
|                                                         |
|  If N consecutive heartbeats have no reply:             |
|    remove direct route -> trigger re-punching           |
+---------------------------------------------------------+

Advantages:
  - Reuses existing EchoRequest/EchoReply protocol (responder already implemented)
  - Solves both RTT measurement and heartbeat keepalive simultaneously
  - Sent via QUIC datagram; underlying UDP packets automatically refresh NAT mapping
  - 15s interval < typical NAT aging time (30-120s)
```

#### 7.2.2 NAT Mapping Keepalive

```
Proposed: Underlying UDP keepalive packets

+---------------------------------------------------------+
|  Problem: QUIC data goes through virtual socket, may    |
|  not traverse the underlying UDP socket                 |
|                                                         |
|  Solution: In maintenance loop, periodically send       |
|  keepalive packets via the underlying socket            |
|                                                         |
|  for peer in known_peers:                               |
|    if has_direct_route(peer):                           |
|      if last_nat_keepalive(peer) > 20s:                 |
|        addr = direct_route_addr(peer)                   |
|        pool.send_to(keepalive_packet, addr)             |
|        <- Send directly via underlying UDP socket       |
|        <- Refresh both ends' NAT mappings               |
|                                                         |
|  keepalive_packet can be:                               |
|    - An empty PunchRequest (peer will reply PunchReply) |
|    - Or a new ProtocolType::Keepalive (lightweight)     |
+---------------------------------------------------------+
```

#### 7.2.3 Connection Failure Auto-Cleanup

```
Proposed: Listen for QUIC connection close events

+---------------------------------------------------------+
|  Add route cleanup in cleanup_connection:               |
|                                                         |
|  fn cleanup_connection(&self, stable_id, peer_id):      |
|    // Existing logic: remove connection and virtual peer|
|    self.connections.remove(&peer_id);                   |
|    self.socket.release_virtual_peer(&peer_id);          |
|                                                         |
|    // New: remove direct route                          |
|    if let Some(peer_id) = peer_id {                     |
|      self.transport.remove_direct_route(&peer_id);      |
|      // -> Next maintenance_loop detects no direct route|
|      // -> Automatically triggers re-punching           |
|    }                                                    |
+---------------------------------------------------------+
```

### 7.3 Risk Assessment

| Risk | Severity | Trigger Condition | Impact |
|------|----------|------------------|--------|
| NAT mapping aging causes direct connection loss | **Medium** | Long idle period + NatObserve not executing | Direct degrades to relay; re-punching needed |
| Lingering direct route causes packet loss | **Medium** | QUIC connection dropped but route not cleaned | Packets sent to dead connection are lost |
| Inaccurate RTT affects route selection | **Low** | When using LowestLatency strategy | Suboptimal route selection; no connectivity impact |
| No proactive latency monitoring | **Low** | When link quality monitoring is needed | Cannot detect link quality degradation |

### 7.4 Currently Available Indirect Keepalive

Although explicit heartbeat mechanisms are missing, the following mechanisms **indirectly** provide some keepalive effect:

1. **NatObserveRequest (every 10s)**: Sent to direct peers via QUIC datagram. The underlying transport sends UDP packets, which objectively refresh the NAT mapping.

2. **Quinn internal PING frames**: Quinn's default behavior sends PING frames to keep connections alive when idle (default max_idle_timeout ~30s). These PING frames also traverse the underlying UDP transport.

3. **Application-layer data**: If there is continuous application-layer data transmission, the NAT mapping naturally stays active.

> **Conclusion**: Under normal usage scenarios, NatObserve + Quinn PING frames can provide basic
> keepalive. However, in extreme cases (e.g., NatObserve failure, Quinn configuration anomaly,
> prolonged complete data silence), the NAT mapping may age. Implementing an explicit heartbeat
> mechanism is recommended for improved reliability.

---

## Appendix: Key Code Location Index

| Function | File | Key Lines |
|----------|------|-----------|
| NAT type definition | `rustp2p-core/src/nat/mod.rs` | 54-58 |
| STUN detection entry | `rustp2p-core/src/stun/mod.rs` | 90-147 |
| Cone/Symmetric determination | `rustp2p-core/src/stun/mod.rs` | 210-263 |
| punch_udp core logic | `rustp2p-core/src/punch/mod.rs` | 175-319 |
| Symmetric Phase 1 | `rustp2p-core/src/punch/mod.rs` | 211-270 |
| Cone direct send | `rustp2p-core/src/punch/mod.rs` | 304-315 |
| punch_symmetric send | `rustp2p-core/src/punch/mod.rs` | 321-341 |
| apply_nat_model | `rustp2p-core/src/endpoint/service.rs` | 244-272 |
| try_send_via_all | `rustp2p-core/src/endpoint/pool.rs` | 237-242 |
| PunchRequest handler | `rustp2p-quic/src/protocol.rs` | 769-829 |
| PunchReply handler | `rustp2p-quic/src/protocol.rs` | 830-874 |
| confirm_direct_and_promote | `rustp2p-quic/src/protocol.rs` | 1265-1269 |
| try_execute_punch | `rustp2p-quic/src/protocol.rs` | 1005-1063 |
| start_maintenance_loop | `rustp2p-quic/src/protocol.rs` | 1309-1444 |
| start_nat_maintenance | `rustp2p-quic/src/protocol.rs` | 579-639 |
| EchoRequest handler | `rustp2p-quic/src/protocol.rs` | 875-884 |
| apply_stun_result_to_nat_info | `rustp2p-quic/src/protocol.rs` | 1447-1460 |
| apply_observation_to_nat_info | `rustp2p-quic/src/protocol.rs` | 1462-1501 |
| IdleRouteManager | `rustp2p-core/src/idle.rs` | 7-40 |
| DEFAULT_RTT | `rustp2p-core/src/route_table/mod.rs` | 28 |
| Quinn TransportConfig | `rustp2p-quic/src/quic.rs` | 245-249 |
| cleanup_connection | `rustp2p-quic/src/quic.rs` | 585-591 |
