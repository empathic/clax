//! The daemon's web source (spec
//! `docs/superpowers/specs/2026-11-02-claxbot-web-tools-design.md` §4.3,
//! §5.3, §10): [`RestSource`], which fetches pages with the daemon's own
//! HTTP client and converts HTML to Markdown itself
//! ([`clax_bot::web::markdown`]), and, with a Cloudflare account, searches
//! and converts PDF and Office documents through Cloudflare's REST API;
//! and [`DaemonWeb`], which builds one from the instance's settings and
//! sealed token at each call. Without an account the daemon still
//! fetches.
//!
//! # The host check
//!
//! A [`PublicUrl`] refuses private and local IP literals and names like
//! `localhost`; the daemon also resolves every name it fetches, on the
//! first request and on every redirect hop, and refuses the request when
//! any address the name resolves to is not public ([`is_public`], the
//! ranges of [`clax_bot::web::url::is_local_addr`]). The checking happens
//! in the HTTP client's resolver ([`CheckedResolver`]), so the client
//! connects only to an address it checked: an answer that changes between
//! a check and the connection is never followed. The client follows no
//! redirect itself and uses no proxy (a proxy would resolve the name
//! instead).
//!
//! # Test mode
//!
//! With `CLAX_TEST_WEB_FIXTURES=<path>` in its environment the daemon uses
//! a [`FakeWebSource`] loaded from that JSON file
//! ([`FakeWebSource::from_fixtures`]'s shape) instead, and web use is
//! configured ([`fixtures`]).

use clax_bot::web::markdown::html_to_markdown;
use clax_bot::web::page::{
    Handling, decode, decode_html, handling, http_error, is_sniffed, links_of_body, links_of_html,
    links_of_markdown, media_type, media_type_of, pretty_json, user_agent,
};
use clax_bot::web::url::{is_local_addr, redirect_allowed};
use clax_bot::web::{
    CODE_URL_NOT_ALLOWED, CODE_WEB_BLOCKED, CODE_WEB_TIMEOUT, CODE_WEB_TOO_LARGE,
    CODE_WEB_UNAVAILABLE, CODE_WEB_UNSUPPORTED_CONTENT_TYPE, FakeWebSource, FetchLimits,
    FetchedPage, Format, OwnHosts, PublicUrl, SearchAnswer, SearchHit, SearchQuery, WebError,
    WebShape, WebSource, WithOwnHosts,
};
use clax_bot_sandbox::{CODE_CANCELLED, CancelToken};
use clax_core::secret::Secret;
use clax_store::Store;
use clax_store::bot_settings::Setting;
use clax_store::secrets::{CLOUDFLARE_ORIGIN, Kek, SecretSlot};
use serde_json::{Value, json};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use url::Url;

/// The `Accept` header of a fetch: Markdown first, as Cloudflare's own
/// fetch tool asks.
pub const ACCEPT: &str = "text/markdown, text/html;q=0.9, */*;q=0.8";

/// The environment variable naming the test mode's fixtures file.
pub const FIXTURES_ENV: &str = "CLAX_TEST_WEB_FIXTURES";

/// How long a search or a document's conversion request to Cloudflare may
/// take on its own (a fetch's conversion is also within the fetch's
/// deadline).
const API_TIMEOUT: Duration = Duration::from_secs(30);

/// The most bytes of a search answer read from Cloudflare.
const SEARCH_ANSWER_BYTES: u64 = 1 << 20;

/// The most bytes of a Markdown conversion's answer read from Cloudflare.
const CONVERSION_ANSWER_BYTES: u64 = 16 << 20;

/// What a search answers on a daemon without a Cloudflare account.
const NO_ACCOUNT: &str = "web search is not configured on this instance: an admin sets the Cloudflare account ID and API token";

/// `body`, an HTML page served as `declared`, from `at`: as Markdown, and
/// its links. Decoding, the link scan and the conversion are linear in the
/// page, but a page is up to 5 MB, so they run on a blocking thread, not
/// on the run driver's task, and a conversion that fails there answers
/// `web_unavailable` rather than ending that task.
async fn convert_html(
    body: Vec<u8>,
    declared: String,
    at: PublicUrl,
) -> Result<(String, Vec<String>), WebError> {
    tokio::task::spawn_blocking(move || {
        let html = decode_html(&body, &declared);
        (html_to_markdown(&html, &at), links_of_html(&html, &at))
    })
    .await
    .map_err(|e| {
        unavailable(
            format!("the page could not be converted to Markdown ({e})"),
            false,
        )
    })
}

/// Whether `ip` is a public unicast address a fetch may connect to: not
/// in any range [`is_local_addr`] refuses, the one list the URL check and
/// the DNS check share.
#[must_use]
pub fn is_public(ip: IpAddr) -> bool {
    !is_local_addr(ip)
}

/// The future of a [`Lookup`].
pub type LookupFuture = Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send>>;

/// Resolves a host name to its addresses: the system's resolver in
/// production, a stub in tests.
pub trait Lookup: Send + Sync + 'static {
    fn lookup(&self, host: &str) -> LookupFuture;
}

/// The system's resolver (`getaddrinfo`, through tokio).
struct SystemLookup;

impl Lookup for SystemLookup {
    fn lookup(&self, host: &str) -> LookupFuture {
        let host = host.to_string();
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .map(|a| a.ip())
                .collect())
        })
    }
}

