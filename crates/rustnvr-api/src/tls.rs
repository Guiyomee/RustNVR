//! Built-in HTTPS (rustls).
//!
//! The certificate comes from PEM files (`web.tls_cert`, `web.tls_key`), or is
//! generated once, self-signed, in the data directory (`web.tls_self_signed`).
//! A self-signed certificate encrypts the traffic but browsers warn about it
//! until it is accepted; its SHA-256 fingerprint is logged so that it can be
//! compared with the one the browser shows.

use axum::Router;
use axum::extract::ConnectInfo;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

/// A client must finish the TLS handshake within this delay.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections still open at shutdown are closed after this delay.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// SHA-256 fingerprint of a DER certificate, as browsers show it (`AA:BB:…`).
pub fn fingerprint(der: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Paths of the self-signed certificate kept in `dir` for `names` (host
/// names and addresses): created on first use, kept while it covers every
/// name (they are listed in `names.txt` next to it), made again otherwise
/// (browsers then ask to accept the new one). Blocking.
pub fn self_signed(dir: &Path, names: Vec<String>) -> Result<(PathBuf, PathBuf), String> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let names_path = dir.join("names.txt");
    let mut names: Vec<String> = names
        .into_iter()
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    names.dedup();
    if cert_path.is_file() && key_path.is_file() {
        // Without the list (made by an older version): kept as it is.
        let covered = std::fs::read_to_string(&names_path).map_or(true, |known| {
            let known: Vec<&str> = known.lines().collect();
            names.iter().all(|n| known.contains(&n.as_str()))
        });
        if covered {
            return Ok((cert_path, key_path));
        }
        tracing::info!(names = %names.join(", "), "new names: self-signed HTTPS certificate made again");
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let generated = rcgen::generate_simple_self_signed(names.clone()).map_err(|e| e.to_string())?;
    write_private(&key_path, generated.signing_key.serialize_pem().as_bytes())
        .map_err(|e| format!("{}: {e}", key_path.display()))?;
    std::fs::write(&cert_path, generated.cert.pem())
        .map_err(|e| format!("{}: {e}", cert_path.display()))?;
    std::fs::write(&names_path, names.join("\n"))
        .map_err(|e| format!("{}: {e}", names_path.display()))?;
    tracing::info!(cert = %cert_path.display(), names = %names.join(", "), "self-signed HTTPS certificate created");
    Ok((cert_path, key_path))
}

/// The answer to a plain HTTP request sent to the HTTPS port: a redirect to
/// the same address over HTTPS (old bookmarks, `http://` links). `None` if
/// the request cannot be read.
fn https_redirect(head: &str) -> Option<String> {
    let mut lines = head.lines();
    let target = lines.next()?.split_whitespace().nth(1)?;
    let host = lines.find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("host").then(|| v.trim())
    })?;
    // Nothing that could break out of the header.
    let safe = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_graphic());
    if !safe(host) || !safe(target) || !target.starts_with('/') {
        return None;
    }
    Some(format!(
        "HTTP/1.1 308 Permanent Redirect\r\nLocation: https://{host}{target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    ))
}

/// A connection that sent plain HTTP instead of a TLS handshake: read its
/// request and redirect it to HTTPS.
async fn redirect_plain_http(mut tcp: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    let read = async {
        while head.len() < 8192 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match tcp.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
        }
    };
    if tokio::time::timeout(HANDSHAKE_TIMEOUT, read).await.is_err() {
        return;
    }
    if let Some(answer) = https_redirect(&String::from_utf8_lossy(&head)) {
        let _ = tcp.write_all(answer.as_bytes()).await;
        let _ = tcp.shutdown().await;
    }
}

/// Writes a private key readable by its owner only.
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// TLS configuration from PEM files; also returns the certificate fingerprint.
pub fn server_config(cert: &Path, key: &Path) -> Result<(Arc<ServerConfig>, String), String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .map_err(|e| format!("web.tls_cert {}: {e}", cert.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("web.tls_cert {}: {e}", cert.display()))?;
    let first = certs
        .first()
        .ok_or_else(|| format!("web.tls_cert {}: no certificate", cert.display()))?;
    let print = fingerprint(first);
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("web.tls_key {}: {e}", key.display()))?;
    let mut config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| format!("certificat ou clé HTTPS invalide : {e}"))?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok((Arc::new(config), print))
}

