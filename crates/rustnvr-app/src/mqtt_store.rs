//! MQTT settings entered in the web interface ([`MANAGED_MQTT_FILE`], next to
//! the configuration file; it replaces `[mqtt]` of the configuration).
//!
//! The file may contain the broker password, so it is written readable by
//! its owner only (mode 0600 on Unix), atomically.

use rustnvr_core::config::{MANAGED_MQTT_FILE, MqttConfig};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::Path;

const HEADER: &str = "# Réglages MQTT saisis dans l'interface web de RustNVR (fichier géré\n\
# automatiquement). Ils remplacent la section [mqtt] de config.toml.\n\
# Ce fichier peut contenir le mot de passe du broker : gardez-le privé.\n\n";

#[derive(Serialize)]
struct StoredFile<'a> {
    mqtt: Stored<'a>,
}

#[derive(Serialize)]
struct Stored<'a> {
    enabled: bool,
    host: &'a str,
    port: u16,
    tls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    ca_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password_env: Option<&'a str>,
    client_id: &'a str,
    topic_prefix: &'a str,
    discovery: bool,
    discovery_prefix: &'a str,
}

/// Writes `config` to `path`.
pub fn save(path: &Path, c: &MqttConfig) -> io::Result<()> {
    let file = StoredFile {
        mqtt: Stored {
            enabled: c.enabled,
            host: &c.host,
            port: c.port,
            tls: c.tls,
            ca_file: c.ca_file.as_ref().map(|p| p.display().to_string()),
            username: c.username.as_deref(),
            password: c.password.as_ref().map(|p| p.expose()),
            password_env: c.password_env.as_deref(),
            client_id: &c.client_id,
            topic_prefix: &c.topic_prefix,
            discovery: c.discovery,
            discovery_prefix: &c.discovery_prefix,
        },
    };
    let body = toml::to_string(&file).map_err(io::Error::other)?;
    let tmp = path.with_file_name(format!("{MANAGED_MQTT_FILE}.tmp"));
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp)?;
    f.write_all(HEADER.as_bytes())?;
    f.write_all(body.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::Secret;
    use rustnvr_core::config::Config;

    #[test]
    fn saved_settings_replace_the_configuration_section() {
        let dir = tempfile::tempdir().expect("tmp");
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "[mqtt]\nenabled = false\nhost = \"old\"\n").expect("write");
        let mqtt = MqttConfig {
            enabled: true,
            host: "192.168.1.10".into(),
            username: Some("ha".into()),
            password: Some(Secret::new("p\"w")),
            ..MqttConfig::default()
        };
        let path = dir.path().join(MANAGED_MQTT_FILE);
        save(&path, &mqtt).expect("save");
        let loaded = Config::load_with_env(&config, |_| None).expect("load");
        assert_eq!(loaded.mqtt, mqtt);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
