//! Cameras on the local network (`POST /discovery/scan`) and their streams
//! (`POST /discovery/streams`), to prefill the camera form. Nothing is stored
//! here; the password sent for `streams` is only used for that request.

use crate::routes::ApiError;
use crate::state::ApiState;
use axum::Json;
use axum::extract::State;
use rustnvr_core::config::RtspTransport;
use rustnvr_core::{RtspTarget, Secret};
use rustnvr_discovery::onvif::{self, OnvifError};
use rustnvr_discovery::scan::{self, Network};
use rustnvr_discovery::{paths, wsd};
use rustnvr_stream::{SessionOptions, probe};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long cameras may take to answer WS-Discovery.
const WSD_WAIT: Duration = Duration::from_secs(3);
/// Connection delay of the RTSP port scan.
const SCAN_TIMEOUT: Duration = Duration::from_millis(500);
/// Connections at a time during the scan.
const SCAN_CONCURRENCY: usize = 128;
/// Per ONVIF request.
const ONVIF_TIMEOUT: Duration = Duration::from_secs(6);
/// Per RTSP path tried.
const PATH_TIMEOUT: Duration = Duration::from_secs(5);

/// `POST /discovery/scan` body.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScanInput {
    /// `192.168.1.0/24`; absent: the /24 of this machine.
    #[serde(default)]
    network: Option<String>,
}

/// A device found.
#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct FoundDevice {
    address: String,
    /// Answered ONVIF WS-Discovery.
    onvif: bool,
    /// ONVIF device service.
    xaddr: Option<String>,
    /// ONVIF name scope.
    name: Option<String>,
    /// ONVIF hardware scope.
    hardware: Option<String>,
    /// RTSP port that answered the scan.
    rtsp_port: Option<u16>,
    /// RTSP `Server` header.
    server: Option<String>,
    /// Camera already configured at this address.
    camera: Option<String>,
}

/// `POST /discovery/scan` answer.
#[derive(Serialize)]
pub(crate) struct ScanResult {
    /// Network scanned for RTSP, `null` when unknown (only WS-Discovery).
    network: Option<String>,
    devices: Vec<FoundDevice>,
    took_ms: u64,
}

/// Host of `rtsp://host:port`.
fn endpoint_host(endpoint: &str) -> &str {
    let rest = endpoint.split_once("://").map_or(endpoint, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or(rest);
    match authority.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => host.trim_matches(['[', ']']),
        _ => authority,
    }
}

/// Merges WS-Discovery answers and the RTSP scan, one entry per address.
fn merge(
    onvif: Vec<wsd::OnvifDevice>,
    rtsp: Vec<scan::RtspServer>,
    cameras: &[(String, String)],
) -> Vec<FoundDevice> {
    let mut by_ip: BTreeMap<IpAddr, FoundDevice> = BTreeMap::new();
    let blank = |address: IpAddr| FoundDevice {
        address: address.to_string(),
        onvif: false,
        xaddr: None,
        name: None,
        hardware: None,
        rtsp_port: None,
        server: None,
        camera: cameras
            .iter()
            .find(|(_, host)| *host == address.to_string())
            .map(|(id, _)| id.clone()),
    };
    for d in onvif {
        let e = by_ip.entry(d.address).or_insert_with(|| blank(d.address));
        e.onvif = true;
        e.xaddr = d.xaddr;
        e.name = d.name;
        e.hardware = d.hardware;
    }
    for s in rtsp {
        let ip = IpAddr::V4(s.address);
        let e = by_ip.entry(ip).or_insert_with(|| blank(ip));
        // 554 before 8554 (the scan is sorted).
        if e.rtsp_port.is_none() {
            e.rtsp_port = Some(s.port);
            e.server = s.server;
        }
    }
    by_ip.into_values().collect()
}

