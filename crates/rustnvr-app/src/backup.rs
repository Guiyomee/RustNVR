//! Backup and restore of the configuration: the configuration file, the files
//! written by the web interface (`*.ui.toml`) and the detection settings of
//! the cameras (zones, lines, masks), in one file encrypted with a passphrase.
//!
//! These files hold the passwords of the cameras, MQTT and storage: the whole
//! backup is encrypted (key derived from the passphrase with Argon2id,
//! ChaCha20-Poly1305), never only some fields. Recordings, events and accounts
//! are not part of it.
//!
//! File layout: [`MAGIC`], a 16-byte salt, a 12-byte nonce, then the
//! encrypted JSON of [`Backup`] followed by its 16-byte authentication tag.

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use rustnvr_core::MotionSettings;
use rustnvr_core::config::{
    Config, MANAGED_AI_FILE, MANAGED_CAMERAS_FILE, MANAGED_MQTT_FILE, MANAGED_SETTINGS_FILE,
    MANAGED_STORAGE_FILE, MANAGED_UPDATES_FILE, MANAGED_WEBHOOKS_FILE,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Start of every backup file.
const MAGIC: &[u8; 16] = b"RUSTNVR-BACKUP1\n";
const SALT_LEN: usize = 16;
const TAG_LEN: usize = 16;
/// Shortest passphrase accepted for a new backup.
pub const MIN_PASSPHRASE: usize = 8;

/// Name under which the configuration file is stored, whatever its real name.
pub const CONFIG_FILE: &str = "config.toml";
/// Files of a backup, in this order; nothing else is ever written back.
pub const FILES: [&str; 8] = [
    CONFIG_FILE,
    MANAGED_CAMERAS_FILE,
    MANAGED_STORAGE_FILE,
    MANAGED_SETTINGS_FILE,
    MANAGED_MQTT_FILE,
    MANAGED_AI_FILE,
    MANAGED_WEBHOOKS_FILE,
    MANAGED_UPDATES_FILE,
];

/// Content of a backup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Backup {
    /// Format version.
    pub format: u32,
    /// When it was made, in milliseconds since the Unix epoch.
    pub created_ms: i64,
    /// Version of RustNVR that made it.
    pub rustnvr: String,
    /// Content of each file of [`FILES`] that existed.
    pub files: BTreeMap<String, String>,
    /// Detection settings, by camera.
    pub motion: BTreeMap<String, MotionSettings>,
}

/// Why a backup cannot be read.
#[derive(Debug, PartialEq, Eq)]
pub enum BackupError {
    /// Not a RustNVR backup file.
    NotABackup,
    /// Wrong passphrase, or damaged file.
    Undecryptable,
    /// Encryption failed (should not happen).
    Crypto,
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BackupError::NotABackup => "ce fichier n'est pas une sauvegarde RustNVR",
            BackupError::Undecryptable => "phrase secrète incorrecte, ou fichier abîmé",
            BackupError::Crypto => "échec du chiffrement",
        })
    }
}

fn key(passphrase: &str, salt: &[u8]) -> Result<LessSafeKey, BackupError> {
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| BackupError::Crypto)?;
    let unbound = UnboundKey::new(&CHACHA20_POLY1305, &key).map_err(|_| BackupError::Crypto)?;
    Ok(LessSafeKey::new(unbound))
}

/// Encrypts `backup` with `passphrase`. Slow on purpose (Argon2id).
pub fn seal(backup: &Backup, passphrase: &str) -> Result<Vec<u8>, BackupError> {
    let rng = SystemRandom::new();
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    rng.fill(&mut salt).map_err(|_| BackupError::Crypto)?;
    rng.fill(&mut nonce).map_err(|_| BackupError::Crypto)?;
    let mut body = serde_json::to_vec(backup).map_err(|_| BackupError::Crypto)?;
    key(passphrase, &salt)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(MAGIC),
            &mut body,
        )
        .map_err(|_| BackupError::Crypto)?;
    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decrypts a backup made by [`seal`].
