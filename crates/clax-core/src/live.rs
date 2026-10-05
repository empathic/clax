//! Live pages (spec 2026-10-05-chrome-overlay-design §7): the key a page
//! URL names (origin and path) and the route a thread on it was made at
//! (the query, less tracking parameters, and a hash route).

use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// `artifacts.kind` of a published HTML artifact.
pub const KIND_HTML: &str = "html";
/// `artifacts.kind` of a live page.
pub const KIND_LIVE: &str = "live";
/// The longest page URL accepted, in bytes.
pub const MAX_URL: usize = 4096;
/// The longest route kept, in bytes; a longer one is cut at a character boundary.
pub const MAX_ROUTE: usize = 512;
/// Query parameters that never form part of a route, besides every `utm_*`.
const TRACKING: &[&str] = &["fbclid", "gclid"];

/// A live page's identity: `origin` is the scheme, the lowercased host and
/// the port when it is not the scheme's default; `path` is the parsed path
/// (dot segments resolved, a trailing slash kept).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PageKey {
    pub origin: String,
    pub path: String,
}

/// A page URL split into its key and its route (`None` when empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageUrl {
    pub key: PageKey,
    pub route: Option<String>,
}

impl PageKey {
    /// The page's URL without a route: `origin` followed by `path`.
    pub fn page_url(&self) -> String {
        format!("{}{}", self.origin, self.path)
    }

    /// Whether a scope watch on `scope` covers this page: the same origin,
    /// and a path equal to the scope's or below it (`/` covers every path).
    pub fn covered_by(&self, scope: &PageKey) -> bool {
        if self.origin != scope.origin {
            return false;
        }
        if scope.path == "/" || self.path == scope.path {
            return true;
        }
        let base = scope.path.trim_end_matches('/');
        self.path.starts_with(&format!("{base}/"))
    }
}

/// `s` cut to at most `max` bytes at a character boundary.
fn cut(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// Splits a page URL into its key and route.
///
/// # Errors
/// `invalid_url` for text that is not a URL, names no host or is longer than
/// [`MAX_URL`]; `unsupported_url` for any scheme but `http` and `https`.
pub fn parse_page_url(raw: &str) -> Result<PageUrl> {
    let raw = raw.trim();
    if raw.len() > MAX_URL {
        return Err(CoreError::invalid(
            "invalid_url",
            format!("a page URL is at most {MAX_URL} bytes"),
        ));
    }
    let u = url::Url::parse(raw)
        .map_err(|e| CoreError::invalid("invalid_url", format!("not a URL: {e}")))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(CoreError::invalid(
            "unsupported_url",
            "only http and https pages can be live pages",
        ));
    }
    let host = u
        .host_str()
        .ok_or_else(|| CoreError::invalid("invalid_url", "the URL names no host"))?;
    let origin = match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    };
    let path = if u.path().is_empty() {
        "/".to_string()
    } else {
        u.path().to_string()
    };
    let mut route = String::new();
    if let Some(q) = u.query() {
        let kept: Vec<&str> = q
            .split('&')
            .filter(|kv| {
                let k = kv.split('=').next().unwrap_or("");
                !kv.is_empty() && !k.starts_with("utm_") && !TRACKING.contains(&k)
            })
            .collect();
        if !kept.is_empty() {
            route.push('?');
            route.push_str(&kept.join("&"));
        }
    }
    if let Some(f) = u.fragment()
        && (f.starts_with('/') || f.starts_with("!/"))
    {
        route.push('#');
        route.push_str(f);
    }
    let route = cut(route, MAX_ROUTE);
    Ok(PageUrl {
        key: PageKey { origin, path },
        route: (!route.is_empty()).then_some(route),
    })
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Version 1 of a live page created before any snapshot (spec L3): a page
/// saying no snapshot exists yet, following the page contract.
pub fn placeholder_html(key: &PageKey) -> String {
    let u = escape_html(&key.page_url());
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{u}</title><style>:root{{color-scheme:light dark;--bg:#f7f7f5;--ink:#1c1b19;--muted:#6b6862}}@media (prefers-color-scheme:dark){{:root{{--bg:#141413;--ink:#ecebe7;--muted:#a8a59e}}}}body{{margin:0;padding:48px 16px;background:var(--bg);color:var(--ink);font:16px/1.5 ui-sans-serif,-apple-system,system-ui,sans-serif}}main{{max-width:40rem;margin:0 auto}}p{{color:var(--muted)}}a{{color:inherit}}</style></head><body><main><h1>No snapshot yet</h1><p>Open <a href=\"{u}\">{u}</a> in Chrome and comment on it with the Clax extension. Each comment saves a snapshot of the page here.</p></main></body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        url: String,
        origin: Option<String>,
        path: Option<String>,
        route: Option<String>,
        error: Option<String>,
    }

    #[test]
    fn page_urls_normalize_as_the_cases_say() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("live-url-cases.json")).unwrap();
        for c in cases {
            match (parse_page_url(&c.url), c.error) {
                (Ok(p), None) => {
                    assert_eq!(Some(p.key.origin), c.origin, "{}", c.url);
                    assert_eq!(Some(p.key.path), c.path, "{}", c.url);
                    assert_eq!(p.route, c.route, "{}", c.url);
                }
                (Err(crate::CoreError::Invalid { code, .. }), Some(want)) => {
                    assert_eq!(code, want, "{}", c.url)
                }
                (got, want) => panic!("{}: got {got:?}, want error {want:?}", c.url),
            }
        }
    }

    #[test]
    fn routes_are_cut_at_a_character_boundary() {
        let long = format!("http://localhost/?q={}", "é".repeat(400));
        let r = parse_page_url(&long).unwrap().route.unwrap();
        assert!(r.len() <= MAX_ROUTE);
        assert!(r.is_char_boundary(r.len()));
    }

    #[test]
    fn urls_over_the_limit_are_refused() {
        let long = format!("http://localhost/{}", "a".repeat(MAX_URL));
        assert!(
            matches!(parse_page_url(&long), Err(crate::CoreError::Invalid { code, .. }) if code == "invalid_url")
        );
    }

    #[test]
    fn a_scope_covers_its_path_and_below_on_the_same_origin() {
        let k = |o: &str, p: &str| PageKey {
            origin: o.into(),
            path: p.into(),
        };
        let root = k("http://localhost:5173", "/");
        assert!(k("http://localhost:5173", "/settings").covered_by(&root));
        assert!(
            k("http://localhost:5173", "/docs/a").covered_by(&k("http://localhost:5173", "/docs"))
        );
        assert!(
            k("http://localhost:5173", "/docs/a").covered_by(&k("http://localhost:5173", "/docs/"))
        );
        assert!(
            k("http://localhost:5173", "/docs").covered_by(&k("http://localhost:5173", "/docs"))
        );
        assert!(
            !k("http://localhost:5173", "/docsx").covered_by(&k("http://localhost:5173", "/docs"))
        );
        assert!(!k("http://localhost:3000", "/").covered_by(&root));
    }

    #[test]
    fn the_placeholder_escapes_the_url() {
        let html = placeholder_html(&PageKey {
            origin: "http://localhost:1".into(),
            path: "/<b>\"".into(),
        });
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("/&lt;b&gt;&quot;"));
        assert!(!html.contains("<b>\""));
    }
}
