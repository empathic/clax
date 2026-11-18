//! [`PublicUrl`]: a URL a source may be asked to fetch, and the URL
//! checks that hold under every rule (spec §5.3): the scheme, credentials,
//! the length, private and local hosts, and `https` to `http` redirects.
//! The daemon's DNS check of the addresses a name resolves to is its own
//! (`RestSource`); a [`PublicUrl`] checks only what the URL says.

use super::{CODE_URL_NOT_ALLOWED, WebError};
use crate::api::CODE_BAD_ARGS;
use ::url::{Host, Url};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The longest URL, in characters, a fetch takes.
pub const MAX_URL_CHARS: usize = 2_000;

/// The query parameter name components that make a URL look
/// credentialed ([`looks_credentialed`]). `token` is one too, unless a
/// [`PAGINATION_WORDS`] component comes immediately before it.
const CREDENTIAL_WORDS: [&str; 13] = [
    "key",
    "apikey",
    "accesstoken",
    "secret",
    "password",
    "passwd",
    "pwd",
    "sig",
    "signature",
    "credential",
    "credentials",
    "auth",
    "authorization",
];

/// The components that make a later `token` a place in a list of results
/// (`page_token`, `nextPageToken`, `continuation_token`, `sync_token`),
/// not a credential.
const PAGINATION_WORDS: [&str; 5] = ["page", "next", "continuation", "pagination", "sync"];

/// An `http` or `https` URL without credentials, at most
/// [`MAX_URL_CHARS`] characters, whose host is not a private or local
/// address or name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicUrl {
    url: Url,
    normalized: String,
}

impl PublicUrl {
    /// Checks `s`.
    ///
    /// # Errors
    /// [`CODE_BAD_ARGS`] for a string that is not an absolute URL;
    /// [`CODE_URL_NOT_ALLOWED`] for another scheme than `http` or `https`,
    /// credentials, more than [`MAX_URL_CHARS`] characters, an IP literal
    /// in a private, loopback, link-local or unique-local range, or
    /// `localhost` and its subdomains.
    pub fn parse(s: &str) -> Result<PublicUrl, WebError> {
        let refuse = |why: &str| {
            WebError::new(
                CODE_URL_NOT_ALLOWED,
                format!("{why}; fetch a public http or https URL instead"),
                false,
            )
        };
        if s.chars().count() > MAX_URL_CHARS {
            return Err(refuse(&format!(
                "the URL is longer than {MAX_URL_CHARS} characters"
            )));
        }
        let url = Url::parse(s.trim()).map_err(|e| {
            WebError::new(CODE_BAD_ARGS, format!("not an absolute URL ({e})"), false)
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(refuse(&format!(
                "the scheme {} is not fetched",
                url.scheme()
            )));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(refuse("the URL holds credentials"));
        }
        if url.as_str().chars().count() > MAX_URL_CHARS {
            return Err(refuse(&format!(
                "the URL is longer than {MAX_URL_CHARS} characters"
            )));
        }
        let local = match url.host() {
            Some(Host::Ipv4(a)) => local_v4(a),
            Some(Host::Ipv6(a)) => local_v6(a),
            Some(Host::Domain(d)) => {
                let d = d.strip_suffix('.').unwrap_or(d);
                d.eq_ignore_ascii_case("localhost")
                    || d.to_ascii_lowercase().ends_with(".localhost")
            }
            None => true,
        };
        if local {
            return Err(refuse(&format!(
                "{} is a private or local address",
                url.host_str().unwrap_or("the host")
            )));
        }
        let mut bare = url.clone();
        bare.set_fragment(None);
        let normalized = upper_percent(bare.as_str());
        Ok(PublicUrl { url, normalized })
    }

    /// The URL as it is requested: the parsed form, fragment kept.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    /// The form two URLs are compared in: scheme and host lowercase, the
    /// default port and the fragment dropped, percent-encoding in
    /// uppercase hex.
    #[must_use]
    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// The host, lowercase; an IPv6 literal in brackets.
    #[must_use]
    pub fn host(&self) -> &str {
        self.url.host_str().unwrap_or("")
    }

    /// The `https` or `http` scheme.
    #[must_use]
    pub fn scheme(&self) -> &str {
        self.url.scheme()
    }

    /// The parsed URL.
    #[must_use]
    pub fn as_url(&self) -> &Url {
        &self.url
    }
}

/// Whether `u` looks like it holds a credential (spec §5.2): a query
/// parameter whose name, split into components at `-`, `_`, `.` and
/// camelCase boundaries and compared ignoring case, has a component that
/// is a credential word (`key`, `apikey`, `secret`, `password`, `passwd`,
/// `pwd`, `sig`, `signature`, `credential(s)`, `auth`, `authorization`,
/// `accesstoken`), or a `token` whose immediately preceding component is
/// not a pagination word (`page`, `next`, `continuation`, `pagination`,
/// `sync`). So `api_key`, `apiKey`, `access_token`, `page_auth_token`,
/// `cursor_access_token`, `X-Amz-Signature` and `sig` look credentialed,
/// and `keywords`, `page_token`, `pageToken`, `next_page_token`, `cursor`,
/// `design`, `signup` and `assignee` do not. Such a URL is fetched only
/// when the sender's own words hold it.
#[must_use]
pub fn looks_credentialed(u: &PublicUrl) -> bool {
    u.url.query_pairs().any(|(name, _)| credential_name(&name))
}

/// Whether a query parameter named `name` holds a credential, by
/// [`looks_credentialed`]'s rule.
fn credential_name(name: &str) -> bool {
    let parts = name_components(name);
    parts.iter().enumerate().any(|(i, part)| {
        CREDENTIAL_WORDS.contains(&part.as_str())
            || (part == "token" && !(i > 0 && PAGINATION_WORDS.contains(&parts[i - 1].as_str())))
    })
}

/// `name`'s components, lowercase: split at `-`, `_`, `.` and anything
/// else that is not a letter or digit, and at camelCase boundaries (a
/// lowercase letter or digit then an uppercase one, and the last capital
/// of a run of them before a lowercase letter: `AWSAccessKeyId` is `aws`,
/// `access`, `key`, `id`).
fn name_components(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut parts = Vec::new();
    let mut part = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !part.is_empty() {
                parts.push(std::mem::take(&mut part));
            }
            continue;
        }
        if c.is_uppercase() && !part.is_empty() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_numeric() || (prev.is_uppercase() && next_lower) {
                parts.push(std::mem::take(&mut part));
            }
        }
        part.extend(c.to_lowercase());
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

