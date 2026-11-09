//! Check for new versions: the latest release published on the watched
//! repository (`[updates] repository`, or `updates.ui.toml`) is compared with
//! the running version. Nothing is downloaded nor installed: the interface
//! shows the release notes and how to update (Docker or systemd).

use super::Shared;
use rustnvr_api::ControlError;
use rustnvr_api::dto::UpdateStatus;
use rustnvr_core::config::{
    MANAGED_UPDATES_FILE, UpdatesConfig, is_newer_version, latest_release_api,
};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// First check after startup (the web interface is up by then).
const FIRST_CHECK: Duration = Duration::from_secs(30);
/// Then one check every…
const CHECK_INTERVAL: Duration = Duration::from_secs(12 * 3600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Longest release notes kept.
const MAX_NOTES: usize = 20_000;

/// The running version.
fn current() -> String {
    rustnvr_core::release_version().to_owned()
}

/// How this RustNVR runs, for the installation instructions.
pub(super) fn install_kind() -> &'static str {
    if Path::new("/.dockerenv").exists() {
        "docker"
    } else if std::env::var_os("INVOCATION_ID").is_some() {
        // Set by systemd for the processes of a service.
        "systemd"
    } else {
        "other"
    }
}

/// Latest release, as given by Gitea, Forgejo and GitHub.
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    published_at: Option<String>,
}

/// Asks the forge for the latest release of `config.repository`.
/// `Ok(None)`: nothing published yet.
async fn latest(config: &UpdatesConfig, repository: &str) -> Result<Option<Release>, String> {
    let api = latest_release_api(repository)?;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(format!("RustNVR/{}", rustnvr_core::release_version()))
        .build()
        .map_err(|e| e.to_string())?;
    let mut request = client.get(&api).header("Accept", "application/json");
    if let Some(name) = &config.token_env {
        let token = std::env::var(name)
            .map_err(|_| format!("variable d'environnement {name:?} absente"))?;
        let scheme = if api.starts_with("https://api.github.com/") {
            "Bearer"
        } else {
            "token"
        };
        request = request.header("Authorization", format!("{scheme} {token}"));
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("dépôt injoignable : {e}"))?;
    match response.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        401 | 403 => {
            return Err(
                "accès refusé par le dépôt (dépôt privé : jeton à donner par updates.token_env)"
                    .to_owned(),
            );
        }
        code => return Err(format!("réponse inattendue du dépôt ({code})")),
    }
    response
        .json::<Release>()
        .await
        .map(Some)
        .map_err(|e| format!("réponse illisible du dépôt : {e}"))
}

/// Checks the configured repository and records the result for the API.
pub(super) async fn check(shared: &Shared) -> UpdateStatus {
    // One check at a time (startup, timer, button).
    let _guard = shared.update_lock.lock().await;
    let config = shared
        .updates
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let checked_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok());
    let mut status = UpdateStatus {
        current: current(),
        repository: config.watched().map(str::to_owned),
        install: install_kind().to_owned(),
        ..UpdateStatus::default()
    };
    if let Some(repository) = config.watched() {
        status.checked_at_ms = checked_at_ms;
        match latest(&config, repository).await {
            Ok(Some(r)) => {
                status.available = is_newer_version(&r.tag_name, &status.current);
                if status.available {
                    info!(current = %status.current, latest = %r.tag_name, "new version published");
                } else {
                    debug!(latest = %r.tag_name, "no newer version");
                }
                status.latest = Some(r.tag_name);
                status.name = r.name.filter(|n| !n.trim().is_empty());
                status.notes = r.body.filter(|b| !b.trim().is_empty()).map(|mut b| {
                    if b.len() > MAX_NOTES {
                        let mut end = MAX_NOTES;
                        while !b.is_char_boundary(end) {
                            end -= 1;
                        }
                        b.truncate(end);
                        b.push_str("\n…");
                    }
                    b
                });
                status.url = r.html_url;
                status.published_at = r.published_at;
            }
            Ok(None) => debug!("no release published on the watched repository"),
            Err(e) => {
                warn!(error = %e, "cannot check for new versions");
                status.error = Some(e);
            }
        }
    }
    if let Some(api) = shared.api.get() {
        api.set_update_status(status.clone());
    }
    status
}

