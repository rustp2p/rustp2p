use rustp2p_quic::{Endpoint, GroupCode, Identity, PeerAddr};
use serial_test::serial;
use std::net::SocketAddr;
use std::time::Duration;

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn peer_addr(endpoint: &Endpoint, identity: &Identity) -> PeerAddr {
    PeerAddr::new(endpoint.peer_id(), vec![endpoint.local_addr().unwrap()])
        .with_public_key(identity.public_key())
}

#[tokio::test]
#[serial]
async fn direct_unreliable_send_recv() {
    let group = GroupCode::try_from("test-direct").unwrap();
    let id_a = Identity::generate().unwrap();
    let id_b = Identity::generate().unwrap();

    let a = Endpoint::builder()
        .identity(id_a.clone())
        .group(group)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let b = Endpoint::builder()
        .identity(id_b.clone())
        .group(group)
        .bind(loopback())
        .build()
        .await
        .unwrap();

    a.add_peer(peer_addr(&b, &id_b)).await.unwrap();
    b.add_peer(peer_addr(&a, &id_a)).await.unwrap();

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
async fn direct_reliable_stream_send_recv() {
    let group = GroupCode::try_from("test-stream").unwrap();
    let id_a = Identity::generate().unwrap();
    let id_b = Identity::generate().unwrap();

    let a = Endpoint::builder()
        .identity(id_a.clone())
        .group(group)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let b = Endpoint::builder()
        .identity(id_b.clone())
        .group(group)
        .bind(loopback())
        .build()
        .await
        .unwrap();

    a.add_peer(peer_addr(&b, &id_b)).await.unwrap();
    b.add_peer(peer_addr(&a, &id_a)).await.unwrap();

    let mut out = a.open_stream_to(b.peer_id()).await.unwrap();
    out.write_all(b"secret").await.unwrap();

    let mut inbound = tokio::time::timeout(Duration::from_secs(15), b.accept_stream())
        .await
        .unwrap()
        .unwrap();
    let mut buf = [0u8; 32];
    let n = inbound.read(&mut buf).await.unwrap().unwrap();
    assert_eq!(&buf[..n], b"secret");
}

#[tokio::test]
#[serial]
async fn reliable_stream_can_relay_through_middle_peer() {
    let group = GroupCode::try_from("test-relay").unwrap();
    let id_a = Identity::generate().unwrap();
    let id_b = Identity::generate().unwrap();
    let id_c = Identity::generate().unwrap();

    let a = Endpoint::builder()
        .identity(id_a.clone())
        .group(group)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let b = Endpoint::builder()
        .identity(id_b.clone())
        .group(group)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let c = Endpoint::builder()
        .identity(id_c.clone())
        .group(group)
        .bind(loopback())
        .build()
        .await
        .unwrap();

    a.add_peer(peer_addr(&b, &id_b)).await.unwrap();
    b.add_peer(peer_addr(&a, &id_a)).await.unwrap();
    b.add_peer(peer_addr(&c, &id_c)).await.unwrap();
    c.add_peer(peer_addr(&b, &id_b)).await.unwrap();

    let c_via_b = PeerAddr::new(c.peer_id(), Vec::new())
        .with_public_key(id_c.public_key())
        .with_relay_hint(b.peer_id());
    a.add_peer(c_via_b).await.unwrap();

    let mut out = a.open_stream_to(c.peer_id()).await.unwrap();
    out.write_all(b"through relay").await.unwrap();

    let mut inbound = tokio::time::timeout(Duration::from_secs(15), c.accept_stream())
        .await
        .unwrap()
        .unwrap();
    let mut buf = [0u8; 64];
    let n = inbound.read(&mut buf).await.unwrap().unwrap();
    assert_eq!(&buf[..n], b"through relay");
}

#[tokio::test]
#[serial]
async fn unreliable_message_can_relay_through_different_group_peer() {
    let edge_group = GroupCode::try_from("edge-group").unwrap();
    let relay_group = GroupCode::try_from("relay-group").unwrap();
    let id_a = Identity::generate().unwrap();
    let id_b = Identity::generate().unwrap();
    let id_c = Identity::generate().unwrap();

    let a = Endpoint::builder()
        .identity(id_a.clone())
        .group(edge_group)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let b = Endpoint::builder()
        .identity(id_b.clone())
        .group(relay_group)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let c = Endpoint::builder()
        .identity(id_c.clone())
        .group(edge_group)
        .bind(loopback())
        .build()
        .await
        .unwrap();

    a.add_peer(peer_addr(&b, &id_b)).await.unwrap();
    b.add_peer(peer_addr(&a, &id_a)).await.unwrap();
    b.add_peer(peer_addr(&c, &id_c)).await.unwrap();
    c.add_peer(peer_addr(&b, &id_b)).await.unwrap();

    let c_via_b = PeerAddr::new(c.peer_id(), Vec::new())
        .with_public_key(id_c.public_key())
        .with_relay_hint(b.peer_id());
    a.add_peer(c_via_b).await.unwrap();

    a.send_to(c.peer_id(), b"cross group relay").await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), c.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.src, a.peer_id());
    assert_eq!(msg.dest, c.peer_id());
    assert_eq!(msg.payload.as_ref(), b"cross group relay");
}

