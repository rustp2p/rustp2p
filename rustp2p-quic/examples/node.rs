use clap::Parser;
use rustp2p_quic::{Endpoint, Identity, PeerId, ReliableRecvStream, ReliableSendStream};
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const DEFAULT_STUN_SERVERS: &[&str] = &[
    "stun.miwifi.com:3478",
    "stun.chat.bilibili.com:3478",
    "stun.hitv.com:3478",
];

/// A simple P2P node example for rustp2p-quic.
#[derive(Parser)]
#[command(name = "node", about = "A simple P2P node example for rustp2p-quic")]
struct Args {
    /// Local node identity.
    #[arg(long)]
    id: String,

    /// Seed for identity derivation (defaults to "{id}-seed").
    #[arg(long)]
    seed: Option<String>,

    /// Local bind address.
    #[arg(long, default_value = "127.0.0.1:0")]
    bind: SocketAddr,

    /// Bootstrap peer address (can be specified multiple times).
    #[arg(long)]
    bootstrap: Vec<SocketAddr>,

    /// STUN server address (can be specified multiple times; replaces built-in defaults).
    #[arg(long)]
    stun: Vec<String>,

    /// Disable STUN entirely.
    #[arg(long)]
    no_stun: bool,

    /// Number of assistant UDP sockets for symmetric NAT punching.
    #[arg(long, default_value_t = 4)]
    max_assistant_sockets: usize,
}

#[tokio::main]
async fn main() -> rustp2p_quic::Result<()> {
    env_logger::init();

    let args = Args::parse();

    let stun_servers = if args.no_stun {
        Vec::new()
    } else if args.stun.is_empty() {
        DEFAULT_STUN_SERVERS
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    } else {
        args.stun.clone()
    };

    let seed = args
        .seed
        .clone()
        .unwrap_or_else(|| format!("{}-seed", args.id));

    let endpoint = Endpoint::builder()
        .identity(Identity::new(args.id.clone(), seed)?)
        .bind(args.bind)
        .bootstrap(args.bootstrap.clone())
        .stun_servers(stun_servers.clone())
        .max_assistant_sockets(args.max_assistant_sockets)
        .build()
        .await?;

    println!("peer_id={}", endpoint.peer_id());
    println!("addr={}", endpoint.local_addr().unwrap());
    println!("stun_servers={:?}", stun_servers);
    println!("commands:");
    println!("  connect <addr>");
    println!("  send <peer_id> <message>");
    println!("  stream <peer_id> <message>");
    println!("  broadcast <message>");
    println!("  punch <peer_id>");
    println!("  peers");
    println!("  routes");
    println!("  quit");

    let recv_endpoint = endpoint.clone();
    tokio::spawn(async move {
        loop {
            match recv_endpoint.recv().await {
                Ok(msg) => {
                    println!(
                        "[datagram] from={} relay={} {}",
                        msg.src,
                        msg.is_relay,
                        String::from_utf8_lossy(&msg.payload)
                    );
                }
                Err(e) => {
                    eprintln!("recv datagram failed: {e}");
                    break;
                }
            }
        }
    });

    let stream_endpoint = endpoint.clone();
    tokio::spawn(async move {
        loop {
            match stream_endpoint.accept_bi().await {
                Ok(mut stream) => {
                    let endpoint = stream_endpoint.clone();
                    tokio::spawn(async move {
                        match read_frame(&mut stream.recv, 1024 * 1024).await {
                            Ok(data) => {
                                let link = endpoint
                                    .link_mode(stream.peer_id.clone())
                                    .map(|mode| format!("{mode:?}"))
                                    .unwrap_or_else(|| "Unknown".to_string());
                                println!(
                                    "[stream] from={} link={} {}",
                                    stream.peer_id,
                                    link,
                                    String::from_utf8_lossy(&data)
                                );
                                let mut response = b"echo: ".to_vec();
                                response.extend_from_slice(&data);
                                let _ = write_frame(&mut stream.send, &response).await;
                                let _ = stream.send.finish();
                            }
                            Err(e) => eprintln!("read stream failed: {e}"),
                        }
                    });
                }
                Err(e) => {
                    eprintln!("accept stream failed: {e}");
                    break;
                }
            }
        }
    });

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "quit" || line == "exit" {
            break;
        }
        if let Err(e) = handle_command(&endpoint, line).await {
            eprintln!("{e}");
        }
    }

    endpoint.close().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

/// Parse the rest of a command line as `<peer_id> <message>`.
///
/// The message preserves all spaces — only the first space separates the
/// peer id from the payload.
fn split_peer_and_payload(rest: &str) -> rustp2p_quic::Result<(&str, &str)> {
    let rest = rest.trim_start();
    match rest.split_once(' ') {
        Some((peer, tail)) if !peer.is_empty() => {
            let payload = tail.trim_start();
            if payload.is_empty() {
                return Err(invalid("usage: <peer_id> <message>"));
            }
            Ok((peer, payload))
        }
        _ => Err(invalid("usage: <peer_id> <message>")),
    }
}

