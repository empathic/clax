//! Origins and origin patterns. An origin is `scheme://host[:port]` of an
//! `http` or `https` URL with nothing after it, as a browser's `Origin`
//! header names it ([`normalize_origin`]); a reporter token is bound to
//! one. An origin pattern is written the same way with one or more `*`s in
//! its host, each standing for whole labels or for part of one
//! ([`OriginPattern`]); it matches exact origins and is never one itself.
//! Both are kept and served in their normalized text form. A loopback
//! origin names this machine ([`is_loopback_origin`]).

use crate::{CoreError, Result};
use std::fmt;

/// `raw` as a browser's `Origin` header names it: `scheme://host[:port]`
/// of an `http` or `https` URL with nothing after it, the host lowercased
/// and a default port left out. A reporter token is bound to one such
/// exact origin.
///
/// # Errors
/// `invalid_origin` for anything else, an origin pattern (holding a `*`)
/// included.
pub fn normalize_origin(raw: &str) -> Result<String> {
    let bad = || {
        CoreError::invalid(
            "invalid_origin",
            format!("{raw:?} is not an origin (scheme://host[:port])"),
        )
    };
    let u = url::Url::parse(raw.trim()).map_err(|_| bad())?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.path() != "/"
    {
        return Err(bad());
    }
    match u.origin() {
        url::Origin::Tuple(..) => {
            let o = u.origin().ascii_serialization();
            if o.contains('*') {
                return Err(bad());
            }
            Ok(o)
        }
        url::Origin::Opaque(_) => Err(bad()),
    }
}

/// Whether `origin` names this machine by a loopback host: `localhost` or
/// any name under it (`app.localhost`, which RFC 6761 reserves for
/// loopback), an address in `127.0.0.0/8`, or `[::1]`. Browsers treat
/// such an origin as a secure context over `http` too.
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

/// The longest host an origin pattern names, and the longest it matches,
/// in bytes: a DNS name's, so at most 127 labels.
const MAX_PATTERN_HOST_BYTES: usize = 253;

/// An exact origin ([`normalize_origin`]) or an origin pattern, parsed
/// once ([`OriginPattern::parse`]) and matched against exact origins
/// ([`OriginPattern::matches`]). Its text ([`OriginPattern::as_str`], and
/// its `Display`) is its normalized form, the one a queue keeps and
/// serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginPattern {
    /// The normalized form.
    text: String,
    /// The parts a pattern matches by; `None` for an exact origin.
    wild: Option<Wild>,
}

/// An origin pattern's parts: its scheme, its host's labels (lowercased)
/// and its port (`None` for the scheme's default).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Wild {
    scheme: &'static str,
    labels: Vec<String>,
    port: Option<u16>,
}

impl OriginPattern {
    /// `raw` as a queue lists it: an exact origin ([`normalize_origin`]),
    /// or, when it holds a `*`, an origin pattern normalized the same way
    /// (the host lowercased, a default port left out, nothing after
    /// `host[:port]`).
    ///
    /// A pattern's scheme (`http` or `https`) and port are exact, never a
    /// `*`. Its host, at most 253 bytes, is ASCII (an internationalized
    /// label written as its `xn--` form), of labels of `a`-`z`, `0`-`9`,
    /// `-` and `*`, each holding at most one `*`:
    ///
    /// - a label that is only `*` stands for one or more whole labels, at
    ///   any place in the host;
    /// - a label holding a `*` beside other characters (`pr-*`, `*-eu`)
    ///   stands for that label with one or more of `a`-`z`, `0`-`9` and `-`
    ///   in place of the `*`, never a `.`.
    ///
    /// At least two labels without a `*` follow the last label holding one.
    /// So `https://*.example.com`, `https://a.*.example.com` and
    /// `https://app-pr-*.example.com` are patterns, and `https://*`,
    /// `https://*.com`, `https://pr-*.com` and `https://a.*.com` are not.
    ///
    /// # Errors
    /// `invalid_origin` for anything else.
    pub fn parse(raw: &str) -> Result<OriginPattern> {
        if !raw.contains('*') {
            return Ok(OriginPattern {
                text: normalize_origin(raw)?,
                wild: None,
            });
        }
        let (scheme, host, port) = pattern_parts(raw.trim()).ok_or_else(|| {
            CoreError::invalid(
                "invalid_origin",
                format!(
                    "{raw:?} is not an origin pattern: scheme://host[:port] with a * only in the \
                     host, as a whole label (https://*.example.com) or part of one \
                     (https://app-pr-*.example.com), followed by at least two labels without one"
                ),
            )
        })?;
        let text = match port {
            Some(p) => format!("{scheme}://{host}:{p}"),
            None => format!("{scheme}://{host}"),
        };
        let labels = host.split('.').map(str::to_string).collect();
        Ok(OriginPattern {
            text,
            wild: Some(Wild {
                scheme,
                labels,
                port,
            }),
        })
    }

