//! RTSP endpoints with credentials kept apart from the URL.

use crate::secret::Secret;
use percent_encoding::percent_decode_str;
use std::fmt;
use url::Url;

/// Username and password for an RTSP server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    /// User name, sent in clear text in the `Authorization` header (Basic) or hashed (Digest).
    pub username: String,
    /// Password.
    pub password: Secret,
}

/// An RTSP endpoint: a URL guaranteed to contain no credentials, plus optional
/// credentials.
///
/// `Debug` and [`RtspTarget::redacted`] only show `scheme://host:port`, because
/// RTSP paths and query strings frequently embed tokens or channel secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct RtspTarget {
    url: Url,
    credentials: Option<Credentials>,
}

/// Reasons an RTSP URL is rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtspTargetError {
    /// The URL could not be parsed. The URL itself is not included as it may contain secrets.
    #[error("malformed URL: {0}")]
    Malformed(url::ParseError),
    /// The scheme is not `rtsp`.
    #[error(
        "unsupported scheme {0:?}: only rtsp:// is supported (rtsps is not supported by the RTSP client)"
    )]
    UnsupportedScheme(String),
    /// The URL has no host.
    #[error("URL has no host")]
    MissingHost,
    /// Credentials appear both in the URL and in dedicated settings.
    #[error("credentials given both in the URL and in username/password settings")]
    DuplicateCredentials,
    /// A password was given without a user name.
    #[error("password given without a username")]
    PasswordWithoutUsername,
    /// The userinfo part of the URL is not valid UTF-8 once percent-decoded.
    #[error("credentials in URL are not valid UTF-8")]
    InvalidCredentialEncoding,
}

impl RtspTarget {
    /// Parses `raw`, moving any `user:password@` part of the URL into
    /// [`Credentials`]. Explicit `username`/`password` arguments are used when
    /// the URL has no userinfo; giving both is rejected as ambiguous.
    pub fn parse(
        raw: &str,
        username: Option<&str>,
        password: Option<Secret>,
    ) -> Result<Self, RtspTargetError> {
        let mut url = Url::parse(raw).map_err(RtspTargetError::Malformed)?;
        if url.scheme() != "rtsp" {
            return Err(RtspTargetError::UnsupportedScheme(url.scheme().to_owned()));
        }
        if url.host_str().is_none_or(str::is_empty) {
            return Err(RtspTargetError::MissingHost);
        }

        let url_has_creds = !url.username().is_empty() || url.password().is_some();
        let explicit = username.is_some() || password.is_some();
        let credentials = match (url_has_creds, explicit) {
            (true, true) => return Err(RtspTargetError::DuplicateCredentials),
            (true, false) => {
                let decode = |s: &str| {
                    percent_decode_str(s)
                        .decode_utf8()
                        .map(|c| c.into_owned())
                        .map_err(|_| RtspTargetError::InvalidCredentialEncoding)
                };
                let username = decode(url.username())?;
                let password = Secret::new(decode(url.password().unwrap_or(""))?);
                // Both setters only fail for URLs that cannot have a host,
                // which was excluded above.
                let _ = url.set_username("");
                let _ = url.set_password(None);
                Some(Credentials { username, password })
            }
            (false, _) => match (username, password) {
                (Some(u), p) => Some(Credentials {
                    username: u.to_owned(),
                    password: p.unwrap_or_default(),
                }),
                (None, Some(_)) => return Err(RtspTargetError::PasswordWithoutUsername),
                (None, None) => None,
            },
        };
        Ok(Self { url, credentials })
    }

    /// The URL, without credentials. Avoid logging it: use [`Self::redacted`].
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The credentials, if any.
    pub fn credentials(&self) -> Option<&Credentials> {
        self.credentials.as_ref()
    }

    /// A loggable form of the endpoint: `rtsp://host:port`, without path,
    /// query or credentials.
    pub fn redacted(&self) -> String {
        // `host_str` already brackets IPv6 addresses.
        let host = self.url.host_str().unwrap_or("?");
        let port = self.url.port().unwrap_or(554);
        format!("rtsp://{host}:{port}")
    }
}

impl fmt::Debug for RtspTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RtspTarget")
            .field("endpoint", &self.redacted())
            .field("has_credentials", &self.credentials.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_credentials_from_url() {
        let t = RtspTarget::parse(
            "rtsp://admin:p%40ss@192.168.1.10:8554/h264?token=abc",
            None,
            None,
        )
        .expect("valid");
        assert_eq!(t.url().as_str(), "rtsp://192.168.1.10:8554/h264?token=abc");
        let c = t.credentials().expect("creds");
        assert_eq!(c.username, "admin");
        assert_eq!(c.password.expose(), "p@ss");
        assert_eq!(t.redacted(), "rtsp://192.168.1.10:8554");
        let dbg = format!("{t:?}");
        assert!(!dbg.contains("p@ss") && !dbg.contains("token") && !dbg.contains("admin"));
    }

    #[test]
    fn explicit_credentials() {
        let t = RtspTarget::parse("rtsp://cam.local/live", Some("u"), Some(Secret::new("p")))
            .expect("valid");
        assert_eq!(t.credentials().map(|c| c.username.as_str()), Some("u"));
        assert_eq!(t.redacted(), "rtsp://cam.local:554");
    }

    #[test]
    fn rejects_bad_targets() {
        assert!(matches!(
            RtspTarget::parse("http://cam/live", None, None),
            Err(RtspTargetError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            RtspTarget::parse("rtsp://a:b@cam/live", Some("u"), None),
            Err(RtspTargetError::DuplicateCredentials)
        ));
        assert!(matches!(
            RtspTarget::parse("rtsp://cam/live", None, Some(Secret::new("p"))),
            Err(RtspTargetError::PasswordWithoutUsername)
        ));
        assert!(matches!(
            RtspTarget::parse("not a url", None, None),
            Err(RtspTargetError::Malformed(_))
        ));
    }

    #[test]
    fn ipv6_redaction() {
        let t = RtspTarget::parse("rtsp://[::1]:8554/x", None, None).expect("valid");
        assert_eq!(t.redacted(), "rtsp://[::1]:8554");
    }
}
