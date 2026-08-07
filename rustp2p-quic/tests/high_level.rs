use rustp2p_quic::{CertificateVerifier, Endpoint, Identity, LinkMode, PeerId};
use serial_test::serial;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

async fn node(id: &str) -> Endpoint {
    Endpoint::builder()
        .identity(Identity::new(id, format!("{id}-seed")).unwrap())
        .bind(loopback())
        .stun_servers(Vec::new())
        .build()
        .await
        .unwrap()
}

async fn wait_for_peer(endpoint: &Endpoint, peer_id: &str) {
    let peer_id = PeerId::from(peer_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if endpoint.known_peers().iter().any(|p| p.peer_id == peer_id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
}

async fn close_all(nodes: &[&Endpoint]) {
    for node in nodes {
        node.close().await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
}

#[tokio::test]
#[serial]
async fn bootstrap_addr_discovers_peer_id_then_datagram_uses_peer_id_only() {
    let a = node("direct-msg-a").await;
    let b = node("direct-msg-b").await;

    let discovered = a.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    assert_eq!(discovered, b.peer_id());
    let a_nat = a.nat_info();
    assert!(a_nat
        .public_udp_ports
        .contains(&a.local_addr().unwrap().port()));

    a.send_to(b.peer_id(), b"hello").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), b.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(msg.src, a.peer_id());
    assert_eq!(msg.dest, b.peer_id());
    assert_eq!(msg.payload.as_ref(), b"hello");
    close_all(&[&a, &b]).await;
}

#[tokio::test]
#[serial]
async fn direct_reliable_stream_returns_source_info() {
    let a = node("direct-stream-a").await;
    let b = node("direct-stream-b").await;

    a.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    b.add_bootstrap(a.local_addr().unwrap()).await.unwrap();

    let (mut out, _recv) = a.open_bi(b.peer_id()).await.unwrap();
    out.write_all(b"secret").await.unwrap();

    let mut inbound = tokio::time::timeout(Duration::from_secs(15), b.accept_bi())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inbound.peer_id, a.peer_id());
    assert_eq!(b.link_mode(a.peer_id()), Some(LinkMode::Direct));

    let mut buf = [0u8; 32];
    let n = inbound.recv.read(&mut buf).await.unwrap().unwrap();
    assert_eq!(&buf[..n], b"secret");
    close_all(&[&a, &b]).await;
}

#[tokio::test]
#[serial]
async fn three_nodes_discover_and_relay_by_peer_id_only() {
    let a = node("relay-a").await;
    let b = node("relay-b").await;
    let c = node("relay-c").await;

    a.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    b.add_bootstrap(a.local_addr().unwrap()).await.unwrap();
    b.add_bootstrap(c.local_addr().unwrap()).await.unwrap();
    c.add_bootstrap(b.local_addr().unwrap()).await.unwrap();

    wait_for_peer(&a, "relay-c").await;

    a.send_to(c.peer_id(), b"through relay").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), c.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.src, a.peer_id());
    assert_eq!(msg.dest, c.peer_id());
    assert_eq!(msg.payload.as_ref(), b"through relay");

    let (mut out, _recv) = a.open_bi(c.peer_id()).await.unwrap();
    out.write_all(b"reliable relay").await.unwrap();

    let mut inbound = tokio::time::timeout(Duration::from_secs(15), c.accept_bi())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inbound.peer_id, a.peer_id());
    assert_eq!(c.link_mode(a.peer_id()), Some(LinkMode::Relay));

    let mut buf = [0u8; 64];
    let n = inbound.recv.read(&mut buf).await.unwrap().unwrap();
    assert_eq!(&buf[..n], b"reliable relay");

    let relay_stream = tokio::time::timeout(Duration::from_millis(300), b.accept_bi()).await;
    assert!(
        relay_stream.is_err(),
        "relay must not terminate the QUIC stream"
    );
    close_all(&[&a, &b, &c]).await;
}

#[tokio::test]
#[serial]
async fn four_node_chain_discovers_tail_peer() {
    let a = node("chain-a").await;
    let b = node("chain-b").await;
    let c = node("chain-c").await;
    let d = node("chain-d").await;

    a.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    b.add_bootstrap(a.local_addr().unwrap()).await.unwrap();
    b.add_bootstrap(c.local_addr().unwrap()).await.unwrap();
    c.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    c.add_bootstrap(d.local_addr().unwrap()).await.unwrap();
    d.add_bootstrap(c.local_addr().unwrap()).await.unwrap();

    wait_for_peer(&a, "chain-d").await;

    a.send_to(d.peer_id(), b"hello d").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), d.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.src, a.peer_id());
    assert_eq!(msg.payload.as_ref(), b"hello d");
    close_all(&[&a, &b, &c, &d]).await;
}

#[derive(Debug)]
struct RejectVerifier;

impl CertificateVerifier for RejectVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<(), rustls::Error> {
        Err(rustls::Error::General(
            "rejected by test verifier".to_string(),
        ))
    }
}

#[tokio::test]
#[serial]
async fn custom_certificate_verifier_can_reject_server_cert() {
    let a = Endpoint::builder()
        .identity(Identity::new("verify-a", "verify-a-seed").unwrap())
        .certificate_verifier(Arc::new(RejectVerifier))
        .bind(loopback())
        .stun_servers(Vec::new())
        .build()
        .await
        .unwrap();
    let b = node("verify-b").await;

    a.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    assert!(a.open_bi(b.peer_id()).await.is_err());
    close_all(&[&a, &b]).await;
}

#[derive(Debug)]
struct RejectClientVerifier;

impl CertificateVerifier for RejectClientVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<(), rustls::Error> {
        Ok(())
    }

    fn verify_client_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<(), rustls::Error> {
        Err(rustls::Error::General(
            "client rejected by test verifier".to_string(),
        ))
    }
}

