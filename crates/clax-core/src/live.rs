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
    /// and a path equal to the scope's or below it (`/` covers every path):
    /// the scope's path, without one trailing slash, followed by `/`.
    pub fn covered_by(&self, scope: &PageKey) -> bool {
        if self.origin != scope.origin {
            return false;
        }
        if scope.path == "/" || self.path == scope.path {
            return true;
        }
        let base = scope.path.strip_suffix('/').unwrap_or(&scope.path);
        self.path.starts_with(&format!("{base}/"))
    }
}

/// Whether `origin` (normalized, as [`parse_page_url`] writes it) names
/// this machine by a loopback name: `localhost` or any `*.localhost`,
/// `127.0.0.0/8`, or `[::1]`.
pub fn is_loopback_origin(origin: &str) -> bool {
    let Ok(u) = url::Url::parse(origin) else {
        return false;
    };
    match u.host() {
        Some(url::Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost"),
        Some(url::Host::Ipv4(a)) => a.is_loopback(),
        Some(url::Host::Ipv6(a)) => a.is_loopback(),
        None => false,
    }
}

/// Whether two origins are of one host family, so a join of them may be
/// suggested (spec 2026-10-05-chrome-overlay-design §7.2, owner decision
/// 2026-10-06): different origins of one scheme whose hosts are both
/// loopback names ([`is_loopback_origin`]), such as one dev server's ports.
pub fn same_host_family(a: &str, b: &str) -> bool {
    a != b
        && a.split_once("://").map(|x| x.0) == b.split_once("://").map(|x| x.0)
        && is_loopback_origin(a)
        && is_loopback_origin(b)
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

/// The longest path pattern of a merge rule, in bytes.
pub const MAX_PATTERN: usize = 256;
/// The most segments a path pattern has.
pub const MAX_PATTERN_SEGMENTS: usize = 16;
/// The longest `:name` of a path pattern, in characters (without the colon).
pub const MAX_PARAM_NAME: usize = 32;

/// One segment of a [`PathPattern`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    /// Matches exactly this segment.
    Literal(String),
    /// `:name`: matches any one non-empty segment.
    Param,
    /// `*`, last only: matches the rest of the path, one or more segments
    /// whose first is not empty.
    Rest,
}

/// A merge rule's path pattern (spec 2026-10-05-chrome-overlay-design §7.1):
/// `/`-separated segments, each a literal, `:name` (any one non-empty
/// segment) or, last only, `*` (one or more segments). No regular
/// expressions, no empty segments, no trailing slash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathPattern {
    text: String,
    segs: Vec<Seg>,
}

/// Bytes a literal segment may hold besides ASCII letters and digits: path
/// characters the URL parser leaves alone, `%` followed by two hex digits,
/// and `:` past the first byte; never `*`, `/`, `?` or `#`.
const LITERAL_EXTRA: &[u8] = b"-._~!$&'()+,;=@%:";

fn bad_pattern(msg: impl Into<String>) -> CoreError {
    CoreError::invalid("invalid_pattern", msg)
}

/// A checked literal segment, or `None`.
fn literal(p: &str) -> Option<Seg> {
    let b = p.as_bytes();
    let chars_ok = b
        .iter()
        .all(|c| c.is_ascii_alphanumeric() || LITERAL_EXTRA.contains(c));
    let escapes_ok = b.iter().enumerate().all(|(j, c)| {
        *c != b'%'
            || (b.get(j + 1).is_some_and(u8::is_ascii_hexdigit)
                && b.get(j + 2).is_some_and(u8::is_ascii_hexdigit))
    });
    (chars_ok && escapes_ok && p != "." && p != "..").then(|| Seg::Literal(p.to_string()))
}

