//! `tcp` streams: the synth asks this computer to dial a host and pipes
//! bytes through. This is what gives the synth the user's home IP.
//!
//! Policy (TZ 4.1): private, loopback, link-local and cloud-metadata
//! ranges are refused unless the user turned on local network access
//! (`--allow-private`). The host name is resolved FIRST, every resolved
//! address is checked, and the dial then goes to the checked address (not
//! to the name again), so a DNS answer cannot change between check and
//! dial.
//!
//! Only host, port, byte counts and duration are logged. Content never is.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;

use crate::proto::{reply_ok, reply_refusal, MuxStream, Refusal, StreamHeader};
use crate::{logln, Config};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// Overall bound for resolve + dial. The synth waits 30 s for the reply.
const CONNECT_BUDGET: Duration = Duration::from_secs(20);
const COPY_BUF: usize = 64 * 1024;
/// After the synth closed its side, how long it may go without taking any
/// of the target's bytes before the stream is reset. A synth-side client
/// that went away mid-download leaves exactly this state behind.
const STALL_AFTER_SYNTH_CLOSE: Duration = Duration::from_secs(30);

pub async fn handle(mut s: MuxStream, hdr: StreamHeader, cfg: &Config) {
    let started = Instant::now();
    let target = format!("{}:{}", hdr.host, hdr.port);

    let tcp = match tokio::time::timeout(CONNECT_BUDGET, connect(&hdr, cfg.allow_private)).await {
        Ok(Ok(tcp)) => tcp,
        Ok(Err(why)) => {
            logln!("tcp {target}: refused: {why}");
            reply_refusal(&mut s, &why).await;
            return;
        }
        Err(_) => {
            let why = Refusal::Failed(format!(
                "dial failed: {target} did not connect within {} s",
                CONNECT_BUDGET.as_secs()
            ));
            logln!("tcp {target}: refused: {why}");
            reply_refusal(&mut s, &why).await;
            return;
        }
    };
    let _ = tcp.set_nodelay(true);
    if reply_ok(&mut s).await.is_err() {
        return;
    }

    let up = AtomicU64::new(0);
    let down = AtomicU64::new(0);
    let res = pump(s, tcp, &up, &down).await;
    let outcome = match res {
        Ok(()) => "closed".to_string(),
        Err(e) => format!("aborted: {e}"),
    };
    logln!(
        "tcp {target}: {outcome}, up={} down={} in {:.1}s",
        up.load(Ordering::Relaxed),
        down.load(Ordering::Relaxed),
        started.elapsed().as_secs_f64()
    );
}

/// Pipe bytes both ways until both directions finished.
///
/// Backpressure: a write to the stream waits for window from the synth, a
/// write to the socket waits for the target, and each direction reads its
/// next chunk only after the previous one was written. Memory per stream
/// is two copy buffers plus what the multiplexer window holds.
///
/// On an error either way the function returns early; dropping the stream
/// without closing it resets it, so the synth sees an error, not a clean
/// EOF.
async fn pump(s: MuxStream, tcp: TcpStream, up: &AtomicU64, down: &AtomicU64) -> io::Result<()> {
    let (mut sr, mut sw) = tokio::io::split(s);
    let (mut tr, mut tw) = tcp.into_split();
    let synth_closed = AtomicBool::new(false);
    let synth_closed_notify = Notify::new();

    let to_target = async {
        let mut buf = vec![0u8; COPY_BUF];
        loop {
            let n = sr.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            tw.write_all(&buf[..n]).await?;
            up.fetch_add(n as u64, Ordering::Relaxed);
        }
        // The synth finished sending: pass the half-close on.
        synth_closed.store(true, Ordering::Release);
        synth_closed_notify.notify_waiters();
        let _ = tw.shutdown().await;
        io::Result::Ok(())
    };

    let to_synth = async {
        let mut buf = vec![0u8; COPY_BUF];
        loop {
            let n = tr.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            tokio::select! {
                r = sw.write_all(&buf[..n]) => r?,
                _ = stalled_after_close(&synth_closed, &synth_closed_notify) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "the synth closed its side and took no data for {} s",
                            STALL_AFTER_SYNTH_CLOSE.as_secs()
                        ),
                    ));
                }
            }
            down.fetch_add(n as u64, Ordering::Relaxed);
        }
        // The target finished sending: half-close toward the synth.
        sw.shutdown().await?;
        io::Result::Ok(())
    };

    tokio::try_join!(to_target, to_synth).map(|_| ())
}

/// Resolves once the synth has closed its side AND a further
/// `STALL_AFTER_SYNTH_CLOSE` passed. Created per write, so the clock
/// restarts with every chunk the synth does accept.
async fn stalled_after_close(closed: &AtomicBool, notify: &Notify) {
    loop {
        let notified = notify.notified();
        if closed.load(Ordering::Acquire) {
            break;
        }
        notified.await;
    }
    tokio::time::sleep(STALL_AFTER_SYNTH_CLOSE).await;
}