    /// The normalized form.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether this allows exact origin `origin`: an exact origin equal
    /// to it, or, for a pattern, `origin` written as [`normalize_origin`]
    /// writes it with a DNS name for its host (never an IP address), of
    /// the same scheme and port, and a host whose labels, each of `a`-`z`,
    /// `0`-`9` and `-`, are the pattern's, with one or more whole labels
    /// in place of each `*` label and one or more of those characters in
    /// place of each other `*`. A pattern is never compared as text:
    /// `origin` equal to a pattern is not allowed by it.
    pub fn matches(&self, origin: &str) -> bool {
        match &self.wild {
            None => self.text == origin,
            Some(w) => w.matches(origin),
        }
    }
}

impl fmt::Display for OriginPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Wild {
    /// Whether this pattern matches `origin`, an exact origin as
    /// [`normalize_origin`] writes it.
    fn matches(&self, origin: &str) -> bool {
        if normalize_origin(origin).ok().as_deref() != Some(origin) {
            return false;
        }
        let Ok(u) = url::Url::parse(origin) else {
            return false;
        };
        let Some(url::Host::Domain(o_host)) = u.host() else {
            return false;
        };
        if o_host.len() > MAX_PATTERN_HOST_BYTES {
            return false;
        }
        let o_labels: Vec<&str> = o_host.split('.').collect();
        if !o_labels
            .iter()
            .all(|l| !l.is_empty() && l.bytes().all(label_byte))
        {
            return false;
        }
        u.scheme() == self.scheme && u.port() == self.port && labels_match(&self.labels, &o_labels)
    }
}

/// The scheme, lowercased host and port (`None` for the scheme's default)
/// of origin pattern `s`, when it is one ([`OriginPattern::parse`]).
fn pattern_parts(s: &str) -> Option<(&'static str, String, Option<u16>)> {
    let (scheme, rest) = s.split_once("://")?;
    let (scheme, default_port) = match scheme.to_ascii_lowercase().as_str() {
        "http" => ("http", 80),
        "https" => ("https", 443),
        _ => return None,
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (h, Some(p.parse::<u16>().ok()?))
        }
        None => (rest, None),
    };
    let host = host.to_ascii_lowercase();
    if host.len() > MAX_PATTERN_HOST_BYTES {
        return None;
    }
    let mut wild = false;
    let mut tail = 0;
    for label in host.split('.') {
        let stars = label.bytes().filter(|&b| b == b'*').count();
        if label.is_empty()
            || label.len() > 63
            || stars > 1
            || !label.bytes().all(|b| label_byte(b) || b == b'*')
        {
            return None;
        }
        if stars == 1 {
            wild = true;
            tail = 0;
        } else {
            tail += 1;
        }
    }
    if !wild || tail < 2 {
        return None;
    }
    Some((scheme, host, port.filter(|&p| p != default_port)))
}

/// Whether `b` may stand in a pattern's label, and in what its `*` matches.
fn label_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
}

/// Whether host labels `o` are pattern labels `p`: each `*` label of `p`
/// in place of one or more labels of `o`, and each other label of `p` in
/// place of one ([`label_matches`]).
///
/// Greedy, without recursion: a `*` label first takes one label, and
/// when what follows fails, the last `*` met takes one label more and
/// the labels after it are tried again. The labels between two `*`s each
/// take exactly one, so the first place they fit is never worse than a
/// later one, and only the last `*` ever needs to grow. Each retry grows
/// it by one label, so the work is at most `p.len()` label comparisons
/// per label of `o`: 127 × 127 for the longest hosts
/// ([`MAX_PATTERN_HOST_BYTES`]).
fn labels_match(p: &[String], o: &[&str]) -> bool {
    let (mut i, mut j) = (0, 0);
    // The pattern label after the last `*` met, and the label of `o`
    // after the labels it takes.
    let mut star: Option<(usize, usize)> = None;
    while j < o.len() {
        match p.get(i).map(String::as_str) {
            Some("*") => {
                i += 1;
                j += 1;
                star = Some((i, j));
            }
            Some(l) if label_matches(l, o[j]) => {
                i += 1;
                j += 1;
            }
            _ => match star {
                Some((after, taken)) => {
                    i = after;
                    j = taken + 1;
                    star = Some((after, j));
                }
                None => return false,
            },
        }
    }
    i == p.len()
}

