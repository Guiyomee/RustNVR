//! RTSP port scan of a local network: finds cameras that do not answer
//! WS-Discovery (no ONVIF, or multicast not reaching RustNVR as under WSL).
//! Each open port gets an RTSP `OPTIONS` to confirm it is RTSP; its
//! `Server` header often names the maker.

use futures::StreamExt as _;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// Usual RTSP ports.
pub const RTSP_PORTS: [u16; 2] = [554, 8554];
/// Smallest network prefix scanned (/22: 1022 hosts).
pub const MIN_PREFIX: u8 = 22;

/// An IPv4 network (`192.168.1.0/24`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    /// Network address.
    pub address: Ipv4Addr,
    /// Prefix length.
    pub prefix: u8,
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix)
    }
}

impl Network {
    /// `192.168.1.0/24` (any address of the network is accepted); prefix
    /// between [`MIN_PREFIX`] and 30.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (ip, prefix) = s.trim().split_once('/').unwrap_or((s.trim(), "24"));
        let ip: Ipv4Addr = ip
            .parse()
            .map_err(|_| format!("adresse IPv4 attendue, pas {ip:?}"))?;
        let prefix: u8 = prefix
            .parse()
            .map_err(|_| format!("préfixe invalide : {prefix:?}"))?;
        if !(MIN_PREFIX..=30).contains(&prefix) {
            return Err(format!(
                "réseau de /{MIN_PREFIX} à /30 seulement (pas /{prefix}) : /24 pour un réseau domestique"
            ));
        }
        Ok(Self::new(ip, prefix))
    }

    fn new(ip: Ipv4Addr, prefix: u8) -> Self {
        let mask = u32::MAX << (32 - u32::from(prefix));
        Self {
            address: Ipv4Addr::from(u32::from(ip) & mask),
            prefix,
        }
    }

    /// Host addresses (without the network and broadcast addresses).
    pub fn hosts(&self) -> Vec<Ipv4Addr> {
        let base = u32::from(self.address);
        let size = 1u32 << (32 - u32::from(self.prefix));
        (1..size - 1).map(|i| Ipv4Addr::from(base + i)).collect()
    }

    /// The /24 of this machine's address on the default route, when it is a
    /// private address (no packet is sent).
    pub fn local() -> Option<Self> {
        let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        // TEST-NET-1: only selects the route.
        socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
        match socket.local_addr().ok()?.ip() {
            std::net::IpAddr::V4(ip) if ip.is_private() => Some(Self::new(ip, 24)),
            _ => None,
        }
    }
}

/// An RTSP server found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtspServer {
    /// Its address.
    pub address: Ipv4Addr,
    /// RTSP port.
    pub port: u16,
    /// `Server` header of its `OPTIONS` answer.
    pub server: Option<String>,
}

/// Whether `addr` speaks RTSP: its `Server` header (`Some(None)` without one).
async fn rtsp_options(addr: SocketAddr, timeout: Duration) -> Option<Option<String>> {
    let mut stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
        .await
        .ok()?
        .ok()?;
    let request =
        format!("OPTIONS rtsp://{addr}/ RTSP/1.0\r\nCSeq: 1\r\nUser-Agent: RustNVR\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut buf = vec![0u8; 2048];
    let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
        .await
        .ok()?
        .ok()?;
    let answer = String::from_utf8_lossy(&buf[..n]);
    if !answer.starts_with("RTSP/1.") {
        return None;
    }
    Some(answer.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim().eq_ignore_ascii_case("server") && !v.trim().is_empty())
            .then(|| v.trim().chars().take(80).collect())
    }))
}

/// Whether a `Server` header is the one of RustNVR's RTSP republication
/// (`RustNVR/<version>`).
fn is_rustnvr(server: &str) -> bool {
    server
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("rustnvr/"))
}

/// RTSP servers on `hosts` × `ports`, `concurrency` connections at a time.
pub async fn scan(
    hosts: &[Ipv4Addr],
    ports: &[u16],
    timeout: Duration,
    concurrency: usize,
) -> Vec<RtspServer> {
    let targets: Vec<(Ipv4Addr, u16)> = hosts
        .iter()
        .flat_map(|h| ports.iter().map(move |p| (*h, *p)))
        .collect();
    let mut found: Vec<RtspServer> = futures::stream::iter(targets)
        .map(|(address, port)| async move {
            rtsp_options(SocketAddr::from((address, port)), timeout)
                .await
                .map(|server| RtspServer {
                    address,
                    port,
                    server,
                })
        })
        .buffer_unordered(concurrency.max(1))
        .filter_map(|r| async move { r })
        // The RTSP republication of RustNVR (this one or another) serves
        // cameras already known to it: not a camera to add.
        .filter(|s| {
            let own = s.server.as_deref().is_some_and(is_rustnvr);
            async move { !own }
        })
        .collect()
        .await;
    found.sort_by_key(|s| (s.address, s.port));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_networks() {
        let n = Network::parse("192.168.1.57/24").expect("net");
        assert_eq!(n.to_string(), "192.168.1.0/24");
        assert_eq!(n.hosts().len(), 254);
        assert_eq!(n.hosts()[0], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(Network::parse("10.0.0.0").expect("net").prefix, 24);
        assert_eq!(
            Network::parse("10.0.0.0/22").expect("net").hosts().len(),
            1022
        );
        assert!(Network::parse("10.0.0.0/16").is_err());
        assert!(Network::parse("pas une adresse").is_err());
    }

    #[tokio::test]
    async fn finds_rtsp_servers_only() {
        use tokio::net::TcpListener;
        // An RTSP server, and something else on another port.
        let rtsp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let other = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let (rtsp_port, other_port) = (
            rtsp.local_addr().expect("addr").port(),
            other.local_addr().expect("addr").port(),
        );
        tokio::spawn(async move {
            while let Ok((mut s, _)) = rtsp.accept().await {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nServer: Hikvision-Webs\r\nPublic: OPTIONS, DESCRIBE\r\n\r\n")
                    .await;
            }
        });
        // RustNVR's own republication: answers RTSP, but is not a camera.
        let own = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let own_port = own.local_addr().expect("addr").port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = own.accept().await {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nServer: RustNVR/2026.11.5\r\n\r\n")
                    .await;
            }
        });
        tokio::spawn(async move {
            while let Ok((mut s, _)) = other.accept().await {
                let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
            }
        });
        let found = scan(
            &[Ipv4Addr::LOCALHOST],
            &[rtsp_port, other_port, own_port, 9],
            Duration::from_millis(500),
            8,
        )
        .await;
        assert_eq!(
            found,
            [RtspServer {
                address: Ipv4Addr::LOCALHOST,
                port: rtsp_port,
                server: Some("Hikvision-Webs".into())
            }]
        );
    }
}
