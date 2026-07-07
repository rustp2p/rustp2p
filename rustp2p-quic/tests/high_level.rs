use rustp2p_quic::{CertificateVerifier, Endpoint, Identity, PeerId};
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

#[tokio::test]
#[serial]
async fn bootstrap_addr_discovers_peer_id_then_datagram_uses_peer_id_only() {
    let a = node("direct-msg-a").await;
    let b = node("direct-msg-b").await;

    let discovered = a.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    assert_eq!(discovered, b.peer_id());

    a.send_to(b.peer_id(), b"hello").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), b.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(msg.src, a.peer_id());
    assert_eq!(msg.dest, b.peer_id());
    assert_eq!(msg.payload.as_ref(), b"hello");
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
    assert!(!inbound.is_relay);

    let mut buf = [0u8; 32];
    let n = inbound.recv.read(&mut buf).await.unwrap().unwrap();
    assert_eq!(&buf[..n], b"secret");
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
    assert!(inbound.is_relay);

    let mut buf = [0u8; 64];
    let n = inbound.recv.read(&mut buf).await.unwrap().unwrap();
    assert_eq!(&buf[..n], b"reliable relay");

    let relay_stream = tokio::time::timeout(Duration::from_millis(300), b.accept_bi()).await;
    assert!(
        relay_stream.is_err(),
        "relay must not terminate the QUIC stream"
    );
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
async fn custom_certificate_verifier_can_reject_reliable_connection() {
    let a = Endpoint::builder()
        .identity(Identity::new("verify-a", "verify-a-seed").unwrap())
        .certificate_verifier(Arc::new(RejectVerifier))
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let b = node("verify-b").await;

    a.add_bootstrap(b.local_addr().unwrap()).await.unwrap();
    assert!(a.open_bi(b.peer_id()).await.is_err());
}
