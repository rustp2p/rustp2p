A decentralized p2p library powered by Rust, which is devoted to simple use. 

[![Crates.io](https://img.shields.io/crates/v/rustp2p.svg)](https://crates.io/crates/rustp2p)
![rustp2p](https://docs.rs/rustp2p/badge.svg)

### Features
1.  UDP hole punching for both Cone and Symmetric Nat
2.  TCP hole punching for NAT1 


### Description
For connecting two peers, all you need to do is to give the configuration as done in the example. In short, provide a peer named `C`, peer `A` and `B` can directly connect to `C`, then `A` and `B` will find each other by `C`, `A` and `C` can directly connect by hole-punching, the whole process is done by this library. If two peers `D` and `F` cannot directly connect via hole-punching, this library can find the best link for indirectly connection(i.e. through some middle nodes).  

### Example

```rust
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
```

- [example/node](https://github.com/rustp2p/rustp2p/blob/master/examples/node.rs)
- [https://github.com/rustp2p/netlink](https://github.com/rustp2p/netlink)
- [https://github.com/rustp2p/rustp2p-transport](https://github.com/rustp2p/rustp2p-transport)



### 为什么rustp2p的打洞成功率这么高？
> Why the rate of successful hole-punching of rustp2p is so high

rustp2p打洞成功率这么高核心原理是什么呢？对于两个再Cone型Nat后的两个主机A和B, rustp2p会分别在A和B上创建一个udp socket并且通过公网服务获取公网地址和对外映射的端口，通过协调服务器向对方发送各自的NAT信息，假设A的公网地址是`10.0.0.1:8080`, B的公网地址是`20.0.0.1:9090`并且各自的NAT类型是Cone, 那么A将收到协调服务器发过来的B的信息`{ipaddr:20.0.0.1:9090,nat_type:Cone}`,同样的B将收到`{ipaddr:10.0.0.1:8080,nat_type:Cone}`,此时A向B的地址发送信息以刷新NAT的过滤规则，同样B向A的地址发送一些信息以刷新B的NAT的规律规则，此后A发送给B的信息以及B发送给A的信息就能顺利的被各自的NAT转发到内网了。这是Cone对Cone的场景，这种网络类型相对来说比较简单。

如果A的NAT是`Cone`而B的NAT是`Symmetric`,这种网络环境就很不利于打洞，不过这也是rustp2p的核心所在。面对这种场景，对于探测出自身NAT类型是A的主机和上面一样只会创建一个udp socket并且公网IP地址是`10.0.0.1:8080`,而B的NAT类型是`Symmetric`除了创建一个用来对外探测出其NAT类型和公网地址为`20.0.0.1:9090`的主要udp socket外，还会再创建若干个备用udp socket, 此时B收到协调服务器发过来的A的信息`{ipaddr:10.0.0.1:8080,nat_type:Cone}`，B会同时通过主要udp和创建的若干个备用udp往A的地址发送消息，此时A收到协调服务器发过来的B的信息`{ipaddr:20.0.0.1:9090,nat_type:Symmetric}`, 那么A除了通过它的唯一的udp往该地址发送消息外，还会通过`9090`这个端口信息猜测以及枚举出B那些备用udp对公网的可能的端口号P，并通过唯一udp往`20.0.0.1:P`这些地址发送信息以碰撞出穿透概率。