/// Checks shortly after startup, then every 12 hours.
pub(super) async fn run_checks(shared: Arc<Shared>, cancel: CancellationToken) {
    let mut wait = FIRST_CHECK;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
        wait = CHECK_INTERVAL;
        check(&shared).await;
    }
}

/// Turns the check on or off (the repository stays as configured) and
/// checks.
pub(super) async fn set(shared: &Arc<Shared>, on: bool) -> Result<UpdateStatus, ControlError> {
    let path = shared.managed_updates_file.clone().ok_or_else(|| {
        ControlError::Internal("no configuration file to store settings next to".into())
    })?;
    let next = {
        let mut config = shared.updates.lock().unwrap_or_else(|e| e.into_inner());
        config.check = on;
        config.clone()
    };
    save(&path, &next)
        .map_err(|e| ControlError::Internal(format!("cannot write {}: {e}", path.display())))?;
    info!(
        repository = next.watched().unwrap_or("-"),
        "update check settings changed"
    );
    Ok(check(shared).await)
}

const HEADER: &str = "# Vérification des nouvelles versions, réglée dans l'interface web de RustNVR\n\
# (fichier géré automatiquement). Elle remplace la section [updates] de config.toml.\n\n";

#[derive(Serialize)]
struct StoredFile<'a> {
    updates: &'a UpdatesConfig,
}

/// Writes `settings` to `path`, atomically.
fn save(path: &Path, settings: &UpdatesConfig) -> io::Result<()> {
    let body = toml::to_string(&StoredFile { updates: settings }).map_err(io::Error::other)?;
    let tmp = path.with_file_name(format!("{MANAGED_UPDATES_FILE}.tmp"));
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
    fn saved_settings_load_back() {
        let dir = tempfile::tempdir().expect("tmp");
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").expect("config");
        let settings = UpdatesConfig {
            check: true,
            repository: Some("https://gitea.example.org/me/RustNVR".into()),
            token_env: None,
        };
        save(&dir.path().join(MANAGED_UPDATES_FILE), &settings).expect("save");
        let loaded = Config::load_with_env(&config_path, |_| None).expect("load");
        assert_eq!(loaded.updates, settings);
        // No repository: the official one.
        save(
            &dir.path().join(MANAGED_UPDATES_FILE),
            &UpdatesConfig::default(),
        )
        .expect("save");
        let loaded = Config::load_with_env(&config_path, |_| None).expect("load");
        assert_eq!(loaded.updates, UpdatesConfig::default());
        assert_eq!(
            loaded.updates.watched(),
            Some(rustnvr_core::config::OFFICIAL_REPOSITORY)
        );
        // Turned off in the interface: nothing checked.
        let off = UpdatesConfig {
            check: false,
            ..UpdatesConfig::default()
        };
        save(&dir.path().join(MANAGED_UPDATES_FILE), &off).expect("save");
        let loaded = Config::load_with_env(&config_path, |_| None).expect("load");
        assert_eq!(loaded.updates.watched(), None);
        // A file of an earlier version (no `check`): checked.
        std::fs::write(
            dir.path().join(MANAGED_UPDATES_FILE),
            "[updates]
repository = \"https://gitea.example.org/me/RustNVR\"
",
        )
        .expect("write");
        let loaded = Config::load_with_env(&config_path, |_| None).expect("load");
        assert_eq!(
            loaded.updates.watched(),
            Some("https://gitea.example.org/me/RustNVR")
        );
    }

    #[test]
    fn reads_a_gitea_release() {
        let json = r#"{"id":1,"tag_name":"v2026.12.01","name":"RustNVR 2026.12.01","body":"- export\n- IA",
            "html_url":"https://gitea.example.org/me/RustNVR/releases/tag/v2026.12.01",
            "published_at":"2026-11-03T17:00:00Z","draft":false,"prerelease":false}"#;
        let r: Release = serde_json::from_str(json).expect("release");
        assert_eq!(r.tag_name, "v2026.12.01");
        assert!(is_newer_version(&r.tag_name, "2026.11.1"));
        assert_eq!(r.body.as_deref(), Some("- export\n- IA"));
    }
}
