//! A small SOCKS5 load generator.
//!
//! Opens `--conns` concurrent SOCKS5 sessions through a RustProxy gateway,
//! each handshaking and `CONNECT`ing to a shared target, then streaming a
//! fixed-size payload for `--duration` seconds. Prints aggregate throughput
//! (MiB/s) and the success ratio.
//!
//! Usage:
//!   cargo run -p rustproxy --example loadgen -- \
//!       --gateway 127.0.0.1:1080 --target 127.0.0.1:9001 \
//!       --conns 64 --duration 10

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use rustproxy_protocol::host::{Host, SocksAddr};
use rustproxy_protocol::request::{Command, Request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const PROTOCOL_VERSION: u8 = 0x05;
const METHOD_NO_AUTH: u8 = 0x00;
const REPLY_SUCCESS: u8 = 0x00;
const CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone)]
struct Args {
    gateway: SocketAddr,
    target: SocketAddr,
    conns: usize,
    duration_secs: u64,
}

fn parse_args() -> Args {
    let mut gateway = None;
    let mut target = None;
    let mut conns = None;
    let mut duration_secs = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--gateway" => gateway = it.next().map(|s| s.parse().unwrap()),
            "--target" => target = it.next().map(|s| s.parse().unwrap()),
            "--conns" => conns = it.next().map(|s| s.parse().unwrap()),
            "--duration" => duration_secs = it.next().map(|s| s.parse().unwrap()),
            other => panic!("unknown arg: {other}"),
        }
    }
    Args {
        gateway: gateway.expect("--gateway required"),
        target: target.expect("--target required"),
        conns: conns.unwrap_or(32),
        duration_secs: duration_secs.unwrap_or(5),
    }
}

async fn connect_through_proxy(
    gateway: SocketAddr,
    target: SocketAddr,
) -> std::io::Result<TcpStream> {
    let mut s = TcpStream::connect(gateway).await?;

    // Greeting: offer NO_AUTH only.
    s.write_all(&[PROTOCOL_VERSION, 1, METHOD_NO_AUTH]).await?;
    let mut sel = [0u8; 2];
    s.read_exact(&mut sel).await?;
    assert_eq!(sel[0], PROTOCOL_VERSION);
    assert_eq!(sel[1], METHOD_NO_AUTH, "gateway did not accept NO_AUTH");

    // CONNECT: IPv4 address + port.
    let host = Host::Ip(target.ip());
    let addr = SocksAddr {
        host,
        port: target.port(),
    };
    let req = Request {
        command: Command::Connect,
        addr,
    };
    s.write_all(&req.to_bytes()).await?;

    // Read the reply; require success.
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await?;
    if reply[1] != REPLY_SUCCESS {
        return Err(std::io::Error::other(format!(
            "proxy CONNECT failed (reply {:#04x})",
            reply[1]
        )));
    }
    Ok(s)
}

async fn pump(
    s: &mut TcpStream,
    payload: &[u8],
    duration: std::time::Duration,
) -> std::io::Result<u64> {
    let start = Instant::now();
    let mut bytes: u64 = 0;
    while start.elapsed() < duration {
        s.write_all(payload).await?;
        bytes += payload.len() as u64;
    }
    Ok(bytes)
}

#[tokio::main]
async fn main() {
    let args = parse_args();

    // Shared counters across the concurrent sessions.
    let ok_count = Arc::new(AtomicU64::new(0));
    let fail_count = Arc::new(AtomicU64::new(0));
    let total_bytes = Arc::new(AtomicU64::new(0));

    let payload = vec![0xABu8; CHUNK_SIZE];
    let duration = std::time::Duration::from_secs(args.duration_secs);

    let start = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..args.conns {
        let gateway = args.gateway;
        let target = args.target;
        let payload = payload.clone();
        let ok_count = ok_count.clone();
        let fail_count = fail_count.clone();
        let total_bytes = total_bytes.clone();
        handles.push(tokio::spawn(async move {
            match connect_through_proxy(gateway, target).await {
                Ok(mut s) => match pump(&mut s, &payload, duration).await {
                    Ok(b) => {
                        total_bytes.fetch_add(b, Ordering::Relaxed);
                        ok_count.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        fail_count.fetch_add(1, Ordering::Relaxed);
                    }
                },
                Err(_) => {
                    fail_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    let elapsed = start.elapsed().as_secs_f64();

    let ok = ok_count.load(Ordering::Relaxed);
    let fail = fail_count.load(Ordering::Relaxed);
    let bytes = total_bytes.load(Ordering::Relaxed);
    let mi_b = bytes as f64 / (1024.0 * 1024.0);
    let success = if ok + fail > 0 {
        100.0 * ok as f64 / (ok + fail) as f64
    } else {
        0.0
    };

    println!(
        "connections={} ok={} failed={} elapsed={:.2}s bytes={} throughput={:.2} MiB/s success={:.1}%",
        args.conns, ok, fail, elapsed, bytes, mi_b / elapsed, success
    );
}
