use rustp2p_quic::{Endpoint, Identity, PeerId};
use std::env;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> rustp2p_quic::Result<()> {
    env_logger::init();

    let args = Args::parse()?;
    let endpoint = Endpoint::builder()
        .identity(Identity::new(args.id, args.seed)?)
        .bind(args.bind)
        .bootstrap(args.bootstrap.clone())
        .build()
        .await?;

    println!("peer_id={}", endpoint.peer_id());
    println!("addr={}", endpoint.local_addr().unwrap());
    println!("commands:");
    println!("  connect <addr>");
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
                Ok(mut stream) => {
                    tokio::spawn(async move {
                        match stream.recv.read_to_end(1024 * 1024).await {
                            Ok(data) => {
                                println!(
                                    "[stream] from={} relay={} {}",
                                    stream.peer_id,
                                    stream.is_relay,
                                    String::from_utf8_lossy(&data)
                                );
                                let _ = stream.send.write_all(b"echo: ").await;
                                let _ = stream.send.write_all(&data).await;
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

async fn handle_command(endpoint: &Endpoint, line: &str) -> rustp2p_quic::Result<()> {
    let mut parts = line.splitn(3, ' ');
    match parts.next().unwrap_or_default() {
        "connect" => {
            let addr: SocketAddr = parts
                .next()
                .ok_or_else(|| invalid("usage: connect <addr>"))?
                .parse()
                .map_err(|e| invalid(format!("invalid address: {e}")))?;
            let peer_id = endpoint.add_bootstrap(addr).await?;
            println!("connected {peer_id} at {addr}");
        }
        "send" => {
            let peer = parts
                .next()
                .ok_or_else(|| invalid("usage: send <peer_id> <message>"))
                .map(PeerId::from)?;
            let payload = parts
                .next()
                .ok_or_else(|| invalid("usage: send <peer_id> <message>"))?;
            endpoint.send_to(peer.clone(), payload.as_bytes()).await?;
            println!("sent datagram to {peer}");
        }
        "stream" => {
            let peer = parts
                .next()
                .ok_or_else(|| invalid("usage: stream <peer_id> <message>"))
                .map(PeerId::from)?;
            let payload = parts
                .next()
                .ok_or_else(|| invalid("usage: stream <peer_id> <message>"))?;
            let (mut send, mut recv) = endpoint.open_bi(peer.clone()).await?;
            send.write_all(payload.as_bytes()).await?;
            send.finish()?;
            let response = recv.read_to_end(1024 * 1024).await?;
            println!(
                "[stream response from {peer}] {}",
                String::from_utf8_lossy(&response)
            );
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
                println!(
                    "{} direct={} relay={:?} addrs={:?}",
                    peer.peer_id, peer.is_direct, peer.relay_hint, peer.addrs
                );
            }
        }
        _ => {
            eprintln!("unknown command");
        }
    }
    Ok(())
}

struct Args {
    id: String,
    seed: String,
    bind: SocketAddr,
    bootstrap: Vec<SocketAddr>,
}

impl Args {
    fn parse() -> io::Result<Self> {
        let mut id = None;
        let mut seed = None;
        let mut bind = "127.0.0.1:0".parse().unwrap();
        let mut bootstrap = Vec::new();
        let mut args = env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--id" => {
                    id = Some(
                        args.next()
                            .ok_or_else(|| invalid("--id requires a value"))?,
                    )
                }
                "--seed" => {
                    seed = Some(
                        args.next()
                            .ok_or_else(|| invalid("--seed requires a value"))?,
                    )
                }
                "--bind" => {
                    bind = args
                        .next()
                        .ok_or_else(|| invalid("--bind requires an address"))?
                        .parse()
                        .map_err(|e| invalid(format!("invalid bind address: {e}")))?;
                }
                "--bootstrap" => {
                    bootstrap.push(
                        args.next()
                            .ok_or_else(|| invalid("--bootstrap requires an address"))?
                            .parse()
                            .map_err(|e| invalid(format!("invalid bootstrap address: {e}")))?,
                    );
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                _ => return Err(invalid(format!("unknown argument: {arg}"))),
            }
        }

        let id = id.ok_or_else(|| invalid("--id is required"))?;
        let seed = seed.unwrap_or_else(|| format!("{id}-seed"));
        Ok(Self {
            id,
            seed,
            bind,
            bootstrap,
        })
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn print_usage() {
    println!("cargo run -p rustp2p-quic --example node -- --id node-a --seed seed-a --bind 127.0.0.1:7001");
    println!("cargo run -p rustp2p-quic --example node -- --id node-b --seed seed-b --bind 127.0.0.1:7002 --bootstrap 127.0.0.1:7001");
}