/// A name refused by the host check: it resolved to a non-public address.
#[derive(Debug)]
struct Refused(String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// The fetch client's resolver: resolves a name and answers its addresses
/// only when every one is public ([`is_public`]), so the client connects
/// to nothing else. `allow_loopback` also admits loopback addresses, for
/// tests against a local server; nothing else sets it. Each fetch has its
/// own, which sets `answered` once it has answered checked addresses: the
/// client then begins connecting to them, and the fetch has gone out
/// ([`RestSource::fetch`]).
#[derive(Clone)]
pub struct CheckedResolver {
    lookup: Arc<dyn Lookup>,
    allow_loopback: bool,
    answered: Arc<AtomicBool>,
}

impl CheckedResolver {
    /// `host`'s addresses, when every one may be connected to.
    async fn check(
        &self,
        host: &str,
    ) -> Result<Vec<IpAddr>, Box<dyn std::error::Error + Send + Sync>> {
        let addrs = self.lookup.lookup(host).await?;
        if addrs.is_empty() {
            return Err(format!("{host} resolves to no address").into());
        }
        if let Some(bad) = addrs
            .iter()
            .find(|ip| !(is_public(**ip) || (self.allow_loopback && ip.is_loopback())))
        {
            return Err(Box::new(Refused(format!(
                "{host} resolves to {bad}, a private or local address"
            ))));
        }
        Ok(addrs)
    }
}

impl reqwest::dns::Resolve for CheckedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let this = self.clone();
        Box::pin(async move {
            let addrs = this.check(name.as_str()).await?;
            this.answered.store(true, Ordering::SeqCst);
            // Port 0: the client puts the URL's port on each address.
            let addrs: reqwest::dns::Addrs =
                Box::new(addrs.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// The host check's refusal somewhere in `e`'s chain of causes.
fn refusal_in(e: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut at = Some(e);
    while let Some(e) = at {
        if let Some(r) = e.downcast_ref::<Refused>() {
            return Some(r.0.clone());
        }
        at = e.source();
    }
    None
}

fn cancelled() -> WebError {
    WebError::new(CODE_CANCELLED, "the run was cancelled", false)
}

fn unavailable(message: impl Into<String>, retryable: bool) -> WebError {
    WebError::new(CODE_WEB_UNAVAILABLE, message, retryable)
}

/// A [`WebSource`] of pages fetched by the daemon itself, with the host
/// check ([`CheckedResolver`]) on every request, and HTML converted to
/// Markdown by the daemon ([`html_to_markdown`]): the page goes nowhere
/// else. With a Cloudflare account it also searches, through
/// `POST {api_base}/client/v4/accounts/{id}/ai/websearch/`, and converts
/// PDF and Office documents through
/// `POST {api_base}/client/v4/accounts/{id}/ai/tomarkdown`; without one a
/// search answers `web_unavailable` before anything is sent, and a
/// document `web_unsupported_content_type` once it has been fetched.
pub struct RestSource {
    account: Option<Account>,
    /// Cloudflare's API: no redirects followed (the token goes nowhere
    /// else). The client is the reason it could not be built otherwise,
    /// and every call then answers `web_unavailable`: never a client
    /// without these rules.
    api: Result<reqwest::Client, String>,
    /// What each fetch's own client resolves names with
    /// ([`RestSource::web_client`]).
    lookup: Arc<dyn Lookup>,
    allow_loopback: bool,
    /// The TLS configuration every fetch's client takes, built once
    /// ([`fetch_tls`]); the reason it could not be otherwise.
    tls: Result<rustls::ClientConfig, String>,
    /// Sent on every fetch ([`user_agent`]).
    user_agent: String,
}

/// The Cloudflare account a [`RestSource`] searches and converts
/// documents through: where its API is, its ID and its token.
struct Account {
    api_base: Url,
    id: String,
    token: Secret,
}

impl RestSource {
    /// A source for `account_id` with `token`, at `api_base`
    /// (`https://api.cloudflare.com` in production). `allow_loopback`
    /// admits loopback addresses to the fetch client, for tests against a
    /// local server only.
    #[must_use]
    pub fn new(
        api_base: Url,
        account_id: String,
        token: Secret,
        allow_loopback: bool,
    ) -> RestSource {
        RestSource::with_lookup(
            api_base,
            account_id,
            token,
            allow_loopback,
            Arc::new(SystemLookup),
        )
    }

    /// [`RestSource::new`], resolving names with `lookup`.
    #[must_use]
    pub fn with_lookup(
        api_base: Url,
        account_id: String,
        token: Secret,
        allow_loopback: bool,
        lookup: Arc<dyn Lookup>,
    ) -> RestSource {
        RestSource::of(
            Some(Account {
                api_base,
                id: account_id,
                token,
            }),
            allow_loopback,
            lookup,
        )
    }

    /// A source without a Cloudflare account: it fetches, converting HTML
    /// itself, and searches nothing. `allow_loopback` as for
    /// [`RestSource::new`]; names resolved with `lookup`.
    #[must_use]
    pub fn without_account(allow_loopback: bool, lookup: Arc<dyn Lookup>) -> RestSource {
        RestSource::of(None, allow_loopback, lookup)
    }

    fn of(account: Option<Account>, allow_loopback: bool, lookup: Arc<dyn Lookup>) -> RestSource {
        let api = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| e.to_string());
        RestSource {
            account,
            api,
            lookup,
            allow_loopback,
            tls: fetch_tls(),
            user_agent: user_agent(None),
        }
    }

    /// The client of one fetch: the checked resolver, which sets
    /// `answered` once it has answered checked addresses, no redirects, no
    /// proxy, and no connection shared with another fetch (each fetch's
    /// first request resolves its name, so it is known when it went out).
    fn web_client(&self, answered: Arc<AtomicBool>) -> Result<reqwest::Client, WebError> {
        let failed = |e: &dyn std::fmt::Display| {
            unavailable(format!("the daemon's HTTP client failed ({e})"), false)
        };
        let tls = self.tls.as_ref().map_err(|e| failed(e))?;
        reqwest::Client::builder()
            .use_preconfigured_tls(tls.clone())
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .dns_resolver(Arc::new(CheckedResolver {
                lookup: self.lookup.clone(),
                allow_loopback: self.allow_loopback,
                answered,
            }))
            .build()
            .map_err(|e| failed(&e))
    }

    /// This source, sending `user_agent` on every fetch ([`user_agent`]).
    #[must_use]
    pub fn with_user_agent(mut self, user_agent: String) -> RestSource {
        self.user_agent = user_agent;
        self
    }

    fn client(c: &Result<reqwest::Client, String>) -> Result<&reqwest::Client, WebError> {
        c.as_ref()
            .map_err(|e| unavailable(format!("the daemon's HTTP client failed ({e})"), false))
    }

    /// `{api_base}/client/v4/accounts/{id}/ai/{path}` of `account`.
    fn endpoint(account: &Account, path: &str) -> String {
        format!(
            "{}/client/v4/accounts/{}/ai/{path}",
            account.api_base.as_str().trim_end_matches('/'),
            account.id
        )
    }

    /// Cloudflare's answer `v` with `status`, as its `result`, or the error
    /// of a failed call.
    fn api_result(what: &str, status: reqwest::StatusCode, v: Value) -> Result<Value, WebError> {
        let errors = v
            .get("errors")
            .and_then(Value::as_array)
            .map(|es| {
                es.iter()
                    .filter_map(|e| e.get("message").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|m| !m.is_empty());
        if !status.is_success() || v.get("success") == Some(&Value::Bool(false)) {
            let retryable = status.is_server_error() || status.as_u16() == 429;
            let why = errors.unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
            return Err(unavailable(
                format!("Cloudflare's {what} failed ({}): {why}", status.as_u16()),
                retryable,
            ));
        }
        Ok(match v.get("result") {
            Some(r) if !r.is_null() => r.clone(),
            _ => v,
        })
    }

    /// `body`, a file named `name` of type `mime`, converted to Markdown by
    /// Cloudflare through `account`.
    async fn to_markdown(
        &self,
        account: &Account,
        body: Vec<u8>,
        name: String,
        mime: &str,
    ) -> Result<String, WebError> {
        let part = reqwest::multipart::Part::bytes(body)
            .file_name(name)
            .mime_str(mime)
            .map_err(|e| unavailable(format!("the page's type cannot be sent ({e})"), false))?;
        let form = reqwest::multipart::Form::new().part("files", part);
        let r = RestSource::client(&self.api)?
            .post(RestSource::endpoint(account, "tomarkdown"))
            .bearer_auth(account.token.expose())
            .multipart(form)
            .timeout(API_TIMEOUT)
            .send()
            .await
            .map_err(|e| api_error("Markdown conversion", &e))?;
        let status = r.status();
        let v = api_json(r, CONVERSION_ANSWER_BYTES, "Markdown conversion").await?;
        let result = RestSource::api_result("Markdown conversion", status, v)?;
        let first = result.as_array().and_then(|a| a.first());
        match first {
            Some(f) if f.get("format").and_then(Value::as_str) == Some("markdown") => Ok(f
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()),
            Some(f) => Err(unavailable(
                format!(
                    "the page could not be converted to Markdown: {}",
                    f.get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("no reason given")
                ),
                false,
            )),
            None => Err(unavailable(
                "Cloudflare's Markdown conversion answered no result",
                true,
            )),
        }
    }

    /// The fetch, without its deadline or cancellation; `out` is set once
    /// it has gone out: when the first request's name has resolved to
    /// checked addresses (by the client's resolver), or, for a host that
    /// is an IP address, as the first request is sent.
    async fn fetch_page(
        &self,
        url: &PublicUrl,
        format: Format,
        limits: &FetchLimits,
        hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
        out: &Arc<AtomicBool>,
    ) -> Result<FetchedPage, WebError> {
        let client = self.web_client(out.clone())?;
        if goes_out_on_send(url) {
            out.store(true, Ordering::SeqCst);
        }
        let mut at = url.clone();
        let mut hops = 0u8;
        let resp = loop {
            let r = client
                .get(at.as_str())
                .header(reqwest::header::ACCEPT, ACCEPT)
                .header(reqwest::header::USER_AGENT, &self.user_agent)
                .send()
                .await
                .map_err(|e| fetch_error(&e, &at, hops > 0))?;
            if !matches!(r.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                break r;
            }
            if hops >= limits.max_redirects {
                return Err(unavailable(
                    format!(
                        "{} redirected more than {} times",
                        url.normalized(),
                        limits.max_redirects
                    ),
                    false,
                )
                .went_out(true));
            }
            hops += 1;
            let target = r
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|l| at.as_url().join(l).ok());
            let no_url = || {
                unavailable(format!("{} redirected to no URL", at.normalized()), false)
                    .went_out(true)
            };
            let Some(target) = target else {
                return Err(no_url());
            };
            let to = match PublicUrl::parse(target.as_str()) {
                Ok(to) => to,
                Err(e) if e.code == CODE_URL_NOT_ALLOWED => return Err(e.went_out(true)),
                Err(_) => return Err(no_url()),
            };
            if !redirect_allowed(&at, &to) {
                return Err(WebError::new(
                    CODE_URL_NOT_ALLOWED,
                    format!("{} redirected from https to http", at.normalized()),
                    false,
                )
                .went_out(true));
            }
            hop(&to).map_err(|e| e.went_out(true))?;
            at = to;
        };
        let gone = |e: WebError| e.went_out(true);
        if resp
            .headers()
            .get("cf-mitigated")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("challenge"))
        {
            return Err(gone(WebError::new(
                CODE_WEB_BLOCKED,
                format!(
                    "{} answered with a bot challenge; try another source",
                    at.normalized()
                ),
                false,
            )));
        }
        let status = resp.status().as_u16();
        let declared = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let unsupported = |media: &str, format: Format| {
            let what = if media.is_empty() {
                "of no known type"
            } else {
                media
            };
            let raw = if format == Format::Raw
                && handling(media, Format::Auto) != Handling::Unsupported
            {
                " with format \"raw\"; fetch it without"
            } else {
                ""
            };
            gone(WebError::new(
                CODE_WEB_UNSUPPORTED_CONTENT_TYPE,
                format!(
                    "{} is {what}, which cannot be read as text{raw}. Use another source.",
                    at.normalized()
                ),
                false,
            ))
        };
        // Before the body is read: an error status whose served type is not
        // text has no readable body, and a type the rules refuse is
        // refused. A type that says too little is sniffed from the body.
        // A document is read only through a Cloudflare account.
        let no_account = |media: &str| {
            gone(WebError::new(
                CODE_WEB_UNSUPPORTED_CONTENT_TYPE,
                format!(
                    "{} is {media}, which this instance reads only with a Cloudflare account (an admin sets one). Use another source.",
                    at.normalized()
                ),
                false,
            ))
        };
        let served = media_type(&declared);
        if !is_sniffed(&served) && !handling(&served, format).is_textual() {
            if status >= 400 {
                return Err(http_error(&at, status));
            }
            if handling(&served, format) == Handling::Unsupported {
                return Err(unsupported(&served, format));
            }
            if matches!(handling(&served, format), Handling::Document(_)) && self.account.is_none()
            {
                return Err(no_account(&served));
            }
        }
        let body = read_capped(resp, limits.max_bytes, &at)
            .await
            .map_err(gone)?;
        let content_type = media_type_of(&declared, &body);
        // An error status is a page only with a readable body, as on the
        // Worker.
        if status >= 400 && (body.is_empty() || !handling(&content_type, format).is_textual()) {
            return Err(http_error(&at, status));
        }
        let (text, links) = match handling(&content_type, format) {
            Handling::Unsupported => return Err(unsupported(&content_type, format)),
            Handling::Html => {
                let (md, links) = convert_html(body, declared.clone(), at.clone())
                    .await
                    .map_err(gone)?;
                // An error page whose conversion is empty has nothing to
                // read, as on the Worker.
                if status >= 400 && md.trim().is_empty() {
                    return Err(http_error(&at, status));
                }
                (md, links)
            }
            Handling::RawHtml => {
                let html = decode_html(&body, &declared);
                let links = links_of_html(&html, &at);
                (html, links)
            }
            Handling::Document(ext) => {
                let Some(account) = &self.account else {
                    return Err(no_account(&content_type));
                };
                let md = self
                    .to_markdown(account, body, format!("document.{ext}"), &content_type)
                    .await
                    .map_err(gone)?;
                if status >= 400 && md.trim().is_empty() {
                    return Err(http_error(&at, status));
                }
                let links = links_of_markdown(&md, &at);
                (md, links)
            }
            Handling::Json => {
                let s = decode(&body, &declared);
                let s = pretty_json(&s).unwrap_or(s);
                let links = links_of_body(&s, &at, &content_type);
                (s, links)
            }
            Handling::Text => {
                let s = decode(&body, &declared);
                let links = links_of_body(&s, &at, &content_type);
                (s, links)
            }
        };
        Ok(FetchedPage {
            final_url: at.normalized().to_string(),
            status,
            content_type,
            text,
            links,
            request_id: None,
        })
    }
}

/// The TLS configuration of a fetch's client, as reqwest's `rustls-tls`
/// builds its own: ring, TLS 1.2 and 1.3, the webpki roots, HTTP/1.1 by
/// ALPN. Built once per source, so a fetch's client parses no roots.
fn fetch_tls() -> Result<rustls::ClientConfig, String> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(tls)
}

/// Whether a fetch of `url` goes out as its first request is sent: its
/// host is an IP address, so there is no name to resolve and the
/// connection begins at once. A host name goes out only once the checked
/// resolver has answered it ([`CheckedResolver`]).
fn goes_out_on_send(url: &PublicUrl) -> bool {
    !matches!(url.as_url().host(), Some(url::Host::Domain(_)))
}

/// The error of a call to Cloudflare's API that got no answer.
fn api_error(what: &str, e: &reqwest::Error) -> WebError {
    if e.is_timeout() {
        WebError::new(
            CODE_WEB_TIMEOUT,
            format!("Cloudflare's {what} did not answer in time; try again"),
            true,
        )
    } else {
        unavailable(
            format!("Cloudflare's {what} could not be reached ({e})"),
            true,
        )
    }
}

/// The error of a request to `at` that got no answer; `sent_before` when
/// an earlier request of the fetch was already answered.
fn fetch_error(e: &reqwest::Error, at: &PublicUrl, sent_before: bool) -> WebError {
    if let Some(why) = refusal_in(e) {
        return WebError::new(
            CODE_URL_NOT_ALLOWED,
            format!("{why}; fetch a public URL instead"),
            false,
        )
        .went_out(sent_before);
    }
    if e.is_timeout() {
        return WebError::new(
            CODE_WEB_TIMEOUT,
            format!("{} did not answer in time", at.normalized()),
            true,
        )
        .went_out(true);
    }
    if e.is_connect() {
        return unavailable(format!("{} could not be reached ({e})", at.host()), true)
            .went_out(sent_before);
    }
    // A request that could not be built or sent: gone out only if an
    // earlier one was.
    unavailable(
        format!("the fetch of {} failed ({e})", at.normalized()),
        true,
    )
    .went_out(sent_before)
}

/// Cloudflare's JSON answer `r`, read up to `max` bytes; `Null` when it is
/// not JSON (the status then says what failed).
///
/// # Errors
/// `web_unavailable` past `max` bytes, `web_timeout` or `web_unavailable`
/// when the body could not be read.
async fn api_json(mut r: reqwest::Response, max: u64, what: &str) -> Result<Value, WebError> {
    let too_large = || {
        unavailable(
            format!("Cloudflare's {what} answered more than {max} bytes"),
            false,
        )
    };
    if r.content_length().is_some_and(|n| n > max) {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|e| api_error(what, &e))? {
        if (body.len() + chunk.len()) as u64 > max {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// `resp`'s body, refused past `max` bytes as it streams.
async fn read_capped(
    mut resp: reqwest::Response,
    max: u64,
    at: &PublicUrl,
) -> Result<Vec<u8>, WebError> {
    let too_large = || {
        WebError::new(
            CODE_WEB_TOO_LARGE,
            format!("{} is larger than {max} bytes", at.normalized()),
            false,
        )
    };
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| fetch_error(&e, at, true))? {
        if (body.len() + chunk.len()) as u64 > max {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// `v`'s `result` (or `v` itself) as a search answer: each item with a
/// URL ([`SearchHit::of_item`]), and the request ID of
/// `metadata.requestId`.
fn parse_search(v: Value) -> SearchAnswer {
    let result = match v.get("result") {
        Some(r) if r.is_object() => r.clone(),
        _ => v.clone(),
    };
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
    let hits = SearchHit::of_items(&result);
    SearchAnswer {
        hits,
        request_id: result.get("metadata").and_then(|m| s(m, "requestId")),
        raw: v,
    }
}

#[async_trait::async_trait]
impl WebSource for RestSource {
    async fn search(
        &self,
        q: &SearchQuery,
        cancel: &CancelToken,
    ) -> Result<SearchAnswer, WebError> {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let Some(account) = &self.account else {
            return Err(unavailable(NO_ACCOUNT, false));
        };
        let mut body = json!({
            "query": q.query,
            "provider": q.provider.as_str(),
            "limit": q.limit,
            "options": {"gateway": {"id": q.gateway}},
        });
        if let Some(alias) = q.byok_alias.as_ref().filter(|a| !a.is_empty()) {
            body["byokAlias"] = json!(alias);
        }
        let call = async {
            let r = RestSource::client(&self.api)?
                .post(RestSource::endpoint(account, "websearch/"))
                .bearer_auth(account.token.expose())
                .json(&body)
                .timeout(API_TIMEOUT)
                .send()
                .await
                .map_err(|e| api_error("web search", &e))?;
            let status = r.status();
            let v = api_json(r, SEARCH_ANSWER_BYTES, "web search").await?;
            RestSource::api_result("web search", status, v.clone())?;
            Ok(parse_search(v))
        };
        tokio::select! {
            r = call => r,
            () = cancel.cancelled() => Err(cancelled()),
        }
    }

    async fn fetch(
        &self,
        url: &PublicUrl,
        format: Format,
        limits: &FetchLimits,
        hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
        cancel: &CancelToken,
    ) -> Result<FetchedPage, WebError> {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        // Gone out once the first request's name resolved to checked
        // addresses and the connection to them began: a deadline or a
        // cancellation before that charges nothing.
        let out = Arc::new(AtomicBool::new(false));
        let work = tokio::time::timeout(
            limits.timeout,
            self.fetch_page(url, format, limits, hop, &out),
        );
        tokio::select! {
            r = work => r.unwrap_or_else(|_| Err(WebError::new(
                CODE_WEB_TIMEOUT,
                format!(
                    "{} did not answer within {} s; try again or another source",
                    url.normalized(),
                    limits.timeout.as_secs_f64()
                ),
                true,
            )
            .went_out(out.load(Ordering::SeqCst)))),
            () = cancel.cancelled() => Err(cancelled().went_out(out.load(Ordering::SeqCst))),
        }
    }
}

/// A Cloudflare account ID and its opened token.
type Credentials = (String, Secret);

/// The daemon's [`WebSource`]: at each call, a [`RestSource`] for the
/// account ID and sealed Cloudflare token the instance holds now, or one
/// without an account when it holds neither, kept while they stay the
/// same. The token is opened for the API base it is sent to, which the
/// token's slot binds to [`CLOUDFLARE_ORIGIN`]: any other base is
/// refused, and reads as no account. It always fetches; it searches only
/// with an account ([`WebSource::configured`]), so without one a run's
/// searches are refused `web_disabled` before the source is called, and a
/// search that reaches it anyway answers `web_unavailable`, not gone out.
pub struct DaemonWeb {
    store: Arc<Store>,
    kek: Option<Kek>,
    api_base: Url,
    /// Tests only: where requests go instead of `api_base`, the token
    /// still opened for `api_base` (Cloudflare's origin).
    #[cfg(test)]
    send_to: Option<Url>,
    allow_loopback: bool,
    lookup: Arc<dyn Lookup>,
    user_agent: String,
    /// The last credentials (none: no account), and their source.
    current: Mutex<Option<(Option<Credentials>, Arc<RestSource>)>>,
}

impl DaemonWeb {
    /// The source of the instance on `store`, opening the token with `kek`.
    #[must_use]
    pub fn new(store: Arc<Store>, kek: Option<Kek>) -> DaemonWeb {
        DaemonWeb {
            store,
            kek,
            api_base: Url::parse(CLOUDFLARE_ORIGIN).expect("Cloudflare's origin is a URL"),
            #[cfg(test)]
            send_to: None,
            allow_loopback: false,
            lookup: Arc::new(SystemLookup),
            user_agent: user_agent(None),
            current: Mutex::new(None),
        }
    }

    /// Tests only: this source sending to `base` (a local server),
    /// resolving fetched names with `lookup` and admitting loopback, while
    /// the token is still opened for Cloudflare's origin alone.
    #[cfg(test)]
    #[warn(clippy::disallowed_methods)]
    fn sending_to(mut self, base: Url, lookup: Arc<dyn Lookup>) -> DaemonWeb {
        self.send_to = Some(base);
        self.lookup = lookup;
        self.allow_loopback = true;
        self
    }

    /// The base requests go to: `api_base` (in tests, `send_to`).
    fn target(&self) -> Url {
        #[cfg(test)]
        if let Some(t) = &self.send_to {
            return t.clone();
        }
        self.api_base.clone()
    }

    /// This source, sending `user_agent` on every fetch ([`user_agent`]).
    #[must_use]
    pub fn with_user_agent(mut self, user_agent: String) -> DaemonWeb {
        self.user_agent = user_agent;
        self
    }

    /// The source for the credentials held now.
    async fn source(&self) -> Result<Arc<RestSource>, WebError> {
        let (store, kek, base) = (
            self.store.clone(),
            self.kek.clone(),
            self.api_base.as_str().to_string(),
        );
        let read_error =
            |e: String| unavailable(format!("the web settings could not be read ({e})"), true);
        let creds =
            tokio::task::spawn_blocking(move || usable(credentials(&store, kek.as_ref(), &base)))
                .await
                .map_err(|e| read_error(e.to_string()))?
                .map_err(|e| read_error(e.to_string()))?;
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((held, s)) = current.as_ref() {
            let same = match (held, &creds) {
                (Some((a, t)), Some((account, token))) => {
                    a == account && t.expose() == token.expose()
                }
                (None, None) => true,
                _ => false,
            };
            if same {
                return Ok(s.clone());
            }
        }
        let source = match &creds {
            Some((account, token)) => RestSource::with_lookup(
                self.target(),
                account.clone(),
                token.clone(),
                self.allow_loopback,
                self.lookup.clone(),
            ),
            None => RestSource::without_account(self.allow_loopback, self.lookup.clone()),
        };
        let source = Arc::new(source.with_user_agent(self.user_agent.clone()));
        *current = Some((creds, source.clone()));
        Ok(source)
    }
}

/// The account ID and token `st` holds now, the token opened with `kek`
/// for `base` (the URL it is sent to); `None` without both (or without
/// `kek`).
///
/// # Errors
/// `kek_mismatch` or `key_host_mismatch` from opening the token; the
/// store's.
fn credentials(
    st: &Store,
    kek: Option<&Kek>,
    base: &str,
) -> clax_core::Result<Option<(String, Secret)>> {
    let Some(kek) = kek else {
        return Ok(None);
    };
    let account = st.bot_setting(Setting::WebCloudflareAccountId)?;
    let account = account.as_str().unwrap_or_default().trim().to_string();
    if account.is_empty() {
        return Ok(None);
    }
    let token = st.open_for_use(kek, &SecretSlot::Cloudflare, base)?;
    Ok(token.map(|t| (account, t)))
}

/// `creds` with a token that cannot be opened (sealed under another
/// key-encryption key, or bound to another origin) read as none: the admin
/// sets it again, and until then web use is not configured.
fn usable(
    creds: clax_core::Result<Option<(String, Secret)>>,
) -> clax_core::Result<Option<(String, Secret)>> {
    match creds {
        Err(clax_core::CoreError::Invalid {
            code: "kek_mismatch" | "key_host_mismatch",
            ..
        }) => Ok(None),
        other => other,
    }
}

#[async_trait::async_trait]
impl WebSource for DaemonWeb {
    async fn search(
        &self,
        q: &SearchQuery,
        cancel: &CancelToken,
    ) -> Result<SearchAnswer, WebError> {
        self.source().await?.search(q, cancel).await
    }

    async fn fetch(
        &self,
        url: &PublicUrl,
        format: Format,
        limits: &FetchLimits,
        hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
        cancel: &CancelToken,
    ) -> Result<FetchedPage, WebError> {
        self.source()
            .await?
            .fetch(url, format, limits, hop, cancel)
            .await
    }

    /// Fetch always; search when the instance holds an account ID and a
    /// token that opens now ([`has_credentials`]), a store error reading as
    /// none. It reads the store and opens the token on the calling thread
    /// (a performer's, once per web call, beside its other store reads),
    /// not on a blocking thread as [`DaemonWeb`]'s calls do: one indexed
    /// row and one decryption of a short token.
    fn configured(&self) -> WebShape {
        WebShape {
            search: has_credentials(&self.store, self.kek.as_ref()).unwrap_or(false),
            fetch: true,
        }
    }
}

/// Whether the instance holds what [`DaemonWeb`] needs: a key-encryption
/// key, a Cloudflare account ID and a sealed token that opens with that key
/// for [`CLOUDFLARE_ORIGIN`]. A token sealed under another key reads as
/// none, as [`DaemonWeb`] reads it.
///
/// # Errors
/// The store's.
pub fn has_credentials(st: &Store, kek: Option<&Kek>) -> clax_core::Result<bool> {
    Ok(usable(credentials(st, kek, CLOUDFLARE_ORIGIN))?.is_some())
}

/// The fixtures source loaded from the JSON file at `path`.
///
/// # Errors
/// The file's, or what in it is not [`FakeWebSource::from_fixtures`]'s
/// shape.
pub fn fixtures_at(path: &std::path::Path) -> Result<FakeWebSource, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    FakeWebSource::from_fixtures(&v).map_err(|e| format!("{}: {e}", path.display()))
}

/// The test mode's source: the fixtures `CLAX_TEST_WEB_FIXTURES` names,
/// loaded once per process; `None` without the variable, or when the file
/// does not load (logged).
pub fn fixtures() -> Option<Arc<FakeWebSource>> {
    static FIXTURES: OnceLock<Option<Arc<FakeWebSource>>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| {
            let path = std::env::var_os(FIXTURES_ENV).filter(|p| !p.is_empty())?;
            match fixtures_at(std::path::Path::new(&path)) {
                Ok(f) => {
                    tracing::warn!(path = ?path, "ClaxBot's web tools answer from test fixtures");
                    Some(Arc::new(f))
                }
                Err(e) => {
                    tracing::warn!(error = %e, "{FIXTURES_ENV} is not taken");
                    None
                }
            }
        })
        .clone()
}

/// The daemon's web source: the test mode's fixtures when they are set
/// ([`fixtures`]), else a [`DaemonWeb`] on `store` whose fetches name a
/// hosted daemon's public `origin` ([`user_agent`]; `None` on a local
/// daemon). A hosted daemon's source answers its origin's host and its
/// `site_domain` as its own hosts, which no fetch reaches
/// ([`WithOwnHosts`]); a local daemon's hosts are loopback, which every
/// fetch refuses already. Both are taken here, once: hosted mode reads
/// them at start (`[hosted] public_origin`, `site_domain`) and they do not
/// change before a restart.
///
/// Config loading has already refused an origin or site domain that is not
/// one; were one to reach here, the daemon could not name its own hosts, so
/// [`daemon_web`] gives [`NoHostsWeb`] and the reason.
///
/// # Errors
/// Why the origin or site domain names no host.
pub fn for_daemon(
    store: Arc<Store>,
    kek: Option<Kek>,
    origin: Option<&str>,
    site_domain: Option<&str>,
) -> Result<Arc<dyn WebSource>, String> {
    let own = match origin {
        Some(o) => Some(OwnHosts::new(Some(o), site_domain).map_err(|e| e.to_string())?),
        None => None,
    };
    let source: Arc<dyn WebSource> = match fixtures() {
        Some(f) => f,
        None => Arc::new(DaemonWeb::new(store, kek).with_user_agent(user_agent(origin))),
    };
    Ok(match own {
        Some(own) => Arc::new(WithOwnHosts::new(source, own)),
        None => source,
    })
}

/// [`for_daemon`]'s source and the reason web use is off. When the hosts
/// cannot be named the reason is logged and the source is [`NoHostsWeb`],
/// which is not configured and refuses every call; the reason goes to
/// `clax_http::bot::Bot::web_check`, which the admin Web view shows.
#[must_use]
pub fn daemon_web(
    store: Arc<Store>,
    kek: Option<Kek>,
    origin: Option<&str>,
    site_domain: Option<&str>,
) -> (Arc<dyn WebSource>, Option<String>) {
    match for_daemon(store, kek, origin, site_domain) {
        Ok(web) => (web, None),
        Err(why) => {
            tracing::error!(%why, "ClaxBot's web tools are off");
            (Arc::new(NoHostsWeb(why.clone())), Some(why))
        }
    }
}

/// A source that is not configured and answers every call
/// `web_unavailable`, with the reason the daemon's hosts could not be
/// named ([`daemon_web`]).
struct NoHostsWeb(String);

#[async_trait::async_trait]
impl WebSource for NoHostsWeb {
    async fn search(
        &self,
        _q: &SearchQuery,
        _cancel: &CancelToken,
    ) -> Result<SearchAnswer, WebError> {
        Err(unavailable(self.0.clone(), false))
    }

    async fn fetch(
        &self,
        _url: &PublicUrl,
        _format: Format,
        _limits: &FetchLimits,
        _hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
        _cancel: &CancelToken,
    ) -> Result<FetchedPage, WebError> {
        Err(unavailable(self.0.clone(), false))
    }

    fn configured(&self) -> WebShape {
        WebShape::default()
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use clax_bot::web::{OP_FETCH, Provider, charged};
    use clax_core::Home;
    use clax_core::audit::AuditCtx;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
    const TOKEN: &str = "cf-test-token-1234";

    /// A request the scripted server read.
    #[derive(Clone, Debug)]
    struct Req {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    impl Req {
        fn header(&self, name: &str) -> &str {
            self.headers.get(name).map_or("", String::as_str)
        }
    }

    /// An answer: status, headers, body. `None` from a handler never
    /// answers.
    struct Resp {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    }

    fn resp(status: u16, ct: &str, body: impl Into<Vec<u8>>) -> Option<Resp> {
        Some(Resp {
            status,
            headers: vec![("content-type", ct.to_string())],
            body: body.into(),
        })
    }

    fn redirect(to: &str) -> Option<Resp> {
        Some(Resp {
            status: 302,
            headers: vec![("location", to.to_string())],
            body: Vec::new(),
        })
    }

    type Handler = dyn Fn(&Req) -> Option<Resp> + Send + Sync;

    /// A scripted HTTP/1.1 server on 127.0.0.1, one request a connection.
    struct Server {
        port: u16,
        seen: Arc<Mutex<Vec<Req>>>,
    }

    impl Server {
        async fn start(handler: impl Fn(&Req) -> Option<Resp> + Send + Sync + 'static) -> Server {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            let seen: Arc<Mutex<Vec<Req>>> = Arc::default();
            let handler: Arc<Handler> = Arc::new(handler);
            let s = seen.clone();
            tokio::spawn(async move {
                while let Ok((sock, _)) = l.accept().await {
                    let (h, s) = (handler.clone(), s.clone());
                    tokio::spawn(async move { serve_one(sock, &*h, &s).await });
                }
            });
            Server { port, seen }
        }

        fn requests(&self) -> Vec<Req> {
            self.seen.lock().unwrap().clone()
        }

        fn api_base(&self) -> Url {
            Url::parse(&format!("http://127.0.0.1:{}", self.port)).unwrap()
        }

        /// `http://<host>:<port><path>`.
        fn url(&self, host: &str, path: &str) -> PublicUrl {
            PublicUrl::parse(&format!("http://{host}:{}{path}", self.port)).unwrap()
        }
    }

    async fn serve_one(mut sock: tokio::net::TcpStream, h: &Handler, seen: &Mutex<Vec<Req>>) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            match sock.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let mut lines = head.split("\r\n");
        let mut first = lines.next().unwrap_or("").split(' ');
        let method = first.next().unwrap_or("").to_string();
        let path = first.next().unwrap_or("").to_string();
        let headers: HashMap<String, String> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        let mut body = buf[head_end..].to_vec();
        if let Some(n) = headers
            .get("content-length")
            .and_then(|n| n.parse::<usize>().ok())
        {
            while body.len() < n {
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(k) => body.extend_from_slice(&chunk[..k]),
                }
            }
        } else if headers
            .get("transfer-encoding")
            .is_some_and(|t| t.eq_ignore_ascii_case("chunked"))
        {
            while !body.windows(5).any(|w| w == b"0\r\n\r\n") {
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(k) => body.extend_from_slice(&chunk[..k]),
                }
            }
            body = dechunk(&body);
        }
        let req = Req {
            method,
            path,
            headers,
            body,
        };
        seen.lock().unwrap().push(req.clone());
        let Some(r) = h(&req) else {
            // Never answer, holding the connection open.
            std::future::pending::<()>().await;
            return;
        };
        let mut out = format!("HTTP/1.1 {} X\r\n", r.status);
        for (k, v) in &r.headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str(&format!(
            "content-length: {}\r\nconnection: close\r\n\r\n",
            r.body.len()
        ));
        let _ = sock.write_all(out.as_bytes()).await;
        let _ = sock.write_all(&r.body).await;
        let _ = sock.shutdown().await;
    }

    fn dechunk(mut b: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let Some(i) = b.windows(2).position(|w| w == b"\r\n") else {
                return out;
            };
            let n =
                usize::from_str_radix(std::str::from_utf8(&b[..i]).unwrap().trim(), 16).unwrap();
            if n == 0 {
                return out;
            }
            out.extend_from_slice(&b[i + 2..i + 2 + n]);
            b = &b[i + 4 + n..];
        }
    }

    /// Names to addresses, for tests.
    struct Stub(HashMap<&'static str, Vec<IpAddr>>);

    impl Lookup for Stub {
        fn lookup(&self, host: &str) -> LookupFuture {
            let r = self.0.get(host).cloned().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, format!("no such host {host}"))
            });
            Box::pin(async move { r })
        }
    }

    fn stub() -> Arc<dyn Lookup> {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        Arc::new(Stub(HashMap::from([
            ("site.test", vec![ip("127.0.0.1")]),
            ("other.test", vec![ip("127.0.0.1")]),
            ("evil.test", vec![ip("10.0.0.1")]),
            ("mixed.test", vec![ip("127.0.0.1"), ip("192.168.1.1")]),
        ])))
    }

    fn source(server: &Server, allow_loopback: bool) -> RestSource {
        RestSource::with_lookup(
            server.api_base(),
            ACCOUNT.into(),
            Secret::new(TOKEN.into()),
            allow_loopback,
            stub(),
        )
    }

    fn any(_: &PublicUrl) -> Result<(), WebError> {
        Ok(())
    }

    async fn fetch(
        s: &RestSource,
        url: &PublicUrl,
        format: Format,
        limits: FetchLimits,
    ) -> Result<FetchedPage, WebError> {
        s.fetch(url, format, &limits, &any, &CancelToken::new())
            .await
    }

    fn query() -> SearchQuery {
        SearchQuery {
            query: "rust web".into(),
            limit: 3,
            provider: Provider::Exa,
            gateway: "gw".into(),
            byok_alias: Some("mine".into()),
        }
    }

    #[test]
    fn is_public_refuses_private_ranges() {
        for local in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.0.0.8",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:10.0.0.1",
            "64:ff9b::7f00:1",
            "2002:c0a8:101::1",
            "fe80::1",
            "fd00::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(!is_public(local.parse().unwrap()), "{local}");
        }
        for public in ["1.1.1.1", "8.8.8.8", "2606:4700::1111", "::ffff:1.1.1.1"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[tokio::test]
    async fn a_name_resolving_privately_is_refused() {
        let server = Server::start(|_| resp(200, "text/plain", "hi")).await;
        let s = source(&server, true);
        for host in ["evil.test", "mixed.test"] {
            let e = fetch(
                &s,
                &server.url(host, "/"),
                Format::Auto,
                FetchLimits::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(e.code, CODE_URL_NOT_ALLOWED, "{host}: {e}");
            assert!(!e.went_out, "{host}: nothing was sent");
            assert!(!e.retryable);
        }
        // Loopback, too, unless the test admits it.
        let strict = source(&server, false);
        let e = fetch(
            &strict,
            &server.url("site.test", "/"),
            Format::Auto,
            FetchLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_URL_NOT_ALLOWED, false), "{e}");
        assert!(
            server.requests().is_empty(),
            "no connection reached the server"
        );
        // An unknown name is not a refusal.
        let e = fetch(
            &s,
            &server.url("nowhere.test", "/"),
            Format::Auto,
            FetchLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_UNAVAILABLE, false), "{e}");
    }

    #[tokio::test]
    async fn a_redirect_to_a_private_name_is_refused_and_went_out() {
        let port = Arc::new(OnceLock::<u16>::new());
        let p = port.clone();
        let server = Server::start(move |r| match r.path.as_str() {
            "/go" => redirect(&format!("http://evil.test:{}/x", p.get().unwrap())),
            "/local" => redirect("http://127.0.0.1/admin"),
            "/nowhere" => redirect("http://[::1"),
            "/down" => redirect("http://site.test/"),
            _ => resp(200, "text/plain", "x"),
        })
        .await;
        port.set(server.port).unwrap();
        let s = source(&server, true);
        let e = fetch(
            &s,
            &server.url("site.test", "/go"),
            Format::Auto,
            FetchLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_URL_NOT_ALLOWED, true), "{e}");
        assert!(charged(OP_FETCH, &e));
        let e = fetch(
            &s,
            &server.url("site.test", "/local"),
            Format::Auto,
            FetchLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_URL_NOT_ALLOWED, true), "{e}");
        let e = fetch(
            &s,
            &server.url("site.test", "/nowhere"),
            Format::Auto,
            FetchLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_UNAVAILABLE, true), "{e}");
        // `hop` is called on the target, and its refusal answered.
        let refuse = |_: &PublicUrl| {
            Err(WebError::new(
                CODE_URL_NOT_ALLOWED,
                "not on the list",
                false,
            ))
        };
        let e = s
            .fetch(
                &server.url("site.test", "/down"),
                Format::Auto,
                &FetchLimits::default(),
                &refuse,
                &CancelToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_URL_NOT_ALLOWED, true), "{e}");
        assert_eq!(e.message, "not on the list");
    }

    /// A resolver whose lookups never answer.
    struct Silent;

    impl Lookup for Silent {
        fn lookup(&self, _: &str) -> LookupFuture {
            Box::pin(std::future::pending())
        }
    }

    #[test]
    fn each_fetch_client_takes_the_sources_tls_configuration() {
        let s = RestSource::with_lookup(
            Url::parse("http://127.0.0.1:9").unwrap(),
            ACCOUNT.into(),
            Secret::new(TOKEN.into()),
            false,
            stub(),
        );
        let tls = s.tls.as_ref().unwrap();
        assert!(
            !webpki_roots::TLS_SERVER_ROOTS.is_empty(),
            "the roots it trusts"
        );
        assert!(!tls.crypto_provider().cipher_suites.is_empty());
        assert_eq!(tls.alpn_protocols, [b"http/1.1".to_vec()]);
        // reqwest takes it as its own rustls configuration: a client it
        // did not recognise would not build.
        s.web_client(Arc::default()).unwrap();
        s.web_client(Arc::default()).unwrap();
    }

    #[test]
    fn only_a_host_name_waits_for_its_resolution_to_go_out() {
        for (url, at_once) in [
            ("http://93.184.215.14/", true),
            ("http://[2606:2800:21f:cb07:6820:80da:af6b:8b2e]/", true),
            ("http://site.test/", false),
            ("https://example.com:8443/x", false),
        ] {
            let u = PublicUrl::parse(url).unwrap();
            assert_eq!(goes_out_on_send(&u), at_once, "{url}");
        }
    }

    // Paused: the deadline passes on the runtime's clock, with no waiting.
    #[tokio::test(start_paused = true)]
    async fn a_deadline_or_cancel_before_the_first_name_resolved_did_not_go_out() {
        let s = RestSource::with_lookup(
            Url::parse("http://127.0.0.1:9").unwrap(),
            ACCOUNT.into(),
            Secret::new(TOKEN.into()),
            true,
            Arc::new(Silent),
        );
        let url = PublicUrl::parse("http://site.test/").unwrap();
        let limits = FetchLimits {
            timeout: Duration::from_secs(30),
            ..FetchLimits::default()
        };
        let e = fetch(&s, &url, Format::Auto, limits).await.unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_TIMEOUT, false), "{e}");
        assert!(!charged(OP_FETCH, &e));
        // Cancelled while the name resolves: not gone out either.
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let limits = FetchLimits::default();
        let call = s.fetch(&url, Format::Auto, &limits, &any, &cancel);
        let (r, ()) = tokio::join!(call, async move {
            tokio::task::yield_now().await;
            c.cancel();
        });
        let e = r.unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_CANCELLED, false), "{e}");
        assert!(!charged(OP_FETCH, &e));
    }

    #[tokio::test]
    async fn a_refused_first_connection_did_not_go_out_and_a_later_one_did() {
        // Port 0: no connection to it can be made (refused, or the address
        // cannot be assigned), whatever else listens on the host.
        let closed = 0;
        let server = Server::start(move |r| match r.path.as_str() {
            "/away" => redirect(&format!("http://site.test:{closed}/")),
            "/lost" => redirect("http://nowhere.test/"),
            _ => resp(200, "text/plain", "x"),
        })
        .await;
        let s = source(&server, true);
        let refused = PublicUrl::parse(&format!("http://site.test:{closed}/")).unwrap();
        let e = fetch(&s, &refused, Format::Auto, FetchLimits::default())
            .await
            .unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_UNAVAILABLE, false), "{e}");
        assert!(!charged(OP_FETCH, &e));
        // After a redirect came back, a refused connection or a name that
        // does not resolve has gone out.
        for path in ["/away", "/lost"] {
            let e = fetch(
                &s,
                &server.url("site.test", path),
                Format::Auto,
                FetchLimits::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(
                (e.code, e.went_out),
                (CODE_WEB_UNAVAILABLE, true),
                "{path}: {e}"
            );
            assert!(charged(OP_FETCH, &e));
        }
    }

    #[tokio::test]
    async fn search_posts_the_gateway_and_reads_items() {
        let server = Server::start(|r| {
            (r.path == format!("/client/v4/accounts/{ACCOUNT}/ai/websearch/")).then(|| Resp {
                status: 200,
                headers: vec![("content-type", "application/json".into())],
                body: json!({
                    "success": true,
                    "result": {
                        "items": [
                            {"url": "https://a.example/", "title": "A", "description": "First",
                             "lastModifiedDate": "2026-10-01"},
                            {"url": "https://b.example/", "age": "2d"},
                            {"title": "no url"}
                        ],
                        "metadata": {"requestId": "req-1"}
                    }
                })
                .to_string()
                .into_bytes(),
            })
        })
        .await;
        let s = source(&server, false);
        let a = s.search(&query(), &CancelToken::new()).await.unwrap();
        assert_eq!(
            a.hits,
            vec![
                SearchHit {
                    url: "https://a.example/".into(),
                    title: Some("A".into()),
                    description: Some("First".into()),
                    age: Some("2026-10-01".into()),
                },
                SearchHit {
                    url: "https://b.example/".into(),
                    title: None,
                    description: None,
                    age: Some("2d".into()),
                },
            ]
        );
        assert_eq!(a.request_id.as_deref(), Some("req-1"));
        assert_eq!(a.raw["result"]["metadata"]["requestId"], "req-1");
        let reqs = server.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(reqs[0].header("authorization"), format!("Bearer {TOKEN}"));
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(
            body,
            json!({
                "query": "rust web",
                "provider": "exa",
                "limit": 3,
                "byokAlias": "mine",
                "options": {"gateway": {"id": "gw"}}
            })
        );
        // Without an alias, none is sent.
        let mut q = query();
        q.byok_alias = None;
        s.search(&q, &CancelToken::new()).await.unwrap();
        let body: Value = serde_json::from_slice(&server.requests()[1].body).unwrap();
        assert!(body.get("byokAlias").is_none(), "{body}");
    }

    #[tokio::test]
    async fn a_failed_search_is_unavailable() {
        let server = Server::start(|r| match r.header("authorization") {
            "Bearer busy" => resp(503, "application/json", r#"{"success":false,"errors":[]}"#),
            _ => resp(
                401,
                "application/json",
                r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}]}"#,
            ),
        })
        .await;
        let s = source(&server, false);
        let e = s.search(&query(), &CancelToken::new()).await.unwrap_err();
        assert_eq!((e.code, e.retryable), (CODE_WEB_UNAVAILABLE, false), "{e}");
        assert!(e.message.contains("Authentication error"), "{e}");
        assert!(!e.message.contains(TOKEN), "{e}");
        let busy = RestSource::with_lookup(
            server.api_base(),
            ACCOUNT.into(),
            Secret::new("busy".into()),
            false,
            stub(),
        );
        let e = busy
            .search(&query(), &CancelToken::new())
            .await
            .unwrap_err();
        assert_eq!((e.code, e.retryable), (CODE_WEB_UNAVAILABLE, true), "{e}");
    }

    #[tokio::test]
    async fn fetch_converts_html_itself() {
        let server = Server::start(|r| match r.path.as_str() {
            "/page" => {
                assert_eq!(r.header("accept"), ACCEPT);
                resp(
                    200,
                    "text/html; charset=utf-8",
                    "<html><body><h1>Hi</h1><a href=\"/next\">next</a></body></html>",
                )
            }
            "/latin" => resp(
                200,
                "text/html; charset=iso-8859-1",
                b"<p>caf\xe9</p>".to_vec(),
            ),
            "/meta" => resp(
                200,
                "text/html",
                b"<meta charset=\"iso-8859-1\"><p>caf\xe9</p>".to_vec(),
            ),
            _ => resp(404, "text/plain", "no"),
        })
        .await;
        // With an account and without one alike, and nothing goes to
        // Cloudflare.
        for s in [
            source(&server, true),
            RestSource::without_account(true, stub()),
        ] {
            let url = server.url("site.test", "/page");
            let p = fetch(&s, &url, Format::Auto, FetchLimits::default())
                .await
                .unwrap();
            let next = format!("http://site.test:{}/next", server.port);
            assert_eq!(p.text, format!("# Hi\n\n[next]({next})"));
            assert_eq!(p.status, 200);
            assert_eq!(p.content_type, "text/html");
            assert_eq!(p.final_url, url.normalized());
            assert_eq!(p.links, vec![next]);
            // Raw returns the HTML itself.
            let p = fetch(&s, &url, Format::Raw, FetchLimits::default())
                .await
                .unwrap();
            assert!(p.text.contains("<h1>Hi</h1>"), "{}", p.text);
            // A page in another charset is converted from its text.
            let latin = server.url("site.test", "/latin");
            let p = fetch(&s, &latin, Format::Auto, FetchLimits::default())
                .await
                .unwrap();
            assert_eq!(p.text, "caf\u{e9}");
            // So is a page whose charset only its meta declares, and its
            // raw HTML is decoded by it too.
            let meta = server.url("site.test", "/meta");
            let p = fetch(&s, &meta, Format::Auto, FetchLimits::default())
                .await
                .unwrap();
            assert_eq!(p.text, "caf\u{e9}");
            let p = fetch(&s, &meta, Format::Raw, FetchLimits::default())
                .await
                .unwrap();
            assert_eq!(p.text, "<meta charset=\"iso-8859-1\"><p>caf\u{e9}</p>");
        }
        assert!(
            server
                .requests()
                .iter()
                .all(|r| !r.path.starts_with("/client/")),
            "nothing is sent to Cloudflare's API"
        );
    }

    #[tokio::test]
    async fn a_document_is_converted_through_the_account_only() {
        let server = Server::start(|r| match r.path.as_str() {
            "/doc.pdf" => resp(200, "application/pdf", b"%PDF-1.4 doc".to_vec()),
            "/sniffed" => resp(200, "", b"%PDF-1.4 doc".to_vec()),
            p if p == format!("/client/v4/accounts/{ACCOUNT}/ai/tomarkdown") => {
                assert_eq!(r.header("authorization"), format!("Bearer {TOKEN}"));
                let body = String::from_utf8_lossy(&r.body);
                assert!(body.contains("filename=\"document.pdf\""), "{body}");
                resp(
                    200,
                    "application/json",
                    json!({"success": true, "result": [
                        {"name": "document.pdf", "mimeType": "application/pdf",
                         "format": "markdown", "data": "# Doc\n\nSee https://x.example/."}
                    ]})
                    .to_string(),
                )
            }
            _ => resp(404, "text/plain", "no"),
        })
        .await;
        let url = server.url("site.test", "/doc.pdf");
        let p = fetch(
            &source(&server, true),
            &url,
            Format::Auto,
            FetchLimits::default(),
        )
        .await
        .unwrap();
        assert_eq!(p.text, "# Doc\n\nSee https://x.example/.");
        assert_eq!(p.links, vec!["https://x.example/".to_string()]);
        // Without an account: fetched, then refused, before the body is read
        // when the type is declared and after it when it is sniffed.
        let local = RestSource::without_account(true, stub());
        let before = server.requests().len();
        for path in ["/doc.pdf", "/sniffed"] {
            let e = fetch(
                &local,
                &server.url("site.test", path),
                Format::Auto,
                FetchLimits::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(
                (e.code, e.went_out, e.retryable),
                (CODE_WEB_UNSUPPORTED_CONTENT_TYPE, true, false),
                "{path}: {e}"
            );
            assert!(e.message.contains("only with a Cloudflare account"), "{e}");
        }
        assert_eq!(server.requests().len(), before + 2, "no conversion request");
    }

    #[tokio::test]
    async fn a_search_without_an_account_sends_nothing() {
        let server = Server::start(|_| resp(200, "application/json", "{}")).await;
        let local = RestSource::without_account(true, stub());
        let e = local
            .search(&query(), &CancelToken::new())
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.went_out, e.retryable),
            (CODE_WEB_UNAVAILABLE, false, false),
            "{e}"
        );
        assert!(e.message.contains("Cloudflare account ID"), "{e}");
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn fetch_handles_each_type() {
        let server = Server::start(|r| match r.path.as_str() {
            "/json" => resp(200, "application/json", r#"{"a":[1,2]}"#),
            "/md" => resp(200, "text/markdown", "see [x](https://x.example/)"),
            "/latin" => resp(200, "text/plain; charset=iso-8859-1", b"caf\xe9".to_vec()),
            "/img" => resp(200, "image/png", vec![0x89, b'P', b'N', b'G']),
            "/pdf" => resp(200, "application/pdf", b"%PDF-1.4".to_vec()),
            "/bin" => resp(200, "application/octet-stream", vec![0, 1, 2]),
            "/missing" => resp(404, "text/plain", "not here"),
            "/challenge" => Some(Resp {
                status: 403,
                headers: vec![
                    ("content-type", "text/html".into()),
                    ("cf-mitigated", "challenge".into()),
                ],
                body: b"<html>challenge</html>".to_vec(),
            }),
            "/big" => resp(200, "text/plain", vec![b'a'; 2_000]),
            _ => resp(500, "text/plain", "?"),
        })
        .await;
        let s = source(&server, true);
        let get = |path: &str, format: Format| {
            let url = server.url("site.test", path);
            let s = &s;
            async move { fetch(s, &url, format, FetchLimits::default()).await }
        };
        assert_eq!(
            get("/json", Format::Auto).await.unwrap().text,
            "{\n  \"a\": [\n    1,\n    2\n  ]\n}"
        );
        assert_eq!(
            get("/json", Format::Raw).await.unwrap().text,
            r#"{"a":[1,2]}"#
        );
        let md = get("/md", Format::Auto).await.unwrap();
        assert_eq!(md.links, vec!["https://x.example/".to_string()]);
        assert_eq!(get("/latin", Format::Auto).await.unwrap().text, "café");
        for (path, format) in [
            ("/img", Format::Auto),
            ("/img", Format::Raw),
            ("/pdf", Format::Raw),
            ("/bin", Format::Auto),
        ] {
            let e = get(path, format).await.unwrap_err();
            assert_eq!(
                (e.code, e.went_out),
                (CODE_WEB_UNSUPPORTED_CONTENT_TYPE, true),
                "{path}: {e}"
            );
        }
        let p = get("/missing", Format::Auto).await.unwrap();
        assert_eq!((p.status, p.text.as_str()), (404, "not here"));
        let e = get("/challenge", Format::Auto).await.unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_BLOCKED, true), "{e}");
        let small = FetchLimits {
            max_bytes: 1_000,
            ..FetchLimits::default()
        };
        let e = fetch(&s, &server.url("site.test", "/big"), Format::Auto, small)
            .await
            .unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_TOO_LARGE, true), "{e}");
    }

    #[tokio::test]
    async fn an_error_status_without_a_readable_body_is_unavailable() {
        let server = Server::start(|r| {
            let (status, rest) = r.path[1..].split_once('/').unwrap();
            let status: u16 = status.parse().unwrap();
            match rest {
                "empty" => resp(status, "text/plain", Vec::new()),
                "binary" => resp(status, "application/octet-stream", vec![0, 1, 2, 0]),
                "image" => resp(status, "image/png", vec![0x89, b'P', b'N', b'G']),
                "pdf" => resp(status, "application/pdf", b"%PDF-1.4".to_vec()),
                "blank-html" => resp(status, "text/html", "<html><body></body></html>"),
                "html" => resp(status, "text/html", "<html><body><p>gone</p></body></html>"),
                _ => resp(status, "text/plain", "gone"),
            }
        })
        .await;
        let s = source(&server, true);
        for (status, retryable) in [(404, false), (500, true)] {
            // Empty and sniffed binary bodies after the download, an image
            // and a raw document before it.
            for (body, format) in [
                ("empty", Format::Auto),
                ("binary", Format::Auto),
                ("image", Format::Auto),
                ("pdf", Format::Raw),
                // An HTML error page whose conversion is empty, after it.
                ("blank-html", Format::Auto),
            ] {
                let url = server.url("site.test", &format!("/{status}/{body}"));
                let e = fetch(&s, &url, format, FetchLimits::default())
                    .await
                    .unwrap_err();
                assert_eq!(
                    (e.code, e.went_out, e.retryable),
                    (CODE_WEB_UNAVAILABLE, true, retryable),
                    "{status} {body}: {e}"
                );
                assert!(
                    e.message
                        .contains(&format!("answered HTTP {status} without a readable body")),
                    "{e}"
                );
            }
            // A readable error page is a page with its status.
            let url = server.url("site.test", &format!("/{status}/text"));
            let p = fetch(&s, &url, Format::Auto, FetchLimits::default())
                .await
                .unwrap();
            assert_eq!((p.status, p.text.as_str()), (status, "gone"));
            let url = server.url("site.test", &format!("/{status}/html"));
            let p = fetch(&s, &url, Format::Auto, FetchLimits::default())
                .await
                .unwrap();
            assert_eq!((p.status, p.text.as_str()), (status, "gone"));
        }
        // A 200 image is still unsupported, not unavailable.
        let e = fetch(
            &s,
            &server.url("site.test", "/200/image"),
            Format::Auto,
            FetchLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, CODE_WEB_UNSUPPORTED_CONTENT_TYPE, "{e}");
    }

    #[tokio::test]
    async fn a_redirect_loop_ends_after_five() {
        let port = Arc::new(OnceLock::<u16>::new());
        let p = port.clone();
        let server = Server::start(move |r| {
            let port = p.get().unwrap();
            match r.path.as_str() {
                "/a" => redirect(&format!("http://other.test:{port}/b")),
                "/b" => redirect("/a"),
                _ => resp(200, "text/plain", "x"),
            }
        })
        .await;
        port.set(server.port).unwrap();
        let s = source(&server, true);
        let hops = Mutex::new(Vec::new());
        let record = |u: &PublicUrl| {
            hops.lock().unwrap().push(u.normalized().to_string());
            Ok(())
        };
        let e = s
            .fetch(
                &server.url("site.test", "/a"),
                Format::Auto,
                &FetchLimits::default(),
                &record,
                &CancelToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_UNAVAILABLE, true), "{e}");
        assert!(charged(OP_FETCH, &e), "one fetch, charged once");
        // The first request and the five redirects it followed.
        assert_eq!(server.requests().len(), 6);
        assert_eq!(hops.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn a_page_that_never_answers_times_out() {
        let arrived = Arc::new(tokio::sync::Notify::new());
        let a = arrived.clone();
        let server = Server::start(move |_| {
            a.notify_one();
            None
        })
        .await;
        let s = source(&server, true);
        let deadline = Duration::from_secs(3600);
        let limits = FetchLimits {
            timeout: deadline,
            ..FetchLimits::default()
        };
        // The deadline passes on the paused clock once the request has
        // reached the page, so the fetch has gone out by then.
        let url = server.url("site.test", "/");
        let (r, ()) = tokio::join!(fetch(&s, &url, Format::Auto, limits), async {
            arrived.notified().await;
            tokio::time::pause();
            tokio::time::advance(deadline).await;
        });
        let e = r.unwrap_err();
        assert_eq!(
            (e.code, e.went_out, e.retryable),
            (CODE_WEB_TIMEOUT, true, true),
            "{e}"
        );
        // Cancelled, it ends cancelled.
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let limits = FetchLimits::default();
        let call = s.fetch(&url, Format::Auto, &limits, &any, &cancel);
        let (r, ()) = tokio::join!(call, async move { c.cancel() });
        assert_eq!(r.unwrap_err().code, CODE_CANCELLED);
    }

    fn store() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, Arc::new(st))
    }

    #[test]
    fn web_is_not_configured_without_a_token() {
        let (_d, st) = store();
        let kek = Kek::generate();
        assert!(!has_credentials(&st, Some(&kek)).unwrap());
        st.set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::WebCloudflareAccountId, Some(json!(ACCOUNT)))],
        )
        .unwrap();
        assert!(!has_credentials(&st, Some(&kek)).unwrap(), "no token");
        st.set_secret(&AuditCtx::DAEMON, &kek, &SecretSlot::Cloudflare, TOKEN)
            .unwrap();
        assert!(has_credentials(&st, Some(&kek)).unwrap());
        assert!(
            !has_credentials(&st, None).unwrap(),
            "no key-encryption key"
        );
        st.set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::WebCloudflareAccountId, None)],
        )
        .unwrap();
        assert!(!has_credentials(&st, Some(&kek)).unwrap(), "no account ID");
    }

    #[tokio::test]
    async fn the_daemon_source_follows_the_stored_credentials() {
        let server = Server::start(|_| resp(500, "text/plain", "never asked")).await;
        let (_d, st) = store();
        let kek = Kek::generate();
        let web = DaemonWeb::new(st.clone(), Some(kek.clone()));
        let fetch_only = WebShape {
            search: false,
            fetch: true,
        };
        assert_eq!(web.configured(), fetch_only);
        let e = web.search(&query(), &CancelToken::new()).await.unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_UNAVAILABLE, false), "{e}");
        assert!(e.message.contains("not configured"), "{e}");
        st.set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::WebCloudflareAccountId, Some(json!(ACCOUNT)))],
        )
        .unwrap();
        st.set_secret(&AuditCtx::DAEMON, &kek, &SecretSlot::Cloudflare, TOKEN)
            .unwrap();
        assert_eq!(web.configured(), WebShape::BOTH);
        // The token opens only for the base it is sent to, which its slot
        // binds to Cloudflare's origin: another base is not configured,
        // and nothing is sent.
        let mut elsewhere = DaemonWeb::new(st.clone(), Some(kek.clone()));
        elsewhere.api_base = server.api_base();
        let e = elsewhere
            .search(&query(), &CancelToken::new())
            .await
            .unwrap_err();
        assert!(e.message.contains("not configured"), "{e}");
        assert!(server.requests().is_empty());
        // A token sealed under another key-encryption key is none.
        let other = DaemonWeb::new(st.clone(), Some(Kek::generate()));
        assert_eq!(other.configured(), fetch_only);
        assert!(!has_credentials(&st, Some(&Kek::generate())).unwrap());
        st.clear_secret(&AuditCtx::DAEMON, &SecretSlot::Cloudflare)
            .unwrap();
        assert_eq!(web.configured(), fetch_only);
    }

    #[tokio::test]
    async fn the_daemon_source_fetches_without_an_account() {
        let server = Server::start(|r| match r.path.as_str() {
            "/page" => resp(200, "text/html", "<h1>Local</h1>"),
            _ => resp(500, "text/plain", "never asked"),
        })
        .await;
        let (_d, st) = store();
        let web = DaemonWeb::new(st, None).sending_to(server.api_base(), stub());
        let url = server.url("site.test", "/page");
        let p = web
            .fetch(
                &url,
                Format::Auto,
                &FetchLimits::default(),
                &any,
                &CancelToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(p.text, "# Local");
        assert_eq!(
            server
                .requests()
                .iter()
                .map(|r| r.path.clone())
                .collect::<Vec<_>>(),
            ["/page"]
        );
    }

    #[tokio::test]
    async fn a_fetch_names_the_daemon() {
        let server =
            Server::start(|r| resp(200, "text/plain", r.header("user-agent").to_string())).await;
        let ua = |origin: Option<&str>| {
            let s = source(&server, true).with_user_agent(user_agent(origin));
            let url = server.url("site.test", "/");
            async move {
                fetch(&s, &url, Format::Auto, FetchLimits::default())
                    .await
                    .unwrap()
                    .text
            }
        };
        // A hosted daemon names its public origin.
        assert_eq!(
            ua(Some("https://clax.example/")).await,
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ClaxBot/1.0; +https://clax.example)"
        );
        // A local daemon names no address of its own.
        assert_eq!(ua(None).await, user_agent(None));
        assert_eq!(ua(Some("")).await, user_agent(None));
    }

    #[tokio::test]
    async fn the_daemon_source_sends_the_token_held_now() {
        let server = Server::start(|r| match r.path.as_str() {
            "/page" => resp(200, "text/plain", r.header("user-agent").to_string()),
            _ => resp(
                200,
                "application/json",
                json!({"success": true, "result": {"items": [{"url": "https://a.example/"}]}})
                    .to_string(),
            ),
        })
        .await;
        let (_d, st) = store();
        let kek = Kek::generate();
        let web = DaemonWeb::new(st.clone(), Some(kek.clone()))
            .with_user_agent(user_agent(Some("https://clax.example")))
            .sending_to(server.api_base(), stub());
        st.set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::WebCloudflareAccountId, Some(json!(ACCOUNT)))],
        )
        .unwrap();
        st.set_secret(&AuditCtx::DAEMON, &kek, &SecretSlot::Cloudflare, TOKEN)
            .unwrap();
        let bearers = || -> Vec<String> {
            server
                .requests()
                .iter()
                .filter(|r| r.path.ends_with("/ai/websearch/"))
                .map(|r| r.header("authorization").to_string())
                .collect()
        };
        assert_eq!(
            web.search(&query(), &CancelToken::new())
                .await
                .unwrap()
                .hits
                .len(),
            1
        );
        assert_eq!(
            server.requests()[0].path,
            format!("/client/v4/accounts/{ACCOUNT}/ai/websearch/")
        );
        // The same credentials reuse the source; a rotated token never
        // goes out as the old one.
        web.search(&query(), &CancelToken::new()).await.unwrap();
        st.set_secret(
            &AuditCtx::DAEMON,
            &kek,
            &SecretSlot::Cloudflare,
            "cf-rotated-5678",
        )
        .unwrap();
        web.search(&query(), &CancelToken::new()).await.unwrap();
        assert_eq!(
            bearers(),
            [
                format!("Bearer {TOKEN}"),
                format!("Bearer {TOKEN}"),
                "Bearer cf-rotated-5678".to_string()
            ]
        );
        // Cleared, then set again: the new one, not the cached one.
        st.clear_secret(&AuditCtx::DAEMON, &SecretSlot::Cloudflare)
            .unwrap();
        let e = web.search(&query(), &CancelToken::new()).await.unwrap_err();
        assert!(e.message.contains("not configured"), "{e}");
        st.set_secret(
            &AuditCtx::DAEMON,
            &kek,
            &SecretSlot::Cloudflare,
            "cf-third-9012",
        )
        .unwrap();
        web.search(&query(), &CancelToken::new()).await.unwrap();
        assert_eq!(bearers().last().unwrap(), "Bearer cf-third-9012");
        // Its fetches carry the daemon's User-Agent.
        let p = web
            .fetch(
                &server.url("site.test", "/page"),
                Format::Auto,
                &FetchLimits::default(),
                &any,
                &CancelToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            p.text,
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ClaxBot/1.0; +https://clax.example)"
        );
    }

    #[tokio::test]
    async fn a_huge_cloudflare_answer_is_refused() {
        let server = Server::start(|_| {
            resp(
                200,
                "application/json",
                vec![b' '; usize::try_from(SEARCH_ANSWER_BYTES).unwrap() + 1],
            )
        })
        .await;
        let e = source(&server, false)
            .search(&query(), &CancelToken::new())
            .await
            .unwrap_err();
        assert_eq!(e.code, CODE_WEB_UNAVAILABLE, "{e}");
        assert!(e.message.contains("more than"), "{e}");
    }

    #[test]
    fn a_hosted_daemons_source_names_its_own_hosts() {
        let (_d, st) = store();
        let hosted = for_daemon(
            st.clone(),
            None,
            Some("https://clax.example"),
            Some("sites.example"),
        )
        .unwrap();
        let own = hosted.own_hosts();
        assert!(own.holds("clax.example"));
        assert!(own.holds("abcdefghijkl.sites.example"));
        assert!(!own.holds("example.com"));
        // A local daemon has none: its hosts are loopback.
        let local = for_daemon(st.clone(), None, None, None).unwrap();
        assert_eq!(local.own_hosts(), OwnHosts::default());
        // A site domain that is no host name leaves the web off, not open.
        let why = for_daemon(
            st.clone(),
            None,
            Some("https://clax.example"),
            Some("https://sites.example/x"),
        )
        .err()
        .unwrap();
        assert!(why.contains("\"https://sites.example/x\""), "{why}");
        let (bad, unavailable) = daemon_web(
            st,
            None,
            Some("https://clax.example"),
            Some("https://sites.example/x"),
        );
        assert_eq!(bad.configured(), WebShape::default());
        assert_eq!(unavailable, Some(why));
    }

    #[test]
    fn fixtures_load_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web.json");
        std::fs::write(
            &path,
            r#"{"searches": {"q": [{"url": "https://a.example/"}]}}"#,
        )
        .unwrap();
        assert!(fixtures_at(&path).is_ok());
        std::fs::write(&path, "[]").unwrap();
        assert!(fixtures_at(&path).is_err());
        assert!(fixtures_at(&dir.path().join("none.json")).is_err());
    }
}
