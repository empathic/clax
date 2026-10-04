//! Browser caching rules. Every HTML response is revalidated on every load
//! (`no-cache`) and carries an `ETag`, answering `304 Not Modified` when the
//! browser's copy is current; a long lifetime is only ever given to a URL
//! whose bytes can never change (a version's non-HTML file, an asset, the
//! bridge at its versioned URL).

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use std::hash::Hasher;

/// For a URL whose bytes never change.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Store, but ask the daemon before every use.
pub const REVALIDATE: &str = "no-cache";

/// A strong entity tag for `bytes`: a quoted 64-bit hash, stable for a build
/// of the daemon (a different build may tag the same bytes differently, which
/// costs one full response).
pub fn etag_of(bytes: &[u8]) -> String {
    let mut h = std::hash::DefaultHasher::new();
    h.write(bytes);
    format!("\"{:016x}\"", h.finish())
}

/// Whether the request's `If-None-Match` names `etag` (or is `*`); a weak
/// `W/` prefix is ignored, as the weak comparison `If-None-Match` uses.
pub fn matches(req: &HeaderMap, etag: &str) -> bool {
    req.get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim())
        .any(|t| t == "*" || t.strip_prefix("W/").unwrap_or(t) == etag)
}

/// A response with `cache_control` and an `ETag` for `body`: `304` with no
/// body when the request already holds it, else `200` with `body` from
/// `respond`.
pub fn tagged(
    req: &HeaderMap,
    body: &[u8],
    cache_control: &'static str,
    respond: impl FnOnce() -> Response,
) -> Response {
    tagged_as(req, etag_of(body), cache_control, respond)
}

/// [`tagged`] with the entity tag given (from [`etag_of`]) rather than
/// computed from the body.
pub fn tagged_as(
    req: &HeaderMap,
    etag: String,
    cache_control: &'static str,
    respond: impl FnOnce() -> Response,
) -> Response {
    let mut res = if matches(req, &etag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        respond()
    };
    let h = res.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    h.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).expect("hex in quotes is a valid header value"),
    );
    res
}

/// An HTML page: revalidated on every load, with an `ETag`.
pub fn html(req: &HeaderMap, page: &str) -> Response {
    tagged(req, page.as_bytes(), REVALIDATE, || {
        Html(page.to_string()).into_response()
    })
}

/// Middleware for the JSON `GET` routes: a `200` JSON response gets an
/// `ETag` of its bytes and, unless the route set its own, `Cache-Control:
/// no-cache`; a request whose `If-None-Match` names that tag gets `304` with
/// no body. The tag hashes the bytes sent, so a response that differs by
/// caller (the token, a viewer cookie) never matches another caller's copy.
pub async fn api_etag(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if req.method() != axum::http::Method::GET {
        return next.run(req).await;
    }
    let inm = req.headers().clone();
    let res = next.run(req).await;
    let json = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if res.status() != StatusCode::OK || !json || res.headers().contains_key(header::ETAG) {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    // JSON responses are whole buffers already.
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let tag = etag_of(&bytes);
    parts.headers.insert(
        header::ETAG,
        HeaderValue::from_str(&tag).expect("hex in quotes is a valid header value"),
    );
    parts
        .headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static(REVALIDATE));
    if matches(&inm, &tag) {
        parts.status = StatusCode::NOT_MODIFIED;
        parts.headers.remove(header::CONTENT_LENGTH);
        parts.headers.remove(header::CONTENT_TYPE);
        return Response::from_parts(parts, axum::body::Body::empty());
    }
    Response::from_parts(parts, axum::body::Body::from(bytes))
}

/// Smallest body worth compressing.
pub const COMPRESS_MIN_BYTES: u16 = 1024;

/// Whether a response of `content_type` is text worth compressing: JSON,
/// HTML, JavaScript, CSS, SVG and plain text. Event streams, images, fonts
/// and other already-compressed or binary types are sent as they are.
pub fn compressible(content_type: &str) -> bool {
    [
        "application/json",
        "text/html",
        "text/javascript",
        "application/javascript",
        "text/css",
        "image/svg+xml",
        "text/plain",
    ]
    .iter()
    .any(|t| content_type.starts_with(t))
}

/// gzip and brotli, as the request's `Accept-Encoding` prefers, for bodies
/// of [`COMPRESS_MIN_BYTES`] or more whose type is [`compressible`].
pub fn compression()
-> tower_http::compression::CompressionLayer<impl tower_http::compression::Predicate> {
    use tower_http::compression::predicate::{Predicate, SizeAbove};
    let text =
        |_: StatusCode, _: axum::http::Version, h: &HeaderMap, _: &axum::http::Extensions| {
            h.get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(compressible)
        };
    // Level 4 of either: most of the size win at a small part of the CPU.
    tower_http::compression::CompressionLayer::new()
        .no_deflate()
        .no_zstd()
        .quality(tower_http::CompressionLevel::Precise(4))
        .compress_when(SizeAbove::new(COMPRESS_MIN_BYTES).and(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(inm: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_str(inm).unwrap());
        h
    }

    #[test]
    fn if_none_match_names_the_tag_in_a_list_weakly_or_by_star() {
        let tag = etag_of(b"page");
        assert_eq!(tag, etag_of(b"page"));
        assert_ne!(tag, etag_of(b"other"));
        assert!(matches(&with(&tag), &tag));
        assert!(matches(&with(&format!("\"x\", W/{tag}")), &tag));
        assert!(matches(&with("*"), &tag));
        assert!(!matches(&with("\"x\""), &tag));
        assert!(!matches(&HeaderMap::new(), &tag));
    }

    #[test]
    fn only_text_types_compress() {
        for t in [
            "application/json",
            "text/html; charset=utf-8",
            "text/javascript",
            "image/svg+xml",
        ] {
            assert!(compressible(t), "{t}");
        }
        for t in [
            "text/event-stream",
            "image/png",
            "font/woff2",
            "application/zip",
            "application/octet-stream",
        ] {
            assert!(!compressible(t), "{t}");
        }
    }

    #[test]
    fn html_is_no_cache_and_not_modified_when_current() {
        let res = html(&HeaderMap::new(), "<p>x</p>");
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
        let tag = res.headers()[header::ETAG].to_str().unwrap().to_string();
        let res = html(&with(&tag), "<p>x</p>");
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(res.headers()[header::ETAG], tag.as_str());
    }
}