impl PathPattern {
    /// Parses and checks a pattern.
    ///
    /// # Errors
    /// `invalid_pattern` for a pattern that does not start with `/`, is
    /// longer than [`MAX_PATTERN`] bytes or has more than
    /// [`MAX_PATTERN_SEGMENTS`] segments, has an empty or dot segment, a
    /// `:name` that is not 1 to [`MAX_PARAM_NAME`] ASCII letters, digits or
    /// `_`, a `*` that is not the whole last segment, a literal with any
    /// other character, no `:name` or `*` at all, or no literal segment (so
    /// no rule merges a whole site).
    pub fn parse(text: &str) -> Result<PathPattern> {
        if text.len() > MAX_PATTERN {
            return Err(bad_pattern(format!(
                "a pattern is at most {MAX_PATTERN} bytes"
            )));
        }
        let Some(rest) = text.strip_prefix('/') else {
            return Err(bad_pattern("a pattern starts with /"));
        };
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() > MAX_PATTERN_SEGMENTS {
            return Err(bad_pattern(format!(
                "a pattern has at most {MAX_PATTERN_SEGMENTS} segments"
            )));
        }
        let mut segs = Vec::with_capacity(parts.len());
        for (i, p) in parts.iter().enumerate() {
            let seg = if p.is_empty() {
                return Err(bad_pattern(
                    "a pattern has no empty segment and no trailing slash",
                ));
            } else if *p == "*" {
                if i + 1 != parts.len() {
                    return Err(bad_pattern("* is only the last segment"));
                }
                Seg::Rest
            } else if let Some(name) = p.strip_prefix(':') {
                let ok = !name.is_empty()
                    && name.chars().count() <= MAX_PARAM_NAME
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
                if !ok {
                    return Err(bad_pattern(format!(
                        ":name is 1 to {MAX_PARAM_NAME} ASCII letters, digits or _"
                    )));
                }
                Seg::Param
            } else {
                literal(p).ok_or_else(|| {
                    bad_pattern(format!("segment '{p}' is not a literal, :name or *"))
                })?
            };
            segs.push(seg);
        }
        if !segs.iter().any(|s| matches!(s, Seg::Param | Seg::Rest)) {
            return Err(bad_pattern("a pattern has a :name or * segment"));
        }
        if !segs.iter().any(|s| matches!(s, Seg::Literal(_))) {
            return Err(bad_pattern("a pattern has a literal segment"));
        }
        Ok(PathPattern {
            text: text.to_string(),
            segs,
        })
    }

    /// The pattern as written; also the path of its canonical live page.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether `path` (a live page's path) matches.
    pub fn matches(&self, path: &str) -> bool {
        let Some(rest) = path.strip_prefix('/') else {
            return false;
        };
        let parts: Vec<&str> = rest.split('/').collect();
        for (i, seg) in self.segs.iter().enumerate() {
            let part = parts.get(i).copied();
            match seg {
                Seg::Rest => return part.is_some_and(|p| !p.is_empty()),
                Seg::Param if part.is_none_or(str::is_empty) => return false,
                Seg::Literal(l) if part != Some(l.as_str()) => return false,
                _ => {}
            }
        }
        parts.len() == self.segs.len()
    }

    /// How specific the pattern is: its literal segments, then its `:name`
    /// segments; the greater wins.
    pub fn specificity(&self) -> (usize, usize) {
        let literals = self
            .segs
            .iter()
            .filter(|s| matches!(s, Seg::Literal(_)))
            .count();
        let params = self.segs.iter().filter(|s| matches!(s, Seg::Param)).count();
        (literals, params)
    }
}

