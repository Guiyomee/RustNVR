//! Authentication of RTSP clients: Digest (RFC 2617, MD5) and Basic, against
//! the credentials of the stream asked for, and the lockout of addresses
//! that keep failing.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use md5::{Digest as _, Md5};
use rustnvr_core::Secret;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Realm announced to clients.
pub const REALM: &str = "RustNVR";
/// Wrong passwords an address may send before it is refused.
pub const MAX_FAILURES: u32 = 5;
/// How long failures count, and how long an address stays refused.
pub const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Addresses remembered at most (beyond it, the oldest failures are dropped).
const MAX_TRACKED: usize = 4096;

/// What a client must present to read a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    /// User name.
    pub username: String,
    /// Password.
    pub password: Secret,
}

/// A random nonce (32 hexadecimal digits), one per connection.
pub fn new_nonce() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // No system randomness: still distinct per connection and unknown
        // in advance to another client.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let address = std::ptr::from_ref(&bytes) as u128;
        bytes = (now.as_nanos() ^ address.rotate_left(64)).to_le_bytes();
    }
    hex(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn md5_hex(parts: &[&str]) -> String {
    let mut h = Md5::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update(b":");
        }
        h.update(p.as_bytes());
    }
    hex(&h.finalize())
}

/// Compares without stopping at the first difference.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The `WWW-Authenticate` challenges sent with a 401: Digest first (the
/// password does not travel), Basic for clients that know nothing else.
pub fn challenges(nonce: &str) -> [String; 2] {
    [
        format!("Digest realm=\"{REALM}\", nonce=\"{nonce}\", algorithm=MD5"),
        format!("Basic realm=\"{REALM}\""),
    ]
}

/// Parameters of a Digest `Authorization` header (`key="value", key=value`).
fn digest_params(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some((key, after)) = rest.split_once('=') else {
            break;
        };
        let after = after.trim_start();
        let (value, tail) = match after.strip_prefix('"') {
            Some(quoted) => match quoted.split_once('"') {
                Some((v, t)) => (v, t),
                None => (quoted, ""),
            },
            None => after.split_once(',').unwrap_or((after, "")),
        };
        out.insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
        rest = tail.trim_start_matches([',', ' ', '\t']);
    }
    out
}

/// Whether the `Authorization` header of a `method` request proves the
/// knowledge of `credentials`. `nonce` is the one given to this connection.
pub fn authorized(header: &str, method: &str, credentials: &Credentials, nonce: &str) -> bool {
    let header = header.trim();
    let (scheme, value) = header.split_once(' ').unwrap_or((header, ""));
    if scheme.eq_ignore_ascii_case("Basic") {
        let Ok(decoded) = B64.decode(value.trim()) else {
            return false;
        };
        let expected = format!("{}:{}", credentials.username, credentials.password.expose());
        return constant_time_eq(&decoded, expected.as_bytes());
    }
    if !scheme.eq_ignore_ascii_case("Digest") {
        return false;
    }
    let p = digest_params(value);
    let get = |k: &str| p.get(k).map(String::as_str).unwrap_or_default();
    if p.get("algorithm")
        .is_some_and(|a| !a.eq_ignore_ascii_case("MD5"))
    {
        return false;
    }
    // Both checked, whatever the first says.
    let known = constant_time_eq(get("username").as_bytes(), credentials.username.as_bytes())
        & constant_time_eq(get("nonce").as_bytes(), nonce.as_bytes())
        & (get("realm") == REALM);
    let ha1 = md5_hex(&[&credentials.username, REALM, credentials.password.expose()]);
    let ha2 = md5_hex(&[method, get("uri")]);
    let expected = match p.get("qop") {
        Some(qop) => md5_hex(&[&ha1, nonce, get("nc"), get("cnonce"), qop, &ha2]),
        None => md5_hex(&[&ha1, nonce, &ha2]),
    };
    known
        & constant_time_eq(
            get("response").to_ascii_lowercase().as_bytes(),
            expected.as_bytes(),
        )
}

/// Wrong passwords, by client address.
#[derive(Default)]
pub struct FailureGuard {
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

impl FailureGuard {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<IpAddr, (u32, Instant)>> {
        self.failures.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether `address` sent too many wrong passwords lately.
    pub fn blocked(&self, address: IpAddr) -> bool {
        let mut map = self.lock();
        map.retain(|_, (_, since)| since.elapsed() < FAILURE_WINDOW);
        map.get(&address).is_some_and(|(n, _)| *n >= MAX_FAILURES)
    }

