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
    let etag = etag_of(body);
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
