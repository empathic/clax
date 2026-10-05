//! Serves the embedded web UI: the shell's two entries (the gallery and the
//! artifact view) and static assets.

use crate::error::ApiError;
use crate::http_cache::{self, IMMUTABLE, REVALIDATE};
use crate::routes::artifacts::path;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, RawQuery};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
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

/// A shell page, which no other page may frame: the shell holds the viewer's
/// consent and comment controls, and a page that framed it could lay its own
/// content over them or post to it. `frame-ancestors 'none'` is its own
/// policy and restricts nothing else; `X-Frame-Options` covers browsers
/// without CSP.
fn unframeable(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    res
}

/// An entry of the shell (`index.html` or `artifact.html`) as an HTML page.
fn entry(req: &HeaderMap, name: &str) -> Result<Response, ApiError> {
    match asset(name) {
        Some(f) => Ok(unframeable(http_cache::html(
            req,
            &String::from_utf8_lossy(&f.data),
        ))),
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
        HeaderValue::from_static(PRIVATE_REVALIDATE),
    );
    h.insert(header::VARY, HeaderValue::from_static("Cookie"));
    Ok(unframeable(res))
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

/// Where the runtime contract's type definitions are served under `/_clax/`.
const CONTRACT_PREFIX: &str = "contract/0.2.61/";

/// Clax's additions to the contract (`web/contract/clax-extensions.d.ts`),
/// served under `/_clax/` beside the unchanged 0.2.61 files.
const EXTENSIONS_PATH: &str = "contract/clax-extensions.d.ts";
const EXTENSIONS: &str = include_str!("../../../../web/contract/clax-extensions.d.ts");

/// The type definitions of runtime contract 0.2.61 (`web/contract/0.2.61/`),
/// claude.ai's files byte for byte, built into the binary so agents can read
/// them from their daemon at `/_clax/contract/0.2.61/<name>.d.ts`.
const CONTRACT_FILES: &[(&str, &str)] = &[
    (
        "artifact.d.ts",
        include_str!("../../../../web/contract/0.2.61/artifact.d.ts"),
    ),
    (
        "assets.d.ts",
        include_str!("../../../../web/contract/0.2.61/assets.d.ts"),
    ),
    (
        "claude.d.ts",
        include_str!("../../../../web/contract/0.2.61/claude.d.ts"),
    ),
    (
        "comments.d.ts",
        include_str!("../../../../web/contract/0.2.61/comments.d.ts"),
    ),
    (
        "db.d.ts",
        include_str!("../../../../web/contract/0.2.61/db.d.ts"),
    ),
    (
        "downloads.d.ts",
        include_str!("../../../../web/contract/0.2.61/downloads.d.ts"),
    ),
    (
        "files.d.ts",
        include_str!("../../../../web/contract/0.2.61/files.d.ts"),
    ),
    (
        "mcp.d.ts",
        include_str!("../../../../web/contract/0.2.61/mcp.d.ts"),
    ),
    (
        "permissions.d.ts",
        include_str!("../../../../web/contract/0.2.61/permissions.d.ts"),
    ),
    (
        "room.d.ts",
        include_str!("../../../../web/contract/0.2.61/room.d.ts"),
    ),
    (
        "sample.d.ts",
        include_str!("../../../../web/contract/0.2.61/sample.d.ts"),
    ),
    (
        "self.d.ts",
        include_str!("../../../../web/contract/0.2.61/self.d.ts"),
    ),
    (
        "user.d.ts",
        include_str!("../../../../web/contract/0.2.61/user.d.ts"),
    ),
];

/// `/_clax/<path>`. The bridge is immutable at its versioned URL and
/// revalidated at the bare one (both carry an `ETag`); the shell's bundles
/// and the bridge's lazy parts (`bridge/…`) have content-hashed names.
/// `contract/0.2.61/<name>.d.ts` is one of the runtime contract's type
/// definitions, and `contract/clax-extensions.d.ts` Clax's additions to
/// them, as plain text.
pub async fn static_file(
    p: Result<Path<String>, PathRejection>,
    RawQuery(query): RawQuery,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let path = path(p)?;
    let contract = if path == EXTENSIONS_PATH {
        Some(EXTENSIONS)
    } else if let Some(name) = path.strip_prefix(CONTRACT_PREFIX) {
        let (_, text) = CONTRACT_FILES
            .iter()
            .find(|(n, _)| *n == name)
            .ok_or_else(ApiError::not_found)?;
        Some(*text)
    } else {
        None
    };
    if let Some(text) = contract {
        return Ok((
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            text,
        )
            .into_response());
    }
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
    // The shell's bundles are named by a hash of their bytes. A debug build
    // reads them from disk, where a rebuild may replace them, so it revalidates.
    if path.starts_with("shell/") && hashed_name(&path) {
        let cc = if cfg!(debug_assertions) {
            REVALIDATE
        } else {
            IMMUTABLE
        };
        return Ok(http_cache::tagged(&req, &f.data, cc, || {
            ([(header::CONTENT_TYPE, ct)], f.data.clone().into_owned()).into_response()
        }));
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

/// Whether the file name ends in a bundler's content hash: `<name>-<8 of
/// [A-Za-z0-9_-]>.<ext>`.
fn hashed_name(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    let Some((stem, _ext)) = file.rsplit_once('.') else {
        return false;
    };
    stem.len() > 9
        && stem.is_char_boundary(stem.len() - 9)
        && stem[stem.len() - 9..].starts_with('-')
        && stem[stem.len() - 8..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
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
    fn hashed_names_are_recognised() {
        assert!(hashed_name("shell/artifact-CbAhy43s.js"));
        assert!(hashed_name("shell/HaikuLine-Be2ioBd-.js"));
        assert!(hashed_name("shell/haiku-DXXqhL_5.json"));
        assert!(!hashed_name("shell/manifest.json"));
        assert!(!hashed_name("mark.svg"));
        assert!(!hashed_name("shell/x-short.js"));
    }

    #[test]
    fn every_contract_file_is_built_in() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/contract/0.2.61");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        on_disk.sort();
        let built: Vec<String> = CONTRACT_FILES.iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(built, on_disk);
    }

    /// SHA-256 of claude.ai's 0.2.61 files as shipped; Clax's additions go
    /// in `clax-extensions.d.ts`, never into these.
    const UPSTREAM_SHA256: &[(&str, &str)] = &[
        (
            "artifact.d.ts",
            "978bbdde2dadc7b7d888bd28987bef97a988a6c75b35c244ede518dd645e06a8",
        ),
        (
            "assets.d.ts",
            "160805f8e9906d3de0f75ae03236128735002ac8d47f0be2726356ddd30ba667",
        ),
        (
            "claude.d.ts",
            "54bfa849203cd184725473e365669e501612a826a13651c531d5d01c7ad8ae42",
        ),
        (
            "comments.d.ts",
            "09b354e492a1006e9f593843459975ab4ffa58902c646736b0d60ded4394520b",
        ),
        (
            "db.d.ts",
            "fd2989b7a812c9e925483dd217856cb13ac3515bd7597ab7e90825cd62105f97",
        ),
        (
            "downloads.d.ts",
            "5875d79313416c4a99e9ce0e5021083f15c2cb61e53b0ae482e34ca9883b3ad9",
        ),
        (
            "files.d.ts",
            "13b170a86bc9e98a61aecc419d1ab151ed1a29612d1068c3cc945258f542a48a",
        ),
        (
            "mcp.d.ts",
            "b508739a82a60199abf28ede69495b510bfd40c80ff123bf061aecf3defa3c7d",
        ),
        (
            "permissions.d.ts",
            "2152b995eba82f96e66e41ed9e4c8fda14a1e9eaa0b284289737cf99743fbdcc",
        ),
        (
            "room.d.ts",
            "2e846b4678eb852bc4030b8fea1e4b8d071c278e6a441c6f07fd6f9bfd092729",
        ),
        (
            "sample.d.ts",
            "aff7f58ecbe77359d179b9a103f4572dd40b98ada302e8c3fc8a4c8807fd256f",
        ),
        (
            "self.d.ts",
            "162ec5ebc98126b8a532348a2a6af9815f8cb667822105dce02748cb165e3515",
        ),
        (
            "user.d.ts",
            "ddb94ee7b94f02d932dbbf08f2ae96986a0d3aa248177407eb384e0fb467994e",
        ),
    ];

    #[test]
    fn the_contract_files_are_the_upstream_bytes() {
        use sha2::Digest;
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let got: Vec<(&str, String)> = CONTRACT_FILES
            .iter()
            .map(|(n, text)| (*n, hex(&sha2::Sha256::digest(text.as_bytes()))))
            .collect();
        let want: Vec<(&str, String)> = UPSTREAM_SHA256
            .iter()
            .map(|(n, h)| (*n, h.to_string()))
            .collect();
        assert_eq!(got, want);
        assert!(EXTENSIONS.contains("shell_input_recent"));
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
