//! HTTP authentication of a camera's photo address: Digest (RFC 7616 with
//! MD5, the one cameras use: Hikvision, Dahua, Reolink…) or Basic.

use base64::Engine as _;
use md5::{Digest as _, Md5};

/// The parameters of a `WWW-Authenticate: Digest …` challenge.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Challenge {
    realm: String,
    nonce: String,
    opaque: Option<String>,
    /// `auth` offered (`qop="auth,auth-int"`): the client nonce is used.
    qop_auth: bool,
    /// `MD5-sess` instead of `MD5`.
    session: bool,
}

/// Reads a Digest challenge; `None` for Basic, an algorithm other than MD5
/// or a challenge without nonce.
pub(crate) fn parse_digest(header: &str) -> Option<Challenge> {
    let rest = header.trim();
    let rest = rest
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("digest "))
        .map(|_| &rest[7..])?;
    let mut c = Challenge::default();
    for (key, value) in params(rest) {
        match key.to_ascii_lowercase().as_str() {
            "realm" => c.realm = value,
            "nonce" => c.nonce = value,
            "opaque" => c.opaque = Some(value),
            "qop" => c.qop_auth = value.split(',').any(|q| q.trim() == "auth"),
            "algorithm" => match value.to_ascii_uppercase().as_str() {
                "MD5" => {}
                "MD5-SESS" => c.session = true,
                _ => return None,
            },
            _ => {}
        }
    }
    (!c.nonce.is_empty()).then_some(c)
}

/// `key=value` or `key="value, with commas"` pairs, separated by commas.
fn params(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ',' || c.is_whitespace()) {
            chars.next();
        }
        let key: String = std::iter::from_fn(|| chars.next_if(|c| *c != '=' && *c != ','))
            .collect::<String>()
            .trim()
            .to_owned();
        if key.is_empty() {
            return out;
        }
        if chars.next_if_eq(&'=').is_none() {
            continue;
        }
        let value = if chars.next_if_eq(&'"').is_some() {
            let mut v = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => v.extend(chars.next()),
                    c => v.push(c),
                }
            }
            v
        } else {
            std::iter::from_fn(|| chars.next_if(|c| *c != ','))
                .collect::<String>()
                .trim()
                .to_owned()
        };
        out.push((key, value));
    }
}

fn md5_hex(parts: &[&str]) -> String {
    let mut h = Md5::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update(b":");
        }
        h.update(p.as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn quoted(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The `Authorization` header answering `c` for a `GET` of `uri` (path and
/// query), with the client nonce `cnonce`.
pub(crate) fn digest_header(
    c: &Challenge,
    user: &str,
    password: &str,
    uri: &str,
    cnonce: &str,
) -> String {
    let mut ha1 = md5_hex(&[user, &c.realm, password]);
    if c.session {
        ha1 = md5_hex(&[&ha1, &c.nonce, cnonce]);
    }
    let ha2 = md5_hex(&["GET", uri]);
    let nc = "00000001";
    let response = if c.qop_auth {
        md5_hex(&[&ha1, &c.nonce, nc, cnonce, "auth", &ha2])
    } else {
        md5_hex(&[&ha1, &c.nonce, &ha2])
    };
    let mut h = format!(
        r#"Digest username="{}", realm="{}", nonce="{}", uri="{}", response="{response}""#,
        quoted(user),
        quoted(&c.realm),
        quoted(&c.nonce),
        quoted(uri)
    );
    if c.session {
        h.push_str(", algorithm=MD5-sess");
    } else {
        h.push_str(", algorithm=MD5");
    }
    if c.qop_auth {
        h.push_str(&format!(r#", qop=auth, nc={nc}, cnonce="{cnonce}""#));
    }
    if let Some(o) = &c.opaque {
        h.push_str(&format!(r#", opaque="{}""#, quoted(o)));
    }
    h
}

/// The `Authorization` header of HTTP Basic.
pub(crate) fn basic_header(user: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_challenges() {
        let c = parse_digest(
            r#"Digest realm="IP Camera(C2358)", qop="auth,auth-int", nonce="ab, c", opaque="5ccc", algorithm="MD5""#,
        )
        .expect("digest");
        assert_eq!(c.realm, "IP Camera(C2358)");
        assert_eq!(c.nonce, "ab, c");
        assert_eq!(c.opaque.as_deref(), Some("5ccc"));
        assert!(c.qop_auth && !c.session);
        assert!(parse_digest(r#"Basic realm="cam""#).is_none());
        assert!(parse_digest(r#"Digest realm="x", nonce="n", algorithm=SHA-256"#).is_none());
        assert!(
            parse_digest(r#"digest realm=x, nonce=n, algorithm=MD5-sess"#)
                .is_some_and(|c| c.session)
        );
    }

    /// The example of RFC 2617 (section 3.5).
    #[test]
    fn answers_like_the_rfc() {
        let c = parse_digest(
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .expect("digest");
        let h = digest_header(
            &c,
            "Mufasa",
            "Circle Of Life",
            "/dir/index.html",
            "0a4f113b",
        );
        assert!(
            h.contains(r#"response="6629fae49393a05397450978507c4ef1""#),
            "{h}"
        );
        assert!(
            h.contains("nc=00000001") && h.contains(r#"opaque="5ccc069c403ebaf9f0171e9517f40e41""#)
        );
        assert_eq!(basic_header("admin", "pw"), "Basic YWRtaW46cHc=");
    }
}
