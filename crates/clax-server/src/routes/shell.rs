//! Serves the embedded web UI: the shell's two entries (the gallery and the
//! artifact view) and static assets.

use crate::error::ApiError;
use crate::http_cache::{self, IMMUTABLE, REVALIDATE};
use crate::routes::artifacts::path;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, RawQuery};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct Assets;

/// Where the bridge bundle sits in [`Assets`].
const BRIDGE: &str = "_clax/bridge.js";

/// Debug builds only: the directory the web UI is read from in place of
/// `web/dist` (see [`set_web_dist`]).
#[cfg(debug_assertions)]
static WEB_DIST: std::sync::RwLock<Option<std::path::PathBuf>> = std::sync::RwLock::new(None);

/// Debug builds only: serves the web UI from `dir` (laid out like `web/dist`)
/// instead of `web/dist`, for the whole process. Tests that change the built
/// files use it, so no test writes the real `web/dist` that `just dev` serves.
#[cfg(debug_assertions)]
pub fn set_web_dist(dir: std::path::PathBuf) {
    *WEB_DIST.write().unwrap_or_else(|e| e.into_inner()) = Some(dir);
}

/// The directory a debug build reads the web UI from.
#[cfg(debug_assertions)]
fn web_dist() -> std::path::PathBuf {
    WEB_DIST
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/dist").into())
}

/// A file of the web UI (`path` relative to `web/dist`): embedded in a release
/// build, read from disk (see [`set_web_dist`]) in a debug build.
fn asset(path: &str) -> Option<rust_embed::EmbeddedFile> {
    #[cfg(debug_assertions)]
    if WEB_DIST.read().unwrap_or_else(|e| e.into_inner()).is_some() {
        let rel = std::path::Path::new(path);
        if !rel
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            return None;
        }
        return rust_embed::utils::read_file_from_fs(&web_dist().join(rel)).ok();
    }
    Assets::get(path)
}

/// A short content hash: the first 12 hex digits of a SHA-256.
fn short_hash(sha256: &[u8; 32]) -> String {
    sha256[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// The short hash of the bridge bundle as it reads now; empty when the UI is
/// not built.
fn hash_bridge() -> String {
    asset(BRIDGE)
        .map(|f| short_hash(&f.metadata.sha256_hash()))
        .unwrap_or_default()
}

/// The bridge version: a short hash of the bridge bundle, which bridge tags
/// carry as `/_clax/bridge.js?v=<version>`. Empty when the UI is not built.
///
/// A release build embeds the bundle, so the version is computed once (the
/// router computes it at startup).
#[cfg(not(debug_assertions))]
pub fn bridge_version() -> String {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(hash_bridge).clone()
}

/// The bridge version: a short hash of the bridge bundle, which bridge tags
/// carry as `/_clax/bridge.js?v=<version>`. Empty when the UI is not built.
///
/// A debug build reads `web/dist` from disk on every request, and `just dev`
/// rebuilds the bridge under a running daemon, so the version follows the
/// file: it is rehashed whenever the file's path, modification time or size
/// changes.
#[cfg(debug_assertions)]
pub fn bridge_version() -> String {
    type Stamp = (std::path::PathBuf, Option<std::time::SystemTime>, u64);
    static SEEN: std::sync::Mutex<Option<(Stamp, String)>> = std::sync::Mutex::new(None);
    let path = web_dist().join(BRIDGE);
    let Ok(m) = std::fs::metadata(&path) else {
        return String::new();
    };
    let stamp = (path, m.modified().ok(), m.len());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((s, v)) = seen.as_ref()
        && *s == stamp
    {
        return v.clone();
    }
    let v = hash_bridge();
    *seen = Some((stamp, v.clone()));
    v
}

/// The bridge's `Cache-Control`: in a release build, immutable at the URL
/// naming the version of the bytes served (`?v=<hash of them>`) and
/// revalidated at any other; in a debug build, where the file can change
/// under the same daemon, always revalidated.
fn bridge_cache_control(query: Option<&str>, served: &str) -> &'static str {
    let v = query
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("v="));
    if !cfg!(debug_assertions) && v == Some(served) {
        IMMUTABLE
    } else {
        REVALIDATE
    }
}

/// An entry of the shell (`index.html` or `artifact.html`) as an HTML page.
fn entry(req: &HeaderMap, name: &str) -> Result<Response, ApiError> {
    match asset(name) {
        Some(f) => Ok(http_cache::html(req, &String::from_utf8_lossy(&f.data))),
        None => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "ui_not_built",
            "the web UI has not been built; run `just web`",
        )),
    }
}

/// `/`: the gallery.
pub async fn gallery_page(req: HeaderMap) -> Result<Response, ApiError> {
    entry(&req, "index.html")
}

