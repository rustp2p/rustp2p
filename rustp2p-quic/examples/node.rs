use rustp2p_quic::{Endpoint, GroupCode, Identity, PeerAddr, PeerId};
use std::env;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> rustp2p_quic::Result<()> {
    env_logger::init();

    let args = Args::parse()?;
    let identity = Identity::generate()?;
    let endpoint = Endpoint::builder()
        .identity(identity)
        .group(args.group)
        .bind(args.bind)
        .bootstrap(args.peers.clone())
        .build()
        .await?;

    println!("peer_id={}", endpoint.peer_id());
    println!("group={:?}", endpoint.group_code());
    println!("addr={}", endpoint.local_addr().unwrap());
    println!("commands:");
    println!("  add <peer_id>@<addr>");
    println!("  send <peer_id> <message>");
    println!("  stream <peer_id> <message>");
    println!("  broadcast <message>");
    println!("  peers");
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
                Ok((mut send, mut recv)) => {
                    tokio::spawn(async move {
                        match recv.read_to_end(1024 * 1024).await {
                            Ok(data) => {
                                println!("[stream] {}", String::from_utf8_lossy(&data));
                                let _ = send.write_all(b"echo: ").await;
                                let _ = send.write_all(&data).await;
                                let _ = send.finish();
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

    for peer in args.peers {
        endpoint.add_peer(peer).await?;
    }

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

async fn handle_command(endpoint: &Endpoint, line: &str) -> rustp2p_quic::Result<()> {
    let mut parts = line.splitn(3, ' ');
    match parts.next().unwrap_or_default() {
        "add" => {
            let peer = parts
                .next()
                .ok_or_else(|| invalid("usage: add <peer_id>@<addr>"))
                .and_then(parse_peer_addr)?;
            endpoint.add_peer(peer.clone()).await?;
            println!("added {}", peer.peer_id);
        }
        "send" => {
            let peer = parts
                .next()
                .ok_or_else(|| invalid("usage: send <peer_id> <message>"))
                .and_then(parse_peer_id)?;
            let payload = parts
                .next()
                .ok_or_else(|| invalid("usage: send <peer_id> <message>"))?;
            endpoint.send_to(peer, payload.as_bytes()).await?;
            println!("sent datagram to {peer}");
        }
        "stream" => {
            let peer = parts
                .next()
                .ok_or_else(|| invalid("usage: stream <peer_id> <message>"))
                .and_then(parse_peer_id)?;
            let payload = parts
                .next()
                .ok_or_else(|| invalid("usage: stream <peer_id> <message>"))?;
            let (mut send, mut recv) = endpoint.open_bi(peer).await?;
            send.write_all(payload.as_bytes()).await?;
            send.finish()?;
            let response = recv.read_to_end(1024 * 1024).await?;
            println!("[stream response] {}", String::from_utf8_lossy(&response));
        }
        "broadcast" => {
            let payload = parts
                .next()
                .ok_or_else(|| invalid("usage: broadcast <message>"))?;
            endpoint.broadcast(payload.as_bytes()).await?;
            println!("broadcast sent");
        }
        "peers" => {
            for peer in endpoint.known_peers() {
                println!("{} {:?}", peer.peer_id, peer.addrs);
            }
        }
        _ => {
            eprintln!("unknown command");
        }
    }
    Ok(())
}

struct Args {
    bind: SocketAddr,
    group: GroupCode,
    peers: Vec<PeerAddr>,
}

impl Args {
    fn parse() -> io::Result<Self> {
        let mut bind = "127.0.0.1:0".parse().unwrap();
        let mut group = GroupCode::try_from("demo")?;
        let mut peers = Vec::new();
        let mut args = env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--bind" => {
                    bind = args
                        .next()
                        .ok_or_else(|| invalid("--bind requires an address"))?
                        .parse()
                        .map_err(|e| invalid(format!("invalid bind address: {e}")))?;
                }
                "--group" => {
                    group = GroupCode::try_from(
                        args.next()
                            .ok_or_else(|| invalid("--group requires a value"))?
                            .as_str(),
                    )?;
                }
                "--peer" => {
                    peers.push(parse_peer_addr(
                        &args
                            .next()
                            .ok_or_else(|| invalid("--peer requires <peer_id>@<addr>"))?,
                    )?);
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                _ => return Err(invalid(format!("unknown argument: {arg}"))),
            }
        }

        Ok(Self { bind, group, peers })
    }
}

fn parse_peer_addr(value: &str) -> io::Result<PeerAddr> {
    let (peer, addr) = value
        .split_once('@')
        .ok_or_else(|| invalid("peer must be <peer_id>@<addr>"))?;
    Ok(PeerAddr::new(
        parse_peer_id(peer)?,
        vec![addr
            .parse()
            .map_err(|e| invalid(format!("invalid peer address: {e}")))?],
    ))
}

fn parse_peer_id(value: &str) -> io::Result<PeerId> {
    let bytes = hex::decode(value).map_err(|e| invalid(format!("invalid peer id hex: {e}")))?;
    let id: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid("peer id must be 32 bytes / 64 hex chars"))?;
    Ok(PeerId(id))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn print_usage() {
    println!("cargo run -p rustp2p-quic --example node -- --bind 127.0.0.1:7001");
    println!(
        "cargo run -p rustp2p-quic --example node -- --bind 127.0.0.1:7002 --peer <peer_id>@127.0.0.1:7001"
    );
}
