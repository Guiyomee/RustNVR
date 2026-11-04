//! Storage settings changed from the web interface
//! ([`MANAGED_STORAGE_FILE`], next to the configuration file): retention of
//! the local disk and the destinations added from the interface.

use rustnvr_core::config::{
    DestinationConfig, MANAGED_STORAGE_FILE, RemoteConfig, RetentionConfig,
};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::Path;

const HEADER: &str = "# Stockage configuré depuis l'interface web de RustNVR (fichier géré\n\
# automatiquement). Les réglages de config.toml ne sont pas concernés, sauf la\n\
# rétention du disque local, remplacée par la section [local] ci-dessous.\n\
# Il peut contenir des secrets S3 ou SFTP : gardez-le privé.\n\n";

#[derive(Serialize)]
struct StoredFile<'a> {
    local: StoredRetention,
    destinations: Vec<StoredDestination<'a>>,
}

#[derive(Serialize)]
struct StoredRetention {
    #[serde(skip_serializing_if = "Option::is_none")]
    retention_days: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_size_gb: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_free_gb: Option<u32>,
}

#[derive(Serialize)]
struct StoredDestination<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    path: String,
    require_mount: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    retention_days: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_size_gb: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_free_gb: Option<u32>,
    // Last: a sub-table.
    #[serde(skip_serializing_if = "Option::is_none")]
    remote: Option<StoredRemote<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum StoredRemote<'a> {
    S3 {
        #[serde(skip_serializing_if = "Option::is_none")]
        endpoint: Option<&'a str>,
        region: &'a str,
        bucket: &'a str,
        prefix: &'a str,
        access_key_id: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        secret_access_key: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        secret_access_key_env: Option<&'a str>,
    },
    Sftp {
        host: &'a str,
        port: u16,
        username: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        password: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        password_env: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        private_key: Option<String>,
        remote_dir: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        host_key: Option<&'a str>,
    },
}

fn stored_remote(r: &RemoteConfig) -> StoredRemote<'_> {
    match r {
        RemoteConfig::S3(s) => StoredRemote::S3 {
            endpoint: s.endpoint.as_deref(),
            region: &s.region,
            bucket: &s.bucket,
            prefix: &s.prefix,
            access_key_id: &s.access_key_id,
            secret_access_key: s.secret_access_key.as_ref().map(|k| k.expose()),
            secret_access_key_env: s.secret_access_key_env.as_deref(),
        },
        RemoteConfig::Sftp(s) => StoredRemote::Sftp {
            host: &s.host,
            port: s.port,
            username: &s.username,
            password: s.password.as_ref().map(|p| p.expose()),
            password_env: s.password_env.as_deref(),
            private_key: s.private_key.as_ref().map(|p| p.display().to_string()),
            remote_dir: &s.remote_dir,
            host_key: s.host_key.as_deref(),
        },
    }
}

/// Writes the local retention and the managed destinations to `path`.
pub fn save<'a>(
    path: &Path,
    local: RetentionConfig,
    destinations: impl IntoIterator<Item = &'a DestinationConfig>,
) -> io::Result<()> {
    let file = StoredFile {
        local: StoredRetention {
            retention_days: local.retention_days,
            max_size_gb: local.max_size_gb,
            min_free_gb: local.min_free_gb,
        },
        destinations: destinations
            .into_iter()
            .map(|d| StoredDestination {
                id: &d.id,
                name: d.name.as_deref(),
                path: d.path.display().to_string(),
                require_mount: d.require_mount,
                retention_days: d.retention_days,
                max_size_gb: d.max_size_gb,
                min_free_gb: d.min_free_gb,
                remote: d.remote.as_ref().map(stored_remote),
            })
            .collect(),
    };
    let body = toml::to_string(&file).map_err(io::Error::other)?;
    let tmp = path.with_file_name(format!("{MANAGED_STORAGE_FILE}.tmp"));
    // Readable by the owner only: it may hold S3 or SFTP secrets.
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
    use rustnvr_core::config::Config;

    #[test]
    fn saved_storage_loads_back() {
        let dir = tempfile::tempdir().expect("tmp");
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            "[storage]\nrecordings_dir = \"/rec\"\nretention_days = 3\n",
        )
        .expect("write");
        let nas = DestinationConfig {
            id: "nas".into(),
            name: Some("NAS \"salon\"".into()),
            path: "/mnt/nas/rustnvr".into(),
            require_mount: true,
            retention_days: Some(90),
            max_size_gb: None,
            min_free_gb: Some(20),
            remote: None,
            managed: true,
        };
        let managed = dir.path().join(MANAGED_STORAGE_FILE);
        let local = RetentionConfig {
            retention_days: Some(30),
            max_size_gb: Some(500),
            min_free_gb: Some(10),
        };
        save(&managed, local, [&nas]).expect("save");
        let c = Config::load_with_env(&config, |_| None).expect("load");
        assert_eq!(
            c.storage.local_retention(),
            local,
            "interface overrides config.toml"
        );
        let d = &c.storage.destinations[0];
        assert!(d.managed);
        assert_eq!(
            (d.id.as_str(), d.name.as_deref(), d.retention_days),
            ("nas", Some("NAS \"salon\""), Some(90))
        );
    }
}
