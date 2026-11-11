//! Home Assistant ingress: the interface shown in Home Assistant's side panel.
//!
//! Home Assistant serves the page under `/api/hassio_ingress/<token>/`, strips
//! that prefix before forwarding the request (through an app such as
//! « RustNVR Proxy ») and names it in the `X-Ingress-Path` header. RustNVR
//! then:
//! - writes the prefix in the addresses of `index.html` and gives it to the
//!   interface (`<meta name="rustnvr-base">`), which puts it before its API
//!   calls, WebSocket and links;
//! - lets Home Assistant show the page in its frame (same origin only).
//!
//! The header is only taken when it is a plain path: it cannot inject markup.
//! A browser cannot send it from another site (a custom header needs CORS,
//! which RustNVR does not allow), so a page elsewhere still cannot frame
//! RustNVR.

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::path::Path;

/// Header set by the Home Assistant supervisor (and forwarded by the proxy).
pub(crate) const INGRESS_HEADER: &str = "x-ingress-path";

/// Longest prefix accepted (`/api/hassio_ingress/<token>` is about 70).
const MAX_LEN: usize = 256;

/// The ingress prefix of the request, when it is a plain absolute path
/// (`/seg/seg`, unreserved URL characters, no `.`/`..` segment, no trailing
/// `/`). Anything else is ignored: the page is served as without ingress.
pub(crate) fn ingress_path(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(INGRESS_HEADER)?.to_str().ok()?;
    valid_prefix(value).then_some(value)
}

fn valid_prefix(p: &str) -> bool {
    let Some(rest) = p.strip_prefix('/') else {
        return false;
    };
    p.len() <= MAX_LEN
        && rest.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
        })
}

/// `index.html` with its addresses under `base` (`""` without ingress).
///
/// The interface is built with relative addresses (`./assets/…`); they become
/// absolute here, so the page also works on client-side routes
/// (`/cameras/x`). With a prefix, the interface also learns it from a
/// `<meta>` tag (the content security policy forbids inline scripts).
pub(crate) fn rewrite_index(html: &str, base: &str) -> String {
    let mut page = html
        .replace("=\"./", &format!("=\"{base}/"))
        .replace("='./", &format!("='{base}/"));
    if !base.is_empty()
        && let Some(at) = page.find("</head>")
    {
        page.insert_str(
            at,
            &format!("<meta name=\"rustnvr-base\" content=\"{base}\" />\n"),
        );
    }
    page
}

/// The interface page (`/`, `/index.html` and every client-side route).
/// Read at each request: a rebuilt interface is used at once.
pub(crate) async fn index_page(index: &Path, headers: &HeaderMap) -> Response {
    let html = match tokio::fs::read_to_string(index).await {
        Ok(html) => html,
        Err(e) => {
            tracing::warn!(path = %index.display(), error = %e, "cannot read the interface page");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let page = rewrite_index(&html, ingress_path(headers).unwrap_or(""));
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::VARY, "x-ingress-path"),
        ],
        page,
    )
        .into_response()
}

/// Under ingress, Home Assistant shows the page in a frame of its own origin:
/// allowed there only (`SAMEORIGIN`, `frame-ancestors 'self'`). Without the
/// header the outer layers keep `DENY` / `frame-ancestors 'none'`.
pub(crate) async fn framing(req: Request, next: Next) -> Response {
    let ingress = ingress_path(req.headers()).is_some();
    let mut res = next.run(req).await;
    if ingress {
        let h = res.headers_mut();
        h.insert(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("SAMEORIGIN"),
        );
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(crate::routes::CONTENT_SECURITY_POLICY_INGRESS),
        );
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            INGRESS_HEADER,
            HeaderValue::from_str(v).expect("header value"),
        );
        h
    }

    #[test]
    fn accepts_only_plain_paths() {
        let ha = "/api/hassio_ingress/7wTq-Zk_3sJ9aBcDeF0gHiJkLmNoPqRsTuVwXyZ";
        assert_eq!(ingress_path(&headers(ha)), Some(ha));
        assert_eq!(ingress_path(&headers("/a/b.c~d")), Some("/a/b.c~d"));
        for bad in [
            "",
            "/",
            "api/x",
            "/api/",
            "/api//x",
            "/api/../x",
            "/api/./x",
            "/a\"><script>",
            "/a b",
            "/a?b",
            "/a#b",
            "/a%2e",
            "//evil.example/x",
            "https://evil.example",
        ] {
            assert_eq!(ingress_path(&headers(bad)), None, "{bad:?}");
        }
        assert_eq!(
            ingress_path(&headers(&format!("/{}", "a".repeat(255)))).map(str::len),
            Some(256)
        );
        assert_eq!(
            ingress_path(&headers(&format!("/{}", "a".repeat(256)))),
            None
        );
        assert_eq!(ingress_path(&HeaderMap::new()), None);
    }

    #[test]
    fn rewrites_the_interface_page() {
        let html = "<head>\n<link rel=\"icon\" href=\"./favicon.svg?v=2\" />\n\
                    <script type=\"module\" crossorigin src=\"./assets/index-a.js\"></script>\n\
                    </head><body><a href=\"https://x/./y\">x</a></body>";
        let plain = rewrite_index(html, "");
        assert!(plain.contains("href=\"/favicon.svg?v=2\""));
        assert!(plain.contains("src=\"/assets/index-a.js\""));
        assert!(!plain.contains("rustnvr-base"));
        assert!(plain.contains("https://x/./y"));

        let ha = rewrite_index(html, "/api/hassio_ingress/abc");
        assert!(ha.contains("href=\"/api/hassio_ingress/abc/favicon.svg?v=2\""));
        assert!(ha.contains("src=\"/api/hassio_ingress/abc/assets/index-a.js\""));
        assert!(ha.contains(
            "<meta name=\"rustnvr-base\" content=\"/api/hassio_ingress/abc\" />\n</head>"
        ));
    }
}
