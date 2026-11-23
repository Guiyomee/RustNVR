//! ONVIF device and media services: the camera's maker and model, its video
//! profiles and their RTSP addresses (`GetStreamUri`), with WS-Security
//! `UsernameToken` authentication (password digest, dated with the camera's
//! own clock so that a camera on the wrong time still accepts it).

use crate::http_auth;
use crate::xml::{self, Element, escape};
use base64::Engine as _;
use rustnvr_core::Secret;
use sha1::{Digest as _, Sha1};
use std::time::Duration;

/// Usual ports of the ONVIF device service, tried when WS-Discovery gave no
/// address (Hikvision, Dahua: 80; Reolink: 8000; Tapo: 2020).
pub const DEVICE_PORTS: [u16; 5] = [80, 8000, 2020, 8080, 8899];

/// Why the camera could not be read.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum OnvifError {
    /// Nothing ONVIF answers there.
    #[error("pas de service ONVIF à cette adresse")]
    NotOnvif,
    /// Wrong user name or password (or ONVIF user not created on the camera).
    #[error(
        "la caméra refuse l'identifiant ou le mot de passe ONVIF (sur certaines caméras, il faut créer un utilisateur ONVIF dans leur interface)"
    )]
    Unauthorized,
    /// ONVIF answers, but announces no PTZ service (or no profile with a
    /// PTZ configuration).
    #[error("la caméra n'annonce pas de commande PTZ en ONVIF")]
    NoPtz,
    /// ONVIF answers, but does not implement this optional method (a camera
    /// with no presets, for instance).
    #[error("la caméra n'implémente pas cette méthode ONVIF optionnelle")]
    NotImplemented,
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

/// Maker, model, firmware and serial number (`GetDeviceInformation`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    /// `Manufacturer`.
    pub manufacturer: Option<String>,
    /// `Model`.
    pub model: Option<String>,
    /// `FirmwareVersion`.
    pub firmware: Option<String>,
    /// `SerialNumber`.
    pub serial: Option<String>,
    /// `HardwareId`.
    pub hardware: Option<String>,
}

/// A video stream of the camera (one ONVIF profile).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnvifStream {
    /// Profile name (`mainStream`, `Profile_1`…).
    pub profile: String,
    /// RTSP address, without credentials.
    pub url: String,
    /// `H264`, `H265`, `JPEG`…
    pub encoding: Option<String>,
    /// Picture size.
    pub width: Option<u32>,
    /// Picture size.
    pub height: Option<u32>,
}

/// An ONVIF client for one camera.
pub struct Client {
    http: reqwest::Client,
    credentials: Option<(String, Secret)>,
    /// Camera clock minus ours, in seconds.
    clock_offset: i64,
}

const NAMESPACES: &str = r#"xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:tds="http://www.onvif.org/ver10/device/wsdl" xmlns:trt="http://www.onvif.org/ver10/media/wsdl" xmlns:tt="http://www.onvif.org/ver10/schema" xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl""#;

fn now_seconds() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// `PasswordDigest = Base64(SHA-1(nonce + created + password))`.
fn password_digest(nonce: &[u8], created: &str, password: &str) -> String {
    let mut h = Sha1::new();
    h.update(nonce);
    h.update(created.as_bytes());
    h.update(password.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(h.finalize())
}

/// RTSP address without `user:password@`.
fn strip_credentials(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            let host = authority.rsplit('@').next().unwrap_or(authority);
            format!("{scheme}://{host}{path}")
        }
        None => url.to_owned(),
    }
}