async fn handle_command(endpoint: &Endpoint, line: &str) -> rustp2p_quic::Result<()> {
    // Split into command + rest.  The rest preserves all remaining spaces so
    // that multi-word messages (e.g. `broadcast hello world foo`) are not
    // truncated.
    let (cmd, rest) = match line.split_once(' ') {
        Some((cmd, rest)) => (cmd, rest),
        None => (line, ""),
    };

    match cmd {
        "connect" => {
            let addr: SocketAddr = rest
                .trim()
                .parse()
                .map_err(|e| invalid(format!("invalid address: {e}")))?;
            let peer_id = endpoint.add_bootstrap(addr).await?;
            println!("connected {peer_id} at {addr}");
        }
        "send" => {
            let (peer_str, payload) = split_peer_and_payload(rest)?;
            let peer = PeerId::from(peer_str);
            endpoint.send_to(peer.clone(), payload.as_bytes()).await?;
            println!("sent datagram to {peer}");
        }
        "stream" => {
            let (peer_str, payload) = split_peer_and_payload(rest)?;
            let peer = PeerId::from(peer_str);
            let (mut send, mut recv) = endpoint.open_bi(peer.clone()).await?;
            write_frame(&mut send, payload.as_bytes()).await?;
            send.finish()?;
            let response = read_frame(&mut recv, 1024 * 1024).await?;
            println!(
                "[stream response from {peer}] {}",
                String::from_utf8_lossy(&response)
            );
        }
        "broadcast" => {
            if rest.is_empty() {
                return Err(invalid("usage: broadcast <message>"));
            }
            endpoint.broadcast(rest.as_bytes()).await?;
            println!("broadcast sent");
        }
        "punch" => {
            let peer_str = rest.trim();
            if peer_str.is_empty() {
                return Err(invalid("usage: punch <peer_id>"));
            }
            let peer = PeerId::from(peer_str);
            match endpoint.punch(peer.clone()).await {
                Ok(()) => println!("punch sent to {peer}"),
                Err(e) => eprintln!("punch failed: {e}"),
            }
        }
        "peers" => {
            for peer in endpoint.known_peers() {
                println!(
                    "{} direct={} relay={:?} addrs={:?}",
                    peer.peer_id, peer.is_direct, peer.relay_hint, peer.addrs
                );
            }
        }
        "routes" => {
            let peers = endpoint.known_peers();
            if peers.is_empty() {
                println!("no known peers");
            }
            for peer in &peers {
                let routes = endpoint.routes(peer.peer_id.clone());
                println!("── {} ──", peer.peer_id);
                if routes.is_empty() {
                    println!("  (no routes)");
                    continue;
                }
                for route in &routes {
                    let kind = if route.is_direct() { "direct" } else { "relay" };
                    let rtt = route
                        .rtt()
                        .map(|v| format!("{}ms", v))
                        .unwrap_or_else(|| "-".to_string());
                    println!(
                        "  [{:>5}] {} metric={} rtt={}",
                        kind,
                        route.route_key(),
                        route.metric(),
                        rtt
                    );
                }
            }
        }
        _ => {
            eprintln!("unknown command");
        }
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

async fn write_frame(send: &mut ReliableSendStream, payload: &[u8]) -> io::Result<()> {
    if payload.len() > u32::MAX as usize {
        return Err(invalid("frame too large"));
    }
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    send.write_all(payload).await
}

async fn read_frame(recv: &mut ReliableRecvStream, max_size: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    read_exact(recv, &mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max_size {
        return Err(invalid("frame exceeds max size"));
    }
    let mut payload = vec![0u8; len];
    read_exact(recv, &mut payload).await?;
    Ok(payload)
}

async fn read_exact(recv: &mut ReliableRecvStream, mut out: &mut [u8]) -> io::Result<()> {
    while !out.is_empty() {
        match recv.read(out).await? {
            Some(0) | None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "stream closed",
                ))
            }

            Some(n) => {
                let tmp = out;
                out = &mut tmp[n..];
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::split_peer_and_payload;

    #[test]
    fn split_peer_and_payload_accepts_normal_spacing() {
        let (peer, payload) = split_peer_and_payload("node-b hello").unwrap();
        assert_eq!(peer, "node-b");
        assert_eq!(payload, "hello");
    }

    #[test]
    fn split_peer_and_payload_accepts_extra_spaces_after_command() {
        let (peer, payload) = split_peer_and_payload("   node-b    hello world").unwrap();
        assert_eq!(peer, "node-b");
        assert_eq!(payload, "hello world");
    }

    #[test]
    fn split_peer_and_payload_rejects_missing_payload() {
        assert!(split_peer_and_payload("node-b").is_err());
        assert!(split_peer_and_payload("node-b   ").is_err());
    }
}