/// Resolve, apply the address policy, dial.
async fn connect(hdr: &StreamHeader, allow_private: bool) -> Result<TcpStream, Refusal> {
    if hdr.host.is_empty() || hdr.host.len() > 253 {
        return Err(Refusal::Denied("tcp header has no valid host".into()));
    }
    if hdr.port == 0 || hdr.port > 65535 {
        return Err(Refusal::Denied(format!("port {} is not valid", hdr.port)));
    }
    let port = hdr.port as u16;
    let host = hdr.host.trim_start_matches('[').trim_end_matches(']');

    let addrs: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => {
            let lookup = tokio::net::lookup_host((host, port));
            match tokio::time::timeout(RESOLVE_TIMEOUT, lookup).await {
                Ok(Ok(it)) => it.collect(),
                Ok(Err(e)) => {
                    return Err(Refusal::Failed(format!("dns failed for {host}: {e}")));
                }
                Err(_) => {
                    return Err(Refusal::Failed(format!(
                        "dns failed for {host}: no answer in {} s",
                        RESOLVE_TIMEOUT.as_secs()
                    )));
                }
            }
        }
    };
    if addrs.is_empty() {
        return Err(Refusal::Failed(format!("dns failed for {host}: no addresses")));
    }

    if !allow_private {
        // One private address in the answer refuses the whole name: a
        // name that mixes public and private answers is how DNS rebinding
        // gets past a per-address filter.
        for a in &addrs {
            if let Some(class) = non_public(a.ip()) {
                return Err(Refusal::Denied(format!(
                    "{host} is a {class} address ({}); local network access is off",
                    a.ip()
                )));
            }
        }
    }

    let mut last = String::from("no address tried");
    for a in addrs {
        match tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect(a)).await {
            Ok(Ok(tcp)) => return Ok(tcp),
            Ok(Err(e)) => last = format!("{a}: {e}"),
            Err(_) => last = format!("{a}: no answer in {} s", DIAL_TIMEOUT.as_secs()),
        }
    }
    Err(Refusal::Failed(format!("dial failed: {last}")))
}

/// Returns the class of `ip` when it is not a public unicast address.
pub fn non_public(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => non_public_v4(v4),
        IpAddr::V6(v6) => non_public_v6(v6),
    }
}

fn non_public_v4(ip: Ipv4Addr) -> Option<&'static str> {
    let o = ip.octets();
    if ip.is_loopback() {
        return Some("loopback");
    }
    if ip.is_private() {
        return Some("private");
    }
    if o[0] == 169 && o[1] == 254 {
        // Includes the cloud metadata endpoint 169.254.169.254.
        return Some("link-local");
    }
    if o[0] == 0 {
        return Some("unspecified");
    }
    if o[0] == 100 && (o[1] & 0xc0) == 64 {
        return Some("carrier-grade NAT");
    }
    if ip.is_multicast() || ip.is_broadcast() {
        return Some("multicast or broadcast");
    }
    if o[0] >= 240 {
        return Some("reserved");
    }
    // 192.0.0.0/24 protocol assignments, 192.0.2.0/24, 198.51.100.0/24 and
    // 203.0.113.0/24 documentation, 198.18.0.0/15 benchmarking.
    if (o[0] == 192 && o[1] == 0 && (o[2] == 0 || o[2] == 2))
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
    {
        return Some("reserved");
    }
    None
}

fn non_public_v6(ip: Ipv6Addr) -> Option<&'static str> {
    let s = ip.segments();
    if ip.is_loopback() {
        return Some("loopback");
    }
    if ip.is_unspecified() {
        return Some("unspecified");
    }
    // Addresses that carry an IPv4 address inside are judged by it:
    // v4-mapped ::ffff:a.b.c.d, v4-compatible ::a.b.c.d, NAT64
    // 64:ff9b::/96, 6to4 2002::/16.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return non_public_v4(v4);
    }
    if s[..6] == [0, 0, 0, 0, 0, 0] {
        return non_public_v4(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ))
        .or(Some("reserved"));
    }
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        return non_public_v4(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
    }
    if s[0] == 0x2002 {
        return non_public_v4(Ipv4Addr::new(
            (s[1] >> 8) as u8,
            s[1] as u8,
            (s[2] >> 8) as u8,
            s[2] as u8,
        ));
    }
    if (s[0] & 0xfe00) == 0xfc00 {
        // Unique local, includes the AWS IPv6 metadata address fd00:ec2::254.
        return Some("private");
    }
    if (s[0] & 0xffc0) == 0xfe80 {
        return Some("link-local");
    }
    if (s[0] & 0xffc0) == 0xfec0 {
        return Some("site-local");
    }
    if ip.is_multicast() {
        return Some("multicast");
    }
    if s[0] == 0x2001 && s[1] == 0x0db8 {
        return Some("reserved");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_policy() {
        let denied = [
            "127.0.0.1",
            "127.255.255.254",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "240.0.0.1",
            "198.18.0.1",
            "192.0.2.1",
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "64:ff9b::a00:1",
            "2002:7f00:1::1",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
        ];
        for a in denied {
            let ip: IpAddr = a.parse().unwrap();
            assert!(non_public(ip).is_some(), "{a} must be refused");
        }
        let allowed = [
            "1.1.1.1",
            "8.8.8.8",
            "172.32.0.1",
            "172.15.0.1",
            "100.128.0.1",
            "192.0.1.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
        ];
        for a in allowed {
            let ip: IpAddr = a.parse().unwrap();
            assert!(non_public(ip).is_none(), "{a} must be allowed");
        }
    }
}