impl Client {
    /// Client with `timeout` per request.
    pub fn new(
        credentials: Option<(String, Secret)>,
        timeout: Duration,
    ) -> Result<Self, OnvifError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(3)))
            .build()
            .map_err(|e| OnvifError::Other(e.without_url().to_string()))?;
        Ok(Self {
            http,
            credentials,
            clock_offset: 0,
        })
    }

    fn security_header(&self) -> String {
        let Some((user, password)) = &self.credentials else {
            return String::new();
        };
        let mut nonce = [0u8; 16];
        let _ = getrandom::fill(&mut nonce);
        let created = jiff::Timestamp::from_second(now_seconds() + self.clock_offset)
            .map(|t| t.to_string())
            .unwrap_or_default();
        let digest = password_digest(&nonce, &created, password.expose());
        let b64 = base64::engine::general_purpose::STANDARD.encode(nonce);
        format!(
            r#"<s:Header><wsse:Security s:mustUnderstand="1" xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd"><wsse:UsernameToken><wsse:Username>{}</wsse:Username><wsse:Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{digest}</wsse:Password><wsse:Nonce EncodingType="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary">{b64}</wsse:Nonce><wsu:Created>{created}</wsu:Created></wsse:UsernameToken></wsse:Security></s:Header>"#,
            escape(user)
        )
    }

    /// Posts `body` to `url`; the SOAP `Body`.
    pub(crate) async fn call(
        &self,
        url: &str,
        body: &str,
        authenticated: bool,
    ) -> Result<Element, OnvifError> {
        let header = if authenticated {
            self.security_header()
        } else {
            String::new()
        };
        let envelope = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope {NAMESPACES}>{header}<s:Body>{body}</s:Body></s:Envelope>"#
        );
        let response = self
            .http
            .post(url)
            .header("Content-Type", "application/soap+xml; charset=utf-8")
            .body(envelope)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() || e.is_timeout() {
                    OnvifError::NotOnvif
                } else {
                    OnvifError::Other(e.without_url().to_string())
                }
            })?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| OnvifError::Other(e.without_url().to_string()))?;
        if status.as_u16() == 401 {
            return Err(OnvifError::Unauthorized);
        }
        let root = xml::parse(&text).ok_or(OnvifError::NotOnvif)?;
        let body = root.find("Body").ok_or(OnvifError::NotOnvif)?;
        if let Some(fault) = body.find("Fault") {
            let reason = fault
                .find("Reason")
                .and_then(|r| r.find("Text"))
                .map(|t| t.text.trim().to_owned())
                .unwrap_or_default();
            let codes = {
                let mut v = Vec::new();
                fault.find_all("Value", &mut v);
                v.iter()
                    .map(|c| c.text.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let refused = codes.contains("NotAuthorized")
                || codes.contains("FailedAuthentication")
                || reason.to_ascii_lowercase().contains("auth");
            let missing = codes.contains("NotImplemented")
                || codes.contains("NotSupported")
                || reason.to_ascii_lowercase().contains("not implemented");
            return Err(if refused {
                OnvifError::Unauthorized
            } else if missing {
                OnvifError::NotImplemented
            } else {
                OnvifError::Other(format!(
                    "la caméra répond une erreur ONVIF : {}",
                    if reason.is_empty() { codes } else { reason }
                ))
            });
        }
        if !status.is_success() {
            return Err(OnvifError::Other(format!("réponse HTTP {status}")));
        }
        Ok(body.clone())
    }

    /// Reads the camera clock (no authentication: also proves ONVIF answers
    /// at `device_url`).
    pub async fn sync_clock(&mut self, device_url: &str) -> Result<(), OnvifError> {
        let body = self
            .call(device_url, "<tds:GetSystemDateAndTime/>", false)
            .await?;
        let utc = body.find("UTCDateTime").ok_or(OnvifError::NotOnvif)?;
        let n = |path: &[&str]| utc.text_at(path).and_then(|v| v.parse::<i16>().ok());
        if let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) = (
            n(&["Date", "Year"]),
            n(&["Date", "Month"]),
            n(&["Date", "Day"]),
            n(&["Time", "Hour"]),
            n(&["Time", "Minute"]),
            n(&["Time", "Second"]),
        ) && let Ok(t) =
            jiff::civil::datetime(y, mo as i8, d as i8, h as i8, mi as i8, s as i8, 0)
                .to_zoned(jiff::tz::TimeZone::UTC)
        {
            self.clock_offset = t.timestamp().as_second() - now_seconds();
        }
        Ok(())
    }

    /// Restarts the camera (`SystemReboot`; needs an administrator account
    /// of the camera); the message it gives (« Rebooting in 30 seconds »…).
    pub async fn system_reboot(&self, device_url: &str) -> Result<String, OnvifError> {
        let body = self.call(device_url, "<tds:SystemReboot/>", true).await?;
        Ok(body
            .find("SystemRebootResponse")
            .and_then(|r| r.text_at(&["Message"]))
            .map(|m| m.trim().to_owned())
            .unwrap_or_default())
    }

    /// Maker, model, firmware, serial number and hardware.
    pub async fn device_information(&self, device_url: &str) -> Result<DeviceInfo, OnvifError> {
        let body = self
            .call(device_url, "<tds:GetDeviceInformation/>", true)
            .await?;
        let r = body
            .find("GetDeviceInformationResponse")
            .ok_or(OnvifError::NotOnvif)?;
        // Blank values (some cameras send them empty) are left out.
        let get = |n: &str| {
            r.text_at(&[n])
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        Ok(DeviceInfo {
            manufacturer: get("Manufacturer"),
            model: get("Model"),
            firmware: get("FirmwareVersion"),
            serial: get("SerialNumber"),
            hardware: get("HardwareId"),
        })
    }

    /// Address of the media service (the device service when not told).
    pub async fn media_url(&self, device_url: &str) -> Result<String, OnvifError> {
        let body = self
            .call(
                device_url,
                "<tds:GetCapabilities><tds:Category>Media</tds:Category></tds:GetCapabilities>",
                true,
            )
            .await?;
        Ok(body
            .find("Media")
            .and_then(|m| m.text_at(&["XAddr"]))
            .map(str::to_owned)
            .unwrap_or_else(|| device_url.to_owned()))
    }

    /// Video profiles and their RTSP addresses, largest picture first.
    pub async fn streams(&self, media_url: &str) -> Result<Vec<OnvifStream>, OnvifError> {
        let body = self.call(media_url, "<trt:GetProfiles/>", true).await?;
        let mut profiles = Vec::new();
        body.find_all("Profiles", &mut profiles);
        let mut out = Vec::new();
        for p in profiles {
            let Some(token) = p.attr("token") else {
                continue;
            };
            let encoder = p.find("VideoEncoderConfiguration");
            let size = |n: &str| {
                encoder
                    .and_then(|e| e.find("Resolution"))
                    .and_then(|r| r.text_at(&[n]))
                    .and_then(|v| v.parse().ok())
            };
            let request = format!(
                "<trt:GetStreamUri><trt:StreamSetup><tt:Stream>RTP-Unicast</tt:Stream><tt:Transport><tt:Protocol>RTSP</tt:Protocol></tt:Transport></trt:StreamSetup><trt:ProfileToken>{}</trt:ProfileToken></trt:GetStreamUri>",
                escape(token)
            );
            let uri = match self.call(media_url, &request, true).await {
                Ok(b) => b
                    .find("MediaUri")
                    .and_then(|m| m.text_at(&["Uri"]))
                    .map(strip_credentials),
                Err(OnvifError::Unauthorized) => return Err(OnvifError::Unauthorized),
                Err(e) => {
                    tracing::debug!(profile = token, error = %e, "ONVIF GetStreamUri failed");
                    None
                }
            };
            let Some(url) = uri.filter(|u| u.starts_with("rtsp://")) else {
                continue;
            };
            // Several profiles may share one stream.
            if out.iter().any(|s: &OnvifStream| s.url == url) {
                continue;
            }
            out.push(OnvifStream {
                profile: p.text_at(&["Name"]).unwrap_or(token).to_owned(),
                url,
                encoding: encoder
                    .and_then(|e| e.text_at(&["Encoding"]))
                    .map(str::to_owned),
                width: size("Width"),
                height: size("Height"),
            });
        }
        out.sort_by_key(|s| {
            std::cmp::Reverse(u64::from(s.width.unwrap_or(0)) * u64::from(s.height.unwrap_or(0)))
        });
        Ok(out)
    }

    /// Addresses of the photos the camera gives (`GetSnapshotUri`), one per
    /// video profile that has one (the same address once), largest
    /// announced profile first. Some cameras announce no size, or give the
    /// same small photo whatever the profile: [`Client::largest_photo`]
    /// tries them all. They may hold credentials: never logged.
    pub async fn snapshot_urls(&self, media_url: &str) -> Result<Vec<String>, OnvifError> {
        let body = self.call(media_url, "<trt:GetProfiles/>", true).await?;
        let mut profiles = Vec::new();
        body.find_all("Profiles", &mut profiles);
        let mut tokens: Vec<(u64, &str)> = profiles
            .iter()
            .filter_map(|p| {
                let size = |n: &str| {
                    p.find("VideoEncoderConfiguration")
                        .and_then(|e| e.find("Resolution"))
                        .and_then(|r| r.text_at(&[n]))
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0)
                };
                Some((size("Width") * size("Height"), p.attr("token")?))
            })
            .collect();
        tokens.sort_by_key(|(area, _)| std::cmp::Reverse(*area));
        let mut last = OnvifError::NotImplemented;
        let mut urls: Vec<String> = Vec::new();
        for (_, token) in tokens {
            let request = format!(
                "<trt:GetSnapshotUri><trt:ProfileToken>{}</trt:ProfileToken></trt:GetSnapshotUri>",
                escape(token)
            );
            match self.call(media_url, &request, true).await {
                Ok(b) => {
                    if let Some(uri) = b
                        .find("MediaUri")
                        .and_then(|m| m.text_at(&["Uri"]))
                        .map(str::trim)
                        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
                        && !urls.iter().any(|u| u == uri)
                    {
                        urls.push(uri.to_owned());
                    }
                }
                Err(OnvifError::Unauthorized) => return Err(OnvifError::Unauthorized),
                Err(e) => last = e,
            }
        }
        if !urls.is_empty() {
            return Ok(urls);
        }
        Err(match last {
            OnvifError::NotImplemented => OnvifError::Other(
                "la caméra ne donne pas de photo en ONVIF (GetSnapshotUri)".to_owned(),
            ),
            e => e,
        })
    }

    /// The largest photo among those of `urls` (from
    /// [`Client::snapshot_urls`]): its address, the photo and its size. A
    /// refused password stops at once; otherwise the last failure is given
    /// when no photo came.
    pub async fn largest_photo(
        &self,
        urls: &[String],
    ) -> Result<(String, Vec<u8>, (u16, u16)), OnvifError> {
        let mut best: Option<(String, Vec<u8>, (u16, u16))> = None;
        let mut last = OnvifError::Other("la caméra ne donne pas de photo en ONVIF".into());
        for url in urls {
            match self.photo(url).await {
                Ok(jpeg) => {
                    let size = jpeg_size(&jpeg).unwrap_or((0, 0));
                    let area = |s: (u16, u16)| u32::from(s.0) * u32::from(s.1);
                    if best.as_ref().is_none_or(|(_, _, b)| area(size) > area(*b)) {
                        best = Some((url.clone(), jpeg, size));
                    }
                }
                Err(OnvifError::Unauthorized) => return Err(OnvifError::Unauthorized),
                Err(e) => last = e,
            }
        }
        best.ok_or(last)
    }

    /// Downloads the photo at `url` (from [`Client::snapshot_urls`]), with
    /// HTTP Digest or Basic when the camera asks for it; a JPEG, at most
    /// [`MAX_PHOTO`] bytes.
    pub async fn photo(&self, url: &str) -> Result<Vec<u8>, OnvifError> {
        let send = |authorization: Option<String>| {
            let mut request = self.http.get(url);
            if let Some(a) = authorization {
                request = request.header("Authorization", a);
            }
            async move {
                request.send().await.map_err(|e| {
                    if e.is_timeout() {
                        OnvifError::Other("la caméra met trop de temps à donner la photo".into())
                    } else if e.is_connect() {
                        // ONVIF answered (it gave this address): the photo
                        // is served elsewhere, often on a port the camera
                        // keeps closed (HTTP off on recent Reolink).
                        let port = reqwest::Url::parse(url)
                            .ok()
                            .and_then(|u| u.port_or_known_default())
                            .map_or_else(String::new, |p| format!(" (port {p})"));
                        OnvifError::Other(format!(
                            "la caméra refuse la connexion à l'adresse de sa photo{port} : son port HTTP est sans doute coupé (Reolink : Réseau → Avancé → Réglage des ports → HTTP)"
                        ))
                    } else {
                        OnvifError::Other(e.without_url().to_string())
                    }
                })
            }
        };
        let mut response = send(None).await?;
        if response.status().as_u16() == 401
            && let Some((user, password)) = &self.credentials
        {
            let challenges: Vec<String> = response
                .headers()
                .get_all("WWW-Authenticate")
                .iter()
                .filter_map(|v| v.to_str().ok().map(str::to_owned))
                .collect();
            let path = url
                .split_once("://")
                .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
                .unwrap_or("/");
            let authorization = match challenges.iter().find_map(|c| http_auth::parse_digest(c)) {
                Some(challenge) => {
                    let mut nonce = [0u8; 8];
                    let _ = getrandom::fill(&mut nonce);
                    let cnonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
                    http_auth::digest_header(&challenge, user, password.expose(), path, &cnonce)
                }
                None => http_auth::basic_header(user, password.expose()),
            };
            response = send(Some(authorization)).await?;
        }
        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(OnvifError::Unauthorized);
        }
        if !status.is_success() {
            return Err(OnvifError::Other(format!("photo : réponse HTTP {status}")));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_PHOTO as u64)
        {
            return Err(OnvifError::Other("photo trop grande".into()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| OnvifError::Other(e.without_url().to_string()))?;
        if bytes.len() > MAX_PHOTO {
            return Err(OnvifError::Other("photo trop grande".into()));
        }
        if !bytes.starts_with(&[0xff, 0xd8]) {
            return Err(OnvifError::Other(
                "la caméra n'a pas renvoyé une image JPEG".into(),
            ));
        }
        Ok(bytes.to_vec())
    }
}