#[tokio::test]
#[serial]
async fn reliable_quic_relay_through_different_group_peer_is_not_accepted_by_relay() {
    let edge_group = GroupCode::try_from("edge-stream").unwrap();
    let relay_group = GroupCode::try_from("relay-stream").unwrap();
    let id_a = Identity::generate().unwrap();
    let id_b = Identity::generate().unwrap();
    let id_c = Identity::generate().unwrap();

    let a = Endpoint::builder()
        .identity(id_a.clone())
        .group(edge_group)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let b = Endpoint::builder()
        .identity(id_b.clone())
        .group(relay_group)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let c = Endpoint::builder()
        .identity(id_c.clone())
        .group(edge_group)
        .bind(loopback())
        .build()
        .await
        .unwrap();

    a.add_peer(peer_addr(&b, &id_b)).await.unwrap();
    b.add_peer(peer_addr(&a, &id_a)).await.unwrap();
    b.add_peer(peer_addr(&c, &id_c)).await.unwrap();
    c.add_peer(peer_addr(&b, &id_b)).await.unwrap();

    let c_via_b = PeerAddr::new(c.peer_id(), Vec::new())
        .with_public_key(id_c.public_key())
        .with_relay_hint(b.peer_id());
    a.add_peer(c_via_b).await.unwrap();

    let mut out = a.open_stream_to(c.peer_id()).await.unwrap();
    out.write_all(b"end to end quic").await.unwrap();

    let mut inbound = tokio::time::timeout(Duration::from_secs(15), c.accept_stream())
        .await
        .unwrap()
        .unwrap();
    let mut buf = [0u8; 64];
    let n = inbound.read(&mut buf).await.unwrap().unwrap();
    assert_eq!(&buf[..n], b"end to end quic");

    let relay_stream = tokio::time::timeout(Duration::from_millis(300), b.accept_stream()).await;
    assert!(
        relay_stream.is_err(),
        "relay must not terminate the QUIC stream"
    );
}

#[tokio::test]
#[serial]
async fn group_mismatch_drops_final_unreliable_delivery_but_not_relay_forwarding() {
    let group_a = GroupCode::try_from("group-a").unwrap();
    let group_b = GroupCode::try_from("group-b").unwrap();
    let group_c = GroupCode::try_from("group-c").unwrap();
    let id_a = Identity::generate().unwrap();
    let id_b = Identity::generate().unwrap();
    let id_c = Identity::generate().unwrap();

    let a = Endpoint::builder()
        .identity(id_a.clone())
        .group(group_a)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let b = Endpoint::builder()
        .identity(id_b.clone())
        .group(group_b)
        .bind(loopback())
        .build()
        .await
        .unwrap();
    let c = Endpoint::builder()
        .identity(id_c.clone())
        .group(group_c)
        .bind(loopback())
        .build()
        .await
        .unwrap();

    a.add_peer(peer_addr(&b, &id_b)).await.unwrap();
    b.add_peer(peer_addr(&a, &id_a)).await.unwrap();
    b.add_peer(peer_addr(&c, &id_c)).await.unwrap();
    c.add_peer(peer_addr(&b, &id_b)).await.unwrap();

    let c_via_b = PeerAddr::new(c.peer_id(), Vec::new())
        .with_public_key(id_c.public_key())
        .with_relay_hint(b.peer_id());
    a.add_peer(c_via_b).await.unwrap();

    a.send_to(c.peer_id(), b"should be dropped").await.unwrap();
    let result = tokio::time::timeout(Duration::from_millis(500), c.recv()).await;
    assert!(
        result.is_err(),
        "final receiver must enforce group delivery"
    );
}
