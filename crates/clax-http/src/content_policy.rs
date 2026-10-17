//! The `Content-Security-Policy` of an artifact's content: what the content
//! routes set on every response they serve, and what
//! [`crate::host::rewrite_artifact_host`] adds to any other response for an
//! artifact's content.

use crate::state::AppState;
use axum::http::{HeaderMap, HeaderValue, header};

/// Content served on the main origin must not run same-origin with the API.
pub const SANDBOX: &str =
    "sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads";

/// The `Content-Security-Policy` of a content response: who may frame it,
/// and, for an artifact's own content (`lockdown`: its ID; not a live page's
/// snapshot), what it may load. `own` is the daemon's host.
///
/// - On the main origin (sandbox mode): the sandbox, and no
///   `frame-ancestors`. A sandboxed page has an opaque origin, which no
///   `frame-ancestors` source matches, so any such list would also refuse an
///   artifact's page framed by another of its pages. Framed by another site,
///   the page reaches nothing of the viewer's: it runs at an opaque origin,
///   and its bridge talks only to a parent at the shell's origin.
/// - On an artifact origin (`<id>.localhost[:port]`, subdomain mode): framed
///   only by the artifact's own pages (`'self'`) and the shell, at
///   `localhost` or `127.0.0.1` on the same port, the only hosts from which
///   the shell uses subdomain frames.
///
/// The policy is a separate one: a page's own `<meta>` policy still applies
/// in full beside it, and cannot set `frame-ancestors` itself.
pub fn content_policy(
    on_artifact_origin: bool,
    req: &HeaderMap,
    own: &str,
    lockdown: Option<&str>,
) -> HeaderValue {
    // The rewrite accepted an artifact host only with a numeric port, or none;
    // on the main origin a `Host` that is not plain is the daemon's own.
    let host = plain_host(req, own);
    let port = host
        .rsplit_once(':')
        .filter(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .map(|(_, p)| format!(":{p}"))
        .unwrap_or_default();
    let frame = if !on_artifact_origin {
        SANDBOX.to_string()
    } else {
        format!("frame-ancestors 'self' http://localhost{port} http://127.0.0.1{port}")
    };
    let policy = match lockdown {
        Some(id) => {
            // An artifact host serves only GET and HEAD, of the artifact's
            // content, `/_clax/`, `/_blob/` and `/healthz` (`host.rs`): no
            // API, so `'self'` names nothing but those.
            let (loadable, files) = if on_artifact_origin {
                ("'self'".to_string(), "'self'".to_string())
            } else {
                (
                    format!(
                        "http://{host}/c/{id}/ http://{host}/_clax/bridge.js http://{host}/_clax/bridge/"
                    ),
                    format!("http://{host}/c/{id}/"),
                )
            };
            format!(
                "{frame}; {}",
                load_policy(&loadable, &files, &blob_sources(host, own, &port, id))
            )
        }
        None => frame,
    };
    HeaderValue::from_str(&policy).unwrap_or_else(|_| HeaderValue::from_static("sandbox"))
}

/// What an artifact's page may load: its own files and the bridge (`own`,
/// source expressions), its own assets under `/_blob/<id>/` on the daemon's
/// hosts (`blobs`), and `data:` and `blob:` URLs. It fetches (`fetch`, XHR,
/// WebSocket, EventSource, beacons, JSON modules) only its own files
/// (`files`) and assets: the network is the `net` capability's, through the
/// shell. No plugins, base URL or form posts. Inline and evaluated scripts
/// and inline styles run, being the page's own.
fn load_policy(own: &str, files: &str, blobs: &str) -> String {
    format!(
        "default-src {own} {blobs} data: blob:; \
         script-src {own} 'unsafe-inline' 'unsafe-eval' data: blob:; \
         style-src {own} 'unsafe-inline' data: blob:; \
         connect-src {files} {blobs}; object-src 'none'; base-uri 'none'; form-action 'none'"
    )
}

/// The artifact's own assets, `/_blob/<id>/`, on each host an asset URL may
/// name: the request's (when plain), the daemon's browser base, and
/// `localhost` and `127.0.0.1` on the request's port.
fn blob_sources(host: &str, own: &str, port: &str, id: &str) -> String {
    let mut hosts: Vec<String> = Vec::new();
    for h in [
        host.to_string(),
        own.to_string(),
        format!("localhost{port}"),
        format!("127.0.0.1{port}"),
    ] {
        if !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    hosts
        .iter()
        .map(|h| format!("http://{h}/_blob/{id}/"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The request's `Host` when it is a plain host and port (letters, digits,
/// `.`, `-`, `:`, `[`, `]`), else `fallback` (the daemon's own host): a
/// crafted `Host` never reaches a policy.
pub fn plain_host<'a>(req: &'a HeaderMap, fallback: &'a str) -> &'a str {
    let plain = |h: &str| {
        !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    };
    req.get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| plain(h))
        .unwrap_or(fallback)
}

/// The host and port in the daemon's browser base URL.
pub fn own_host(s: &AppState) -> String {
    s.browser_base
        .strip_prefix("http://")
        .unwrap_or(&s.browser_base)
        .trim_end_matches('/')
        .to_string()
}

/// Response extension: the content route set this response's policy itself
/// (`framed_by_shell` in the content routes); [`crate::host::rewrite_artifact_host`] adds the artifact's
/// lockdown policy to every other response for an artifact's content.
#[derive(Clone, Copy)]
pub struct ContentPolicySet;
