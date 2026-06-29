use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tokio::net::UdpSocket;

pub async fn local_ipv4() -> io::Result<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect("8.8.8.8:80").await?;
    let addr = socket.local_addr()?;
    match addr.ip() {
        IpAddr::V4(ip) => Ok(ip),
        IpAddr::V6(_) => Ok(Ipv4Addr::UNSPECIFIED),
    }
}
pub async fn local_ipv6() -> io::Result<Ipv6Addr> {
    let socket = UdpSocket::bind("[::]:0").await?;
    socket
        .connect("[2001:4860:4860:0000:0000:0000:0000:8888]:80")
        .await?;
    let addr = socket.local_addr()?;
    match addr.ip() {
        IpAddr::V4(_) => Ok(Ipv6Addr::UNSPECIFIED),
        IpAddr::V6(ip) => Ok(ip),
    }
}

pub const fn is_ipv4_global(ipv4: &Ipv4Addr) -> bool {
    !(ipv4.octets()[0] == 0 // "This network"
        || ipv4.is_private()
        || ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0b1100_0000 == 0b0100_0000)//ipv4.is_shared()
        || ipv4.is_loopback()
        || ipv4.is_link_local()
        // addresses reserved for future protocols (`192.0.0.0/24`)
        // .9 and .10 are documented as globally reachable so they're excluded
        || (
        ipv4.octets()[0] == 192 && ipv4.octets()[1] == 0 && ipv4.octets()[2] == 0
            && ipv4.octets()[3] != 9 && ipv4.octets()[3] != 10
    )
        || ipv4.is_documentation()
        || ipv4.octets()[0] == 198 && (ipv4.octets()[1] & 0xfe) == 18//ipv4.is_benchmarking()
        || ipv4.octets()[0] & 240 == 240 && !ipv4.is_broadcast()//ipv4.is_reserved()
        || ipv4.is_broadcast())
}

pub const fn is_ipv6_global(ipv6addr: &Ipv6Addr) -> bool {
    !(ipv6addr.is_unspecified()
        || ipv6addr.is_loopback()
        // IPv4-mapped Address (`::ffff:0:0/96`)
        || matches!(ipv6addr.segments(), [0, 0, 0, 0, 0, 0xffff, _, _])
        // IPv4-IPv6 Translat. (`64:ff9b:1::/48`)
        || matches!(ipv6addr.segments(), [0x64, 0xff9b, 1, _, _, _, _, _])
        // Discard-Only Address Block (`100::/64`)
        || matches!(ipv6addr.segments(), [0x100, 0, 0, 0, _, _, _, _])
        // IETF Protocol Assignments (`2001::/23`)
        || (matches!(ipv6addr.segments(), [0x2001, b, _, _, _, _, _, _] if b < 0x200)
        && !(
        // Port Control Protocol Anycast (`2001:1::1`)
        u128::from_be_bytes(ipv6addr.octets()) == 0x2001_0001_0000_0000_0000_0000_0000_0001
            // Traversal Using Relays around NAT Anycast (`2001:1::2`)
            || u128::from_be_bytes(ipv6addr.octets()) == 0x2001_0001_0000_0000_0000_0000_0000_0002
            // AMT (`2001:3::/32`)
            || matches!(ipv6addr.segments(), [0x2001, 3, _, _, _, _, _, _])
            // AS112-v6 (`2001:4:112::/48`)
            || matches!(ipv6addr.segments(), [0x2001, 4, 0x112, _, _, _, _, _])
            // ORCHIDv2 (`2001:20::/28`)
            || matches!(ipv6addr.segments(), [0x2001, b, _, _, _, _, _, _] if b >= 0x20 && b <= 0x2F)
    ))
        || (ipv6addr.segments()[0] == 0x2001) && (ipv6addr.segments()[1] == 0xdb8)//ipv6addr.is_documentation()
        || (ipv6addr.segments()[0] & 0xfe00) == 0xfc00//ipv6addr.is_unique_local()
        || (ipv6addr.segments()[0] & 0xffc0) == 0xfe80) //ipv6addr.is_unicast_link_local())
}