/// The instance's own hosts, which no fetch reaches, on the URL asked for
/// or a redirect target (spec §5.3): a hosted instance's public origin's
/// host, and its site domain with every host under it. A local daemon
/// has none: its hosts are loopback, which [`PublicUrl::parse`] refuses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnHosts {
    origin_host: Option<String>,
    site_domain: Option<String>,
}

/// Why [`OwnHosts::new`] refused a configured value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnHostsError {
    /// The public origin is not a URL with a host.
    PublicOrigin(String),
    /// The site domain is not a bare host name.
    SiteDomain(String),
}

impl std::fmt::Display for OwnHostsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OwnHostsError::PublicOrigin(o) => {
                write!(f, "the public origin {o:?} is not an origin with a host")
            }
            OwnHostsError::SiteDomain(d) => {
                write!(f, "the site domain {d:?} is not a bare host name")
            }
        }
    }
}

impl std::error::Error for OwnHostsError {}

impl OwnHosts {
    /// The hosts of an instance at `public_origin` (an origin such as
    /// `https://clax.example`, its port ignored) whose sites are under
    /// `site_domain` (a bare domain, [`clax_core::config::site_domain`]'s
    /// rule); either may be `None`, and a blank one is none.
    ///
    /// # Errors
    ///
    /// [`OwnHostsError`] when a value is set but names no host: an
    /// instance that could not name its own hosts would let a fetch
    /// reach them.
    pub fn new(
        public_origin: Option<&str>,
        site_domain: Option<&str>,
    ) -> Result<OwnHosts, OwnHostsError> {
        let bare = |h: &str| {
            let h = h.trim().trim_end_matches('.').to_ascii_lowercase();
            (!h.is_empty()).then_some(h)
        };
        let origin_host = match public_origin.map(str::trim).filter(|o| !o.is_empty()) {
            None => None,
            Some(o) => Some(
                Url::parse(o)
                    .ok()
                    .and_then(|u| u.host_str().and_then(bare))
                    .ok_or_else(|| OwnHostsError::PublicOrigin(o.to_string()))?,
            ),
        };
        let site_domain = match site_domain.filter(|d| bare(d).is_some()) {
            None => None,
            Some(raw) => Some(
                bare(raw)
                    .and_then(|d| clax_core::config::site_domain(&d))
                    .ok_or_else(|| OwnHostsError::SiteDomain(raw.to_string()))?,
            ),
        };
        Ok(OwnHosts {
            origin_host,
            site_domain,
        })
    }

