//! The `net` capability's fetch (spec 2026-10-12-flutter-real-apps-design
//! C6): the daemon fetches for an app page, whose own content security
//! policy keeps `connect-src 'none'`.
//!
//! Every hop (the first request and each redirect, at most
//! [`MAX_REDIRECTS`]) is vetted the same way before anything is sent:
//! - the URL is `https` with no user name or password;
//! - its host is one the caller allows (declared by the live version and
//!   granted by the owner, [`clax_store::net_grants::NetAccess::allows`]);
//! - the host's every resolved address is a public unicast address
//!   ([`public_ip`]); one loopback, private, link-local, multicast,
//!   unspecified or otherwise special address refuses the hop;
//! - the connection goes to those vetted addresses only (the transport
//!   never resolves again), so a second DNS answer cannot rebind the host.
//!
//! Request and response bodies are capped at [`MAX_BODY`] bytes, and the
//! whole fetch, redirects included, at [`TIMEOUT`]. Cookies, `Authorization`
//! on a redirect to another host, and hop-by-hop headers never pass; no
//! proxy is used. The resolver and the transport are traits so tests can
//! drive every rule without a network. With a [`NetCache`]
//! ([`Net::with_cache`]), a cacheable `GET` is answered from it or
//! revalidated, after the same host check.

use crate::net_cache::{self, NetCache};
use futures::future::BoxFuture;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// Largest request or response body, in bytes.
pub const MAX_BODY: usize = 5 * 1024 * 1024;
/// The whole fetch's deadline, redirects included.
pub const TIMEOUT: Duration = Duration::from_secs(20);
/// Most redirects one fetch follows.
pub const MAX_REDIRECTS: usize = 3;
/// The `User-Agent` a fetch carries when the page sets none: tile and API
/// servers (OpenStreetMap's among them) ask clients to identify themselves
/// and say where to reach their operator.
pub const USER_AGENT: &str = concat!(
    "Clax/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/empathic/clax)"
);

/// Why a fetch did not produce a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// The request is malformed (URL, method, header). `invalid_argument`.
    Invalid(String),
    /// The URL is not `https`. `https_only`.
    NotHttps(String),
    /// The host is not one the app may reach. `not_granted` for the first
    /// hop, `redirect_refused` for a redirect.
    HostRefused { host: String, redirect: bool },
    /// The host resolved to an address that is not public. `blocked_address`.
    Blocked { host: String, ip: IpAddr },
    /// A body is over [`MAX_BODY`]. `too_large`.
    TooLarge,
    /// More than [`MAX_REDIRECTS`] redirects. `too_many_redirects`.
    TooManyRedirects,
    /// The fetch took longer than [`TIMEOUT`]. `timeout`.
    Timeout,
    /// Resolution, connection, TLS or the exchange failed. `upstream_error`.
    Upstream(String),
}

impl NetError {
    /// The error code the route answers.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            NetError::Invalid(_) => "invalid_argument",
            NetError::NotHttps(_) => "https_only",
            NetError::HostRefused {
                redirect: false, ..
            } => "not_granted",
            NetError::HostRefused { redirect: true, .. } => "redirect_refused",
            NetError::Blocked { .. } => "blocked_address",
            NetError::TooLarge => "too_large",
            NetError::TooManyRedirects => "too_many_redirects",
            NetError::Timeout => "timeout",
            NetError::Upstream(_) => "upstream_error",
        }
    }
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::Invalid(m) | NetError::Upstream(m) => f.write_str(m),
            NetError::NotHttps(u) => write!(f, "{u} is not an https URL"),
            NetError::HostRefused {
                host,
                redirect: false,
            } => write!(
                f,
                "this app may not reach {host}: it is not declared in capabilities.net.hosts, or the owner has not allowed it"
            ),
            NetError::HostRefused {
                host,
                redirect: true,
            } => write!(f, "a redirect to {host}, which this app may not reach"),
            NetError::Blocked { host, ip } => {
                write!(f, "{host} resolves to {ip}, which is not a public address")
            }
            NetError::TooLarge => write!(f, "a body is over {MAX_BODY} bytes"),
            NetError::TooManyRedirects => write!(f, "more than {MAX_REDIRECTS} redirects"),
            NetError::Timeout => write!(f, "the fetch took longer than {}s", TIMEOUT.as_secs()),
        }
    }
}

