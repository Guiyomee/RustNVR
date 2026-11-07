//! Storage of the cameras managed from the web interface
//! ([`MANAGED_CAMERAS_FILE`], next to the configuration file).
//!
//! The file contains camera passwords (the cameras need them), so it is
//! written readable by its owner only (mode 0600 on Unix), atomically
//! (temporary file, fsync, rename) so that a crash never leaves it truncated.

use rustnvr_core::config::{
    CameraConfig, MANAGED_CAMERAS_FILE, RecordMode, RtspTransport, StreamConfig,
};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::Path;

const HEADER: &str = "# Caméras ajoutées depuis l'interface web de RustNVR.\n\
# Fichier géré automatiquement : il contient les mots de passe des caméras,\n\
# gardez-le privé. Les caméras de config.toml ne sont pas concernées.\n\n";

// TOML requires plain values before sub-tables: keep field order.
#[derive(Serialize)]
struct StoredFile<'a> {
    cameras: Vec<StoredCamera<'a>>,
}

#[derive(Serialize)]
struct StoredCamera<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    enabled: bool,
    record: StoredRecord<'a>,
    /// Absent for a USB camera.
    #[serde(skip_serializing_if = "Option::is_none")]
    main: Option<StoredStream<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<StoredStream<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usb: Option<&'a rustnvr_core::config::UsbConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    onvif: Option<StoredOnvif<'a>>,
}

#[derive(Serialize)]
struct StoredOnvif<'a> {
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password_env: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredRecord<'a> {
    enabled: bool,
    mode: RecordMode,
    destinations: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    retention_days: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_size_gb: Option<u32>,
    audio: bool,
}

#[derive(Serialize)]
struct StoredStream<'a> {
    url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<&'a str>,
    transport: &'static str,
    connect_timeout_seconds: u32,
    stall_timeout_seconds: u32,
}

fn stream(s: &StreamConfig) -> StoredStream<'_> {
    StoredStream {
        url: &s.url,
        username: s.username.as_deref(),
        password: s.password.as_ref().map(|p| p.expose()),
        transport: match s.transport {
            RtspTransport::Tcp => "tcp",
            RtspTransport::Udp => "udp",
        },
        connect_timeout_seconds: s.connect_timeout_seconds,
        stall_timeout_seconds: s.stall_timeout_seconds,
    }
}

/// Writes `cameras` (the managed ones) to `path`, replacing its content.
pub fn save<'a>(
    path: &Path,
    cameras: impl IntoIterator<Item = &'a CameraConfig>,
) -> io::Result<()> {
    let file = StoredFile {
        cameras: cameras
            .into_iter()
            .map(|c| StoredCamera {
                id: c.id.as_str(),
                name: c.name.as_deref(),
                enabled: c.enabled,
                record: StoredRecord {
                    enabled: c.record.enabled,
                    mode: c.record.mode,
                    destinations: &c.record.destinations,
                    retention_days: c.record.retention_days,
                    max_size_gb: c.record.max_size_gb,
                    audio: c.record.audio,
                },
                main: c.usb.is_none().then(|| stream(&c.main)),
                sub: c.sub.as_ref().map(stream),
                usb: c.usb.as_ref(),
                onvif: c.onvif.as_ref().map(|o| StoredOnvif {
                    port: o.port,
                    url: o.url.as_deref(),
                    username: o.username.as_deref(),
                    password: o.password.as_ref().map(|p| p.expose()),
                    password_env: o.password_env.as_deref(),
                }),
            })
            .collect(),
    };
    let body = toml::to_string(&file).map_err(io::Error::other)?;
    let tmp = path.with_file_name(format!("{MANAGED_CAMERAS_FILE}.tmp"));
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp)?;
    f.write_all(HEADER.as_bytes())?;
    f.write_all(body.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::config::Config;

    #[test]
    fn saved_cameras_load_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            "[[cameras]]\nid = \"fixe\"\nmain = { url = \"rtsp://10.0.0.1/a\" }\n",
        )
        .expect("write");
        let text = "[[cameras]]\nid = \"jardin\"\nname = \"Jardin \\\"nord\\\"\"\nrecord = { enabled = false }\n\
                    main = { url = \"rtsp://10.0.0.2/main\", username = \"admin\", password = \"p@ss w=rd\" }\n\
                    sub = { url = \"rtsp://10.0.0.2/sub\", transport = \"udp\" }\n\
                    onvif = { port = 8000, username = \"ptz\", password = \"p&z\" }\n";
        let mut parsed = Config::from_toml_with_env(text, |_| None).expect("valid");
        parsed.cameras[0].managed = true;
        let managed = dir.path().join(MANAGED_CAMERAS_FILE);
        save(&managed, &parsed.cameras).expect("save");

        let loaded = Config::load_with_env(&config, |_| None).expect("load");
        let cam = loaded
            .cameras
            .iter()
            .find(|c| c.id.as_str() == "jardin")
            .expect("camera");
        assert!(cam.managed);
        assert_eq!(cam.name.as_deref(), Some("Jardin \"nord\""));
        assert!(!cam.record.enabled);
        assert_eq!(
            cam.main.password.as_ref().map(|p| p.expose()),
            Some("p@ss w=rd")
        );
        assert_eq!(
            cam.sub.as_ref().map(|s| s.transport),
            Some(RtspTransport::Udp)
        );
        let onvif = cam
            .onvif_target(|_| None)
            .expect("onvif")
            .expect("configured");
        assert_eq!(
            onvif.device_url,
            "http://10.0.0.2:8000/onvif/device_service"
        );
        let (user, password) = onvif.credentials.expect("credentials");
        assert_eq!((user.as_str(), password.expose()), ("ptz", "p&z"));
        assert!(
            std::fs::read_to_string(&managed)
                .expect("read")
                .starts_with("# Caméras")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&managed)
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