#[tokio::test]
#[serial]
async fn custom_certificate_verifier_can_reject_client_cert() {
    let a = node("verify-client-a").await;
    let b = Endpoint::builder()
        .identity(Identity::new("verify-client-b", "verify-client-b-seed").unwrap())
        .certificate_verifier(Arc::new(RejectClientVerifier))
        .bind(loopback())
        .stun_servers(Vec::new())
        .build()
        .await
        .unwrap();

    a.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    assert!(a.open_bi(b.peer_id()).await.is_err());
    close_all(&[&a, &b]).await;
}

#[tokio::test]
#[serial]
async fn punch_whitelist_can_be_changed_at_runtime() {
    let a = node("punch-api-a").await;
    let b = node("punch-api-b").await;

    assert!(a.punch_whitelist().is_empty());
    a.allow_punch(b.peer_id());
    assert_eq!(a.punch_whitelist(), vec![b.peer_id()]);

    a.deny_punch(b.peer_id());
    assert!(a.punch_whitelist().is_empty());

    a.set_punch_whitelist(vec![b.peer_id()]);
    assert_eq!(a.punch_whitelist(), vec![b.peer_id()]);

    a.deny_punch(b.peer_id());
    let err = a.punch(b.peer_id()).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    close_all(&[&a, &b]).await;
}

#[tokio::test]
#[serial]
async fn one_way_bootstrap_both_directions_can_send() {
    let a = node("oneway-send-a").await;
    let b = node("oneway-send-b").await;

    // Only b bootstraps to a -- one-way route confirmation
    b.add_bootstrap(a.local_addr().unwrap()).await.unwrap();

    // Wait for a to discover b
    wait_for_peer(&a, "oneway-send-b").await;

    // b -> a should succeed (b has a's confirmed route via HelloReply)
    b.send_to(a.peer_id(), b"hello from b").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), a.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.src, b.peer_id());
    assert_eq!(msg.payload.as_ref(), b"hello from b");

    // a -> b should also succeed after the fix (a confirms b's route
    // when it receives the HelloRequest and sends HelloReply back)
    a.send_to(b.peer_id(), b"hello from a").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), b.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.src, a.peer_id());
    assert_eq!(msg.payload.as_ref(), b"hello from a");

    // Verify is_direct is correctly set on both sides
    let peers_a = a.known_peers();
    let peer_b = peers_a
        .iter()
        .find(|p| p.peer_id == b.peer_id())
        .expect("node-a should know node-b");
    assert!(
        peer_b.is_direct,
        "node-a should see node-b as direct after fix"
    );

    let peers_b = b.known_peers();
    let peer_a = peers_b
        .iter()
        .find(|p| p.peer_id == a.peer_id())
        .expect("node-b should know node-a");
    assert!(peer_a.is_direct, "node-b should see node-a as direct");

    close_all(&[&a, &b]).await;
}

#[tokio::test]
#[serial]
async fn one_way_bootstrap_stream_works() {
    let a = node("oneway-stream-a").await;
    let b = node("oneway-stream-b").await;

    // Only b bootstraps to a -- one-way route confirmation
    b.add_bootstrap(a.local_addr().unwrap()).await.unwrap();

    // Wait for a to discover b
    wait_for_peer(&a, "oneway-stream-b").await;

    // b opens a reliable stream to a
    let (mut send, _recv) = b.open_bi(a.peer_id()).await.unwrap();
    send.write_all(b"stream data").await.unwrap();
    send.finish().unwrap();

    // a should receive the stream
    let mut inbound = tokio::time::timeout(Duration::from_secs(15), a.accept_bi())
        .await
        .expect("timed out waiting for stream")
        .expect("accept_bi failed");
    assert_eq!(inbound.peer_id, b.peer_id());

    let mut buf = [0u8; 32];
    let n = inbound.recv.read(&mut buf).await.unwrap().unwrap();
    assert_eq!(&buf[..n], b"stream data");

    close_all(&[&a, &b]).await;
}

#[tokio::test]
#[serial]
async fn direct_and_relay_routes_coexist() {
    let a = node("coexist-a").await;
    let b = node("coexist-b").await;
    let c = node("coexist-c").await;

    // Form a chain: A — B — C
    a.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    b.add_bootstrap(a.local_addr().unwrap()).await.unwrap();
    b.add_bootstrap(c.local_addr().unwrap()).await.unwrap();
    c.add_bootstrap(b.local_addr().unwrap()).await.unwrap();

    // Wait for A to discover C via B (relay route)
    wait_for_peer(&a, "coexist-c").await;

    // A should see C as Relay (no direct route yet)
    assert_eq!(
        a.link_mode(c.peer_id()),
        Some(LinkMode::Relay),
        "A should only have a relay route to C before direct bootstrap"
    );

    // Now A bootstraps directly to C, creating a direct route
    a.add_bootstrap(c.local_addr().unwrap()).await.unwrap();

    // Wait for the direct route to be confirmed
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if a.link_mode(c.peer_id()) == Some(LinkMode::Direct) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timed out waiting for direct route to C");

    // A should now see C as Direct (preferred over relay by sort_key)
    assert_eq!(a.link_mode(c.peer_id()), Some(LinkMode::Direct));

    // A should have BOTH routes: direct and relay (coexistence)
    let routes = a.routes(c.peer_id());
    let direct_count = routes.iter().filter(|r| r.is_direct()).count();
    let relay_count = routes.iter().filter(|r| r.is_relay()).count();
    assert_eq!(direct_count, 1, "should have exactly one direct route to C");
    assert!(
        relay_count >= 1,
        "should have at least one relay route to C (coexistence)"
    );

    close_all(&[&a, &b, &c]).await;
}
