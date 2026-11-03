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

/// The query parameter name parts that make a URL look credentialed.
const CREDENTIAL_PARAMS: [&str; 7] = [
    "key",
    "token",
    "secret",
    "password",
    "passwd",
    "sig",
    "signature",
];

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

/// Whether `u` looks like it holds a credential: a query parameter whose
/// name, ignoring case, contains `key`, `token`, `secret`, `password`,
/// `passwd`, `sig` or `signature` (spec §5.2). Such a URL is fetched only
/// when the sender's own words hold it.
#[must_use]
pub fn looks_credentialed(u: &PublicUrl) -> bool {
    u.url.query_pairs().any(|(name, _)| {
        let name = name.to_ascii_lowercase();
        CREDENTIAL_PARAMS.iter().any(|p| name.contains(p))
    })
}

/// Whether a redirect from `from` to `to` may be followed by every rule:
/// not from `https` to `http`. The target's host is checked separately,
/// by the `hop` check a source is given.
#[must_use]
pub fn redirect_allowed(from: &PublicUrl, to: &PublicUrl) -> bool {
    !(from.scheme() == "https" && to.scheme() == "http")
}

/// Whether `ip` is in a range no fetch reaches: unspecified, loopback,
/// private (RFC 1918), link-local, unique-local or broadcast, or an
/// IPv4-mapped address in one. [`PublicUrl::parse`] applies it to IP
/// literals; a source that resolves names applies it to every address a
/// name resolves to (spec §5.3).
#[must_use]
pub fn is_local_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => local_v4(a),
        IpAddr::V6(a) => local_v6(a),
    }
}

/// Whether `a` is unspecified, loopback, private (RFC 1918), link-local
/// or the limited broadcast address.
fn local_v4(a: Ipv4Addr) -> bool {
    a.is_unspecified()
        || a.octets()[0] == 0
        || a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_broadcast()
}

/// Whether `a` is unspecified, loopback, link-local (`fe80::/10`),
/// unique-local (`fc00::/7`), or an IPv4-mapped or -compatible address
/// [`local_v4`] refuses.
fn local_v6(a: Ipv6Addr) -> bool {
    let first = a.segments()[0];
    if a.is_unspecified()
        || a.is_loopback()
        || (first & 0xffc0) == 0xfe80
        || (first & 0xfe00) == 0xfc00
    {
        return true;
    }
    a.to_ipv4().is_some_and(local_v4)
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
    fn resolved_addresses_are_checked_by_the_same_ranges() {
        for local in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "::1",
            "fd12::1",
            "::ffff:192.168.0.1",
        ] {
            assert!(is_local_addr(local.parse().unwrap()), "{local}");
        }
        for public in ["1.1.1.1", "2606:4700::1111"] {
            assert!(!is_local_addr(public.parse().unwrap()), "{public}");
        }
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
        assert!(looks("https://x.com/?api_key=1"));
        assert!(looks("https://x.com/?X-Amz-Signature=1"));
        assert!(looks("https://x.com/?a=1&AccessToken=2"));
        assert!(!looks("https://x.com/?page=2"));
        assert!(!looks("https://x.com/key/token?q=secret"));
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
