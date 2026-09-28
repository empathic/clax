//! Per-artifact origins: `<id>.localhost[:port]` serves only that artifact's content.

use crate::error::ApiError;
use artifax_core::ArtifactId;
use axum::body::Body;
use axum::extract::Request;
use axum::http::Uri;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Accepts `<12-char lowercase ID>.localhost` with an optional numeric `:port`; anything else is `None`.
pub fn artifact_host(host: &str) -> Option<ArtifactId> {
    let host = host.split_once(':').map_or(host, |(h, port)| {
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            ""
        } else {
            h
        }
    });
    let id = host.strip_suffix(".localhost")?;
    if id.contains('.') {
        return None;
    }
    ArtifactId::parse(id).ok()
}

/// Marker request extension: the request arrived on a per-artifact origin.
#[derive(Clone, Copy)]
pub struct OnArtifactOrigin;

/// On an artifact host: `/healthz`, `/_artifax/*` and `/_blob/*` pass through, `/v/*` is
/// rewritten to `/c/<id>/v/*`, everything else is a JSON 404; other hosts are untouched.
pub async fn rewrite_artifact_host(mut req: Request<Body>, next: Next) -> Response {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    if let Some(id) = artifact_host(&host) {
        let path = req.uri().path().to_string();
        let query = req
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        let new_path = if path == "/healthz"
            || path.starts_with("/_artifax/")
            || path.starts_with("/_blob/")
        {
            path
        } else if path.starts_with("/v/") {
            format!("/c/{}{}", id.as_str(), path)
        } else {
            return ApiError::not_found().into_response();
        };
        let uri: Uri = format!("{new_path}{query}")
            .parse()
            .expect("rewritten uri is valid");
        *req.uri_mut() = uri;
        req.extensions_mut().insert(OnArtifactOrigin);
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::artifact_host;

    #[test]
    fn accepts_only_anchored_lowercase_ids() {
        assert_eq!(
            artifact_host("7q3k9mzx2b4t.localhost").unwrap().as_str(),
            "7q3k9mzx2b4t"
        );
        assert_eq!(
            artifact_host("7q3k9mzx2b4t.localhost:7480")
                .unwrap()
                .as_str(),
            "7q3k9mzx2b4t"
        );
        for bad in [
            "localhost",
            "localhost:7480",
            "evil.localhost",
            "7Q3K9MZX2B4T.localhost",
            "7q3k9mzx2b4t.localhost.attacker.com",
            "x.7q3k9mzx2b4t.localhost",
            "7q3k9mzx2b4t.localhos",
            "7q3k9mzx2b4t.localhost:abc",
            "7q3k9mzx2b4i.localhost",
        ] {
            assert!(artifact_host(bad).is_none(), "{bad}");
        }
    }
}
