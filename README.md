A decentralized p2p library powered by Rust, which is devoted to simple use. 

[![Crates.io](https://img.shields.io/crates/v/rustp2p.svg)](https://crates.io/crates/rustp2p)
![rustp2p](https://docs.rs/rustp2p/badge.svg)

### Features
1.  UDP hole punching for both Cone and Symmetric Nat
2.  TCP hole punching for NAT1 


### Description
For connecting two peers, all you need to do is to give the configuration as done in the example. In short, provide a peer named `C`, peer `A` and `B` can directly connect to `C`, then `A` and `B` will find each other by `C`, `A` and `C` can directly connect by hole-punching, the whole process is done by this library. If two peers `D` and `F` cannot directly connect via hole-punching, this library can find the best link for indirectly connection(i.e. through some middle nodes).  

### Example

````rust
use rustp2p::Builder;
use rustp2p::protocol::node_id::GroupCode;
use rustp2p::cipher::Algorithm;
use rustp2p::tunnel::PeerNodeAddress;
use std::net::Ipv4Addr;

#[tokio::main]
async fn main(){
    let node_id = Ipv4Addr::from([10,0,0,1]);
    let endpoint = Builder::new()
        .node_id(node_id.into())
        .tcp_port(8080)
        .udp_port(8080)
        .peers(vec![PeerNodeAddress::from_str("udp://127.0.0.1:9090").unwrap()])
        .group_code(GroupCode::from(12345))
        .encryption(Algorithm::AesGcm("password".to_string()))
        .build()
        .await?;
    let receiver = endpoint.clone();
    let h = tokio::spawn(async move{
        while let Ok(peer) = receiver.recv().await{
            
        }
    });
    let peer_node_id = Ipv4Addr::from([10,0,0,1]).into();
    endpoint.send(b"hello",peer_node_id);
    _ = h.await;
}
````

- [example/node](https://github.com/rustp2p/rustp2p/blob/master/examples/node.rs)
- [https://github.com/rustp2p/netlink](https://github.com/rustp2p/netlink)
- [https://github.com/rustp2p/rustp2p-transport](https://github.com/rustp2p/rustp2p-transport)