/// Whether label `o` is pattern label `p`, not a `*` label: equal to it,
/// or, when `p` holds a `*`, its text before and after the `*` around one
/// or more [`label_byte`]s.
fn label_matches(p: &str, o: &str) -> bool {
    match p.split_once('*') {
        None => p == o,
        Some((before, after)) => {
            o.len() > before.len() + after.len()
                && o.starts_with(before)
                && o.ends_with(after)
                && o.as_bytes()[before.len()..o.len() - after.len()]
                    .iter()
                    .all(|&b| label_byte(b))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `raw` parsed and normalized ([`OriginPattern::parse`]).
    fn normalize(raw: &str) -> Result<String> {
        OriginPattern::parse(raw).map(|p| p.to_string())
    }

    /// Whether `pattern`, parsed, matches `origin`.
    fn allows(pattern: &str, origin: &str) -> bool {
        OriginPattern::parse(pattern).unwrap().matches(origin)
    }

    #[test]
    fn origins_normalize_as_a_browser_names_them() {
        assert_eq!(
            normalize_origin("https://App.Example.com").unwrap(),
            "https://app.example.com"
        );
        assert_eq!(
            normalize_origin("https://app.example.com:443/").unwrap(),
            "https://app.example.com"
        );
        assert_eq!(
            normalize_origin("http://localhost:5173").unwrap(),
            "http://localhost:5173"
        );
        for bad in [
            "app.example.com",
            "ftp://x.example",
            "https://x.example/path",
            "https://x.example/?q=1",
            "https://u@x.example",
            "https://x.example//",
        ] {
            assert!(normalize_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_exact_origin_holds_no_star() {
        for bad in [
            "https://pr-*.fly.dev",
            "https://*.fly.dev",
            "https://pr-%2A.fly.dev",
        ] {
            assert!(normalize_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn loopback_origins_name_this_machine() {
        for o in [
            "http://localhost:7702",
            "http://app.localhost:3000",
            "http://a.b.localhost",
            "http://LOCALHOST:7702",
            "http://127.0.0.1:7703",
            "http://127.0.0.2",
            "http://127.1.2.3",
            "http://127.255.255.255:80",
            "http://[::1]:8080",
            "http://[0:0:0:0:0:0:0:1]",
            "https://localhost",
        ] {
            assert!(is_loopback_origin(o), "{o}");
        }
        for o in [
            "http://example.com",
            "http://localhost.example.com",
            "http://notlocalhost",
            "http://xlocalhost:7702",
            "http://10.0.0.1:7702",
            "http://128.0.0.1",
            "http://0.0.0.0",
            "http://[::2]",
            "http://[::]",
            "not a url",
        ] {
            assert!(!is_loopback_origin(o), "{o}");
        }
    }

    #[test]
    fn patterns_normalize_as_exact_origins_do() {
        let n = |raw| normalize(raw).unwrap();
        assert_eq!(
            n("https://pathbase-pr-*.fly.dev"),
            "https://pathbase-pr-*.fly.dev"
        );
        assert_eq!(
            n(" HTTPS://PathBase-PR-*.Fly.Dev/ "),
            "https://pathbase-pr-*.fly.dev"
        );
        assert_eq!(n("HTTPS://*.Fly.Dev:443/"), "https://*.fly.dev");
        assert_eq!(n("https://pr-*.fly.dev:443"), "https://pr-*.fly.dev");
        assert_eq!(n("http://*.fly.dev:80/"), "http://*.fly.dev");
        assert_eq!(n("https://pr-*.fly.dev:8443"), "https://pr-*.fly.dev:8443");
        assert_eq!(n("http://*.fly.dev:443"), "http://*.fly.dev:443");
        assert_eq!(n("https://*-eu.example.com"), "https://*-eu.example.com");
        assert_eq!(n("https://a*b.example.com"), "https://a*b.example.com");
        assert_eq!(
            n("https://web.pr-*.xn--bcher-kva.example"),
            "https://web.pr-*.xn--bcher-kva.example"
        );
        // Wildcard labels anywhere before the last two, any number of them.
        for kept in [
            "https://*.fly.dev",
            "https://a.*.example.com",
            "https://*.*.example.com",
            "https://*.a.*.example.com",
            "https://*.api-*.example.com",
            "https://a-*.b-*.example.com",
        ] {
            assert_eq!(n(kept), kept);
        }
        // An exact origin is normalized as before.
        assert_eq!(n("https://App.Example.com:443/"), "https://app.example.com");
    }

    #[test]
    fn a_bare_star_host_or_a_one_label_tail_is_refused_and_so_is_a_malformed_pattern() {
        for bad in [
            // A bare `*` host.
            "https://*",
            "http://*:8080",
            // One label, or none, after the last label holding a `*`.
            "https://*.dev",
            "https://pr-*.com",
            "https://a.*.com",
            "https://*.*",
            "https://pr-*.fly-*.dev",
            "https://*.example.*",
            "https://app.pr-*",
            "https://example.com-*",
            // Two `*`s in one label.
            "https://**.example.com",
            "https://a**.example.com",
            "https://a*b*.example.com",
            "https://*a*.example.com",
            // A `*` outside the host.
            "http*://pr-*.example.com",
            "*://pr-*.example.com",
            "https://pr-*.example.com:*",
            "https://pr-*.example.com:84*",
            "https://pr-*.example.com/*",
            "https://example.com:*",
            "https://example.com/*",
            // Anything after host[:port], userinfo.
            "https://pr-*.example.com/path",
            "https://pr-*.example.com?q=1",
            "https://pr-*.example.com#f",
            "https://u@pr-*.example.com",
            "https://pr-*.example.com//",
            // Another scheme, no scheme, a bad port.
            "ftp://*.example.com",
            "*.example.com",
            "https://pr-*.example.com:",
            "https://pr-*.example.com:65536",
            // Characters a DNS label does not hold, non-ASCII, empty labels.
            "https://pr_*.example.com",
            "https://pr-*.bücher.example",
            "https://*..example.com",
            "https://.*.example.com",
            "https://*.example.com.",
            "https://[*::1]",
        ] {
            let e = normalize(bad).expect_err(bad);
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "invalid_origin",
                        ..
                    }
                ),
                "{bad}: {e:?}"
            );
        }
        let long = format!("https://{}*.example.com", "a".repeat(63));
        assert!(normalize(&long).is_err());
        let longest = format!("https://{}*.example.com", "a".repeat(62));
        assert!(normalize(&longest).is_ok());
        // 253 bytes of host at most.
        let host = |labels: usize| format!("https://{}example.com", "*.".repeat(labels));
        assert_eq!(host(121).len() - "https://".len(), 253);
        assert!(normalize(&host(121)).is_ok());
        assert!(normalize(&host(122)).is_err());
    }

    #[test]
    fn a_partial_star_matches_within_its_label_and_nothing_else() {
        let allows = |o: &str| allows("https://pathbase-pr-*.fly.dev", o);
        assert!(allows("https://pathbase-pr-538.fly.dev"));
        assert!(allows("https://pathbase-pr-1.fly.dev"));
        assert!(allows("https://pathbase-pr-a-b.fly.dev"));
        assert!(allows("https://pathbase-pr-12ab.fly.dev"));
        for not in [
            // Another label, another domain, an empty match.
            "https://pathbase-pr-1.evil.fly.dev",
            "https://pathbase-pr-1.fly.dev.attacker.com",
            "https://evil.pathbase-pr-1.fly.dev",
            "https://pathbase-pr-.fly.dev",
            "https://pathbase-pr.fly.dev",
            "https://xpathbase-pr-1.fly.dev",
            // Another scheme or port.
            "http://pathbase-pr-1.fly.dev",
            "https://pathbase-pr-1.fly.dev:8443",
            // Not written as a browser writes an origin.
            "https://PATHBASE-pr-1.fly.dev",
            "https://pathbase-pr-1.fly.dev/",
            "https://pathbase-pr-1.fly.dev:443",
            "https://pathbase-pr-1.fly.dev/path",
            "pathbase-pr-1.fly.dev",
            "null",
            "",
            // The pattern itself is no origin.
            "https://pathbase-pr-*.fly.dev",
        ] {
            assert!(!allows(not), "{not}");
        }
    }

    #[test]
    fn a_partial_star_keeps_its_port_and_its_text_around_the_star() {
        let listed = [
            OriginPattern::parse("http://dev-*.example.com:8080").unwrap(),
            OriginPattern::parse("https://*-web.a-*.example.com").unwrap(),
        ];
        let allows = |o: &str| listed.iter().any(|p| p.matches(o));
        assert!(allows("http://dev-7.example.com:8080"));
        assert!(!allows("http://dev-7.example.com"));
        assert!(!allows("https://dev-7.example.com:8080"));
        assert!(allows("https://pr-9-web.a-x.example.com"));
        assert!(!allows("https://pr-9-web.a-.example.com"));
        assert!(!allows("https://-web.a-x.example.com"));
        assert!(!allows("https://pr-9-web.b-x.example.com"));
    }

    #[test]
    fn a_bare_star_matches_one_or_more_whole_labels_where_it_stands() {
        let first = "https://*.example.com";
        assert!(allows(first, "https://a.example.com"));
        assert!(allows(first, "https://a.b.example.com"));
        assert!(allows(first, "https://xn--bcher-kva.example.com"));
        for not in [
            "https://example.com",
            "https://aexample.com",
            "https://a.example.com.evil.dev",
            "https://a.example.org",
            "http://a.example.com",
            "https://a.example.com:8443",
            "https://A.example.com",
            "https://a_b.example.com",
            "https://*.example.com",
        ] {
            assert!(!allows(first, not), "{first} {not}");
        }

        let middle = "https://a.*.example.com";
        assert!(allows(middle, "https://a.x.example.com"));
        assert!(allows(middle, "https://a.x.y.example.com"));
        for not in [
            "https://a.example.com",
            "https://b.x.example.com",
            "https://x.a.x.example.com",
            "https://a.x.example.org",
        ] {
            assert!(!allows(middle, not), "{middle} {not}");
        }

        let ported = "http://*.example.com:8080";
        assert!(allows(ported, "http://a.b.example.com:8080"));
        assert!(!allows(ported, "http://a.example.com"));
        assert!(!allows(ported, "https://a.example.com:8080"));

        // A later literal that first fits too early: the `*` grows past it.
        let regrow = "https://*.a.b.example.com";
        assert!(allows(regrow, "https://a.a.b.a.b.example.com"));
        assert!(!allows(regrow, "https://a.b.example.com"));
    }

    #[test]
    fn several_bare_stars_each_take_at_least_one_label() {
        let two = "https://*.*.example.com";
        assert!(allows(two, "https://a.b.example.com"));
        assert!(allows(two, "https://a.b.c.d.example.com"));
        assert!(!allows(two, "https://a.example.com"));

        let apart = "https://*.a.*.example.com";
        assert!(allows(apart, "https://x.a.y.example.com"));
        assert!(allows(apart, "https://x.y.a.z.w.example.com"));
        assert!(allows(apart, "https://a.a.a.example.com"));
        assert!(!allows(apart, "https://a.y.example.com"));
        assert!(!allows(apart, "https://x.a.example.com"));
        assert!(!allows(apart, "https://x.b.y.example.com"));
    }

    #[test]
    fn bare_and_partial_stars_mix() {
        let mixed = "https://web.*.pr-*.example.com";
        assert!(allows(mixed, "https://web.eu.pr-7.example.com"));
        assert!(allows(mixed, "https://web.eu.west.pr-7.example.com"));
        for not in [
            "https://web.pr-7.example.com",
            "https://web.eu.pr-.example.com",
            "https://web.eu.pr-7.x.example.com",
            "https://api.eu.pr-7.example.com",
        ] {
            assert!(!allows(mixed, not), "{mixed} {not}");
        }
        let suffix = "https://*.api-*.example.com";
        assert!(allows(suffix, "https://a.b.api-eu.example.com"));
        assert!(!allows(suffix, "https://api-eu.example.com"));
        let fly = "https://*.fly.dev";
        assert!(allows(fly, "https://pathbase-pr-538.fly.dev"));
        assert!(allows(fly, "https://pathbase-pr-1.evil.fly.dev"));
        assert!(!allows(fly, "https://fly.dev"));
    }

    #[test]
    fn a_pattern_matches_dns_names_never_an_ip_address() {
        // Two labels follow the `*`, so the pattern is kept, but an
        // origin whose host is an IP address is not a name it matches.
        let pattern = "https://10.*.0.1";
        assert!(!allows(pattern, "https://10.1.0.1"));
        assert!(!allows(pattern, "https://10.1.2.0.1"));
    }

    #[test]
    fn matching_stays_bounded_on_a_pathological_host() {
        // 60 `*` labels against 120 labels before `example.com`: a matcher
        // that tried every split of the labels among the `*`s would not
        // finish; this one makes at most a pattern's labels of comparisons
        // per label of the host.
        let pattern = format!("https://{}x.example.com", "*.".repeat(60));
        let miss = format!("https://{}example.com", "b.".repeat(120));
        assert!(!allows(&pattern, &miss));
        let hit = format!("https://{}x.example.com", "b.".repeat(118));
        assert!(allows(&pattern, &hit));
        let short = format!("https://{}x.example.com", "b.".repeat(59));
        assert!(!allows(&pattern, &short));
        // A host longer than a DNS name is matched by nothing.
        let any = "https://*.example.com";
        let long = format!("https://{}example.com", "b.".repeat(122));
        assert!(!allows(any, &long));
        let longest = format!("https://{}example.com", "b.".repeat(121));
        assert!(allows(any, &longest));
    }
}