/// Whether `ip` is a public unicast address a fetch may connect to: not
/// unspecified, loopback, private, shared (CGNAT), link-local, multicast,
/// broadcast, documentation, benchmarking or reserved. An IPv6 address that
/// embeds an IPv4 one (mapped, or the NAT64 prefix `64:ff9b::/96`) is judged
/// by that IPv4 address; other transition and special IPv6 ranges are refused.
#[must_use]
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0                                  // "this network", unspecified
        || a == 10                            // private
        || a == 127                           // loopback
        || (a == 100 && (64..128).contains(&b)) // shared address space (CGNAT)
        || (a == 169 && b == 254)             // link-local
        || (a == 172 && (16..32).contains(&b)) // private
        || (a == 192 && b == 0 && c == 0)     // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)     // documentation
        || (a == 192 && b == 88 && c == 99)   // 6to4 relay anycast
        || (a == 192 && b == 168)             // private
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || (a == 198 && b == 51 && c == 100)  // documentation
        || (a == 203 && b == 0 && c == 113)   // documentation
        || a >= 224) // multicast, reserved, broadcast
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let s = ip.segments();
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        let [a, b] = s[6].to_be_bytes();
        let [c, d] = s[7].to_be_bytes();
        return public_v4(Ipv4Addr::new(a, b, c, d));
    }
    // Global unicast is 2000::/3; everything outside it (unspecified,
    // loopback, IPv4-compatible, unique-local fc00::/7, link-local
    // fe80::/10, site-local, multicast ff00::/8, discard 100::/64, the
    // NAT64 local-use 64:ff9b:1::/48) is refused.
    if s[0] & 0xe000 != 0x2000 {
        return false;
    }
    // Teredo 2001::/32 and IETF 2001::/23; documentation 2001:db8::/32,
    // 3fff::/20; 6to4 2002::/16.
    let special = (s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
        || s[0] == 0x2002
        || (s[0] == 0x3fff && s[1] < 0x1000);
    !special
}

/// One request a page asks the daemon to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// One hop as the transport sends it: to `addrs` only, never resolving
/// `url`'s host again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub method: String,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub addrs: Vec<SocketAddr>,
    /// Time left of the fetch's [`TIMEOUT`].
    pub deadline: Duration,
}

/// A hop's answer: its body already capped at [`MAX_BODY`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HopResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What the route answers: the final URL, status, headers (lowercase names,
/// repeated values joined by `, `; no cookies or hop-by-hop headers), and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    pub url: String,
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

/// Resolves a host to socket addresses.
pub trait Resolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, std::io::Result<Vec<SocketAddr>>>;
}

/// Sends one hop to its vetted addresses, without following redirects.
pub trait Transport: Send + Sync {
    fn send(&self, hop: Hop) -> BoxFuture<'_, Result<HopResponse, NetError>>;
}

/// The system resolver.
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, std::io::Result<Vec<SocketAddr>>> {
        Box::pin(async move { Ok(tokio::net::lookup_host((host, port)).await?.collect()) })
    }
}

/// reqwest, one client per hop pinned to the hop's addresses: no redirects,
/// no proxy, no cookie store, https only.
pub struct ReqwestTransport;