/// Serves `app` over TLS until `shutdown`.
pub(crate) async fn serve(
    listener: tokio::net::TcpListener,
    app: Router,
    config: Arc<ServerConfig>,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let acceptor = TlsAcceptor::from(config);
    let mut connections = JoinSet::new();
    loop {
        let (tcp, peer) = tokio::select! {
            _ = shutdown.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!(error = %e, "accept failed");
                    continue;
                }
            },
        };
        // Finished connections are reaped as new ones arrive.
        while connections.try_join_next().is_some() {}
        let acceptor = acceptor.clone();
        let app = app.clone();
        connections.spawn(async move {
            // A TLS handshake starts with 0x16; anything else is plain HTTP.
            let mut first = [0u8; 1];
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, tcp.peek(&mut first)).await {
                Ok(Ok(1)) if first[0] != 0x16 => {
                    redirect_plain_http(tcp).await;
                    return;
                }
                Ok(Ok(1)) => {}
                _ => return,
            }
            let tls = match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    tracing::debug!(client = %peer, error = %e, "TLS handshake failed");
                    return;
                }
                Err(_) => return,
            };
            // Same request extension as `into_make_service_with_connect_info`.
            let service =
                app.map_request(move |mut req: axum::http::Request<hyper::body::Incoming>| {
                    req.extensions_mut().insert(ConnectInfo(peer));
                    req
                });
            let builder = Builder::new(TokioExecutor::new());
            if let Err(e) = builder
                .serve_connection_with_upgrades(
                    TokioIo::new(tls),
                    TowerToHyperService::new(service),
                )
                .await
            {
                tracing::debug!(client = %peer, error = %e, "HTTPS connection ended");
            }
        });
    }
    // WebSockets close themselves on shutdown; give requests a moment.
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    connections.abort_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_loads_a_self_signed_certificate() {
        let dir = tempfile::tempdir().expect("tmp");
        let (cert, key) = self_signed(dir.path(), vec!["localhost".into(), "127.0.0.1".into()])
            .expect("generate");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&key).expect("meta").permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "private key readable by the owner only"
            );
        }
        let (_, print) = server_config(&cert, &key).expect("config");
        assert_eq!(print.len(), 32 * 3 - 1);
        // Kept on the next start, also for fewer names.
        let again = self_signed(dir.path(), vec!["127.0.0.1".into()]).expect("reuse");
        assert_eq!(again.0, cert);
        assert_eq!(server_config(&again.0, &again.1).expect("config").1, print);
        // A new name (the machine's network address): made again.
        let (cert, key) = self_signed(
            dir.path(),
            vec![
                "localhost".into(),
                "127.0.0.1".into(),
                "192.168.1.201".into(),
            ],
        )
        .expect("again");
        assert_ne!(server_config(&cert, &key).expect("config").1, print);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("names.txt")).expect("names"),
            "127.0.0.1\n192.168.1.201\nlocalhost"
        );
    }

    #[test]
    fn plain_http_is_sent_to_https() {
        assert_eq!(
            https_redirect(
                "GET /cameras?x=1 HTTP/1.1\r\nHost: 192.168.1.201:8080\r\nUser-Agent: a\r\n\r\n"
            )
            .as_deref(),
            Some(
                "HTTP/1.1 308 Permanent Redirect\r\nLocation: https://192.168.1.201:8080/cameras?x=1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
        );
        assert_eq!(https_redirect("GET / HTTP/1.1\r\n\r\n"), None, "no Host");
        assert_eq!(
            https_redirect("GET http://evil/ HTTP/1.1\r\nHost: a\r\n\r\n"),
            None
        );
        assert_eq!(https_redirect(""), None);
    }

    #[tokio::test]
    async fn the_https_port_redirects_plain_http() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let dir = tempfile::tempdir().expect("tmp");
        let (cert, key) = self_signed(dir.path(), vec!["localhost".into()]).expect("cert");
        let (config, _) = server_config(&cert, &key).expect("config");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let shutdown = CancellationToken::new();
        let server = tokio::spawn(serve(listener, Router::new(), config, shutdown.clone()));
        let mut tcp = tokio::net::TcpStream::connect(addr).await.expect("connect");
        tcp.write_all(format!("GET /live HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes())
            .await
            .expect("write");
        let mut answer = String::new();
        tcp.read_to_string(&mut answer).await.expect("read");
        assert!(answer.starts_with("HTTP/1.1 308"), "{answer}");
        assert!(
            answer.contains(&format!("Location: https://{addr}/live")),
            "{answer}"
        );
        shutdown.cancel();
        let _ = server.await;
    }
}