    /// Whether `host` (as [`PublicUrl::host`] gives it) is one of them.
    #[must_use]
    pub fn holds(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.origin_host.as_deref() == Some(host.as_str())
            || self.site_domain.as_deref().is_some_and(|d| {
                host == d
                    || host
                        .strip_suffix(d)
                        .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.'))
            })
    }
}

/// Whether a redirect from `from` to `to` may be followed by every rule:
/// not from `https` to `http`. The target's host is checked separately,
/// by the `hop` check a source is given.
#[must_use]
pub fn redirect_allowed(from: &PublicUrl, to: &PublicUrl) -> bool {
    !(from.scheme() == "https" && to.scheme() == "http")
}

/// Whether `ip` is not a public unicast address, so no fetch reaches it
/// (spec §5.3): unspecified, loopback, private, shared (CGNAT),
/// link-local, unique-local, multicast, broadcast, reserved,
/// documentation and benchmarking ranges, and the IETF's protocol
/// assignments; an IPv6 address that embeds an IPv4 one (mapped,
/// compatible, NAT64 `64:ff9b::/96`, 6to4 `2002::/16`) is judged by the
/// IPv4 address. [`PublicUrl::parse`] applies it to IP literals; a source
/// that resolves names applies it to every address a name resolves to.
#[must_use]
pub fn is_local_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => local_v4(a),
        IpAddr::V6(a) => local_v6(a),
    }
}

/// The IPv4 ranges [`is_local_addr`] refuses, as (network, prefix length).
const LOCAL_V4: [([u8; 4], u32); 14] = [
    ([0, 0, 0, 0], 8),       // "this network", unspecified
    ([10, 0, 0, 0], 8),      // private
    ([100, 64, 0, 0], 10),   // shared address space (CGNAT)
    ([127, 0, 0, 0], 8),     // loopback
    ([169, 254, 0, 0], 16),  // link-local
    ([172, 16, 0, 0], 12),   // private
    ([192, 0, 0, 0], 24),    // IETF protocol assignments
    ([192, 0, 2, 0], 24),    // documentation (TEST-NET-1)
    ([192, 168, 0, 0], 16),  // private
    ([198, 18, 0, 0], 15),   // benchmarking
    ([198, 51, 100, 0], 24), // documentation (TEST-NET-2)
    ([203, 0, 113, 0], 24),  // documentation (TEST-NET-3)
    ([224, 0, 0, 0], 4),     // multicast
    ([240, 0, 0, 0], 4),     // reserved, and the broadcast address
];

fn local_v4(a: Ipv4Addr) -> bool {
    let a = u32::from(a);
    LOCAL_V4.iter().any(|&(net, len)| {
        let mask = u32::MAX << (32 - len);
        a & mask == u32::from_be_bytes(net)
    })
}

fn local_v6(a: Ipv6Addr) -> bool {
    let s = a.segments();
    let v4 = |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
    if a.is_unspecified() || a.is_loopback() {
        return true;
    }
    // Mapped (::ffff:0:0/96) and compatible (::/96).
    if let Some(embedded) = a.to_ipv4() {
        return local_v4(embedded);
    }
    // NAT64: the well-known prefix by its IPv4 address; any other
    // 64:ff9b::/16 address (the local-use 64:ff9b:1::/48) is refused.
    if s[0] == 0x64 && s[1] == 0xff9b {
        return s[2..6] != [0, 0, 0, 0] || local_v4(v4(s[6], s[7]));
    }
    // 6to4.
    if s[0] == 0x2002 {
        return local_v4(v4(s[1], s[2]));
    }
    // Only global unicast (2000::/3) is public: this refuses link-local,
    // unique-local, site-local, multicast and the discard prefix.
    if s[0] & 0xe000 != 0x2000 {
        return true;
    }
    // IETF protocol assignments (2001::/23, with Teredo), documentation
    // (2001:db8::/32 and 3fff::/20).
    (s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8)) || (s[0] == 0x3fff && s[1] < 0x1000)
}

