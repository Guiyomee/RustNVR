//! Update asked from the interface, for an installation made by
//! `install.sh` (Debian, Proxmox LXC).
//!
//! RustNVR runs without root rights: it only writes the wanted version in
//! [`REQUEST`]. The root unit `rustnvr-update.path` sees the file and starts
//! `rustnvr-update.service`, which checks the version, downloads it from the
//! server written in `/etc/rustnvr/update.conf` (not chosen by RustNVR),
//! checks its SHA-256, installs it and restarts RustNVR. Its state and log go
//! to [`STATE_DIR`], a folder of root that RustNVR only reads. Docker has no
//! such unit: the interface shows the commands instead.

use crate::dto::InstallState;
use std::path::Path;

/// The request: a version (`v2026.11.10`), written by RustNVR.
const REQUEST: &str = "/var/lib/rustnvr/update-request";
/// State (`status`) and output (`log`) of the last update, written by root.
const STATE_DIR: &str = "/var/lib/rustnvr-update";
/// Present when `install.sh` set the update up.
const PATH_UNIT: &str = "/etc/systemd/system/rustnvr-update.path";
/// Lines of the log kept in the state of a failed update.
const LOG_LINES: usize = 8;

/// Whether an update can be asked from here: a service of `install.sh`.
pub(crate) fn available() -> bool {
    std::env::var_os("INVOCATION_ID").is_some()
        && !Path::new("/.dockerenv").exists()
        && Path::new(PATH_UNIT).exists()
}

/// Whether `tag` is a version tag (`v` then numbers and dots), the only
/// thing the request may hold.
pub(crate) fn valid_tag(tag: &str) -> bool {
    let Some(rest) = tag.strip_prefix('v') else {
        return false;
    };
    let parts: Vec<&str> = rest.split('.').collect();
    tag.len() <= 32
        && parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Asks root to install `tag`. Blocking (file system).
pub(crate) fn request(tag: &str) -> std::io::Result<()> {
    let tmp = format!("{REQUEST}.tmp");
    std::fs::write(&tmp, format!("{tag}\n"))?;
    std::fs::rename(&tmp, REQUEST)
}

/// Reads a state written by `rustnvr-update` (`key=value` lines) and the
/// end of its log.
fn parse_state(status: &str, log: &str) -> Option<InstallState> {
    let field = |key: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
    };
    let state = field("state")?.to_owned();
    let at_ms = field("at")
        .and_then(|s| s.parse::<i64>().ok())
        .map(|s| s * 1000);
    let message = (state == "failed").then(|| {
        let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(LOG_LINES)..].join("\n")
    });
    Some(InstallState {
        state,
        version: field("version")
            .filter(|v| !v.is_empty())
            .map(str::to_owned),
        at_ms,
        message,
    })
}

/// The state of the last update asked from the interface, if any. Blocking.
pub(crate) fn state() -> Option<InstallState> {
    let dir = Path::new(STATE_DIR);
    let status = std::fs::read_to_string(dir.join("status")).ok()?;
    let log = std::fs::read_to_string(dir.join("log")).unwrap_or_default();
    let mut state = parse_state(&status, &log)?;
    // Asked but not started yet: the request is still there.
    if Path::new(REQUEST).exists() && state.state != "running" {
        state.state = "requested".to_owned();
        state.message = None;
    }
    Some(state)
}

/// Whether a request waits for root to take it. Blocking.
pub(crate) fn pending() -> bool {
    Path::new(REQUEST).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_version_tags() {
        assert!(valid_tag("v2026.11.10"));
        assert!(valid_tag("v2026.1.2"));
        for bad in [
            "2026.11.10",
            "v2026.11",
            "v2026.11.10.1",
            "v2026..10",
            "v2026.11.10; rm -rf /",
            "v2026.11.1x",
            "../../etc",
            "",
        ] {
            assert!(!valid_tag(bad), "{bad}");
        }
    }

    #[test]
    fn reads_the_state_written_by_the_script() {
        let s =
            parse_state("state=running\nversion=v2026.11.10\nat=1763500000\n", "").expect("state");
        assert_eq!(s.state, "running");
        assert_eq!(s.version.as_deref(), Some("v2026.11.10"));
        assert_eq!(s.at_ms, Some(1_763_500_000_000));
        assert_eq!(s.message, None);
        let log = (1..=12)
            .map(|i| format!("ligne {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let failed = parse_state("state=failed\nversion=\nat=x\n", &log).expect("state");
        assert_eq!(failed.version, None);
        assert_eq!(failed.at_ms, None);
        assert_eq!(
            failed.message.as_deref().map(|m| m.lines().count()),
            Some(LOG_LINES)
        );
        assert!(failed.message.unwrap_or_default().ends_with("ligne 12"));
        assert_eq!(parse_state("garbage", ""), None);
    }
}
