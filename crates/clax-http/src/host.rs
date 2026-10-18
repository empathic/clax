//! Per-artifact origins: `<id>.localhost[:port]` serves only that artifact's content.

use crate::content_policy::{ContentPolicySet, content_policy, own_host};
use crate::error::ApiError;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use clax_core::ArtifactId;

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

/// A service worker script for any registration or update on an artifact's
/// content: it installs, then unregisters itself, and handles no fetch. A
/// page's own worker script never runs (a worker answers its frames with
/// documents that carry no policy), and a worker an artifact registered
/// before is replaced and removed at its next update check.
pub const SERVICE_WORKER_KILL_SWITCH: &str = "self.addEventListener(\"install\",()=>self.skipWaiting());\
self.addEventListener(\"activate\",e=>e.waitUntil(self.registration.unregister()));";

/// The artifact a main-origin path is content of: `/c/<id>/...`. `Some(None)`
/// for a `/c/...` path whose ID does not parse.
fn content_path(path: &str) -> Option<Option<ArtifactId>> {
    let rest = path.strip_prefix("/c/")?;
    let id = rest.split('/').next().unwrap_or("");
    Some(ArtifactId::parse(id).ok())
}

/// On an artifact host: `/healthz` and `/_clax/*` pass through, `/v/*` and
/// `/draft/*` are rewritten to `/c/<id>/v/*` and `/c/<id>/draft/*`,
/// `/_blob/<asset>` (or `/_blob/<id>/<asset>`) to `/_blob/<id>/<asset>`, which
/// serves only the artifact's own assets (a `/_blob/<other ID>/<asset>`
/// becomes a path that matches no asset; on a hosted daemon `/_blob/*`
/// passes through, its keyed URLs naming one asset each), and everything
/// else is a JSON 404; other hosts are untouched.
///
/// Every response on an artifact host, and every response under `/c/` on any
/// host, carries the artifact's content policy (`content_policy` with its
/// lockdown) unless the content route set the policy itself
/// ([`ContentPolicySet`]): a 404, `/healthz`, `/_clax/*`, a redirect or a
/// refusal framed or opened by the page is held to the page's own rules,
/// whatever route answers it now or later. A `/c/` path with no valid ID
/// gets `sandbox; default-src 'none'`. A request for a service worker script
/// there (`Service-Worker: script`) is answered with
/// [`SERVICE_WORKER_KILL_SWITCH`].
pub async fn rewrite_artifact_host(
    State(s): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    let (artifact, on_origin) = if let Some(id) = artifact_host(&host) {
        let path = req.uri().path().to_string();
        let query = req
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        // A hosted daemon's assets are served at their keys
        // (`hosted::asset_url`), which already name one asset each: their
        // paths pass through as they are.
        let blob = path
            .strip_prefix("/_blob/")
            .filter(|_| s.hosted.is_none())
            .map(|rest| {
                // The artifact's own assets only: `/_blob/<asset>` and
                // `/_blob/<this ID>/<asset>` both name this artifact's.
                let asset = rest
                    .strip_prefix(&format!("{}/", id.as_str()))
                    .unwrap_or(rest);
                format!("/_blob/{}/{asset}", id.as_str())
            });
        let new_path = if let Some(blob) = blob {
            blob
        } else if path == "/healthz" || path.starts_with("/_clax/") || path.starts_with("/_blob/") {
            path
        } else if path.starts_with("/v/") || path.starts_with("/draft/") {
            format!("/c/{}{}", id.as_str(), path)
        } else {
            return locked(
                ApiError::not_found().into_response(),
                true,
                req.headers(),
                &own_host(&s),
                Some(&id),
            );
        };
        let uri: Uri = format!("{new_path}{query}")
            .parse()
            .expect("rewritten uri is valid");
        *req.uri_mut() = uri;
        req.extensions_mut().insert(OnArtifactOrigin);
        (Some(id), true)
    } else {
        match content_path(req.uri().path()) {
            Some(id) => (id, false),
            None => return next.run(req).await,
        }
    };
    let own = own_host(&s);
    let headers = req.headers().clone();
    if headers
        .get("service-worker")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"script"))
    {
        let res = (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/javascript"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            SERVICE_WORKER_KILL_SWITCH,
        )
            .into_response();
        return locked(res, on_origin, &headers, &own, artifact.as_ref());
    }
    let res = next.run(req).await;
    if res.extensions().get::<ContentPolicySet>().is_some() {
        return res;
    }
    locked(res, on_origin, &headers, &own, artifact.as_ref())
}

/// `res` with artifact `id`'s lockdown policy added beside any policy it
/// has (policies only narrow), and `Referrer-Policy: no-referrer`; with no
/// ID, a policy that loads and runs nothing.
fn locked(
    mut res: Response,
    on_origin: bool,
    req: &axum::http::HeaderMap,
    own: &str,
    id: Option<&ArtifactId>,
) -> Response {
    let policy = match id {
        Some(id) => content_policy(on_origin, req, own, Some(id.as_str())),
        None => HeaderValue::from_static("sandbox; default-src 'none'"),
    };
    res.headers_mut()
        .append(header::CONTENT_SECURITY_POLICY, policy);
    res.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    res
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