/// `s` with the two hex digits of every percent escape in uppercase.
fn upper_percent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
        {
            out.push('%');
            out.push(char::from(b[i + 1].to_ascii_uppercase()));
            out.push(char::from(b[i + 2].to_ascii_uppercase()));
            i += 3;
        } else {
            // A serialised URL is ASCII.
            out.push(char::from(b[i]));
            i += 1;
        }
    }
    out
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn refused(s: &str) -> &'static str {
        PublicUrl::parse(s).expect_err(s).code
    }

    #[test]
    fn normalisation_matches_case_port_and_fragment() {
        let a = PublicUrl::parse("https://Example.com:443/a#x").unwrap();
        let b = PublicUrl::parse("https://example.com/a").unwrap();
        assert_eq!(a.normalized(), b.normalized());
        assert_eq!(a.normalized(), "https://example.com/a");
        assert_eq!(a.host(), "example.com");
        let c = PublicUrl::parse("HTTP://X.com:80/p%2fq?a=%e9").unwrap();
        assert_eq!(c.normalized(), "http://x.com/p%2Fq?a=%E9");
        let d = PublicUrl::parse("https://x.com:8443/").unwrap();
        assert_eq!(d.normalized(), "https://x.com:8443/");
    }

    #[test]
    fn private_and_local_hosts_are_refused() {
        for s in [
            "http://127.0.0.1/",
            "http://10.1.2.3/",
            "http://172.16.0.1/",
            "http://192.168.1.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[::]/",
            "http://169.254.1.1/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://localhost/",
            "http://LOCALHOST./",
            "http://a.localhost/",
            "http://2130706433/",
        ] {
            assert_eq!(refused(s), CODE_URL_NOT_ALLOWED, "{s}");
        }
        for s in [
            "https://example.com/",
            "http://8.8.8.8/",
            "http://[2606:4700::1111]/",
            "https://localhost.example.com/",
        ] {
            assert!(PublicUrl::parse(s).is_ok(), "{s}");
        }
    }

    #[test]
    fn every_non_public_address_is_refused() {
        for local in [
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
            "169.254.169.254",
            "172.31.0.1",
            "192.0.0.8",
            "192.0.2.1",
            "192.168.0.1",
            "198.18.0.1",
            "198.19.255.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:192.168.0.1",
            "::10.0.0.1",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "2002:7f00:1::",
            "2002:a9fe:a9fe::1",
            "fe80::1",
            "fec0::1",
            "fd12::1",
            "ff02::1",
            "100::1",
            "2001::1",
            "2001:db8::1",
            "3fff::1",
        ] {
            assert!(is_local_addr(local.parse().unwrap()), "{local}");
        }
        for public in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.0",
            "198.20.0.1",
            "223.255.255.255",
            "2606:4700::1111",
            "::ffff:1.1.1.1",
            "64:ff9b::808:808",
            "2002:808:808::1",
            "2001:4860:4860::8888",
        ] {
            assert!(!is_local_addr(public.parse().unwrap()), "{public}");
        }
        assert_eq!(refused("http://100.64.0.1/"), CODE_URL_NOT_ALLOWED);
        assert_eq!(refused("http://[ff02::1]/"), CODE_URL_NOT_ALLOWED);
        assert_eq!(refused("http://[64:ff9b::7f00:1]/"), CODE_URL_NOT_ALLOWED);
    }

    #[test]
    fn credentials_and_long_urls_are_refused() {
        assert_eq!(refused("https://u:p@x.com/"), CODE_URL_NOT_ALLOWED);
        assert_eq!(refused("https://u@x.com/"), CODE_URL_NOT_ALLOWED);
        let base = "https://x.com/";
        let long = format!("{base}{}", "a".repeat(MAX_URL_CHARS + 1 - base.len()));
        assert_eq!(long.chars().count(), 2_001);
        assert_eq!(refused(&long), CODE_URL_NOT_ALLOWED);
        assert!(PublicUrl::parse(&long[..MAX_URL_CHARS]).is_ok());
        assert_eq!(refused("ftp://x.com/"), CODE_URL_NOT_ALLOWED);
        assert_eq!(refused("file:///etc/passwd"), CODE_URL_NOT_ALLOWED);
        assert_eq!(refused("not a url"), CODE_BAD_ARGS);
    }

    #[test]
    fn a_token_query_looks_credentialed() {
        let looks = |s: &str| looks_credentialed(&PublicUrl::parse(s).unwrap());
        for name in [
            "key",
            "KEY",
            "apikey",
            "api_key",
            "apiKey",
            "api-key",
            "x.api.key",
            "token",
            "access_token",
            "accessToken",
            "AccessToken",
            "accesstoken",
            "id_token",
            "oauth_token",
            "authToken",
            "X-Amz-Security-Token",
            "AWSAccessKeyId",
            "secret",
            "client_secret",
            "password",
            "passwd",
            "pwd",
            "sig",
            "signature",
            "X-Amz-Signature",
            "X-Goog-Signature",
            "X-Amz-Credential",
            "credentials",
            "auth",
            "authorization",
            // A pagination word after the token does not make it one.
            "token_page",
            // A pagination word counts only directly before `token`.
            "page_session_token",
            "next_user_token",
            "page_auth_token",
            "cursor_access_token",
            "sync.refreshToken",
        ] {
            assert!(credential_name(name), "{name} is refused");
        }
        for name in [
            "keywords",
            "keyword",
            "monkey",
            "page_token",
            "pageToken",
            "nextPageToken",
            "next_page_token",
            "next_token",
            "continuation_token",
            "continuationToken",
            "pagination_token",
            "sync_token",
            "syncToken",
            "cursor",
            "design",
            "signup",
            "assignee",
            "author",
            "tokens",
            "passage",
            "page",
            "q",
            "",
        ] {
            assert!(!credential_name(name), "{name} is accepted");
        }
        assert!(looks("https://x.com/?api_key=1"));
        assert!(looks("https://x.com/?X-Amz-Signature=1"));
        assert!(looks("https://x.com/?a=1&AccessToken=2"));
        assert!(!looks("https://x.com/?page=2"));
        assert!(!looks(
            "https://x.com/?keywords=rust&pageToken=abc&assignee=me"
        ));
        assert!(!looks("https://x.com/key/token?q=secret"));
    }

    #[test]
    fn a_name_splits_at_separators_and_camel_case() {
        assert_eq!(
            name_components("X-Amz-Signature"),
            ["x", "amz", "signature"]
        );
        assert_eq!(
            name_components("AWSAccessKeyId"),
            ["aws", "access", "key", "id"]
        );
        assert_eq!(name_components("nextPageToken"), ["next", "page", "token"]);
        assert_eq!(name_components("v2Token"), ["v2", "token"]);
        assert_eq!(name_components("a.b_c--d"), ["a", "b", "c", "d"]);
        assert!(name_components("").is_empty());
    }

    #[test]
    fn the_instances_own_hosts_are_held() {
        let own = OwnHosts::new(Some("https://Clax.example:8443"), Some("Sites.example.")).unwrap();
        for host in [
            "clax.example",
            "CLAX.example.",
            "sites.example",
            "abcdefghijkl.sites.example",
            "a.b.sites.example",
        ] {
            assert!(own.holds(host), "{host}");
        }
        for host in [
            "www.clax.example",
            "example",
            "xsites.example",
            "sites.example.com",
            "other.example",
        ] {
            assert!(!own.holds(host), "{host}");
        }
        let none = OwnHosts::default();
        assert!(!none.holds("clax.example"));
        assert_eq!(OwnHosts::new(Some(" "), Some(" ")), Ok(none));
        assert_eq!(OwnHosts::new(None, None), Ok(OwnHosts::default()));
        let origin_only = OwnHosts::new(Some("https://clax.example"), None).unwrap();
        assert!(origin_only.holds("clax.example"));
        assert!(!origin_only.holds("a.clax.example"));
    }

    #[test]
    fn a_configured_value_that_names_no_host_is_refused() {
        for bad in ["not an origin", "https://", "mailto:a@b.c"] {
            assert_eq!(
                OwnHosts::new(Some(bad), None),
                Err(OwnHostsError::PublicOrigin(bad.to_string())),
                "{bad}"
            );
        }
        for bad in [
            "https://sites.example",
            "sites.example:8080",
            "a b",
            ".x.y",
            "1.2.3.4",
        ] {
            assert_eq!(
                OwnHosts::new(Some("https://clax.example"), Some(bad)),
                Err(OwnHostsError::SiteDomain(bad.to_string())),
                "{bad}"
            );
        }
        // The error quotes the value as configured.
        let e = OwnHosts::new(Some(" Not An Origin "), Some(" Sites.Example/X. ")).unwrap_err();
        assert_eq!(
            e.to_string(),
            "the public origin \"Not An Origin\" is not an origin with a host"
        );
        let e = OwnHosts::new(None, Some(" Sites.Example/X. ")).unwrap_err();
        assert_eq!(
            e.to_string(),
            "the site domain \" Sites.Example/X. \" is not a bare host name"
        );
    }

    #[test]
    fn https_to_http_is_not_followed() {
        let https = PublicUrl::parse("https://a.com/").unwrap();
        let http = PublicUrl::parse("http://b.com/").unwrap();
        assert!(!redirect_allowed(&https, &http));
        assert!(redirect_allowed(&http, &https));
        assert!(redirect_allowed(&https, &https));
        assert!(redirect_allowed(&http, &http));
    }
}