/// `/a/…`: `artifact.html` with the first-load data ([`crate::boot`]), or the
/// gallery for a path that names no artifact (the shell shows the gallery for
/// it too).
///
/// The data is the API's, so it follows the API's host rule
/// ([`crate::auth::request_host_allowed`], checked before anything is read):
/// a request whose `Host` the API would refuse (a DNS name rebound to this
/// machine) gets the bare entry, whose own API calls then fail. A store error
/// or a store slower than the API's timeout also gives the bare entry, and
/// the shell loads the data itself.
///
/// The page differs by viewer and frame mode, both read from cookies: it is
/// `private, no-cache` with `Vary: Cookie`, and its `ETag` is over the exact
/// bytes sent, so a revalidation answers `304` only for the same bytes.
pub async fn artifact_page(
    axum::extract::State(s): axum::extract::State<crate::state::AppState>,
    uri: axum::http::Uri,
    extensions: axum::http::Extensions,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let route = crate::shell_route::parse_shell_path(uri.path());
    if matches!(route, crate::shell_route::ShellRoute::Gallery) {
        return entry(&req, "index.html");
    }
    let Some(f) = asset("artifact.html") else {
        return entry(&req, "artifact.html");
    };
    let template = String::from_utf8_lossy(&f.data).into_owned();
    let injected = if crate::auth::request_host_allowed(&req, &uri, &extensions) {
        within(s.request_timeout, crate::boot::assemble(&s, route, &req)).await
    } else {
        None
    };
    let mut res = http_cache::html(&req, &crate::boot::inject(&template, injected.as_ref()));
    let h = res.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static(PRIVATE_REVALIDATE),
    );
    h.insert(header::VARY, axum::http::HeaderValue::from_static("Cookie"));
    Ok(res)
}

/// The bootstrap `assemble` makes, or none (logged) when it fails or takes
/// longer than `limit`: the page is then served bare, and the shell loads the
/// data itself.
async fn within(
    limit: std::time::Duration,
    assemble: impl std::future::Future<Output = Result<Option<crate::boot::Injected>, ApiError>>,
) -> Option<crate::boot::Injected> {
    match tokio::time::timeout(limit, assemble).await {
        Ok(Ok(i)) => i,
        Ok(Err(e)) => {
            tracing::warn!(error = ?e, "artifact page served without its bootstrap");
            None
        }
        Err(_) => {
            tracing::warn!(?limit, "artifact page bootstrap timed out");
            None
        }
    }
}

/// The artifact page's `Cache-Control`: it carries one viewer's data, so no
/// shared cache may store it, and the browser asks before every use.
pub const PRIVATE_REVALIDATE: &str = "private, no-cache";

/// `/_clax/<path>`. The bridge is immutable at its versioned URL and
/// revalidated at the bare one (both carry an `ETag`); the shell's bundles
/// and the bridge's lazy parts (`bridge/…`) have content-hashed names.
pub async fn static_file(
    p: Result<Path<String>, PathRejection>,
    RawQuery(query): RawQuery,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let path = path(p)?;
    let file = format!("_clax/{path}");
    // A missing part answers so that a sandboxed page can read the failure
    // (its import is a CORS request); build files whose names start with a
    // dot (a bundler's manifest) are never served.
    let part = path.starts_with("bridge/");
    let hidden = path.split('/').any(|c| c.starts_with('.'));
    let Some(f) = (!hidden).then(|| asset(&file)).flatten() else {
        let mut res = ApiError::not_found().into_response();
        if part {
            res.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                header::HeaderValue::from_static("*"),
            );
        }
        return Ok(res);
    };
    let ct = if path.ends_with(".js") {
        "text/javascript".to_string()
    } else {
        mime_guess::from_path(&path)
            .first_or_octet_stream()
            .to_string()
    };
    if file == BRIDGE {
        let cc = bridge_cache_control(query.as_deref(), &short_hash(&f.metadata.sha256_hash()));
        return Ok(http_cache::tagged(&req, &f.data, cc, || {
            ([(header::CONTENT_TYPE, ct)], f.data.clone().into_owned()).into_response()
        }));
    }
    // The bridge's lazy parts: content-hashed ES modules that pages import,
    // from sandboxed frames too (an opaque origin, so the import is a CORS
    // request). They hold no data, so any origin may read them. A debug
    // build's parts keep one name while `just watch` rebuilds them, so there
    // they are revalidated (with an `ETag`, a `304`).
    if part {
        let cc = if cfg!(debug_assertions) {
            REVALIDATE
        } else {
            IMMUTABLE
        };
        let mut res = http_cache::tagged(&req, &f.data, cc, || {
            ([(header::CONTENT_TYPE, ct)], f.data.clone().into_owned()).into_response()
        });
        res.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            header::HeaderValue::from_static("*"),
        );
        return Ok(res);
    }
    Ok((
        [
            (header::CONTENT_TYPE, ct),
            (header::CACHE_CONTROL, "public, max-age=3600".to_string()),
        ],
        f.data.into_owned(),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_failed_or_slow_bootstrap_is_left_out() {
        let ok = crate::boot::Injected {
            boot: "{}".into(),
            frame: None,
            title: "t".into(),
        };
        let limit = std::time::Duration::from_millis(20);
        assert!(within(limit, async { Ok(Some(ok)) }).await.is_some());
        assert!(
            within(limit, async { Err(ApiError::not_found()) })
                .await
                .is_none()
        );
        assert!(
            within(limit, std::future::pending()).await.is_none(),
            "a store slower than the limit"
        );
    }

    #[test]
    fn only_the_url_naming_the_served_bytes_is_immutable() {
        let served = "0123456789ab";
        let current = if cfg!(debug_assertions) {
            REVALIDATE
        } else {
            IMMUTABLE
        };
        assert_eq!(
            bridge_cache_control(Some("v=0123456789ab"), served),
            current
        );
        assert_eq!(
            bridge_cache_control(Some("x=1&v=0123456789ab"), served),
            current
        );
        assert_eq!(bridge_cache_control(None, served), REVALIDATE);
        assert_eq!(bridge_cache_control(Some(""), served), REVALIDATE);
        assert_eq!(
            bridge_cache_control(Some("v=ffffffffffff"), served),
            REVALIDATE
        );
        assert_eq!(short_hash(&[0xab; 32]), "abababababab");
    }
}