/// `POST /discovery/scan`: ONVIF cameras and RTSP servers of the network.
pub(crate) async fn scan_network(
    State(state): State<Arc<ApiState>>,
    input: Option<Json<ScanInput>>,
) -> Result<Json<ScanResult>, ApiError> {
    let started = Instant::now();
    let input = input.map(|Json(i)| i).unwrap_or_default();
    let network = match input.network.as_deref().map(str::trim) {
        Some(n) if !n.is_empty() => Some(Network::parse(n).map_err(ApiError::bad_request)?),
        _ => Network::local(),
    };
    let hosts = network.map(|n| n.hosts()).unwrap_or_default();
    let (onvif, rtsp) = tokio::join!(
        wsd::discover(wsd::MULTICAST, WSD_WAIT),
        scan::scan(&hosts, &scan::RTSP_PORTS, SCAN_TIMEOUT, SCAN_CONCURRENCY),
    );
    let onvif = onvif.unwrap_or_else(|e| {
        tracing::debug!(error = %e, "WS-Discovery unavailable");
        Vec::new()
    });
    let cameras: Vec<(String, String)> = state
        .cameras()
        .iter()
        .map(|c| (c.id.to_string(), endpoint_host(&c.endpoint).to_owned()))
        .collect();
    let devices = merge(onvif, rtsp, &cameras);
    tracing::info!(
        network = network.map(|n| n.to_string()).unwrap_or_default(),
        found = devices.len(),
        "network scanned for cameras"
    );
    Ok(Json(ScanResult {
        network: network.map(|n| n.to_string()),
        devices,
        took_ms: started.elapsed().as_millis() as u64,
    }))
}

/// `POST /discovery/streams` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StreamsInput {
    address: String,
    #[serde(default)]
    xaddr: Option<String>,
    #[serde(default)]
    rtsp_port: Option<u16>,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<Secret>,
}

/// A stream offered for the camera form.
#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct FoundStream {
    /// RTSP address, without credentials.
    url: String,
    /// `Main`, profile name…
    label: String,
    /// `H264`, `H265`, `JPEG`…
    codec: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    /// RustNVR can record it (H.264, H.265).
    recordable: bool,
}

/// `POST /discovery/streams` answer.
#[derive(Serialize)]
pub(crate) struct StreamsResult {
    /// Found through ONVIF (otherwise: usual paths of the makers).
    onvif: bool,
    manufacturer: Option<String>,
    model: Option<String>,
    /// Largest picture first.
    streams: Vec<FoundStream>,
    /// Why nothing was found.
    error: Option<String>,
}

fn recordable(codec: Option<&str>) -> bool {
    codec.is_some_and(|c| {
        let c = c.to_ascii_uppercase();
        c.contains("264") || c.contains("265") || c.contains("HEVC")
    })
}

/// Tries `url` (DESCRIBE only): the codec it would record, if any.
async fn try_path(
    url: &str,
    username: Option<&str>,
    password: Option<Secret>,
) -> Result<Option<String>, Option<u16>> {
    let target = RtspTarget::parse(url, username, password).map_err(|_| None)?;
    let opts = SessionOptions {
        transport: RtspTransport::Tcp,
        connect_timeout: PATH_TIMEOUT,
        stall_timeout: PATH_TIMEOUT,
        ..SessionOptions::default()
    };
    match probe(&target, &opts, false).await {
        Ok(report) => Ok(report
            .selected
            .and_then(|i| report.streams.get(i))
            .map(|s| s.encoding.clone())),
        Err(e) => Err(e.status_code()),
    }
}

/// Usual paths of the makers, the one named by `server` first.
async fn guess_streams(input: &StreamsInput, port: u16) -> StreamsResult {
    let user = input.username.as_deref().filter(|u| !u.is_empty());
    let base = format!("rtsp://{}:{port}", input.address);
    let mut result = StreamsResult {
        onvif: false,
        manufacturer: None,
        model: None,
        streams: Vec::new(),
        error: None,
    };
    for known in paths::ordered_for(input.server.as_deref()) {
        let main = format!("{base}{}", known.main);
        match try_path(&main, user, input.password.clone()).await {
            Ok(codec) => {
                result.manufacturer = Some(known.makers.to_owned());
                result.streams.push(FoundStream {
                    url: main,
                    label: "Principal".to_owned(),
                    recordable: recordable(codec.as_deref()),
                    codec,
                    width: None,
                    height: None,
                });
                if let Some(sub) = known.sub {
                    let sub = format!("{base}{sub}");
                    if let Ok(codec) = try_path(&sub, user, input.password.clone()).await {
                        result.streams.push(FoundStream {
                            url: sub,
                            label: "Sous-flux".to_owned(),
                            recordable: recordable(codec.as_deref()),
                            codec,
                            width: None,
                            height: None,
                        });
                    }
                }
                return result;
            }
            Err(Some(401)) => {
                result.error =
                    Some("la caméra refuse l'identifiant ou le mot de passe (RTSP 401)".to_owned());
                return result;
            }
            Err(_) => continue,
        }
    }
    result.error = Some(
        "pas d'ONVIF, et aucune des adresses RTSP habituelles ne répond : saisissez l'adresse du flux à la main (voir la notice de la caméra)".to_owned(),
    );
    result
}

