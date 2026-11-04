//! ONVIF WS-Discovery: a `Probe` sent to the multicast group, each camera
//! answers with a `ProbeMatch` giving its ONVIF address (`XAddrs`) and
//! `Scopes` (name, hardware). Needs the local network: under Docker, the host
//! network; under WSL (NAT), nothing answers.

use crate::xml;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::Instant;

/// The WS-Discovery multicast group.
pub const MULTICAST: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(239, 255, 255, 250)), 3702);

/// A device that answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnvifDevice {
    /// Address the answer came from.
    pub address: IpAddr,
    /// ONVIF device service (`http://…/onvif/device_service`).
    pub xaddr: Option<String>,
    /// `onvif://www.onvif.org/name/…` scope.
    pub name: Option<String>,
    /// `onvif://www.onvif.org/hardware/…` scope.
    pub hardware: Option<String>,
}

/// A random-looking message id (unique enough: answers are matched by it).
fn message_id() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        b = n.to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// SOAP 1.2 `Probe` for `types` (`dn:NetworkVideoTransmitter`, `tds:Device`).
fn probe_message(types: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><e:Envelope xmlns:e="http://www.w3.org/2003/05/soap-envelope" xmlns:w="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl" xmlns:tds="http://www.onvif.org/ver10/device/wsdl"><e:Header><w:MessageID>uuid:{}</w:MessageID><w:To e:mustUnderstand="true">urn:schemas-xmlsoap-org:ws:2005:04:discovery</w:To><w:Action e:mustUnderstand="true">http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</w:Action></e:Header><e:Body><d:Probe><d:Types>{types}</d:Types></d:Probe></e:Body></e:Envelope>"#,
        message_id()
    )
}

/// `%20`… decoded.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads a `ProbeMatch` sent by `from`.
fn parse_match(text: &str, from: IpAddr) -> Option<OnvifDevice> {
    let root = xml::parse(text)?;
    let m = root.find("ProbeMatch")?;
    let xaddrs: Vec<&str> = m
        .find("XAddrs")
        .map(|x| x.text.split_whitespace().collect())
        .unwrap_or_default();
    // The address on the sender's network, else the first one.
    let host = from.to_string();
    let xaddr = xaddrs
        .iter()
        .find(|a| a.contains(&format!("//{host}")))
        .or(xaddrs.first())
        .map(|a| (*a).to_owned());
    let scopes = m.find("Scopes").map(|s| s.text.clone()).unwrap_or_default();
    let scope = |kind: &str| {
        scopes
            .split_whitespace()
            .find_map(|s| s.strip_prefix(&format!("onvif://www.onvif.org/{kind}/")))
            .map(percent_decode)
            .filter(|v| !v.is_empty())
    };
    Some(OnvifDevice {
        address: from,
        xaddr,
        name: scope("name"),
        hardware: scope("hardware"),
    })
}

/// Sends a `Probe` to `target` (normally [`MULTICAST`]) and collects the
/// answers for `wait`, one per address.
pub async fn discover(target: SocketAddr, wait: Duration) -> std::io::Result<Vec<OnvifDevice>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    let _ = socket.set_multicast_ttl_v4(2);
    // Twice each (UDP may be lost), for both kinds of devices.
    for _ in 0..2 {
        for types in ["dn:NetworkVideoTransmitter", "tds:Device"] {
            socket
                .send_to(probe_message(types).as_bytes(), target)
                .await?;
        }
    }
    let deadline = Instant::now() + wait;
    let mut found: BTreeMap<IpAddr, OnvifDevice> = BTreeMap::new();
    let mut buf = vec![0u8; 65_535];
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        match tokio::time::timeout(deadline - now, socket.recv_from(&mut buf)).await {
            Err(_) => break,
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "WS-Discovery receive failed");
                break;
            }
            Ok(Ok((n, from))) => {
                let text = String::from_utf8_lossy(&buf[..n]);
                if let Some(device) = parse_match(&text, from.ip()) {
                    // Keep the most complete answer of each device.
                    let better = found
                        .get(&device.address)
                        .is_none_or(|old| old.xaddr.is_none() && device.xaddr.is_some());
                    if better {
                        found.insert(device.address, device);
                    }
                }
            }
        }
    }
    Ok(found.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ProbeMatch as sent by a Hikvision camera (shortened).
    const MATCH: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<env:Envelope xmlns:env="http://www.w3.org/2003/05/soap-envelope" xmlns:wsadis="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery">
<env:Header><wsadis:MessageID>urn:uuid:1</wsadis:MessageID></env:Header>
<env:Body><d:ProbeMatches><d:ProbeMatch>
<wsadis:EndpointReference><wsadis:Address>urn:uuid:abc</wsadis:Address></wsadis:EndpointReference>
<d:Types>dn:NetworkVideoTransmitter tds:Device</d:Types>
<d:Scopes>onvif://www.onvif.org/type/video_encoder onvif://www.onvif.org/name/HIKVISION%20DS-2CD2143G2 onvif://www.onvif.org/hardware/DS-2CD2143G2-I onvif://www.onvif.org/location/city/hangzhou</d:Scopes>
<d:XAddrs>http://169.254.12.3/onvif/device_service http://192.168.1.20/onvif/device_service</d:XAddrs>
<d:MetadataVersion>10</d:MetadataVersion>
</d:ProbeMatch></d:ProbeMatches></env:Body></env:Envelope>"#;

    #[test]
    fn reads_probe_matches() {
        let from: IpAddr = "192.168.1.20".parse().expect("ip");
        let d = parse_match(MATCH, from).expect("match");
        assert_eq!(
            d.xaddr.as_deref(),
            Some("http://192.168.1.20/onvif/device_service")
        );
        assert_eq!(d.name.as_deref(), Some("HIKVISION DS-2CD2143G2"));
        assert_eq!(d.hardware.as_deref(), Some("DS-2CD2143G2-I"));
        assert!(parse_match("<not-soap/>", from).is_none());
        let p = probe_message("dn:NetworkVideoTransmitter");
        assert!(p.contains("<d:Types>dn:NetworkVideoTransmitter</d:Types>"));
        assert_ne!(message_id(), message_id());
    }

    #[tokio::test]
    async fn collects_answers_sent_to_a_probe() {
        // A fake camera on the loopback answering every probe.
        let camera = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let target = camera.local_addr().expect("addr");
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            while let Ok((n, from)) = camera.recv_from(&mut buf).await {
                if String::from_utf8_lossy(&buf[..n]).contains("Probe") {
                    let _ = camera.send_to(MATCH.as_bytes(), from).await;
                }
            }
        });
        let found = discover(target, Duration::from_millis(500))
            .await
            .expect("discover");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].address, "127.0.0.1".parse::<IpAddr>().expect("ip"));
        // Not on the sender's address: the first one.
        assert_eq!(
            found[0].xaddr.as_deref(),
            Some("http://169.254.12.3/onvif/device_service")
        );
    }
}
