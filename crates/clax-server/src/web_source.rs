//! The daemon's web source (spec
//! `docs/superpowers/specs/2026-11-02-claxbot-web-tools-design.md` §4.3,
//! §5.3, §10): [`RestSource`], which searches and converts through
//! Cloudflare's REST API and fetches pages with the daemon's own HTTP
//! client, and [`DaemonWeb`], which builds one from the instance's
//! settings and sealed token at each call.
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

use clax_bot::web::page::{decode, links_of_html, links_of_markdown, pretty_json};
use clax_bot::web::url::{is_local_addr, redirect_allowed};
use clax_bot::web::{
    CODE_URL_NOT_ALLOWED, CODE_WEB_BLOCKED, CODE_WEB_TIMEOUT, CODE_WEB_TOO_LARGE,
    CODE_WEB_UNAVAILABLE, CODE_WEB_UNSUPPORTED_CONTENT_TYPE, FakeWebSource, FetchLimits,
    FetchedPage, Format, PublicUrl, SearchAnswer, SearchHit, SearchQuery, WebError, WebSource,
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

/// How long a search or a conversion request to Cloudflare may take on
/// its own (a fetch's conversion is also within the fetch's deadline).
const API_TIMEOUT: Duration = Duration::from_secs(30);

/// The most bytes of a search answer read from Cloudflare.
const SEARCH_ANSWER_BYTES: u64 = 1 << 20;

/// The most bytes of a Markdown conversion's answer read from Cloudflare.
const CONVERSION_ANSWER_BYTES: u64 = 16 << 20;

/// The `User-Agent` of a fetch from a daemon at `origin`: `ClaxBot/1
/// (+<origin>)`, so a site can tell who fetched it and where to ask.
#[must_use]
pub fn user_agent(origin: &str) -> String {
    format!("ClaxBot/1 (+{})", origin.trim_end_matches('/'))
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
/// tests against a local server; nothing else sets it.
#[derive(Clone)]
pub struct CheckedResolver {
    lookup: Arc<dyn Lookup>,
    allow_loopback: bool,
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

/// What a content type is to a fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// HTML: converted in `Auto`, its links read from the HTML.
    Html,
    /// PDF and Office documents: converted in `Auto`, refused in `Raw`.
    Document,
    /// Markdown: as it came, its links read from it.
    Markdown,
    /// JSON: pretty-printed in `Auto`.
    Json,
    /// Other text, as it came.
    Text,
    /// Images, audio and video: always refused.
    Media,
    /// No type, or an unknown one: text when the body is UTF-8 without a
    /// NUL, else refused.
    Unknown,
}

/// The kind of the media type `essence` (lowercase, no parameters).
fn kind_of(essence: &str) -> Kind {
    let (top, sub) = essence.split_once('/').unwrap_or((essence, ""));
    match (top, sub) {
        ("text", "html") | ("application", "xhtml+xml") => Kind::Html,
        ("text", "markdown" | "x-markdown") => Kind::Markdown,
        ("application", "json") => Kind::Json,
        ("application", s) if s.ends_with("+json") => Kind::Json,
        ("application", "pdf" | "msword" | "vnd.ms-excel" | "vnd.apple.numbers") => Kind::Document,
        ("application", s)
            if s.starts_with("vnd.openxmlformats-officedocument.")
                || s.starts_with("vnd.oasis.opendocument.") =>
        {
            Kind::Document
        }
        ("text", _) => Kind::Text,
        ("application", "xml" | "javascript" | "ecmascript" | "x-yaml" | "yaml" | "toml") => {
            Kind::Text
        }
        ("application", s) if s.ends_with("+xml") => Kind::Text,
        ("image" | "audio" | "video", _) => Kind::Media,
        _ => Kind::Unknown,
    }
}

/// A [`WebSource`] over Cloudflare's REST API: search through
/// `POST {api_base}/client/v4/accounts/{id}/ai/websearch/`, conversion to
/// Markdown through `POST {api_base}/client/v4/accounts/{id}/ai/tomarkdown`,
/// and pages fetched by the daemon itself, with the host check
/// ([`CheckedResolver`]) on every request.
pub struct RestSource {
    api_base: Url,
    account_id: String,
    token: Secret,
    /// Cloudflare's API: no redirects followed (the token goes nowhere
    /// else). Each client is the reason it could not be built otherwise,
    /// and every call then answers `web_unavailable`: never a client
    /// without these rules.
    api: Result<reqwest::Client, String>,
    /// Pages: the checked resolver, no redirects, no proxy.
    web: Result<reqwest::Client, String>,
    /// Sent on every fetch ([`user_agent`]).
    user_agent: String,
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
        let api = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| e.to_string());
        let web = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .dns_resolver(Arc::new(CheckedResolver {
                lookup,
                allow_loopback,
            }))
            .build()
            .map_err(|e| e.to_string());
        RestSource {
            api_base,
            account_id,
            token,
            api,
            web,
            user_agent: "ClaxBot/1".into(),
        }
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

    /// `{api_base}/client/v4/accounts/{id}/ai/{path}`.
    fn endpoint(&self, path: &str) -> String {
        format!(
            "{}/client/v4/accounts/{}/ai/{path}",
            self.api_base.as_str().trim_end_matches('/'),
            self.account_id
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

    /// `body` (of type `content_type`) converted to Markdown by Cloudflare.
    async fn to_markdown(&self, body: Vec<u8>, content_type: &str) -> Result<String, WebError> {
        let essence = content_type.split(';').next().unwrap_or("").trim();
        let ext = mime_guess::get_mime_extensions_str(essence)
            .and_then(|e| e.first())
            .copied()
            .unwrap_or("bin");
        let part = reqwest::multipart::Part::bytes(body)
            .file_name(format!("page.{ext}"))
            .mime_str(content_type)
            .map_err(|e| unavailable(format!("the page's type cannot be sent ({e})"), false))?;
        let form = reqwest::multipart::Form::new().part("files", part);
        let r = RestSource::client(&self.api)?
            .post(self.endpoint("tomarkdown"))
            .bearer_auth(self.token.expose())
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

    /// The fetch, without its deadline or cancellation; `sent` is set once
    /// the first request is handed to the client.
    async fn fetch_page(
        &self,
        url: &PublicUrl,
        format: Format,
        limits: &FetchLimits,
        hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
        sent: &AtomicBool,
    ) -> Result<FetchedPage, WebError> {
        let mut at = url.clone();
        let mut hops = 0u8;
        let resp = loop {
            let first = !sent.swap(true, Ordering::SeqCst);
            let r = RestSource::client(&self.web)?
                .get(at.as_str())
                .header(reqwest::header::ACCEPT, ACCEPT)
                .header(reqwest::header::USER_AGENT, &self.user_agent)
                .send()
                .await
                .map_err(|e| fetch_error(&e, &at, !first))?;
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
        let essence = declared
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let unsupported = |what: &str| {
            gone(WebError::new(
                CODE_WEB_UNSUPPORTED_CONTENT_TYPE,
                format!("{} is {what}, which is not read as text", at.normalized()),
                false,
            ))
        };
        let mut kind = kind_of(&essence);
        match (kind, format) {
            (Kind::Media, _) => return Err(unsupported(&essence)),
            (Kind::Document, Format::Raw) => {
                return Err(unsupported(&format!("{essence} (binary, asked for raw)")));
            }
            _ => {}
        }
        let body = read_capped(resp, limits.max_bytes, &at)
            .await
            .map_err(gone)?;
        if kind == Kind::Unknown {
            if !body.contains(&0) && std::str::from_utf8(&body).is_ok() {
                kind = Kind::Text;
            } else {
                let what = if essence.is_empty() {
                    "untyped binary"
                } else {
                    &essence
                };
                return Err(unsupported(what));
            }
        }
        let content_type = if essence.is_empty() {
            "text/plain".to_string()
        } else {
            essence.clone()
        };
        let (text, links) = match (kind, format) {
            (Kind::Html, Format::Auto) => {
                // Sent decoded, as UTF-8, so the conversion never guesses
                // the page's charset.
                let html = decode(&body, &declared);
                let links = links_of_html(&html, &at);
                let md = self
                    .to_markdown(html.into_bytes(), "text/html; charset=utf-8")
                    .await
                    .map_err(gone)?;
                (md, links)
            }
            (Kind::Html, Format::Raw) => {
                let html = decode(&body, &declared);
                let links = links_of_html(&html, &at);
                (html, links)
            }
            (Kind::Document, _) => {
                let md = self.to_markdown(body, &essence).await.map_err(gone)?;
                let links = links_of_markdown(&md, &at);
                (md, links)
            }
            (Kind::Json, Format::Auto) => {
                let s = decode(&body, &declared);
                (pretty_json(&s).unwrap_or(s), Vec::new())
            }
            (Kind::Json, Format::Raw) => (decode(&body, &declared), Vec::new()),
            _ => {
                let s = decode(&body, &declared);
                let links = links_of_markdown(&s, &at);
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
/// URL, and the request ID of `metadata.requestId`.
fn parse_search(v: Value) -> SearchAnswer {
    let result = match v.get("result") {
        Some(r) if r.is_object() => r.clone(),
        _ => v.clone(),
    };
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
    let hits = result
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| {
                    Some(SearchHit {
                        url: s(i, "url").filter(|u| !u.trim().is_empty())?,
                        title: s(i, "title"),
                        description: s(i, "description"),
                        age: s(i, "age"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
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
                .post(self.endpoint("websearch/"))
                .bearer_auth(self.token.expose())
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
        let sent = AtomicBool::new(false);
        let work = tokio::time::timeout(
            limits.timeout,
            self.fetch_page(url, format, limits, hop, &sent),
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
            .went_out(sent.load(Ordering::SeqCst)))),
            () = cancel.cancelled() => Err(cancelled().went_out(sent.load(Ordering::SeqCst))),
        }
    }
}

/// The daemon's [`WebSource`]: at each call, a [`RestSource`] for the
/// account ID and sealed Cloudflare token the instance holds now, kept
/// while they stay the same. The token is opened for the API base it is
/// sent to, which the token's slot binds to [`CLOUDFLARE_ORIGIN`]: any
/// other base is refused, and reads as not configured. Without both,
/// [`WebSource::configured`] is false, so a run's calls are refused
/// `web_disabled` before the source is called; a call that reaches it
/// anyway answers `web_unavailable`, not gone out.
pub struct DaemonWeb {
    store: Arc<Store>,
    kek: Option<Kek>,
    api_base: Url,
    allow_loopback: bool,
    lookup: Arc<dyn Lookup>,
    user_agent: String,
    /// The source of the last credentials, and those credentials.
    current: Mutex<Option<(String, Secret, Arc<RestSource>)>>,
}

impl DaemonWeb {
    /// The source of the instance on `store`, opening the token with `kek`.
    #[must_use]
    pub fn new(store: Arc<Store>, kek: Option<Kek>) -> DaemonWeb {
        DaemonWeb {
            store,
            kek,
            api_base: Url::parse(CLOUDFLARE_ORIGIN).expect("Cloudflare's origin is a URL"),
            allow_loopback: false,
            lookup: Arc::new(SystemLookup),
            user_agent: "ClaxBot/1".into(),
            current: Mutex::new(None),
        }
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
        let Some((account, token)) = creds else {
            return Err(unavailable(
                "web use is not configured on this instance: an admin sets the Cloudflare account ID and API token",
                false,
            ));
        };
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((a, t, s)) = current.as_ref()
            && *a == account
            && t.expose() == token.expose()
        {
            return Ok(s.clone());
        }
        let source = Arc::new(
            RestSource::with_lookup(
                self.api_base.clone(),
                account.clone(),
                token.clone(),
                self.allow_loopback,
                self.lookup.clone(),
            )
            .with_user_agent(self.user_agent.clone()),
        );
        *current = Some((account, token, source.clone()));
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

    /// Whether the instance holds an account ID and a token that opens now
    /// ([`has_credentials`]); a store error reads as not configured.
    fn configured(&self) -> bool {
        has_credentials(&self.store, self.kek.as_ref()).unwrap_or(false)
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
/// ([`fixtures`]), else a [`DaemonWeb`] on `store` whose fetches name the
/// daemon at `origin` ([`user_agent`]).
#[must_use]
pub fn for_daemon(store: Arc<Store>, kek: Option<Kek>, origin: &str) -> Arc<dyn WebSource> {
    match fixtures() {
        Some(f) => f,
        None => Arc::new(DaemonWeb::new(store, kek).with_user_agent(user_agent(origin))),
    }
}

#[cfg(test)]
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
                            {"url": "https://a.example/", "title": "A", "description": "First"},
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
                    age: None,
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
    async fn fetch_converts_html_through_tomarkdown() {
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
            p if p == format!("/client/v4/accounts/{ACCOUNT}/ai/tomarkdown") => {
                assert_eq!(r.header("authorization"), format!("Bearer {TOKEN}"));
                let body = String::from_utf8_lossy(&r.body);
                assert!(body.contains("name=\"files\""), "{body}");
                assert!(body.contains("filename=\"page.htm"), "{body}");
                assert!(body.contains("charset=utf-8"), "{body}");
                // The page arrives decoded, as UTF-8.
                let data = if body.contains("<p>caf\u{e9}</p>") {
                    "caf\u{e9}\n"
                } else {
                    assert!(body.contains("<h1>Hi</h1>"), "{body}");
                    "# Hi\n\n[next](/next)\n"
                };
                resp(
                    200,
                    "application/json",
                    json!({"success": true, "result": [
                        {"name": "page.html", "mimeType": "text/html", "format": "markdown",
                         "data": data}
                    ]})
                    .to_string(),
                )
            }
            _ => resp(404, "text/plain", "no"),
        })
        .await;
        let s = source(&server, true);
        let url = server.url("site.test", "/page");
        let p = fetch(&s, &url, Format::Auto, FetchLimits::default())
            .await
            .unwrap();
        assert_eq!(p.text, "# Hi\n\n[next](/next)\n");
        assert_eq!(p.status, 200);
        assert_eq!(p.content_type, "text/html");
        assert_eq!(p.final_url, url.normalized());
        assert_eq!(
            p.links,
            vec![format!("http://site.test:{}/next", server.port)]
        );
        // Raw returns the HTML itself, with no conversion.
        let before = server.requests().len();
        let p = fetch(&s, &url, Format::Raw, FetchLimits::default())
            .await
            .unwrap();
        assert!(p.text.contains("<h1>Hi</h1>"), "{}", p.text);
        assert_eq!(server.requests().len(), before + 1, "no conversion request");
        // A page in another charset is converted from its text.
        let latin = server.url("site.test", "/latin");
        let p = fetch(&s, &latin, Format::Auto, FetchLimits::default())
            .await
            .unwrap();
        assert_eq!(p.text, "caf\u{e9}\n");
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
        let server = Server::start(|_| None).await;
        let s = source(&server, true);
        let limits = FetchLimits {
            timeout: Duration::from_millis(50),
            ..FetchLimits::default()
        };
        let e = fetch(&s, &server.url("site.test", "/"), Format::Auto, limits)
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.went_out, e.retryable),
            (CODE_WEB_TIMEOUT, true, true),
            "{e}"
        );
        // Cancelled, it ends cancelled.
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let url = server.url("site.test", "/");
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
        assert!(!web.configured());
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
        assert!(web.configured());
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
        assert!(!other.configured());
        assert!(!has_credentials(&st, Some(&Kek::generate())).unwrap());
        st.clear_secret(&AuditCtx::DAEMON, &SecretSlot::Cloudflare)
            .unwrap();
        assert!(!web.configured());
    }

    #[tokio::test]
    async fn a_fetch_names_the_daemon() {
        let server =
            Server::start(|r| resp(200, "text/plain", r.header("user-agent").to_string())).await;
        let s = source(&server, true).with_user_agent(user_agent("https://clax.example/"));
        let p = fetch(
            &s,
            &server.url("site.test", "/"),
            Format::Auto,
            FetchLimits::default(),
        )
        .await
        .unwrap();
        assert_eq!(p.text, "ClaxBot/1 (+https://clax.example)");
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
