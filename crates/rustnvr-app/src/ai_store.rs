//! Object recognition settings entered in the web interface
//! ([`MANAGED_AI_FILE`], next to the configuration file; it replaces `[ai]`
//! of the configuration). Written atomically, readable by its owner only.

use rustnvr_core::config::{AiConfig, MANAGED_AI_FILE};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::Path;

const HEADER: &str = "# Réglages de reconnaissance d'objets saisis dans l'interface web de RustNVR\n\
# (fichier géré automatiquement). Ils remplacent la section [ai] de config.toml.\n\n";

#[derive(Serialize)]
struct StoredFile<'a> {
    ai: &'a AiConfig,
}

/// Writes `settings` to `path`.
pub fn save(path: &Path, settings: &AiConfig) -> io::Result<()> {
    let body = toml::to_string(&StoredFile { ai: settings }).map_err(io::Error::other)?;
    let tmp = path.with_file_name(format!("{MANAGED_AI_FILE}.tmp"));
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
    use rustnvr_core::config::{AiProviderConfig, Config};

    #[test]
    fn saved_settings_load_back() {
        let dir = tempfile::tempdir().expect("tmp");
        let model = dir.path().join("yolox_nano.onnx");
        std::fs::write(&model, b"onnx").expect("model");
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").expect("config");
        let settings = AiConfig {
            provider: Some(AiProviderConfig::Onnx {
                model: model.clone(),
            }),
            min_score: 0.45,
            labels: vec!["person".into(), "car".into()],
            night: Some(rustnvr_core::config::AiNight {
                min_score: 0.8,
                when: rustnvr_core::config::AiNightWhen::NightVision,
                from: "21:30".into(),
                to: "06:15".into(),
            }),
            ..AiConfig::default()
        };
        save(&dir.path().join(MANAGED_AI_FILE), &settings).expect("save");
        let loaded = Config::load_with_env(&config_path, |_| None).expect("load");
        assert_eq!(loaded.ai, settings);
        // Without a provider: recognition off, the rest kept.
        let off = AiConfig {
            provider: None,
            ..settings
        };
        save(&dir.path().join(MANAGED_AI_FILE), &off).expect("save");
        let loaded = Config::load_with_env(&config_path, |_| None).expect("load");
        assert_eq!(loaded.ai, off);
    }
}