/// The rule that maps `path`, of `rules` given oldest first: of those whose
/// pattern matches, the most specific ([`PathPattern::specificity`]), then
/// the oldest. Patterns that do not parse are skipped.
pub fn winning_rule<'a, T>(
    rules: &'a [T],
    pattern: impl Fn(&T) -> &str,
    path: &str,
) -> Option<&'a T> {
    let mut best: Option<(&T, (usize, usize))> = None;
    for r in rules {
        let Ok(p) = PathPattern::parse(pattern(r)) else {
            continue;
        };
        if !p.matches(path) {
            continue;
        }
        let s = p.specificity();
        if best.is_none_or(|(_, b)| s > b) {
            best = Some((r, s));
        }
    }
    best.map(|(r, _)| r)
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
        // `é` is two bytes, so byte 511 falls inside one; the cut backs off to 510.
        let s = "é".repeat(400);
        assert!(!s.is_char_boundary(511));
        let r = cut(s.clone(), 511);
        assert_eq!(r.len(), 510);
        assert!(s.starts_with(&r));
        assert_eq!(cut("abc".into(), 511), "abc");
    }

    #[test]
    fn long_routes_are_cut_to_the_limit() {
        // The parser percent-encodes non-ASCII, so a parsed route is ASCII.
        let long = format!("http://localhost/?q={}", "é".repeat(400));
        let r = parse_page_url(&long).unwrap().route.unwrap();
        assert_eq!(r.len(), MAX_ROUTE);
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
        assert!(
            !k("http://localhost:5173", "/docs").covered_by(&k("http://localhost:5173", "/docs/")),
            "a trailing slash is kept"
        );
        assert!(
            !k("http://localhost:5173", "/docs/x")
                .covered_by(&k("http://localhost:5173", "/docs//")),
            "only one trailing slash ends the scope's path"
        );
        assert!(
            k("http://localhost:5173", "/docs//x")
                .covered_by(&k("http://localhost:5173", "/docs//"))
        );
        assert!(!k("http://localhost:5173", "/x").covered_by(&k("http://localhost:5173", "///")));
    }

    #[test]
    fn loopback_origins_are_one_host_family() {
        for o in [
            "http://localhost:7702",
            "http://app.localhost:3000",
            "http://127.0.0.1:7703",
            "http://127.1.2.3",
            "http://[::1]:8080",
            "https://localhost",
        ] {
            assert!(is_loopback_origin(o), "{o}");
        }
        for o in [
            "http://example.com",
            "http://localhost.example.com",
            "http://10.0.0.1:7702",
            "http://[::2]",
            "not a url",
        ] {
            assert!(!is_loopback_origin(o), "{o}");
        }
        assert!(same_host_family(
            "http://localhost:7702",
            "http://localhost:7703"
        ));
        assert!(same_host_family(
            "http://localhost:7702",
            "http://127.0.0.1:7702"
        ));
        assert!(!same_host_family(
            "http://localhost:7702",
            "http://localhost:7702"
        ));
        assert!(!same_host_family(
            "http://localhost:7702",
            "https://localhost:7703"
        ));
        assert!(!same_host_family(
            "http://localhost:7702",
            "http://example.com:7703"
        ));
    }

    #[test]
    fn patterns_are_checked_strictly() {
        for ok in [
            "/users/:id",
            "/users/:id/edit",
            "/files/*",
            "/:a/b",
            "/a%20b/:x",
            "/v1.2/:id",
            "/a:b/:id",
        ] {
            assert!(PathPattern::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "users/:id",
            "/:a/:b",
            "/*",
            "/:x",
            "/:x/*",
            "/users",
            "/",
            "/users/",
            "/users//:id",
            "/*/x",
            "/a*/:id",
            "/:",
            "/:id-x",
            "/users/(\\d+)",
            "/users/[0-9]+",
            "/a?b/:id",
            "/a#b/:id",
            "/../:id",
            "/./:id",
            "/a%2/:id",
            "/a b/:id",
        ] {
            assert!(
                matches!(PathPattern::parse(bad), Err(CoreError::Invalid { code, .. }) if code == "invalid_pattern"),
                "{bad}"
            );
        }
        let long = format!("/{}/:id", "a".repeat(MAX_PATTERN));
        assert!(PathPattern::parse(&long).is_err());
        let deep = format!("{}/:id", "/a".repeat(MAX_PATTERN_SEGMENTS));
        assert!(PathPattern::parse(&deep).is_err());
        let name = format!("/:{}", "n".repeat(MAX_PARAM_NAME + 1));
        assert!(PathPattern::parse(&name).is_err());
    }

    #[test]
    fn patterns_match_whole_segments() {
        let p = PathPattern::parse("/users/:id").unwrap();
        assert!(p.matches("/users/123"));
        assert!(p.matches("/users/:id"));
        assert!(!p.matches("/users/"));
        assert!(!p.matches("/users/123/"));
        assert!(!p.matches("/users/123/edit"));
        assert!(!p.matches("/users"));
        assert!(!p.matches("/userss/1"));
        let r = PathPattern::parse("/files/*").unwrap();
        assert!(r.matches("/files/a"));
        assert!(r.matches("/files/a/b/"));
        assert!(!r.matches("/files/"));
        assert!(!r.matches("/files"));
        assert_eq!(p.specificity(), (1, 1));
        assert_eq!(r.specificity(), (1, 0));
    }

    #[test]
    fn the_most_specific_rule_wins_then_the_oldest() {
        let rules = ["/users/*", "/teams/:id", "/users/:id", "/:x/edit"];
        let win = |path| winning_rule(&rules, |r| r, path).copied();
        assert_eq!(win("/users/1"), Some("/users/:id"));
        assert_eq!(win("/users/1/x"), Some("/users/*"));
        assert_eq!(win("/teams/1"), Some("/teams/:id"));
        assert_eq!(win("/teams/edit"), Some("/teams/:id"), "a tie: the oldest");
        assert_eq!(win("/x/edit"), Some("/:x/edit"));
        assert_eq!(win("/"), None);
        let tie = ["/:a/x", "/x/:b"];
        assert_eq!(winning_rule(&tie, |r| r, "/x/x").copied(), Some("/:a/x"));
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