    /// Counts a wrong password; true when the address is now refused.
    pub fn failed(&self, address: IpAddr) -> bool {
        let mut map = self.lock();
        if map.len() >= MAX_TRACKED && !map.contains_key(&address) {
            map.retain(|_, (_, since)| since.elapsed() < FAILURE_WINDOW);
            if map.len() >= MAX_TRACKED {
                // Flooded with addresses: forget the oldest half.
                let mut ages: Vec<Instant> = map.values().map(|(_, t)| *t).collect();
                ages.sort();
                let cut = ages[ages.len() / 2];
                map.retain(|_, (_, since)| *since > cut);
            }
        }
        let entry = map.entry(address).or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.0 >= MAX_FAILURES
    }

    /// A right password: the address starts again from zero.
    pub fn succeeded(&self, address: IpAddr) {
        self.lock().remove(&address);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn creds(user: &str, password: &str) -> Credentials {
        Credentials {
            username: user.to_owned(),
            password: Secret::new(password),
        }
    }

    #[test]
    fn digest_matches_the_rfc_2617_example() {
        // RFC 2617, section 3.5 (realm and nonce of the example).
        let ha1 = md5_hex(&["Mufasa", "testrealm@host.com", "Circle Of Life"]);
        let ha2 = md5_hex(&["GET", "/dir/index.html"]);
        let response = md5_hex(&[
            &ha1,
            "dcd98b7102dd2f0e8b11d0f600bfb0c093",
            "00000001",
            "0a4f113b",
            "auth",
            &ha2,
        ]);
        assert_eq!(response, "6629fae49393a05397450978507c4ef1");
    }

    fn digest_header(c: &Credentials, method: &str, uri: &str, nonce: &str, qop: bool) -> String {
        let ha1 = md5_hex(&[&c.username, REALM, c.password.expose()]);
        let ha2 = md5_hex(&[method, uri]);
        if qop {
            let response = md5_hex(&[&ha1, nonce, "00000001", "abc", "auth", &ha2]);
            format!(
                "Digest username=\"{}\", realm=\"{REALM}\", nonce=\"{nonce}\", uri=\"{uri}\", qop=auth, nc=00000001, cnonce=\"abc\", response=\"{response}\"",
                c.username
            )
        } else {
            let response = md5_hex(&[&ha1, nonce, &ha2]);
            format!(
                "Digest username=\"{}\", realm=\"{REALM}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\", algorithm=MD5",
                c.username
            )
        }
    }

    #[test]
    fn accepts_digest_and_basic_of_the_right_account_only() {
        let c = creds("nvr", "s3cret, \"quoted\"");
        let uri = "rtsp://nvr:8554/jardin?x=1,2";
        for qop in [false, true] {
            let h = digest_header(&c, "DESCRIBE", uri, "n1", qop);
            assert!(authorized(&h, "DESCRIBE", &c, "n1"), "{h}");
            // Another method, another connection's nonce, another password.
            assert!(!authorized(&h, "SETUP", &c, "n1"));
            assert!(!authorized(&h, "DESCRIBE", &c, "n2"));
            assert!(!authorized(&h, "DESCRIBE", &creds("nvr", "other"), "n1"));
            assert!(!authorized(
                &h,
                "DESCRIBE",
                &creds("admin", "s3cret, \"quoted\""),
                "n1"
            ));
        }
        let basic = format!("Basic {}", B64.encode("nvr:s3cret, \"quoted\""));
        assert!(authorized(&basic, "PLAY", &c, "n1"));
        assert!(!authorized(
            &format!("Basic {}", B64.encode("nvr:nope")),
            "PLAY",
            &c,
            "n1"
        ));
        assert!(!authorized("Basic !!!", "PLAY", &c, "n1"));
        assert!(!authorized("Bearer abc", "PLAY", &c, "n1"));
        assert!(!authorized("", "PLAY", &c, "n1"));
        assert!(!authorized("Digest", "PLAY", &c, "n1"));
        // A password containing a colon is one password.
        let colon = creds("u", "a:b");
        assert!(authorized(
            &format!("Basic {}", B64.encode("u:a:b")),
            "PLAY",
            &colon,
            "n"
        ));
    }

    #[test]
    fn nonces_differ() {
        let (a, b) = (new_nonce(), new_nonce());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn addresses_are_refused_after_failures_and_forgiven_on_success() {
        let guard = FailureGuard::default();
        let (bad, good): (IpAddr, IpAddr) =
            ("10.0.0.9".parse().unwrap(), "10.0.0.2".parse().unwrap());
        for i in 1..=MAX_FAILURES {
            assert!(!guard.blocked(bad));
            assert_eq!(guard.failed(bad), i == MAX_FAILURES);
        }
        assert!(guard.blocked(bad));
        assert!(!guard.blocked(good));
        guard.failed(good);
        guard.succeeded(good);
        assert!(!guard.blocked(good));
        // The table stays bounded.
        for i in 0..(MAX_TRACKED as u32 + 100) {
            guard.failed(IpAddr::from(std::net::Ipv4Addr::from(0x0b00_0000 + i)));
        }
        assert!(guard.lock().len() <= MAX_TRACKED);
    }
}
