use rustp2p_quic::{Endpoint, Identity, PeerId, ReliableRecvStream, ReliableSendStream};
use std::env;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const DEFAULT_STUN_SERVERS: &[&str] = &[
    "stun.miwifi.com:3478",
    "stun.chat.bilibili.com:3478",
    "stun.hitv.com:3478",
];

#[tokio::main]
async fn main() -> rustp2p_quic::Result<()> {
    env_logger::init();

    let args = Args::parse()?;
    let endpoint = Endpoint::builder()
        .identity(Identity::new(args.id, args.seed)?)
        .bind(args.bind)
        .bootstrap(args.bootstrap.clone())
        .stun_servers(args.stun_servers.clone())
        .build()
        .await?;

    println!("peer_id={}", endpoint.peer_id());
    println!("addr={}", endpoint.local_addr().unwrap());
    println!("stun_servers={:?}", args.stun_servers);
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
            write_frame(&mut send, payload.as_bytes()).await?;
            send.finish()?;
            let response = read_frame(&mut recv, 1024 * 1024).await?;
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
    stun_servers: Vec<String>,
}

impl Args {
    fn parse() -> io::Result<Self> {
        let mut id = None;
        let mut seed = None;
        let mut bind = "127.0.0.1:0".parse().unwrap();
        let mut bootstrap = Vec::new();
        let mut stun_servers = DEFAULT_STUN_SERVERS
            .iter()
            .map(|server| (*server).to_string())
            .collect::<Vec<_>>();
        let mut stun_overridden = false;
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
                "--stun" => {
                    if !stun_overridden {
                        stun_servers.clear();
                        stun_overridden = true;
                    }
                    stun_servers.push(
                        args.next()
                            .ok_or_else(|| invalid("--stun requires a server address"))?,
                    );
                }
                "--no-stun" => {
                    stun_servers.clear();
                    stun_overridden = true;
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
            stun_servers,
        })
    }
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

fn print_usage() {
    println!("cargo run -p rustp2p-quic --example node -- --id node-a --seed seed-a --bind 127.0.0.1:7001");
    println!("cargo run -p rustp2p-quic --example node -- --id node-b --seed seed-b --bind 127.0.0.1:7002 --bootstrap 127.0.0.1:7001");
    println!("default example STUN servers:");
    for server in DEFAULT_STUN_SERVERS {
        println!("  {server}");
    }
    println!(
        "use --stun <server> to replace/append explicit servers, or --no-stun to disable STUN"
    );
}
