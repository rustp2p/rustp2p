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
        .udp_ports(vec![8080])
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



### 打洞原理
1. 穿透原理，简单说是在双方nat留下映射记录，A、B的NAT均阻止外部流量进入，A主动访问B的地址，会在A的nat上留下映射，此后B就可以访问A。同理B端也一样。在A、B两端的的nat均留下映射，双方就能相互访问
2. Cone，这种nat是像一个锥形访问外部，内部的相同地址会映射一个唯一的地址访问外网，这样就能在nat上留下确定映射，所以一般一轮相互访问就能穿透成功
3. Symmetric，内部相同地址访问外部不同地址，会映射多个地址，（中继服务器看到的地址和对端看到的地址不相同，此时直接建立的映射关系就是错的，所以要猜测对方地址）
4. Cone-Symmetric成功率高的原因：Sym端会建立一批socket，映射到外网也就有一批地址，假设ip不变只变端口（一般Sym nat的策略），那猜中一个socket端口的概率是1/65535 。如果建立一百个端口，并且每次探测都随机100个端口，那猜中的概率会接近1，也就是说很容易成功
````
// 假设对方绑定n个端口，通过NAT对外映射出n个 公网ip:公网端口，自己随机尝试k次的情况下
// 猜中的概率 p = 1-((65535-n)/65535)*((65535-n-1)/(65535-1))*...*((65535-n-k+1)/(65535-k+1))
// n取76，k取600，猜中的概率就超过50%了
// 前提 自己是锥形网络，否则猜中了也通信不了
````
