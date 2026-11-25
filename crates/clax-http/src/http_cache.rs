//! Browser caching rules. Every HTML response is revalidated on every load
//! (`no-cache`) and carries an `ETag`, answering `304 Not Modified` when the
//! browser's copy is current; a long lifetime is only ever given to a URL
//! whose bytes can never change (a version's non-HTML file, an asset, the
//! bridge at its versioned URL). A stored file (a version's file, an asset,
//! a draft's file) is served with byte ranges ([`stored_file`]), which a
//! browser needs to seek in a video or audio file.

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use std::hash::Hasher;

/// For a URL whose bytes never change.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Store, but ask the daemon before every use.
pub const REVALIDATE: &str = "no-cache";
/// [`REVALIDATE`] in the browser's own cache only: for an answer that may
/// differ by caller (the token, a viewer cookie).
pub const PRIVATE_REVALIDATE: &str = "private, no-cache";

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

/// What a request's `Range` asks of a stored file of `size` bytes
/// ([`range_of`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ranged {
    /// All of it: no `Range`, one this server ignores, or an `If-Range`
    /// that does not name the file's current `ETag`.
    Full,
    /// Bytes `start..=end`, both within the file.
    Part { start: u64, end: u64 },
    /// One range that starts past the file's end (or a suffix of none).
    Unsatisfiable,
}

/// The byte range a request asks of a stored file of `size` bytes whose
/// strong entity tag is `etag` (RFC 9110 §14): a single `bytes=a-b`,
/// `bytes=a-` or `bytes=-n` is a [`Ranged::Part`] (`b` past the end is cut
/// to it, `-n` longer than the file is all of it), or
/// [`Ranged::Unsatisfiable`] when it starts at or past the end (`-0`, or
/// any range of an empty file). [`Ranged::Full`] otherwise: no `Range`, a
/// malformed one, another unit, more than one range (the whole file is
/// sent, as the RFC allows), or an `If-Range` that is not `etag` itself (a
/// date, a weak tag, another tag, or any `If-Range` when the file has no
/// tag).
pub fn range_of(req: &HeaderMap, size: u64, etag: Option<&str>) -> Ranged {
    let Some(range) = req.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return Ranged::Full;
    };
    if let Some(cond) = req.get(header::IF_RANGE) {
        let cond = cond.to_str().unwrap_or("").trim();
        // Strong comparison: a weak tag never matches.
        if etag.is_none_or(|tag| cond != tag) {
            return Ranged::Full;
        }
    }
    let Some((unit, set)) = range.split_once('=') else {
        return Ranged::Full;
    };
    if !unit.trim().eq_ignore_ascii_case("bytes") || set.contains(',') {
        return Ranged::Full;
    }
    let Some((first, last)) = set.trim().split_once('-') else {
        return Ranged::Full;
    };
    let num = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u64>().ok())
            .flatten()
    };
    let (first, last) = (first.trim(), last.trim());
    if first.is_empty() {
        // A suffix: the last `n` bytes.
        return match num(last) {
            None => Ranged::Full,
            Some(0) => Ranged::Unsatisfiable,
            Some(_) if size == 0 => Ranged::Unsatisfiable,
            Some(n) => Ranged::Part {
                start: size.saturating_sub(n),
                end: size - 1,
            },
        };
    }
    let Some(start) = num(first) else {
        return Ranged::Full;
    };
    let end = if last.is_empty() {
        u64::MAX
    } else {
        match num(last) {
            Some(end) if end >= start => end,
            _ => return Ranged::Full,
        }
    };
    if start >= size {
        return Ranged::Unsatisfiable;
    }
    Ranged::Part {
        start,
        end: end.min(size - 1),
    }
}