/// `POST /discovery/streams`: the streams of a camera, through ONVIF, else
/// the usual RTSP paths of the makers.
pub(crate) async fn streams(
    Json(input): Json<StreamsInput>,
) -> Result<Json<StreamsResult>, ApiError> {
    let ip: IpAddr = input
        .address
        .parse()
        .map_err(|_| ApiError::bad_request("adresse IP attendue"))?;
    let credentials = input
        .username
        .as_deref()
        .filter(|u| !u.is_empty())
        .map(|u| {
            (
                u.to_owned(),
                input.password.clone().unwrap_or_else(|| Secret::new("")),
            )
        });
    match onvif::read_camera(
        &ip.to_string(),
        input.xaddr.as_deref(),
        credentials,
        ONVIF_TIMEOUT,
    )
    .await
    {
        Ok((_, info, found)) if !found.is_empty() => {
            let streams = found
                .into_iter()
                .map(|s| FoundStream {
                    recordable: recordable(s.encoding.as_deref()),
                    url: s.url,
                    label: s.profile,
                    codec: s.encoding,
                    width: s.width,
                    height: s.height,
                })
                .collect();
            return Ok(Json(StreamsResult {
                onvif: true,
                manufacturer: info.manufacturer,
                model: info.model,
                streams,
                error: None,
            }));
        }
        Err(OnvifError::Unauthorized) => {
            return Ok(Json(StreamsResult {
                onvif: true,
                manufacturer: None,
                model: None,
                streams: Vec::new(),
                error: Some(OnvifError::Unauthorized.to_string()),
            }));
        }
        Ok(_) => tracing::debug!(address = %ip, "ONVIF camera without RTSP profile"),
        Err(e) => tracing::debug!(address = %ip, error = %e, "no ONVIF streams"),
    }
    Ok(Json(
        guess_streams(&input, input.rtsp_port.unwrap_or(554)).await,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn merges_onvif_answers_and_rtsp_servers() {
        let onvif = vec![wsd::OnvifDevice {
            address: "192.168.1.20".parse().expect("ip"),
            xaddr: Some("http://192.168.1.20/onvif/device_service".into()),
            name: Some("HIKVISION".into()),
            hardware: None,
        }];
        let rtsp = vec![
            scan::RtspServer {
                address: Ipv4Addr::new(192, 168, 1, 20),
                port: 554,
                server: None,
            },
            scan::RtspServer {
                address: Ipv4Addr::new(192, 168, 1, 30),
                port: 554,
                server: Some("Rtsp Server/3.0".into()),
            },
            scan::RtspServer {
                address: Ipv4Addr::new(192, 168, 1, 30),
                port: 8554,
                server: None,
            },
        ];
        let cameras = vec![("jardin".to_owned(), "192.168.1.30".to_owned())];
        let found = merge(onvif, rtsp, &cameras);
        assert_eq!(found.len(), 2);
        assert!(found[0].onvif && found[0].rtsp_port == Some(554) && found[0].camera.is_none());
        assert_eq!(
            (
                found[1].onvif,
                found[1].rtsp_port,
                found[1].server.as_deref(),
                found[1].camera.as_deref()
            ),
            (false, Some(554), Some("Rtsp Server/3.0"), Some("jardin"))
        );
        assert_eq!(endpoint_host("rtsp://192.168.1.30:554"), "192.168.1.30");
        assert_eq!(endpoint_host("rtsp://[::1]:554"), "::1");
        assert!(recordable(Some("H265")) && recordable(Some("h264")) && !recordable(Some("JPEG")));
    }
}