/// Largest photo accepted from a camera (a 4K JPEG is 1 to 3 MB).
pub const MAX_PHOTO: usize = 16 * 1024 * 1024;

/// Width and height of a JPEG, from its start-of-frame marker.
pub fn jpeg_size(jpeg: &[u8]) -> Option<(u16, u16)> {
    let mut i = 2;
    while i + 9 < jpeg.len() {
        if jpeg[i] != 0xff {
            return None;
        }
        let marker = jpeg[i + 1];
        // Fill bytes and markers without length.
        if marker == 0xff {
            i += 1;
            continue;
        }
        if (0xd0..=0xd9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let h = u16::from_be_bytes([jpeg[i + 5], jpeg[i + 6]]);
            let w = u16::from_be_bytes([jpeg[i + 7], jpeg[i + 8]]);
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

/// Everything about a camera: finds its device service (`device_url`, or
/// the usual ports of `host`), then its maker and streams.
pub async fn read_camera(
    host: &str,
    device_url: Option<&str>,
    credentials: Option<(String, Secret)>,
    timeout: Duration,
) -> Result<(String, DeviceInfo, Vec<OnvifStream>), OnvifError> {
    let mut client = Client::new(credentials, timeout)?;
    let candidates: Vec<String> = match device_url {
        Some(u) => vec![u.to_owned()],
        None => DEVICE_PORTS
            .iter()
            .map(|p| match p {
                80 => format!("http://{host}/onvif/device_service"),
                p => format!("http://{host}:{p}/onvif/device_service"),
            })
            .collect(),
    };
    let mut url = None;
    for c in candidates {
        match client.sync_clock(&c).await {
            Ok(()) => {
                url = Some(c);
                break;
            }
            Err(OnvifError::NotOnvif) => continue,
            // Answered, but differently (auth on everything): ONVIF is there.
            Err(_) => {
                url = Some(c);
                break;
            }
        }
    }
    let url = url.ok_or(OnvifError::NotOnvif)?;
    let info = client.device_information(&url).await?;
    let media = client.media_url(&url).await?;
    let streams = client.streams(&media).await?;
    Ok((url, info, streams))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    #[test]
    fn digest_and_credentials() {
        // Example of the WS-Security UsernameToken profile.
        let nonce = base64::engine::general_purpose::STANDARD
            .decode("LKqI6G/AikKCQrN0zqZFlg==")
            .expect("nonce");
        assert_eq!(
            password_digest(&nonce, "2010-09-16T07:50:45Z", "userpassword"),
            "tuOSpGlFlIXsozq4HFNeeGeFLEI="
        );
        assert_eq!(
            strip_credentials("rtsp://admin:pw@192.168.1.20:554/Streaming/Channels/101"),
            "rtsp://192.168.1.20:554/Streaming/Channels/101"
        );
        assert_eq!(strip_credentials("rtsp://h/a@b"), "rtsp://h/a@b");
    }

    fn soap(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><SOAP-ENV:Envelope xmlns:SOAP-ENV="http://www.w3.org/2003/05/soap-envelope" xmlns:tds="d" xmlns:trt="m" xmlns:tt="s"><SOAP-ENV:Body>{body}</SOAP-ENV:Body></SOAP-ENV:Envelope>"#
        )
    }

    /// A fake ONVIF camera: answers by request, refuses requests without
    /// credentials (except the clock).
    async fn camera() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut req = Vec::new();
                    let mut buf = [0u8; 8192];
                    // Headers then the announced body.
                    loop {
                        let Ok(n) = s.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        req.extend_from_slice(&buf[..n]);
                        let text = String::from_utf8_lossy(&req);
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len = text[..end]
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|v| v.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            if req.len() >= end + 4 + len {
                                break;
                            }
                        }
                    }
                    let text = String::from_utf8_lossy(&req).into_owned();
                    let authed = text.contains("<wsse:Username>admin</wsse:Username>");
                    let body = if text.contains("GetSystemDateAndTime") {
                        soap(
                            "<tds:GetSystemDateAndTimeResponse><tds:SystemDateAndTime><tt:UTCDateTime><tt:Time><tt:Hour>8</tt:Hour><tt:Minute>0</tt:Minute><tt:Second>0</tt:Second></tt:Time><tt:Date><tt:Year>2030</tt:Year><tt:Month>1</tt:Month><tt:Day>2</tt:Day></tt:Date></tt:UTCDateTime></tds:SystemDateAndTime></tds:GetSystemDateAndTimeResponse>",
                        )
                    } else if !authed {
                        soap(
                            "<SOAP-ENV:Fault><SOAP-ENV:Code><SOAP-ENV:Value>SOAP-ENV:Sender</SOAP-ENV:Value><SOAP-ENV:Subcode><SOAP-ENV:Value>ter:NotAuthorized</SOAP-ENV:Value></SOAP-ENV:Subcode></SOAP-ENV:Code><SOAP-ENV:Reason><SOAP-ENV:Text>Sender not Authorized</SOAP-ENV:Text></SOAP-ENV:Reason></SOAP-ENV:Fault>",
                        )
                    } else if text.contains("GetDeviceInformation") {
                        soap(
                            "<tds:GetDeviceInformationResponse><tds:Manufacturer>Dahua</tds:Manufacturer><tds:Model>IPC-HDW2431T</tds:Model><tds:FirmwareVersion>2.8</tds:FirmwareVersion><tds:SerialNumber>7G0123PAZ</tds:SerialNumber><tds:HardwareId> </tds:HardwareId></tds:GetDeviceInformationResponse>",
                        )
                    } else if text.contains("GetCapabilities") {
                        soap(&format!(
                            "<tds:GetCapabilitiesResponse><tds:Capabilities><tt:Media><tt:XAddr>http://{addr}/onvif/media_service</tt:XAddr></tt:Media></tds:Capabilities></tds:GetCapabilitiesResponse>"
                        ))
                    } else if text.contains("GetProfiles") {
                        soap(
                            "<trt:GetProfilesResponse><trt:Profiles token=\"MediaProfile001\"><tt:Name>Sub</tt:Name><tt:VideoEncoderConfiguration><tt:Encoding>H264</tt:Encoding><tt:Resolution><tt:Width>704</tt:Width><tt:Height>576</tt:Height></tt:Resolution></tt:VideoEncoderConfiguration></trt:Profiles><trt:Profiles token=\"MediaProfile000\"><tt:Name>Main</tt:Name><tt:VideoEncoderConfiguration><tt:Encoding>H265</tt:Encoding><tt:Resolution><tt:Width>2688</tt:Width><tt:Height>1520</tt:Height></tt:Resolution></tt:VideoEncoderConfiguration></trt:Profiles></trt:GetProfilesResponse>",
                        )
                    } else if text.contains("MediaProfile000") {
                        soap(
                            "<trt:GetStreamUriResponse><trt:MediaUri><tt:Uri>rtsp://192.168.1.21:554/cam/realmonitor?channel=1&amp;subtype=0&amp;unicast=true</tt:Uri></trt:MediaUri></trt:GetStreamUriResponse>",
                        )
                    } else {
                        soap(
                            "<trt:GetStreamUriResponse><trt:MediaUri><tt:Uri>rtsp://192.168.1.21:554/cam/realmonitor?channel=1&amp;subtype=1&amp;unicast=true</tt:Uri></trt:MediaUri></trt:GetStreamUriResponse>",
                        )
                    };
                    let answer = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/soap+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.write_all(answer.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}/onvif/device_service")
    }

    /// Reads one HTTP request (headers, then the announced body).
    async fn read_request(s: &mut tokio::net::TcpStream) -> Option<String> {
        let mut req = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = s.read(&mut buf).await.ok()?;
            if n == 0 {
                return None;
            }
            req.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&req);
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text[..end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if req.len() >= end + 4 + len {
                    return Some(text.into_owned());
                }
            }
        }
    }

    /// A camera giving its photo: two profiles (the small one first), a
    /// photo address for the large one, behind HTTP Digest (`/digest.jpg`)
    /// or Basic (`/basic.jpg`); `/page` answers HTML.
    async fn photo_camera() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let Some(text) = read_request(&mut s).await else {
                        return;
                    };
                    let line = text.lines().next().unwrap_or_default().to_owned();
                    let auth = text
                        .lines()
                        .find_map(|l| {
                            // Header names come in lower case from reqwest.
                            l.split_once(':')
                                .filter(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                                .map(|(_, v)| v.trim().to_owned())
                        })
                        .unwrap_or_default();
                    let reply = |status: &str, kind: &str, extra: &str, body: &[u8]| {
                        let mut r = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        r.extend_from_slice(body);
                        r
                    };
                    // Start of frame: 1920×1080 (digest), 640×360 (basic).
                    let jpeg: &[u8] = &[
                        0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0x04, 0x38, 0x07, 0x80, 1, 0, 0, 0, 0xff,
                        0xd9,
                    ];
                    let small: &[u8] = &[
                        0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0x01, 0x68, 0x02, 0x80, 1, 0, 0, 0, 0xff,
                        0xd9,
                    ];
                    let answer = if line.starts_with("POST") {
                        let body = if text.contains("GetProfiles") {
                            soap(
                                "<trt:GetProfilesResponse><trt:Profiles token=\"sub\"><tt:VideoEncoderConfiguration><tt:Resolution><tt:Width>640</tt:Width><tt:Height>360</tt:Height></tt:Resolution></tt:VideoEncoderConfiguration></trt:Profiles><trt:Profiles token=\"main\"><tt:VideoEncoderConfiguration><tt:Resolution><tt:Width>2560</tt:Width><tt:Height>1440</tt:Height></tt:Resolution></tt:VideoEncoderConfiguration></trt:Profiles></trt:GetProfilesResponse>",
                            )
                        } else if text.contains(">main<") {
                            soap(&format!(
                                "<trt:GetSnapshotUriResponse><trt:MediaUri><tt:Uri>http://{addr}/digest.jpg?ch=1</tt:Uri></trt:MediaUri></trt:GetSnapshotUriResponse>"
                            ))
                        } else {
                            soap(&format!(
                                "<trt:GetSnapshotUriResponse><trt:MediaUri><tt:Uri>http://{addr}/basic.jpg</tt:Uri></trt:MediaUri></trt:GetSnapshotUriResponse>"
                            ))
                        };
                        reply("200 OK", "application/soap+xml", "", body.as_bytes())
                    } else if line.starts_with("GET /digest.jpg?ch=1 ") {
                        if let Some(cnonce) = auth
                            .split("cnonce=\"")
                            .nth(1)
                            .and_then(|r| r.split('"').next())
                        {
                            let challenge = http_auth::parse_digest(
                                r#"Digest realm="cam", qop="auth", nonce="n1", opaque="o1""#,
                            )
                            .expect("challenge");
                            let expected = http_auth::digest_header(
                                &challenge,
                                "admin",
                                "pw",
                                "/digest.jpg?ch=1",
                                cnonce,
                            );
                            if auth == expected {
                                reply("200 OK", "image/jpeg", "", jpeg)
                            } else {
                                reply("401 Unauthorized", "text/plain", "", b"")
                            }
                        } else {
                            reply(
                                "401 Unauthorized",
                                "text/plain",
                                "WWW-Authenticate: Digest realm=\"cam\", qop=\"auth\", nonce=\"n1\", opaque=\"o1\"\r\n",
                                b"",
                            )
                        }
                    } else if line.starts_with("GET /basic.jpg ") {
                        if auth == "Basic YWRtaW46cHc=" {
                            reply("200 OK", "image/jpeg", "", small)
                        } else {
                            reply(
                                "401 Unauthorized",
                                "text/plain",
                                "WWW-Authenticate: Basic realm=\"cam\"\r\n",
                                b"",
                            )
                        }
                    } else {
                        reply("200 OK", "text/html", "", b"<html></html>")
                    };
                    let _ = s.write_all(&answer).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn gets_the_photo_of_the_camera() {
        let base = photo_camera().await;
        let creds = Some(("admin".to_owned(), Secret::new("pw".to_owned())));
        let client = Client::new(creds, Duration::from_secs(5)).expect("client");
        let urls = client
            .snapshot_urls(&format!("{base}/onvif/media"))
            .await
            .expect("addresses");
        let url = format!("{base}/digest.jpg?ch=1");
        assert_eq!(
            urls,
            [url.clone(), format!("{base}/basic.jpg")],
            "largest profile first"
        );
        let jpeg = client.photo(&url).await.expect("digest");
        assert_eq!(jpeg_size(&jpeg), Some((1920, 1080)));
        // Tried in the other order, the largest photo still wins.
        let reversed: Vec<String> = urls.iter().rev().cloned().collect();
        let (best, _, size) = client.largest_photo(&reversed).await.expect("photo");
        assert_eq!((best.as_str(), size), (url.as_str(), (1920, 1080)));
        assert!(client.photo(&format!("{base}/basic.jpg")).await.is_ok());
        assert!(matches!(
            client.photo(&format!("{base}/page")).await,
            Err(OnvifError::Other(_))
        ));
        // A closed port: said as such, with the port (not « no ONVIF »).
        match client.photo("http://127.0.0.1:9/snap.jpg").await {
            Err(OnvifError::Other(m)) => assert!(m.contains("(port 9)"), "{m}"),
            other => panic!("{other:?}"),
        }
        let wrong = Client::new(
            Some(("admin".to_owned(), Secret::new("bad".to_owned()))),
            Duration::from_secs(5),
        )
        .expect("client");
        assert_eq!(wrong.photo(&url).await, Err(OnvifError::Unauthorized));
        assert_eq!(
            wrong.photo(&format!("{base}/basic.jpg")).await,
            Err(OnvifError::Unauthorized)
        );
    }

    #[tokio::test]
    async fn reads_a_camera_and_its_streams() {
        let url = camera().await;
        let creds = Some(("admin".to_owned(), Secret::new("pw".to_owned())));
        let (found, info, streams) =
            read_camera("127.0.0.1", Some(&url), creds, Duration::from_secs(5))
                .await
                .expect("camera");
        assert_eq!(found, url);
        assert_eq!(info.manufacturer.as_deref(), Some("Dahua"));
        assert_eq!(info.model.as_deref(), Some("IPC-HDW2431T"));
        assert_eq!(info.serial.as_deref(), Some("7G0123PAZ"));
        assert_eq!(info.hardware, None, "blank: left out");
        assert_eq!(streams.len(), 2);
        assert_eq!(
            (
                streams[0].profile.as_str(),
                streams[0].encoding.as_deref(),
                streams[0].width
            ),
            ("Main", Some("H265"), Some(2688))
        );
        assert_eq!(
            streams[0].url,
            "rtsp://192.168.1.21:554/cam/realmonitor?channel=1&subtype=0&unicast=true"
        );
        assert_eq!(streams[1].profile, "Sub");

        // Wrong user: refused, said as such.
        let bad = Some(("other".to_owned(), Secret::new("x".to_owned())));
        assert_eq!(
            read_camera("127.0.0.1", Some(&url), bad, Duration::from_secs(5))
                .await
                .map(|_| ()),
            Err(OnvifError::Unauthorized)
        );
        // Nothing there.
        assert_eq!(
            read_camera(
                "127.0.0.1",
                Some("http://127.0.0.1:9/onvif/device_service"),
                None,
                Duration::from_secs(2)
            )
            .await
            .map(|_| ()),
            Err(OnvifError::NotOnvif)
        );
    }
}