/// A stored file's `bytes` as the request asks, every answer with
/// `Accept-Ranges: bytes`:
///
/// - `304` with no body when `etag` is given and `If-None-Match` names it,
///   as [`tagged_as`] answers (checked before any `Range`);
/// - `206` of the one range [`range_of`] finds, with `Content-Range: bytes
///   a-b/size` and its `Content-Length`;
/// - `416` with `Content-Range: bytes */size`, no body, `Cache-Control:
///   no-store` and no `ETag`, for a range past the end;
/// - else `200` of all of `bytes`.
///
/// `respond` makes the `200`, `206` or `416` from its status and body,
/// with the route's own headers (content type, policy); `cache_control`
/// and `etag`, when given, are set on the `304`, `200` and `206`. A `HEAD`
/// is answered as a `GET` (the router drops the body). The range is cut
/// from `bytes` without a copy.
pub fn stored_file(
    req: &HeaderMap,
    bytes: Vec<u8>,
    etag: Option<String>,
    cache_control: &'static str,
    respond: impl FnOnce(StatusCode, axum::body::Body) -> Response,
) -> Response {
    use axum::body::{Body, Bytes};
    let size = bytes.len() as u64;
    let validated = |mut res: Response| {
        let h = res.headers_mut();
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(cache_control),
        );
        if let Some(tag) = &etag {
            h.insert(
                header::ETAG,
                HeaderValue::from_str(tag).expect("hex in quotes is a valid header value"),
            );
        }
        res
    };
    if etag.as_deref().is_some_and(|tag| matches(req, tag)) {
        return validated(StatusCode::NOT_MODIFIED.into_response());
    }
    let (mut res, len, range) = match range_of(req, size, etag.as_deref()) {
        Ranged::Full => (
            validated(respond(StatusCode::OK, Body::from(bytes))),
            size,
            None,
        ),
        Ranged::Part { start, end } => {
            // Both are below `size`, a `usize`.
            let part = Bytes::from(bytes).slice(start as usize..=end as usize);
            (
                validated(respond(StatusCode::PARTIAL_CONTENT, Body::from(part))),
                end - start + 1,
                Some(format!("bytes {start}-{end}/{size}")),
            )
        }
        Ranged::Unsatisfiable => {
            let mut res = respond(StatusCode::RANGE_NOT_SATISFIABLE, Body::empty());
            let h = res.headers_mut();
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            h.remove(header::ETAG);
            (res, 0, Some(format!("bytes */{size}")))
        }
    };
    let h = res.headers_mut();
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    if let Some(range) = range {
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&range).expect("digits are a valid header value"),
        );
    }
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
/// private, no-cache` (an answer may differ by caller, so no shared cache
/// keeps it); a request whose `If-None-Match` names that tag gets `304` with
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
        .or_insert(HeaderValue::from_static(PRIVATE_REVALIDATE));
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
/// of [`COMPRESS_MIN_BYTES`] or more whose type is [`compressible`]. Never
/// on wasm32: a Worker's runtime encodes a response body as its
/// `Content-Encoding` says, so a body compressed here would be compressed
/// twice; the runtime compresses responses itself.
pub fn compression()
-> tower_http::compression::CompressionLayer<impl tower_http::compression::Predicate> {
    use tower_http::compression::predicate::{Predicate, SizeAbove};
    let text =
        |_: StatusCode, _: axum::http::Version, h: &HeaderMap, _: &axum::http::Extensions| {
            !cfg!(target_arch = "wasm32")
                && h.get(header::CONTENT_TYPE)
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
#[warn(clippy::disallowed_methods)]
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

    fn ranged(range: &str, if_range: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, HeaderValue::from_str(range).unwrap());
        if let Some(c) = if_range {
            h.insert(header::IF_RANGE, HeaderValue::from_str(c).unwrap());
        }
        h
    }

    #[test]
    fn a_single_byte_range_is_a_part_of_the_file() {
        let part = |s, e| Ranged::Part { start: s, end: e };
        let of = |r: &str| range_of(&ranged(r, None), 100, None);
        assert_eq!(of("bytes=0-9"), part(0, 9));
        assert_eq!(of("bytes=90-"), part(90, 99));
        assert_eq!(of("bytes=-10"), part(90, 99));
        assert_eq!(of("bytes=-1000"), part(0, 99));
        assert_eq!(of("bytes=50-1000"), part(50, 99));
        assert_eq!(of("Bytes = 5-5"), part(5, 5));
        assert_eq!(of("bytes=100-"), Ranged::Unsatisfiable);
        assert_eq!(of("bytes=100-200"), Ranged::Unsatisfiable);
        assert_eq!(of("bytes=-0"), Ranged::Unsatisfiable);
        assert_eq!(
            range_of(&ranged("bytes=-5", None), 0, None),
            Ranged::Unsatisfiable
        );
        assert_eq!(
            range_of(&ranged("bytes=0-", None), 0, None),
            Ranged::Unsatisfiable
        );
        // Ignored, so the whole file: several ranges, another unit, and
        // what does not parse.
        for r in [
            "bytes=0-1,5-6",
            "items=0-1",
            "bytes=5-1",
            "bytes=a-b",
            "bytes=-",
            "bytes=+1-2",
            "bytes 0-1",
        ] {
            assert_eq!(of(r), Ranged::Full, "{r}");
        }
        assert_eq!(range_of(&HeaderMap::new(), 100, None), Ranged::Full);
    }

    #[test]
    fn if_range_honours_the_range_only_for_the_current_strong_tag() {
        let tag = etag_of(b"file");
        let of = |c: &str, etag: Option<&str>| range_of(&ranged("bytes=0-0", Some(c)), 10, etag);
        let part = Ranged::Part { start: 0, end: 0 };
        assert_eq!(of(&tag, Some(&tag)), part);
        assert_eq!(of("\"other\"", Some(&tag)), Ranged::Full);
        assert_eq!(of(&format!("W/{tag}"), Some(&tag)), Ranged::Full);
        assert_eq!(
            of("Wed, 21 Oct 2015 07:28:00 GMT", Some(&tag)),
            Ranged::Full
        );
        assert_eq!(of(&tag, None), Ranged::Full);
    }

    async fn body_of(res: Response) -> Vec<u8> {
        axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    fn file(req: &HeaderMap, etag: Option<String>) -> Response {
        let bytes: Vec<u8> = (0..100u8).collect();
        stored_file(req, bytes, etag, IMMUTABLE, |status, body| {
            (status, [(header::CONTENT_TYPE, "video/mp4")], body).into_response()
        })
    }

    #[tokio::test]
    async fn a_stored_file_answers_200_206_416_and_304() {
        let tag = etag_of(b"file");
        let res = file(&HeaderMap::new(), Some(tag.clone()));
        assert_eq!(res.status(), StatusCode::OK);
        let h = res.headers();
        assert_eq!(h[header::ACCEPT_RANGES], "bytes");
        assert_eq!(h[header::CONTENT_LENGTH], "100");
        assert_eq!(h[header::CACHE_CONTROL], IMMUTABLE);
        assert_eq!(h[header::ETAG], tag.as_str());
        assert!(!h.contains_key(header::CONTENT_RANGE));
        assert_eq!(body_of(res).await.len(), 100);

        let res = file(&ranged("bytes=10-19", None), Some(tag.clone()));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        let h = res.headers();
        assert_eq!(h[header::CONTENT_RANGE], "bytes 10-19/100");
        assert_eq!(h[header::CONTENT_LENGTH], "10");
        assert_eq!(h[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(h[header::CACHE_CONTROL], IMMUTABLE);
        assert_eq!(h[header::ETAG], tag.as_str());
        assert_eq!(body_of(res).await, (10..20u8).collect::<Vec<_>>());

        let res = file(&ranged("bytes=-3", None), None);
        assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 97-99/100");
        assert!(!res.headers().contains_key(header::ETAG));
        assert_eq!(body_of(res).await, vec![97, 98, 99]);

        let res = file(&ranged("bytes=200-", None), Some(tag.clone()));
        assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        let h = res.headers();
        assert_eq!(h[header::CONTENT_RANGE], "bytes */100");
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
        assert!(!h.contains_key(header::ETAG));
        assert!(body_of(res).await.is_empty());

        // If-None-Match comes before the range.
        let mut req = ranged("bytes=0-9", None);
        req.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&tag).unwrap());
        let res = file(&req, Some(tag.clone()));
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(res.headers()[header::ETAG], tag.as_str());
        assert!(body_of(res).await.is_empty());
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
