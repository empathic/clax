//! Serves the embedded web UI: the shell document and static assets.

use crate::error::ApiError;
use crate::http_cache::{self, IMMUTABLE, REVALIDATE};
use crate::routes::artifacts::path;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, RawQuery};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use std::sync::OnceLock;

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct Assets;

/// Where the bridge bundle sits in [`Assets`].
const BRIDGE: &str = "_artifax/bridge.js";

/// A short content hash: the first 12 hex digits of a SHA-256.
fn short_hash(sha256: &[u8; 32]) -> String {
    sha256[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// The bridge version: a short hash of the embedded bridge bundle, computed
/// once (the router computes it at startup), which bridge tags carry as
/// `/_artifax/bridge.js?v=<version>`. Empty when the UI is not built.
pub fn bridge_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        Assets::get(BRIDGE)
            .map(|f| short_hash(&f.metadata.sha256_hash()))
            .unwrap_or_default()
    })
}

/// The bridge's `Cache-Control`: immutable at the URL naming the version of
/// the bytes served (`?v=<hash of them>`), revalidated at any other.
fn bridge_cache_control(query: Option<&str>, served: &str) -> &'static str {
    let v = query
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("v="));
    if v == Some(served) {
        IMMUTABLE
    } else {
        REVALIDATE
    }
}

pub async fn shell(req: HeaderMap) -> Result<Response, ApiError> {
    match Assets::get("index.html") {
        Some(f) => Ok(http_cache::html(&req, &String::from_utf8_lossy(&f.data))),
        None => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "ui_not_built",
            "the web UI has not been built; run `just web`",
        )),
    }
}

/// `/_artifax/<path>`. The bridge is immutable at its versioned URL and
/// revalidated at the bare one (both carry an `ETag`); the shell's bundles
/// have content-hashed names.
pub async fn static_file(
    p: Result<Path<String>, PathRejection>,
    RawQuery(query): RawQuery,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let path = path(p)?;
    let asset = format!("_artifax/{path}");
    let f = Assets::get(&asset).ok_or_else(ApiError::not_found)?;
    let ct = if path.ends_with(".js") {
        "text/javascript".to_string()
    } else {
        mime_guess::from_path(&path)
            .first_or_octet_stream()
            .to_string()
    };
    if asset == BRIDGE {
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
    fn only_the_url_naming_the_served_bytes_is_immutable() {
        let served = "0123456789ab";
        assert_eq!(
            bridge_cache_control(Some("v=0123456789ab"), served),
            IMMUTABLE
        );
        assert_eq!(
            bridge_cache_control(Some("x=1&v=0123456789ab"), served),
            IMMUTABLE
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