pub fn open(data: &[u8], passphrase: &str) -> Result<Backup, BackupError> {
    let rest = data
        .strip_prefix(MAGIC.as_slice())
        .ok_or(BackupError::NotABackup)?;
    if rest.len() < SALT_LEN + NONCE_LEN + TAG_LEN {
        return Err(BackupError::NotABackup);
    }
    let (salt, rest) = rest.split_at(SALT_LEN);
    let (nonce, body) = rest.split_at(NONCE_LEN);
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| BackupError::NotABackup)?;
    let mut body = body.to_vec();
    let plain = key(passphrase, salt)?
        .open_in_place(nonce, Aad::from(MAGIC), &mut body)
        .map_err(|_| BackupError::Undecryptable)?;
    let backup: Backup = serde_json::from_slice(plain).map_err(|_| BackupError::Undecryptable)?;
    if backup.format != 1 {
        return Err(BackupError::NotABackup);
    }
    Ok(backup)
}

/// Where file `name` of a backup lives for the configuration `config_file`.
pub fn path_of(config_file: &Path, name: &str) -> PathBuf {
    if name == CONFIG_FILE {
        config_file.to_owned()
    } else {
        config_file.with_file_name(name)
    }
}

/// Gathers the files of the configuration `config_file` and the detection
/// settings `motion`. Blocking.
pub fn collect(
    config_file: &Path,
    motion: BTreeMap<String, MotionSettings>,
    now_ms: i64,
) -> io::Result<Backup> {
    let mut files = BTreeMap::new();
    for name in FILES {
        match std::fs::read_to_string(path_of(config_file, name)) {
            Ok(text) => {
                files.insert(name.to_owned(), text);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound && name != CONFIG_FILE => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Backup {
        format: 1,
        created_ms: now_ms,
        rustnvr: env!("CARGO_PKG_VERSION").to_owned(),
        files,
        motion,
    })
}

/// How a file of the backup compares with the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    /// Same content.
    Same,
    /// Different content.
    Changed,
    /// In the backup, not here.
    New,
    /// Here, not in the backup: kept as it is.
    NotInBackup,
}

/// What restoring would do.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Preview {
    /// The backup itself.
    pub created_ms: i64,
    /// Version that made it.
    pub rustnvr: String,
    /// Each file, in the order of [`FILES`], with why restoring it alone
    /// would not load.
    pub files: Vec<(String, FileState, Option<String>)>,
    /// Cameras of the configuration once everything is restored (id, name).
    pub cameras: Vec<(String, String)>,
    /// Cameras whose detection settings are in the backup.
    pub motion: Vec<String>,
    /// Why the restored configuration would not load (empty when it would).
    pub problems: Vec<String>,
}

/// Compares `backup` with the configuration `config_file` and checks, file
/// by file and all together, that the result would load (paths adapted with
/// [`adapt`] to the models of `models_dir`). Blocking.
pub fn preview(config_file: &Path, backup: &Backup, models_dir: &Path) -> Preview {
    let backup = &adapt(backup, models_dir);
    let mut states = Vec::new();
    for name in FILES {
        let current = std::fs::read_to_string(path_of(config_file, name)).ok();
        let state = match (backup.files.get(name), current) {
            (Some(b), Some(c)) if *b == c => FileState::Same,
            (Some(_), Some(_)) => FileState::Changed,
            (Some(_), None) => FileState::New,
            (None, Some(_)) => FileState::NotInBackup,
            (None, None) => continue,
        };
        states.push((name.to_owned(), state));
    }
    // The selection offered: every file that differs, but config.toml (its
    // folders and port belong to the installation). The files depend on each
    // other (cameras on the destinations of storage.ui.toml…), so they are
    // checked together: a file is blamed for the problems that go away
    // without it, config.toml for those it brings.
    let restorable = |s: &FileState| matches!(s, FileState::Changed | FileState::New);
    let offered: Vec<String> = states
        .iter()
        .filter(|(n, s)| restorable(s) && n != CONFIG_FILE)
        .map(|(n, _)| n.clone())
        .collect();
    let offered_problems = problem_lines(check(config_file, backup, &offered).err());
    let files = states
        .into_iter()
        .map(|(name, state)| {
            let own: Vec<String> = if !restorable(&state) {
                Vec::new()
            } else if name == CONFIG_FILE {
                let mut with = offered.clone();
                with.push(name.clone());
                problem_lines(check(config_file, backup, &with).err())
                    .into_iter()
                    .filter(|p| !offered_problems.contains(p))
                    .collect()
            } else if offered_problems.is_empty() {
                Vec::new()
            } else {
                let without: Vec<String> =
                    offered.iter().filter(|n| **n != name).cloned().collect();
                let left = problem_lines(check(config_file, backup, &without).err());
                offered_problems
                    .iter()
                    .filter(|p| !left.contains(p))
                    .cloned()
                    .collect()
            };
            let problem = (!own.is_empty()).then(|| explain(&own.join("\n"), models_dir));
            (name, state, problem)
        })
        .collect();
    let chosen: Vec<String> = backup.files.keys().cloned().collect();
    let (cameras, problems) = match check(config_file, backup, &chosen) {
        Ok(c) => (
            c.cameras
                .iter()
                .map(|c| {
                    let id = c.id.as_str().to_owned();
                    (id.clone(), c.name.clone().unwrap_or(id))
                })
                .collect(),
            Vec::new(),
        ),
        Err(e) => (Vec::new(), vec![e]),
    };
    Preview {
        created_ms: backup.created_ms,
        rustnvr: backup.rustnvr.clone(),
        files,
        cameras,
        motion: backup.motion.keys().cloned().collect(),
        problems,
    }
}

