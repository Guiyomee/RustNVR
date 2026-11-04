//! SFTP server (any OpenSSH server, NAS with SSH enabled…).
//!
//! The server key is checked like `ssh` does: against the configured
//! fingerprint, or else against the key seen on the first connection, kept in
//! the outbox ([`KNOWN_HOST_FILE`]). A different key is refused (possible
//! interception), until the file is deleted or the fingerprint configured.

use crate::{SftpTarget, Sink, UploadStatus, join_key};
use russh::client::{self, Handle};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::watch;

/// File of the outbox keeping the server key fingerprint seen first.
pub const KNOWN_HOST_FILE: &str = ".rustnvr-known-host";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Fingerprint kept for an outbox, if any.
pub fn fingerprint_file(outbox: &Path) -> Option<String> {
    std::fs::read_to_string(outbox.join(KNOWN_HOST_FILE))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

struct Checker {
    expected: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            // Host certificates would need a certificate authority setting.
            return Ok(false);
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let accepted = self.expected.as_ref().is_none_or(|e| *e == fingerprint);
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(fingerprint);
        Ok(accepted)
    }
}

struct Connection {
    _ssh: Handle<Checker>,
    sftp: SftpSession,
}

pub(crate) struct SftpSink {
    target: SftpTarget,
    known_file: PathBuf,
    status: watch::Sender<UploadStatus>,
    conn: Option<Connection>,
}

impl SftpSink {
    pub(crate) fn new(
        target: SftpTarget,
        known_file: PathBuf,
        status: watch::Sender<UploadStatus>,
    ) -> Self {
        Self {
            target,
            known_file,
            status,
            conn: None,
        }
    }

    async fn connect(&mut self) -> Result<Connection, String> {
        let t = &self.target;
        let kept = std::fs::read_to_string(&self.known_file)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let expected = t.host_key.clone().or(kept.clone());
        let seen = Arc::new(Mutex::new(None));
        let checker = Checker {
            expected: expected.clone(),
            seen: seen.clone(),
        };
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        });
        let connected = tokio::time::timeout(
            CONNECT_TIMEOUT,
            client::connect(config, (t.host.as_str(), t.port), checker),
        )
        .await
        .map_err(|_| format!("SFTP : {}:{} ne répond pas", t.host, t.port))?;
        let fingerprint = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut ssh = match connected {
            Ok(h) => h,
            Err(e) => {
                return Err(match (&expected, &fingerprint) {
                    (Some(want), Some(got)) if want != got => format!(
                        "SFTP : clé du serveur inattendue ({got}, attendu {want}) : connexion refusée"
                    ),
                    _ => format!("SFTP : {e}"),
                });
            }
        };
        self.status
            .send_modify(|s| s.host_key.clone_from(&fingerprint));

        let auth = if let Some(password) = &t.password {
            ssh.authenticate_password(&t.username, password.expose())
                .await
                .map_err(|e| format!("SFTP : {e}"))?
        } else if let Some(path) = &t.private_key {
            let key = russh::keys::load_secret_key(path, None)
                .map_err(|e| format!("SFTP : clé {} : {e}", path.display()))?;
            // RSA keys sign with SHA-256 (SHA-1 is refused by current servers).
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), Some(HashAlg::Sha256));
            ssh.authenticate_publickey(&t.username, key)
                .await
                .map_err(|e| format!("SFTP : {e}"))?
        } else {
            return Err("SFTP : ni mot de passe ni clé privée configurés".to_owned());
        };
        if !auth.success() {
            return Err(format!(
                "SFTP : authentification de {:?} refusée",
                t.username
            ));
        }
        // First connection without a configured fingerprint: keep the key.
        if t.host_key.is_none()
            && kept.is_none()
            && let Some(fp) = &fingerprint
        {
            tracing::warn!(host = %t.host, fingerprint = %fp, "SFTP server key accepted on first connection and kept; it will be required from now on");
            let _ = std::fs::create_dir_all(self.known_file.parent().unwrap_or(Path::new(".")));
            let _ = std::fs::write(&self.known_file, format!("{fp}\n"));
        }
        let channel = ssh
            .channel_open_session()
            .await
            .map_err(|e| format!("SFTP : {e}"))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| format!("SFTP : {e}"))?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| format!("SFTP : {e}"))?;
        Ok(Connection { _ssh: ssh, sftp })
    }

    /// Creates every missing directory of `dir` (`a/b/c` or `/a/b/c`).
    async fn ensure_dir(sftp: &SftpSession, dir: &str) -> Result<(), String> {
        let mut current = if dir.starts_with('/') {
            String::from("/")
        } else {
            String::new()
        };
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            if !current.is_empty() && !current.ends_with('/') {
                current.push('/');
            }
            current.push_str(part);
            if !sftp
                .try_exists(current.as_str())
                .await
                .map_err(|e| e.to_string())?
            {
                sftp.create_dir(current.as_str())
                    .await
                    .map_err(|e| format!("SFTP : création de {current} : {e}"))?;
            }
        }
        Ok(())
    }

    async fn send(conn: &Connection, local: &Path, remote: &str) -> Result<(), String> {
        if let Some((dir, _)) = remote.rsplit_once('/') {
            Self::ensure_dir(&conn.sftp, dir).await?;
        }
        let data = tokio::fs::read(local)
            .await
            .map_err(|e| format!("{}: {e}", local.display()))?;
        // Written under a temporary name, then renamed: a reader never sees
        // a partial segment.
        let partial = format!("{remote}.part");
        let mut file = conn
            .sftp
            .create(partial.as_str())
            .await
            .map_err(|e| format!("SFTP : {partial} : {e}"))?;
        file.write_all(&data)
            .await
            .map_err(|e| format!("SFTP : écriture de {partial} : {e}"))?;
        file.shutdown()
            .await
            .map_err(|e| format!("SFTP : {partial} : {e}"))?;
        drop(file);
        if conn.sftp.try_exists(remote).await.unwrap_or(false) {
            let _ = conn.sftp.remove_file(remote).await;
        }
        conn.sftp
            .rename(partial.as_str(), remote)
            .await
            .map_err(|e| format!("SFTP : renommage de {partial} : {e}"))
    }
}

impl Sink for SftpSink {
    async fn upload(&mut self, local: &Path, key: &str) -> Result<(), String> {
        if self.conn.is_none() {
            self.conn = Some(self.connect().await?);
        }
        let remote = join_key(&self.target.remote_dir, key);
        let remote = if self.target.remote_dir.starts_with('/') {
            format!("/{remote}")
        } else {
            remote
        };
        let Some(conn) = &self.conn else {
            return Err("SFTP : non connecté".to_owned());
        };
        let result = Self::send(conn, local, &remote).await;
        if result.is_err() {
            // Reconnect for the next attempt.
            self.conn = None;
        }
        result
    }
}