impl Transport for ReqwestTransport {
    fn send(&self, hop: Hop) -> BoxFuture<'_, Result<HopResponse, NetError>> {
        Box::pin(async move {
            let host = hop.url.host_str().unwrap_or_default().to_string();
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .https_only(true)
                .resolve_to_addrs(&host, &hop.addrs)
                .connect_timeout(Duration::from_secs(10))
                .timeout(hop.deadline)
                .build()
                .map_err(|e| NetError::Upstream(format!("could not start the request: {e}")))?;
            let method = reqwest::Method::from_bytes(hop.method.as_bytes())
                .map_err(|_| NetError::Invalid(format!("{} is not an HTTP method", hop.method)))?;
            let mut req = client.request(method, hop.url.clone());
            for (k, v) in &hop.headers {
                req = req.header(k, v);
            }
            if let Some(b) = hop.body {
                req = req.body(b);
            }
            let mut res = req.send().await.map_err(upstream)?;
            // A HEAD, 204 or 304 announces a length it does not send.
            let bodiless = hop.method == "HEAD" || matches!(res.status().as_u16(), 204 | 304);
            if !bodiless && res.content_length().is_some_and(|n| n > MAX_BODY as u64) {
                return Err(NetError::TooLarge);
            }
            let status = res.status().as_u16();
            let headers = res
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
                .collect();
            let mut body = Vec::new();
            while let Some(chunk) = res.chunk().await.map_err(upstream)? {
                if body.len() + chunk.len() > MAX_BODY {
                    return Err(NetError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(HopResponse {
                status,
                headers,
                body,
            })
        })
    }
}

fn upstream(e: reqwest::Error) -> NetError {
    if e.is_timeout() {
        NetError::Timeout
    } else {
        // The error's source chain says why (DNS is never asked: addresses are pinned).
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg.push_str(": ");
            msg.push_str(&s.to_string());
            src = s.source();
        }
        NetError::Upstream(msg)
    }
}

/// Request headers a page may not set: the daemon's to decide, or
/// connection-level.
const DROPPED_REQUEST: &[&str] = &[
    "cookie",
    "cookie2",
    "host",
    "content-length",
    "connection",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "expect",
];

/// The page's headers that follow a redirect to another origin; every other
/// one (`authorization`, `x-api-key`, any custom header) is dropped there.
const CROSS_ORIGIN_KEPT: &[&str] = &["accept", "accept-language", "content-type", "user-agent"];

/// Response headers that never reach the page.
const DROPPED_RESPONSE: &[&str] = &[
    "set-cookie",
    "set-cookie2",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// The page's headers, lowercased, minus the ones it may not set and any
/// `Connection` names, with [`USER_AGENT`] when the page sets none; an
/// invalid name or value is `Invalid`.
fn request_headers(given: &[(String, String)]) -> Result<Vec<(String, String)>, NetError> {
    let mut out = Vec::new();
    for (k, v) in given {
        let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| NetError::Invalid(format!("{k:?} is not a header name")))?;
        reqwest::header::HeaderValue::from_str(v)
            .map_err(|_| NetError::Invalid(format!("the value of {k} is not a header value")))?;
        let name = name.as_str().to_string();
        if DROPPED_REQUEST.contains(&name.as_str())
            || name.starts_with("proxy-")
            || name.starts_with("sec-")
        {
            continue;
        }
        out.push((name, v.clone()));
    }
    if !out.iter().any(|(k, _)| k == "user-agent") {
        out.push(("user-agent".into(), USER_AGENT.into()));
    }
    Ok(out)
}

fn response_headers(given: &[(String, String)]) -> BTreeMap<String, String> {
    let named: Vec<String> = given
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, v)| {
            v.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .collect();
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in given {
        let k = k.to_ascii_lowercase();
        if DROPPED_RESPONSE.contains(&k.as_str()) || named.contains(&k) {
            continue;
        }
        out.entry(k)
            .and_modify(|e| {
                e.push_str(", ");
                e.push_str(v);
            })
            .or_insert_with(|| v.clone());
    }
    out
}

/// `raw` as an https URL with a host and no credentials.
pub fn https_url(raw: &str) -> Result<Url, NetError> {
    let url =
        Url::parse(raw).map_err(|e| NetError::Invalid(format!("{raw:?} is not a URL: {e}")))?;
    if url.scheme() != "https" {
        return Err(NetError::NotHttps(url.to_string()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(NetError::Invalid(
            "a URL may not carry a user name or password".into(),
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(NetError::Invalid(format!("{raw} names no host")));
    }
    Ok(url)
}

/// The fetcher: a resolver, a transport and, in the daemon, a cache.
#[derive(Clone)]
pub struct Net {
    resolver: Arc<dyn Resolver>,
    transport: Arc<dyn Transport>,
    cache: Option<Arc<NetCache>>,
}

impl Default for Net {
    fn default() -> Self {
        Net::new(Arc::new(SystemResolver), Arc::new(ReqwestTransport))
    }
}

impl Net {
    #[must_use]
    pub fn new(resolver: Arc<dyn Resolver>, transport: Arc<dyn Transport>) -> Net {
        Net {
            resolver,
            transport,
            cache: None,
        }
    }

    /// This fetcher, answering cacheable requests from `cache`
    /// ([`crate::net_cache`]).
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<NetCache>) -> Net {
        self.cache = Some(cache);
        self
    }

    /// Fetches `req`, reaching only hosts `allows` accepts, under the
    /// module's rules, within [`TIMEOUT`], without the cache.
    ///
    /// # Errors
    /// [`NetError`], naming the first rule the fetch broke.
    pub async fn fetch(
        &self,
        allows: &(dyn Fn(&str) -> bool + Send + Sync),
        req: FetchRequest,
    ) -> Result<FetchResponse, NetError> {
        self.fetch_within(None, allows, req, TIMEOUT).await
    }

    /// [`Net::fetch`] for the artifact `app`, answered from the cache when
    /// cacheable: each artifact has entries of its own, so no app can see
    /// what another fetched.
    ///
    /// # Errors
    /// As [`Net::fetch`].
    pub async fn fetch_for(
        &self,
        app: &str,
        allows: &(dyn Fn(&str) -> bool + Send + Sync),
        req: FetchRequest,
    ) -> Result<FetchResponse, NetError> {
        self.fetch_within(Some(app), allows, req, TIMEOUT).await
    }

    /// [`Net::fetch_for`] (or, without `app`, [`Net::fetch`]) with another deadline.
    ///
    /// # Errors
    /// As [`Net::fetch`].
    pub async fn fetch_within(
        &self,
        app: Option<&str>,
        allows: &(dyn Fn(&str) -> bool + Send + Sync),
        req: FetchRequest,
        limit: Duration,
    ) -> Result<FetchResponse, NetError> {
        let start = tokio::time::Instant::now();
        let work = async {
            let key = app.and_then(|a| net_cache::key_of(a, &req));
            match (&self.cache, key) {
                (Some(cache), Some(key)) => {
                    self.cached(cache, &key, allows, req, start, limit).await
                }
                _ => self
                    .run(allows, req, &[], start, limit)
                    .await
                    .map(|(r, _)| r),
            }
        };
        tokio::time::timeout(limit, work)
            .await
            .unwrap_or(Err(NetError::Timeout))
    }

    /// A cacheable request: the host check first, then a fresh entry, or a
    /// fetch (revalidating a stale entry), whose answer the cache keeps when
    /// [`net_cache::storable`]. The entry's validators go to its own URL
    /// only, never past a redirect, and a `304` that cannot be answered
    /// from the entry is fetched again without them, so a page that sent no
    /// conditional request never sees one.
    async fn cached(
        &self,
        cache: &NetCache,
        key: &str,
        allows: &(dyn Fn(&str) -> bool + Send + Sync),
        req: FetchRequest,
        start: tokio::time::Instant,
        limit: Duration,
    ) -> Result<FetchResponse, NetError> {
        let url = https_url(&req.url)?;
        let host = url.host_str().unwrap_or_default();
        if !allows(host) {
            return Err(NetError::HostRefused {
                host: host.to_string(),
                redirect: false,
            });
        }
        let found = cache.get(key).await;
        let mut conditional = req.clone();
        let mut added = Vec::new();
        if let Some((entry, body)) = &found {
            if entry.fresh(cache.now()) {
                return Ok(entry.response(body.clone(), cache.now()));
            }
            for (k, v) in entry.validators() {
                added.push(k.clone());
                conditional.headers.push((k, v));
            }
        }
        let (mut res, mut set_cookie) = self.run(allows, conditional, &added, start, limit).await?;
        let mut same_url = res.url == url.as_str();
        if res.status == 304 {
            if let (true, Some((entry, body))) = (same_url, found) {
                let now = cache.now();
                let entry = entry.refreshed(&res.headers, now);
                cache.put(key, &entry, &body).await;
                return Ok(entry.response(body, now));
            }
            if !added.is_empty() {
                (res, set_cookie) = self.run(allows, req, &[], start, limit).await?;
                same_url = res.url == url.as_str();
            }
        }
        if same_url && net_cache::storable(res.status, set_cookie, &res.headers) {
            let entry = net_cache::Entry {
                url: res.url.clone(),
                status: res.status,
                headers: res.headers.clone(),
                received: cache.now().timestamp(),
                body_len: res.body.len() as u64,
            };
            cache.put(key, &entry, &res.body).await;
        } else {
            cache.remove(key).await;
        }
        Ok(res)
    }

    /// The fetch, and whether its final answer set a cookie. The headers
    /// named in `first_hop_only` go to the first hop only.
    async fn run(
        &self,
        allows: &(dyn Fn(&str) -> bool + Send + Sync),
        req: FetchRequest,
        first_hop_only: &[String],
        start: tokio::time::Instant,
        limit: Duration,
    ) -> Result<(FetchResponse, bool), NetError> {
        let method = req.method.to_ascii_uppercase();
        if reqwest::Method::from_bytes(method.as_bytes()).is_err()
            || method == "CONNECT"
            || method == "TRACE"
        {
            return Err(NetError::Invalid(format!(
                "{} is not a method a page may use",
                req.method
            )));
        }
        if req.body.as_ref().is_some_and(|b| b.len() > MAX_BODY) {
            return Err(NetError::TooLarge);
        }
        let mut headers = request_headers(&req.headers)?;
        let mut url = https_url(&req.url)?;
        let mut method = method;
        let mut body = req.body;
        let mut redirects = 0;
        loop {
            let host = url.host_str().unwrap_or_default().to_string();
            if !allows(&host) {
                return Err(NetError::HostRefused {
                    host,
                    redirect: redirects > 0,
                });
            }
            let port = url.port_or_known_default().unwrap_or(443);
            let addrs = self
                .resolver
                .resolve(&host, port)
                .await
                .map_err(|e| NetError::Upstream(format!("could not resolve {host}: {e}")))?;
            if addrs.is_empty() {
                return Err(NetError::Upstream(format!("{host} has no address")));
            }
            if let Some(bad) = addrs.iter().find(|a| !public_ip(a.ip())) {
                return Err(NetError::Blocked { host, ip: bad.ip() });
            }
            let res = self
                .transport
                .send(Hop {
                    method: method.clone(),
                    url: url.clone(),
                    headers: headers.clone(),
                    body: body.clone(),
                    addrs,
                    deadline: limit.saturating_sub(start.elapsed()),
                })
                .await?;
            if res.body.len() > MAX_BODY {
                return Err(NetError::TooLarge);
            }
            let location = res
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                .map(|(_, v)| v.clone());
            let redirect = matches!(res.status, 301 | 302 | 303 | 307 | 308);
            let (true, Some(location)) = (redirect, location) else {
                let set_cookie = res
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("set-cookie"));
                return Ok((
                    FetchResponse {
                        url: url.to_string(),
                        status: res.status,
                        headers: response_headers(&res.headers),
                        body: res.body,
                    },
                    set_cookie,
                ));
            };
            redirects += 1;
            headers.retain(|(k, _)| !first_hop_only.contains(k));
            if redirects > MAX_REDIRECTS {
                return Err(NetError::TooManyRedirects);
            }
            let next = url.join(&location).map_err(|_| {
                NetError::Upstream(format!("a redirect to {location:?}, which is not a URL"))
            })?;
            let next = https_url(next.as_str())?;
            // To another origin (host or port), only headers that carry no
            // credential go on.
            if (next.host_str(), next.port_or_known_default())
                != (url.host_str(), url.port_or_known_default())
            {
                headers.retain(|(k, _)| CROSS_ORIGIN_KEPT.contains(&k.as_str()));
            }
            // 303, and 301/302 after a POST, continue as a GET without a body.
            if res.status == 303 || (matches!(res.status, 301 | 302) && method == "POST") {
                if method != "HEAD" {
                    method = "GET".into();
                }
                body = None;
                headers.retain(|(k, _)| k != "content-type");
            }
            url = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn only_public_unicast_addresses_pass() {
        for ok in [
            "93.184.216.34",
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
        ] {
            assert!(public_ip(ok.parse().unwrap()), "{ok}");
        }
        for bad in [
            "0.0.0.0",
            "127.0.0.1",
            "127.255.0.9",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "224.0.0.1",
            "239.255.255.250",
            "255.255.255.255",
            "240.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "fe80::1",
            "fc00::1",
            "fd12:3456::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
            "2001::1",
            "2002:7f00:1::1",
            "100::1",
        ] {
            assert!(!public_ip(bad.parse().unwrap()), "{bad}");
        }
    }

    #[test]
    fn urls_must_be_https_without_credentials() {
        assert!(https_url("https://api.example.com/x?y=1").is_ok());
        assert_eq!(
            https_url("http://api.example.com/").unwrap_err().code(),
            "https_only"
        );
        assert_eq!(
            https_url("ftp://api.example.com/").unwrap_err().code(),
            "https_only"
        );
        assert_eq!(
            https_url("https://u:p@api.example.com/")
                .unwrap_err()
                .code(),
            "invalid_argument"
        );
        assert_eq!(
            https_url("not a url").unwrap_err().code(),
            "invalid_argument"
        );
    }

    /// A resolver answering from a table.
    struct Table(Vec<(&'static str, Vec<&'static str>)>);
    impl Resolver for Table {
        fn resolve<'a>(
            &'a self,
            host: &'a str,
            port: u16,
        ) -> BoxFuture<'a, std::io::Result<Vec<SocketAddr>>> {
            let found = self.0.iter().find(|(h, _)| *h == host).map(|(_, ips)| {
                ips.iter()
                    .map(|ip| SocketAddr::new(ip.parse().unwrap(), port))
                    .collect()
            });
            Box::pin(async move { found.ok_or_else(|| std::io::Error::other("no such host")) })
        }
    }

    /// A transport answering from a script and keeping the hops it got.
    #[derive(Default)]
    struct Script {
        answers: Mutex<Vec<HopResponse>>,
        hops: Mutex<Vec<Hop>>,
    }
    impl Transport for Script {
        fn send(&self, hop: Hop) -> BoxFuture<'_, Result<HopResponse, NetError>> {
            self.hops.lock().unwrap().push(hop);
            let next = self.answers.lock().unwrap().remove(0);
            Box::pin(async move { Ok(next) })
        }
    }

    fn answer(status: u16, headers: &[(&str, &str)], body: &str) -> HopResponse {
        HopResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn setup(answers: Vec<HopResponse>) -> (Net, Arc<Script>) {
        let script = Arc::new(Script {
            answers: Mutex::new(answers),
            hops: Mutex::default(),
        });
        let table = Table(vec![
            ("api.example.com", vec!["93.184.216.34"]),
            ("cdn.example.com", vec!["93.184.216.35", "2606:4700::1"]),
            ("evil.example.com", vec!["93.184.216.36", "127.0.0.1"]),
            ("meta.example.com", vec!["169.254.169.254"]),
            ("other.example.com", vec!["93.184.216.37"]),
        ]);
        (Net::new(Arc::new(table), script.clone()), script)
    }

    fn get(url: &str) -> FetchRequest {
        FetchRequest {
            url: url.into(),
            method: "GET".into(),
            headers: vec![],
            body: None,
        }
    }

    const ALLOWED: &[&str] = &[
        "api.example.com",
        "cdn.example.com",
        "evil.example.com",
        "meta.example.com",
    ];
    fn allows(h: &str) -> bool {
        ALLOWED.contains(&h)
    }

    #[tokio::test]
    async fn a_fetch_goes_to_the_vetted_addresses_and_strips_cookies() {
        let (net, script) = setup(vec![answer(
            200,
            &[
                ("Content-Type", "application/json"),
                ("Set-Cookie", "a=b"),
                ("X-A", "1"),
                ("x-a", "2"),
                ("Connection", "x-secret"),
                ("X-Secret", "s"),
            ],
            "{\"ok\":true}",
        )]);
        let mut req = get("https://api.example.com/v1?q=1");
        req.method = "post".into();
        req.body = Some(b"hi".to_vec());
        req.headers = vec![
            ("Cookie".into(), "c=d".into()),
            ("Authorization".into(), "Bearer k".into()),
            ("Host".into(), "evil".into()),
            ("Content-Type".into(), "text/plain".into()),
        ];
        let res = net.fetch(&allows, req).await.unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(res.body, b"{\"ok\":true}");
        assert_eq!(
            res.headers.into_iter().collect::<Vec<_>>(),
            vec![
                ("content-type".into(), "application/json".into()),
                ("x-a".into(), "1, 2".into())
            ]
        );
        let hops = script.hops.lock().unwrap();
        assert_eq!(hops.len(), 1);
        assert_eq!(hops[0].method, "POST");
        assert_eq!(hops[0].addrs, vec!["93.184.216.34:443".parse().unwrap()]);
        assert_eq!(
            hops[0].headers,
            vec![
                ("authorization".into(), "Bearer k".into()),
                ("content-type".into(), "text/plain".into()),
                ("user-agent".into(), USER_AGENT.into())
            ]
        );
        assert_eq!(hops[0].body.as_deref(), Some(&b"hi"[..]));
    }

    #[tokio::test]
    async fn the_daemon_names_itself_unless_the_page_does() {
        assert!(USER_AGENT.starts_with("Clax/"));
        assert!(USER_AGENT.ends_with(" (+https://github.com/empathic/clax)"));
        let (net, script) = setup(vec![answer(200, &[], ""), answer(200, &[], "")]);
        net.fetch(&allows, get("https://api.example.com/"))
            .await
            .unwrap();
        let mut req = get("https://api.example.com/");
        req.headers = vec![("User-Agent".into(), "Mine/1".into())];
        net.fetch(&allows, req).await.unwrap();
        let hops = script.hops.lock().unwrap();
        assert_eq!(
            hops[0].headers,
            vec![("user-agent".into(), USER_AGENT.into())]
        );
        assert_eq!(
            hops[1].headers,
            vec![("user-agent".into(), "Mine/1".into())]
        );
    }

    #[tokio::test]
    async fn hosts_and_addresses_are_refused_before_anything_is_sent() {
        for (url, code) in [
            ("http://api.example.com/", "https_only"),
            ("https://other.example.com/", "not_granted"),
            ("https://evil.example.com/", "blocked_address"),
            ("https://meta.example.com/", "blocked_address"),
            ("https://127.0.0.1/", "not_granted"),
            ("https://[::1]/", "not_granted"),
        ] {
            let (net, script) = setup(vec![answer(500, &[], "")]);
            let e = net.fetch(&allows, get(url)).await.unwrap_err();
            assert_eq!(e.code(), code, "{url}: {e}");
            assert!(
                script.hops.lock().unwrap().is_empty(),
                "{url}: nothing is sent"
            );
        }
    }

    #[tokio::test]
    async fn redirects_are_followed_three_times_each_vetted() {
        let (net, script) = setup(vec![
            answer(302, &[("location", "https://cdn.example.com/a")], ""),
            answer(303, &[("location", "/b")], ""),
            answer(307, &[("Location", "https://api.example.com/c")], ""),
            answer(200, &[], "done"),
        ]);
        let mut req = get("https://api.example.com/start");
        req.method = "POST".into();
        req.body = Some(b"x".to_vec());
        req.headers = vec![
            ("authorization".into(), "k".into()),
            ("content-type".into(), "text/plain".into()),
        ];
        let res = net.fetch(&allows, req).await.unwrap();
        assert_eq!(
            (res.status, res.url.as_str()),
            (200, "https://api.example.com/c")
        );
        {
            let hops = script.hops.lock().unwrap();
            let seen: Vec<(&str, &str, bool, usize)> = hops
                .iter()
                .map(|h| {
                    (
                        h.method.as_str(),
                        h.url.as_str(),
                        h.body.is_some(),
                        h.headers.len(),
                    )
                })
                .collect();
            assert_eq!(
                seen,
                vec![
                    ("POST", "https://api.example.com/start", true, 3),
                    // A 302 after a POST is a GET; another host loses
                    // Authorization and keeps only the User-Agent.
                    ("GET", "https://cdn.example.com/a", false, 1),
                    ("GET", "https://cdn.example.com/b", false, 1),
                    ("GET", "https://api.example.com/c", false, 1),
                ]
            );
            assert_eq!(hops[1].addrs.len(), 2);
        }

        let (net, _) = setup(vec![
            answer(301, &[("location", "https://api.example.com/1")], ""),
            answer(301, &[("location", "https://api.example.com/2")], ""),
            answer(301, &[("location", "https://api.example.com/3")], ""),
            answer(301, &[("location", "https://api.example.com/4")], ""),
        ]);
        let e = net
            .fetch(&allows, get("https://api.example.com/0"))
            .await
            .unwrap_err();
        assert_eq!(e.code(), "too_many_redirects");

        for (to, code) in [
            ("https://other.example.com/", "redirect_refused"),
            ("http://api.example.com/", "https_only"),
            ("https://evil.example.com/", "blocked_address"),
        ] {
            let (net, script) = setup(vec![
                answer(302, &[("location", to)], ""),
                answer(200, &[], ""),
            ]);
            let e = net
                .fetch(&allows, get("https://api.example.com/"))
                .await
                .unwrap_err();
            assert_eq!(e.code(), code, "{to}");
            assert_eq!(script.hops.lock().unwrap().len(), 1, "{to}: never sent");
        }
    }

    #[tokio::test]
    async fn bodies_and_requests_are_checked() {
        let (net, _) = setup(vec![]);
        let mut big = get("https://api.example.com/");
        big.method = "PUT".into();
        big.body = Some(vec![0; MAX_BODY + 1]);
        assert_eq!(
            net.fetch(&allows, big).await.unwrap_err().code(),
            "too_large"
        );
        let mut bad = get("https://api.example.com/");
        bad.headers = vec![("bad header".into(), "x".into())];
        assert_eq!(
            net.fetch(&allows, bad).await.unwrap_err().code(),
            "invalid_argument"
        );
        let mut m = get("https://api.example.com/");
        m.method = "CONNECT".into();
        assert_eq!(
            net.fetch(&allows, m).await.unwrap_err().code(),
            "invalid_argument"
        );

        let (net, _) = setup(vec![HopResponse {
            status: 200,
            headers: vec![],
            body: vec![0; MAX_BODY + 1],
        }]);
        assert_eq!(
            net.fetch(&allows, get("https://api.example.com/"))
                .await
                .unwrap_err()
                .code(),
            "too_large"
        );
    }

    /// A transport that never answers.
    struct Hang;
    impl Transport for Hang {
        fn send(&self, _hop: Hop) -> BoxFuture<'_, Result<HopResponse, NetError>> {
            Box::pin(futures::future::pending())
        }
    }

    #[tokio::test]
    async fn a_fetch_ends_at_its_deadline() {
        let net = Net::new(
            Arc::new(Table(vec![("api.example.com", vec!["93.184.216.34"])])),
            Arc::new(Hang),
        );
        let e = net
            .fetch_within(
                None,
                &allows,
                get("https://api.example.com/"),
                Duration::from_millis(1),
            )
            .await
            .unwrap_err();
        assert_eq!(e, NetError::Timeout);
    }

    /// The real transport connects to the hop's addresses and never asks
    /// DNS: a host under `.invalid` (which never resolves) reaches a local
    /// listener given as its address. The listener speaks no TLS, so the
    /// exchange then fails as `upstream_error`.
    #[tokio::test]
    async fn the_real_transport_connects_only_to_the_pinned_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            drop(sock);
        });
        let hop = Hop {
            method: "GET".into(),
            url: Url::parse(&format!("https://pinned.invalid:{}/x", addr.port())).unwrap(),
            headers: vec![],
            body: None,
            addrs: vec![addr],
            deadline: Duration::from_secs(10),
        };
        let e = ReqwestTransport.send(hop).await.unwrap_err();
        assert_eq!(e.code(), "upstream_error", "{e}");
        accepted.await.unwrap();
    }

    #[tokio::test]
    async fn a_redirect_to_another_origin_carries_no_credentials() {
        let (net, script) = setup(vec![
            answer(302, &[("location", "/same")], ""),
            answer(
                302,
                &[("location", "https://api.example.com:8443/port")],
                "",
            ),
            answer(200, &[], ""),
        ]);
        let mut req = get("https://api.example.com/start");
        req.headers = vec![
            ("accept".into(), "application/json".into()),
            ("authorization".into(), "Bearer k".into()),
            ("x-api-key".into(), "k".into()),
            ("x-custom".into(), "c".into()),
        ];
        net.fetch(&allows, req).await.unwrap();
        let hops = script.hops.lock().unwrap();
        let names: Vec<Vec<&str>> = hops
            .iter()
            .map(|h| h.headers.iter().map(|(k, _)| k.as_str()).collect())
            .collect();
        let all = vec![
            "accept",
            "authorization",
            "x-api-key",
            "x-custom",
            "user-agent",
        ];
        assert_eq!(names, vec![all.clone(), all, vec!["accept", "user-agent"]]);
    }
}
