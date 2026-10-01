//! Serves the embedded web UI: the shell document and static assets.

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

pub async fn shell(req: HeaderMap) -> Result<Response, ApiError> {
    match asset("index.html") {
        Some(f) => Ok(http_cache::html(&req, &String::from_utf8_lossy(&f.data))),
        None => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "ui_not_built",
            "the web UI has not been built; run `just web`",
        )),
    }
}

/// Where the runtime contract's type definitions are served under `/_clax/`.
const CONTRACT_PREFIX: &str = "contract/0.2.61/";

/// The type definitions of runtime contract 0.2.61 (`web/contract/0.2.61/`),
/// built into the binary so agents can read them from their daemon at
/// `/_clax/contract/0.2.61/<name>.d.ts`.
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
/// have content-hashed names. `contract/0.2.61/<name>.d.ts` is one of the
/// runtime contract's type definitions, as plain text.
pub async fn static_file(
    p: Result<Path<String>, PathRejection>,
    RawQuery(query): RawQuery,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let path = path(p)?;
    if let Some(name) = path.strip_prefix(CONTRACT_PREFIX) {
        let (_, text) = CONTRACT_FILES
            .iter()
            .find(|(n, _)| *n == name)
            .ok_or_else(ApiError::not_found)?;
        return Ok((
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            *text,
        )
            .into_response());
    }
    let file = format!("_clax/{path}");
    let f = asset(&file).ok_or_else(ApiError::not_found)?;
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
