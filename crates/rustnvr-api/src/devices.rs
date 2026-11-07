//! Video devices of the machine (USB webcams, V4L2), to choose one in the
//! camera form: their name, a path that does not change when plugged
//! elsewhere, and the formats and sizes FFmpeg can ask them for.

use crate::routes::ApiError;
use crate::state::ApiState;
use axum::Json;
use axum::extract::State;
use serde::Serialize;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

/// How long FFmpeg may take to list the formats of one device.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// A format a device offers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct VideoFormat {
    /// FFmpeg's name (`mjpeg`, `yuyv422`, `h264`), for `input_format`.
    format: String,
    /// Compressed by the webcam (MJPEG, H.264), raw otherwise.
    compressed: bool,
    /// The driver's description (`Motion-JPEG`).
    description: String,
    /// Picture sizes (`1280x720`; a range for some drivers).
    sizes: Vec<String>,
}

/// A video device.
#[derive(Serialize)]
pub(crate) struct VideoDevice {
    /// `/dev/video0`.
    path: String,
    /// The same device under `/dev/v4l/by-id` (stable), when there is one.
    by_id: Option<String>,
    /// Its name (`HD Pro Webcam C920`).
    name: Option<String>,
    /// What it can send (empty for the extra nodes webcams have, which
    /// carry no pictures).
    formats: Vec<VideoFormat>,
    /// Why its formats could not be read.
    error: Option<String>,
}

/// `GET /devices/video` answer.
#[derive(Serialize)]
pub(crate) struct VideoDevices {
    devices: Vec<VideoDevice>,
}

/// The formats in FFmpeg's `-list_formats all` output.
pub(crate) fn parse_formats(stderr: &str) -> Vec<VideoFormat> {
    stderr
        .lines()
        .filter_map(|line| {
            // `[video4linux2,v4l2 @ 0x…] Compressed:  mjpeg :  Motion-JPEG : 1280x720 640x480`
            let rest = line.split_once("] ").map_or(line, |(_, r)| r);
            let (kind, rest) = rest.split_once(':')?;
            let compressed = match kind.trim() {
                "Compressed" => true,
                "Raw" => false,
                _ => return None,
            };
            let mut parts = rest.splitn(3, " : ");
            let format = parts.next()?.trim().to_owned();
            let description = parts.next()?.trim().to_owned();
            let sizes = parts
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            (!format.is_empty() && format != "Unsupported").then_some(VideoFormat {
                format,
                compressed,
                description,
                sizes,
            })
        })
        .collect()
}

/// Formats of `device`, asked to FFmpeg.
async fn formats(ffmpeg: &str, device: &str) -> Result<Vec<VideoFormat>, String> {
    let run = tokio::process::Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-f",
            "v4l2",
            "-list_formats",
            "all",
            "-i",
            device,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(LIST_TIMEOUT, run)
        .await
        .map_err(|_| "FFmpeg ne répond pas".to_owned())?
        .map_err(|e| format!("FFmpeg ne démarre pas : {e}"))?;
    // FFmpeg lists, then stops on purpose ("Immediate exit requested").
    let text = String::from_utf8_lossy(&out.stderr);
    let found = parse_formats(&text);
    if found.is_empty()
        && let Some(why) = text.lines().map(str::trim).find(|l| {
            l.contains("Permission denied") || l.contains("No such file") || l.contains("busy")
        })
    {
        return Err(why.to_owned());
    }
    Ok(found)
}

/// `/dev/videoN` devices, by number.
fn video_nodes(dev: &Path) -> Vec<(u32, String)> {
    let mut nodes: Vec<(u32, String)> = std::fs::read_dir(dev)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let n = name.strip_prefix("video")?.parse().ok()?;
                    Some((n, dev.join(&name).display().to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    nodes.sort();
    nodes
}

/// `GET /devices/video`: the video devices and what they can send.
pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<VideoDevices>, ApiError> {
    let ffmpeg = state.live_settings().ffmpeg;
    let by_id: Vec<(std::path::PathBuf, String)> = std::fs::read_dir("/dev/v4l/by-id")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let target = std::fs::canonicalize(e.path()).ok()?;
                    Some((target, e.path().display().to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut devices = Vec::new();
    for (n, path) in video_nodes(Path::new("/dev")) {
        let name = std::fs::read_to_string(format!("/sys/class/video4linux/video{n}/name"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let stable = by_id
            .iter()
            .filter(|(target, _)| target == Path::new(&path))
            .map(|(_, link)| link.clone())
            .min();
        let (formats, error) = match formats(&ffmpeg, &path).await {
            Ok(f) => (f, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        devices.push(VideoDevice {
            path,
            by_id: stable,
            name,
            formats,
            error,
        });
    }
    Ok(Json(VideoDevices { devices }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_listed_by_ffmpeg() {
        // FFmpeg 7, Logitech C920.
        let out = "[video4linux2,v4l2 @ 0x55d0c1f0] Raw       :     yuyv422 :           YUYV 4:2:2 : 640x480 160x90 1920x1080\n\
            [video4linux2,v4l2 @ 0x55d0c1f0] Compressed:        h264 :                H.264 : 640x480 1920x1080\n\
            [video4linux2,v4l2 @ 0x55d0c1f0] Compressed:       mjpeg :          Motion-JPEG : 640x480 1280x720 1920x1080\n\
            [in#0 @ 0x55d0c000] Error opening input: Immediate exit requested\n";
        let f = parse_formats(out);
        assert_eq!(f.len(), 3);
        assert_eq!(
            f[0],
            VideoFormat {
                format: "yuyv422".into(),
                compressed: false,
                description: "YUYV 4:2:2".into(),
                sizes: vec!["640x480".into(), "160x90".into(), "1920x1080".into()],
            }
        );
        assert!(f[1].compressed && f[1].format == "h264");
        assert_eq!(f[2].sizes[1], "1280x720");
        // A stepwise driver gives a range.
        let f = parse_formats(
            "[v4l2 @ 0x1] Raw       :     yuyv422 :  YUYV : {32-1920, 2}x{32-1080, 2}\n",
        );
        assert_eq!(f[0].sizes, ["{32-1920,", "2}x{32-1080,", "2}"]);
        assert!(parse_formats("nothing here").is_empty());
    }

    #[test]
    fn video_nodes_by_number() {
        let dir = tempfile::tempdir().expect("tmp");
        for name in ["video10", "video2", "vhci", "video0", "videox"] {
            std::fs::write(dir.path().join(name), b"").expect("node");
        }
        let nodes: Vec<u32> = video_nodes(dir.path())
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(nodes, [0, 2, 10]);
    }
}