/// The problems of a configuration error, one per line (the list of
/// `invalid configuration:` or the whole message).
fn problem_lines(error: Option<String>) -> Vec<String> {
    let Some(error) = error else {
        return Vec::new();
    };
    let items: Vec<String> = error
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(str::to_owned)
        .collect();
    if items.is_empty() { vec![error] } else { items }
}

/// `problems` with what to do about a model not downloaded here.
fn explain(problems: &str, models_dir: &Path) -> String {
    let missing: Vec<String> = problems
        .split("no model file at ")
        .skip(1)
        .filter_map(|rest| {
            let path = rest.split_whitespace().next()?;
            let name = Path::new(path).file_name()?.to_string_lossy().into_owned();
            (!models_dir.join(&name).is_file()).then_some(name)
        })
        .collect();
    if missing.is_empty() {
        problems.to_owned()
    } else {
        format!(
            "{problems}\nModèle à télécharger d'abord ici (Paramètres → IA) : {}, puis analysez de nouveau.",
            missing.join(", ")
        )
    }
}

/// Loads the configuration as it would be once the files `chosen` of
/// `backup` are restored (the others as they are now), in a scratch folder.
fn check(config_file: &Path, backup: &Backup, chosen: &[String]) -> Result<Config, String> {
    let scratch = scratch_dir(config_file).map_err(|e| format!("dossier temporaire : {e}"))?;
    let result = (|| {
        for name in FILES {
            let text = match backup
                .files
                .get(name)
                .filter(|_| chosen.iter().any(|c| c == name))
            {
                Some(t) => Some(t.clone()),
                None => std::fs::read_to_string(path_of(config_file, name)).ok(),
            };
            if let Some(text) = text {
                write_private(&scratch.join(name), &text).map_err(|e| e.to_string())?;
            }
        }
        Config::load(&scratch.join(CONFIG_FILE)).map_err(|e| format!("{e:#}"))
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// A new private folder next to the configuration (same disk, same rights).
fn scratch_dir(config_file: &Path) -> io::Result<PathBuf> {
    let parent = config_file.parent().unwrap_or(Path::new("."));
    let dir = parent.join(format!(".rustnvr-restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Writes `text` to `path` atomically, readable by its owner only.
fn write_private(path: &Path, text: &str) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// `backup` with the paths of another installation made to fit this one: a
/// model file of `ai.ui.toml` (`…/models/yolox_s.onnx`) that is not here but
/// is in `models_dir` (a Docker backup restored in a systemd installation,
/// for instance) points to the one of `models_dir`.
pub fn adapt(backup: &Backup, models_dir: &Path) -> Backup {
    let mut out = backup.clone();
    if let Some(text) = out.files.get_mut(MANAGED_AI_FILE) {
        let quoted: Vec<String> = text
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|s| s.ends_with(".onnx"))
            .map(str::to_owned)
            .collect();
        for old in quoted {
            let Some(name) = Path::new(&old).file_name() else {
                continue;
            };
            let here = models_dir.join(name);
            if !Path::new(&old).exists() && here.is_file() {
                *text = text.replace(&format!("\"{old}\""), &format!("\"{}\"", here.display()));
            }
        }
    }
    out
}

/// Writes back the files `chosen` of `backup`, once the resulting
/// configuration is known to load; the configuration in use changes at the
/// next start. Returns the names written. Blocking.
pub fn restore(
    config_file: &Path,
    backup: &Backup,
    chosen: &[String],
    models_dir: &Path,
) -> Result<Vec<String>, String> {
    let backup = &adapt(backup, models_dir);
    if let Some(bad) = chosen.iter().find(|c| !FILES.contains(&c.as_str())) {
        return Err(format!("fichier inconnu : {bad}"));
    }
    check(config_file, backup, chosen)
        .map_err(|e| format!("la configuration restaurée ne se charge pas : {e}"))?;
    let mut written = Vec::new();
    for name in FILES {
        if !chosen.iter().any(|c| c == name) {
            continue;
        }
        if let Some(text) = backup.files.get(name) {
            write_private(&path_of(config_file, name), text)
                .map_err(|e| format!("{name} : {e}"))?;
            written.push(name.to_owned());
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "[storage]\nrecordings_dir = \"/tmp/rec\"\n\n[[cameras]]\nid = \"jardin\"\nname = \"Jardin\"\nmain = { url = \"rtsp://10.0.0.2/live\" }\n";

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tmp");
        let config = dir.path().join("nvr.toml");
        std::fs::write(&config, CONFIG).expect("config");
        std::fs::write(
            dir.path().join(MANAGED_WEBHOOKS_FILE),
            "[[webhooks]]\nname = \"n8n\"\nurl = \"https://n8n.local/hook?token=s3cret\"\n",
        )
        .expect("webhooks");
        (dir, config)
    }

    #[test]
    fn sealed_backups_open_with_their_passphrase_only() {
        let (_dir, config) = setup();
        let mut motion = BTreeMap::new();
        motion.insert("jardin".to_owned(), MotionSettings::default());
        let backup = collect(&config, motion, 42).expect("collect");
        assert_eq!(
            backup.files.keys().collect::<Vec<_>>(),
            [CONFIG_FILE, MANAGED_WEBHOOKS_FILE]
        );
        let sealed = seal(&backup, "une phrase secrète").expect("seal");
        // Nothing readable inside: the token stays hidden.
        assert!(!sealed.windows(6).any(|w| w == b"s3cret"));
        assert_eq!(open(&sealed, "une phrase secrète"), Ok(backup));
        assert_eq!(
            open(&sealed, "une autre phrase"),
            Err(BackupError::Undecryptable)
        );
        let mut damaged = sealed.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        assert_eq!(
            open(&damaged, "une phrase secrète"),
            Err(BackupError::Undecryptable)
        );
        assert_eq!(open(b"[storage]\n", "x"), Err(BackupError::NotABackup));
    }

    #[test]
    fn previews_then_restores_only_what_is_chosen_and_valid() {
        let (dir, config) = setup();
        let backup = collect(&config, BTreeMap::new(), 1).expect("collect");
        // Since then: a camera renamed, the webhooks removed, MQTT added.
        std::fs::write(&config, CONFIG.replace("Jardin", "Cour")).expect("edit");
        std::fs::remove_file(dir.path().join(MANAGED_WEBHOOKS_FILE)).expect("rm");
        std::fs::write(
            dir.path().join(MANAGED_MQTT_FILE),
            "[mqtt]\nhost = \"broker\"\n",
        )
        .expect("mqtt");
        let models = dir.path().join("models");
        let p = preview(&config, &backup, &models);
        assert_eq!(
            p.files,
            [
                (CONFIG_FILE.to_owned(), FileState::Changed, None),
                (MANAGED_MQTT_FILE.to_owned(), FileState::NotInBackup, None),
                (MANAGED_WEBHOOKS_FILE.to_owned(), FileState::New, None),
            ]
        );
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        assert_eq!(p.cameras, [("jardin".to_owned(), "Jardin".to_owned())]);
        // Only the webhooks: the renamed camera stays.
        let written = restore(
            &config,
            &backup,
            &[MANAGED_WEBHOOKS_FILE.to_owned()],
            &models,
        )
        .expect("restore");
        assert_eq!(written, [MANAGED_WEBHOOKS_FILE]);
        assert!(
            std::fs::read_to_string(&config)
                .expect("read")
                .contains("Cour")
        );
        assert!(dir.path().join(MANAGED_WEBHOOKS_FILE).is_file());
        assert!(dir.path().join(MANAGED_MQTT_FILE).is_file(), "kept");
        // Nothing else outside of the known files, no scratch folder left.
        assert!(restore(&config, &backup, &["../evil".to_owned()], &models).is_err());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".rustnvr-restore") || n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        // A backup whose configuration does not load is refused, untouched.
        let mut broken = backup.clone();
        broken
            .files
            .insert(CONFIG_FILE.to_owned(), "[storage\n".to_owned());
        assert!(!preview(&config, &broken, &models).problems.is_empty());
        assert!(restore(&config, &broken, &[CONFIG_FILE.to_owned()], &models).is_err());
        assert!(
            std::fs::read_to_string(&config)
                .expect("read")
                .contains("Cour")
        );
    }

    #[test]
    fn a_backup_of_another_installation_is_adapted_and_checked_file_by_file() {
        let (dir, config) = setup();
        let models = dir.path().join("models");
        std::fs::create_dir(&models).expect("models");
        // A Docker backup: the model was in /data/rustnvr/models.
        let ai = "[ai]
provider = { type = \"onnx\", model = \"/data/rustnvr/models/yolox_l.onnx\" }
";
        let mut backup = collect(&config, BTreeMap::new(), 1).expect("collect");
        backup
            .files
            .insert(MANAGED_AI_FILE.to_owned(), ai.to_owned());
        // Not downloaded here yet: that file alone does not load, the
        // others are fine and can be restored.
        let p = preview(&config, &backup, &models);
        let ai_state = p.files.iter().find(|f| f.0 == MANAGED_AI_FILE).expect("ai");
        assert_eq!(ai_state.1, FileState::New);
        assert!(
            ai_state
                .2
                .as_deref()
                .is_some_and(|e| e.contains("yolox_l.onnx")),
            "{ai_state:?}"
        );
        assert!(restore(&config, &backup, &[MANAGED_AI_FILE.to_owned()], &models).is_err());
        assert!(
            restore(
                &config,
                &backup,
                &[MANAGED_WEBHOOKS_FILE.to_owned()],
                &models
            )
            .is_ok()
        );
        // Downloaded here: the path is made to point to it.
        std::fs::write(models.join("yolox_l.onnx"), b"model").expect("model");
        let here = models.join("yolox_l.onnx").display().to_string();
        let adapted = adapt(&backup, &models);
        assert!(adapted.files[MANAGED_AI_FILE].contains(&here));
        assert!(!adapted.files[MANAGED_AI_FILE].contains("/data/rustnvr"));
        let written = restore(&config, &backup, &[MANAGED_AI_FILE.to_owned()], &models)
            .expect("restored once the model is here");
        assert_eq!(written, [MANAGED_AI_FILE]);
        let saved = std::fs::read_to_string(dir.path().join(MANAGED_AI_FILE)).expect("saved");
        assert!(saved.contains(&here), "{saved}");
    }

    #[test]
    fn files_that_need_each_other_are_checked_together() {
        let (_dir, config) = setup();
        let models = config.with_file_name("models");
        // Cameras recording to a destination of storage.ui.toml, and a model
        // not downloaded here.
        let mut backup = collect(&config, BTreeMap::new(), 1).expect("collect");
        for (name, text) in [
            (
                MANAGED_STORAGE_FILE,
                "[[destinations]]\nid = \"nas\"\npath = \"/mnt/nas/rustnvr\"\n",
            ),
            (
                MANAGED_CAMERAS_FILE,
                "[[cameras]]\nid = \"rue\"\nmain = { url = \"rtsp://10.0.0.3/live\" }\nrecord = { destinations = [\"nas\"] }\n",
            ),
            (
                MANAGED_AI_FILE,
                "[ai]\nprovider = { type = \"onnx\", model = \"/data/rustnvr/models/yolox_l.onnx\" }\n",
            ),
        ] {
            backup.files.insert(name.to_owned(), text.to_owned());
        }
        let p = preview(&config, &backup, &models);
        let problem = |name: &str| {
            p.files
                .iter()
                .find(|f| f.0 == name)
                .and_then(|f| f.2.clone())
        };
        // Restored with the storage, the cameras load: nothing to say.
        assert_eq!(problem(MANAGED_CAMERAS_FILE), None);
        assert_eq!(problem(MANAGED_STORAGE_FILE), None);
        let ai = problem(MANAGED_AI_FILE).expect("model missing");
        assert!(
            ai.contains("yolox_l.onnx") && ai.contains("Paramètres → IA"),
            "{ai}"
        );
        assert!(!ai.contains("rue"), "{ai}");
        let both = [
            MANAGED_STORAGE_FILE.to_owned(),
            MANAGED_CAMERAS_FILE.to_owned(),
        ];
        // The cameras without their destination are still refused.
        assert!(restore(&config, &backup, &both[1..], &models).is_err());
        assert!(restore(&config, &backup, &both, &models).is_ok());
    }
}
