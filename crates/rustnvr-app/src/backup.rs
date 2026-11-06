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
    /// Each file, in the order of [`FILES`].
    pub files: Vec<(String, FileState)>,
    /// Cameras of the configuration once everything is restored (id, name).
    pub cameras: Vec<(String, String)>,
    /// Cameras whose detection settings are in the backup.
    pub motion: Vec<String>,
    /// Why the restored configuration would not load (empty when it would).
    pub problems: Vec<String>,
}

/// Compares `backup` with the configuration `config_file` and checks that
/// the result of restoring every file would load. Blocking.
pub fn preview(config_file: &Path, backup: &Backup) -> Preview {
    let mut files = Vec::new();
    for name in FILES {
        let current = std::fs::read_to_string(path_of(config_file, name)).ok();
        let state = match (backup.files.get(name), current) {
            (Some(b), Some(c)) if *b == c => FileState::Same,
            (Some(_), Some(_)) => FileState::Changed,
            (Some(_), None) => FileState::New,
            (None, Some(_)) => FileState::NotInBackup,
            (None, None) => continue,
        };
        files.push((name.to_owned(), state));
    }
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

/// Writes back the files `chosen` of `backup`, once the resulting
/// configuration is known to load; the configuration in use changes at the
/// next start. Returns the names written. Blocking.
pub fn restore(
    config_file: &Path,
    backup: &Backup,
    chosen: &[String],
) -> Result<Vec<String>, String> {
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
        let p = preview(&config, &backup);
        assert_eq!(
            p.files,
            [
                (CONFIG_FILE.to_owned(), FileState::Changed),
                (MANAGED_MQTT_FILE.to_owned(), FileState::NotInBackup),
                (MANAGED_WEBHOOKS_FILE.to_owned(), FileState::New),
            ]
        );
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        assert_eq!(p.cameras, [("jardin".to_owned(), "Jardin".to_owned())]);
        // Only the webhooks: the renamed camera stays.
        let written =
            restore(&config, &backup, &[MANAGED_WEBHOOKS_FILE.to_owned()]).expect("restore");
        assert_eq!(written, [MANAGED_WEBHOOKS_FILE]);
        assert!(
            std::fs::read_to_string(&config)
                .expect("read")
                .contains("Cour")
        );
        assert!(dir.path().join(MANAGED_WEBHOOKS_FILE).is_file());
        assert!(dir.path().join(MANAGED_MQTT_FILE).is_file(), "kept");
        // Nothing else outside of the known files, no scratch folder left.
        assert!(restore(&config, &backup, &["../evil".to_owned()]).is_err());
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
        assert!(!preview(&config, &broken).problems.is_empty());
        assert!(restore(&config, &broken, &[CONFIG_FILE.to_owned()]).is_err());
        assert!(
            std::fs::read_to_string(&config)
                .expect("read")
                .contains("Cour")
        );
    }
}
