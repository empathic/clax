//! ClaxBot's web source on the Worker (spec
//! `docs/superpowers/specs/2026-11-02-claxbot-web-tools-design.md` §4.3,
//! §10): [`BindingSource`] (wasm32 only) searches through the Workers AI
//! binding's `websearch()` and fetches with the Worker's own `fetch`,
//! converting HTML, PDF and Office documents with the binding's
//! `toMarkdown()`; [`FixtureWeb`] is the scripted source the test mode
//! uses instead.
//!
//! # What is decided here
//!
//! Everything about a call that does not need JavaScript is a function
//! of this module, compiled for both targets and tested natively:
//!
//! - [`parse_search`] and [`search_failure`]: what `websearch()`'s
//!   response says;
//! - [`follow`]: the redirect loop. Each source owns its own, because
//!   their HTTP clients differ; this one calls `hop` on every target after
//!   refusing an `https` to `http` hop itself, and cuts a chain longer than
//!   [`FetchLimits::max_redirects`];
//! - [`check_head`] and [`read_body`]: what a final response's headers
//!   and body make of the page (a challenge is `web_blocked`, a body past
//!   the byte limit `web_too_large`), by the rules every source shares
//!   ([`page::handling`]: images, audio and video are
//!   `web_unsupported_content_type`), and what goes to `toMarkdown()`
//!   ([`Conversion`]);
//! - [`parse_markdown`]: what `toMarkdown()` answered.
//!
//! # Going out
//!
//! A fetch goes out ([`WebError::went_out`], so it is charged) once its
//! first request is handed to `fetch`: every error after that, a refused
//! or cut redirect, a body too large, a failed conversion, a deadline or a
//! cancellation, is marked as gone out. A search never is (a failed
//! search is not charged).
//!
//! # The deadline
//!
//! A fetch's [`FetchLimits::timeout`] covers the whole call: every
//! redirect, the body and the conversion. Each wait races the deadline and
//! the run's cancellation; when either wins the request is aborted (its
//! `AbortController`) and the call answers `web_timeout` or `cancelled`.
//! `toMarkdown()` and `websearch()` take no signal, so for them the race
//! only stops the wait. A search has [`SEARCH_TIMEOUT`].

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clax_bot::FakeWebSource;
use clax_bot::web::page::{self, Handling, decode, links_of_html, links_of_markdown, pretty_json};
use clax_bot::web::url::redirect_allowed;
use clax_bot::web::{
    CODE_WEB_BLOCKED, CODE_WEB_TOO_LARGE, CODE_WEB_UNAVAILABLE, CODE_WEB_UNSUPPORTED_CONTENT_TYPE,
    FetchLimits, FetchedPage, Format, PublicUrl, SearchAnswer, SearchHit, SearchQuery, WebError,
    WebSource,
};
use clax_bot_sandbox::{CODE_CANCELLED, CancelToken};
use serde_json::Value;

/// The `Accept` header of every fetch: Markdown first, so a site that
/// serves Markdown for agents answers in it and skips the conversion.
pub const ACCEPT: &str = "text/markdown, text/html;q=0.9, */*;q=0.8";

/// How long a search is waited for.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The statuses whose `Location` is followed.
#[must_use]
pub fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn str_of<'v>(v: &'v Value, key: &str) -> Option<&'v str> {
    v.get(key).and_then(Value::as_str)
}

/// The search `v` (`websearch()`'s response, read as JSON) answered: each
/// item with a URL, its title and description when the provider gave
/// them (a blank one is none), its `lastModifiedDate` as its age, and
/// `metadata.requestId`. Items without a URL, and items that are no
/// object, are dropped; a response without `items` has no hits (whether
/// it is an answer at all is [`search_failure`]'s). `raw` is `v`.
#[must_use]
pub fn parse_search(v: &Value) -> SearchAnswer {
    let text = |item: &Value, key: &str| {
        str_of(item, key)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let hits = v
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(SearchHit {
                        url: text(item, "url")?,
                        title: text(item, "title"),
                        description: text(item, "description"),
                        age: text(item, "lastModifiedDate"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    SearchAnswer {
        hits,
        request_id: v
            .get("metadata")
            .and_then(|m| str_of(m, "requestId"))
            .map(str::to_string),
        raw: v.clone(),
    }
}

/// Why a `websearch()` response of `status` and body `v` is no answer, or
/// `None` when it is one (a 2xx with an `items` list, empty or not).
///
/// A rate limit or a server error may pass (`retryable`); a gateway
/// without credits or the provider key it was told to use, or a request
/// the gateway refused, will not until the admin changes something. Every
/// failure is `web_unavailable`; the gateway's request ID is kept when
/// the body names one.
#[must_use]
pub fn search_failure(status: u16, v: &Value) -> Option<WebError> {
    let ok = (200..300).contains(&status);
    if ok && v.get("items").is_some_and(Value::is_array) {
        return None;
    }
    let error = v.get("error");
    let code = error.and_then(|e| str_of(e, "code"));
    let request_id = error
        .and_then(|e| str_of(e, "gatewayRequestId"))
        .or_else(|| v.get("metadata").and_then(|m| str_of(m, "requestId")))
        .map(str::to_string);
    let (message, retryable) = if ok {
        (
            format!("Web search answered HTTP {status} without results. Try again."),
            true,
        )
    } else if status == 402 || code == Some("web_search_payment_required") {
        (
            "Web search is unavailable: the instance's AI Gateway has no credits and no \
             provider key for it. Continue without searching."
                .to_string(),
            false,
        )
    } else if code == Some("web_search_byok_not_configured") {
        (
            "Web search is unavailable: the provider key the instance names is not on its \
             AI Gateway. Continue without searching."
                .to_string(),
            false,
        )
    } else if status == 429 {
        (
            "Web search is rate limited. Wait, then search again, or continue without it."
                .to_string(),
            true,
        )
    } else {
        let retryable = error
            .and_then(|e| e.get("retryable"))
            .and_then(Value::as_bool)
            .unwrap_or(status >= 500);
        let what = code.map_or_else(
            || format!("HTTP {status}"),
            |c| format!("{c}, HTTP {status}"),
        );
        let next = if retryable {
            "Try again, or continue without searching."
        } else {
            "Continue without searching."
        };
        (format!("Web search failed ({what}). {next}"), retryable)
    };
    Some(WebError::new(CODE_WEB_UNAVAILABLE, message, retryable).with_request_id(request_id))
}

/// Why a `websearch()` call that threw failed: by [`search_failure`]'s
/// rules when the error carries an HTTP `status` or a gateway `code` (a
/// code alone stands for its status: payment required, a missing provider
/// key and a refused request are not retryable, a rate limit is), else a
/// retryable `web_unavailable` with the error's `message`.
#[must_use]
pub fn thrown_search_failure(status: Option<u16>, code: Option<&str>, message: &str) -> WebError {
    let status = status.or(match code {
        Some("web_search_payment_required") => Some(402),
        Some("web_search_byok_not_configured" | "invalid_web_search_input") => Some(400),
        Some("web_search_unauthorized") => Some(401),
        Some("web_search_rate_limited") => Some(429),
        Some("web_search_unavailable") => Some(503),
        _ => None,
    });
    let Some(status) = status.filter(|s| !(200..300).contains(s)) else {
        return unavailable(
            format!("Web search failed: {message}. Try again, or continue without searching."),
            true,
        );
    };
    let body = serde_json::json!({"error": {"code": code, "status": status}});
    search_failure(status, &body).unwrap_or_else(|| unavailable(message.to_string(), true))
}

/// The Markdown `toMarkdown()` answered (`v`, its result read as JSON: a
/// list of conversion results, of which the first is the one asked for,
/// or one result).
///
/// # Errors
/// `web_unavailable` (retryable, gone out) when there is no result, it is
/// an `error` one (its message kept), or it has no `data`.
pub fn parse_markdown(v: &Value) -> Result<String, WebError> {
    let first = match v {
        Value::Array(results) => results.first(),
        Value::Object(_) => Some(v),
        _ => None,
    };
    let failed = |why: String| {
        Err(WebError::new(
            CODE_WEB_UNAVAILABLE,
            format!("The page could not be converted to Markdown{why}. Try again, or fetch it with format \"raw\"."),
            true,
        )
        .went_out(true))
    };
    let Some(result) = first else {
        return failed(String::new());
    };
    if str_of(result, "format") == Some("error") {
        return failed(str_of(result, "error").map_or_else(String::new, |e| format!(": {e}")));
    }
    match str_of(result, "data") {
        Some(data) => Ok(data.to_string()),
        None => failed(String::new()),
    }
}

/// One response to one request: its status, its `Location` header, and
/// whatever the source keeps of it (`body`).
pub struct Reply<B> {
    pub status: u16,
    pub location: Option<String>,
    pub body: B,
}

fn cancelled() -> WebError {
    WebError::new(CODE_CANCELLED, "the run was cancelled", false)
}

fn unavailable(message: String, retryable: bool) -> WebError {
    WebError::new(CODE_WEB_UNAVAILABLE, message, retryable)
}

/// Requests `url` with `send` and follows its redirects (see the module
/// docs): a redirect status with a `Location` is resolved against the URL
/// it answered, refused when it goes from `https` to `http`, given to
/// `hop`, and requested; after [`FetchLimits::max_redirects`] of them the
/// next is `web_unavailable`. Answers the URL the last request went to and
/// its reply.
///
/// # Errors
/// `cancelled` when `cancel` is cancelled before a request (gone out after
/// the first); every other error has gone out: `send`'s, `hop`'s, a
/// `Location` that is no URL or a chain too long (`web_unavailable`), an
/// `https` to `http` hop or a target that is no public URL
/// (`url_not_allowed`).
pub async fn follow<B, F, Fut>(
    url: &PublicUrl,
    limits: &FetchLimits,
    hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
    cancel: &CancelToken,
    mut send: F,
) -> Result<(PublicUrl, Reply<B>), WebError>
where
    F: FnMut(PublicUrl) -> Fut,
    Fut: std::future::Future<Output = Result<Reply<B>, WebError>>,
{
    let mut at = url.clone();
    let mut hops = 0u8;
    let mut sent = false;
    loop {
        if cancel.is_cancelled() {
            return Err(cancelled().went_out(sent));
        }
        sent = true;
        let reply = send(at.clone()).await.map_err(|e| e.went_out(true))?;
        let location = match &reply.location {
            Some(l) if is_redirect(reply.status) => l.clone(),
            _ => return Ok((at, reply)),
        };
        if hops >= limits.max_redirects {
            return Err(unavailable(
                format!(
                    "{} redirected more than {} times. Use another source.",
                    url.normalized(),
                    limits.max_redirects
                ),
                false,
            )
            .went_out(true));
        }
        hops += 1;
        let no_url = || {
            unavailable(
                format!(
                    "{} redirected to no URL ({location:?}). Use another source.",
                    at.normalized()
                ),
                false,
            )
            .went_out(true)
        };
        let to = at.as_url().join(&location).map_err(|_| no_url())?;
        let to = match PublicUrl::parse(to.as_str()) {
            Ok(to) => to,
            Err(e) if e.code == clax_bot::api::CODE_BAD_ARGS => return Err(no_url()),
            Err(e) => return Err(e.went_out(true)),
        };
        if !redirect_allowed(&at, &to) {
            return Err(WebError::new(
                clax_bot::web::CODE_URL_NOT_ALLOWED,
                format!(
                    "{} redirected from https to http ({}), which is not followed.",
                    at.normalized(),
                    to.normalized()
                ),
                false,
            )
            .went_out(true));
        }
        hop(&to).map_err(|e| e.went_out(true))?;
        at = to;
    }
}

/// What a final response's headers already decide, before its body is
/// read: a Cloudflare challenge (`cf-mitigated: challenge`) is
/// `web_blocked`, and a declared length past the byte limit
/// `web_too_large`. Both have gone out.
///
/// # Errors
/// As above.
pub fn check_head(
    url: &PublicUrl,
    status: u16,
    content_length: Option<u64>,
    cf_mitigated: Option<&str>,
    limits: &FetchLimits,
) -> Result<(), WebError> {
    if cf_mitigated.is_some_and(|m| m.trim().eq_ignore_ascii_case("challenge")) {
        return Err(WebError::new(
            CODE_WEB_BLOCKED,
            format!(
                "{} answered with a bot challenge (HTTP {status}), which a fetch cannot pass. \
                 Use another source.",
                url.normalized()
            ),
            false,
        )
        .went_out(true));
    }
    if content_length.is_some_and(|n| n > limits.max_bytes) {
        return Err(too_large(url, limits));
    }
    Ok(())
}

/// `web_too_large` for `url`, gone out.
#[must_use]
pub fn too_large(url: &PublicUrl, limits: &FetchLimits) -> WebError {
    WebError::new(
        CODE_WEB_TOO_LARGE,
        format!(
            "{} is larger than {} bytes. Use another source.",
            url.normalized(),
            limits.max_bytes
        ),
        false,
    )
    .went_out(true)
}

/// What a page's body comes to: the page, or a conversion to run first.
#[derive(Debug, PartialEq, Eq)]
pub enum Body {
    Page(FetchedPage),
    Convert(Conversion),
}

/// A body for `toMarkdown()`: the document to give it (`name`, `mime`,
/// `bytes`), and the page its Markdown finishes ([`Conversion::finish`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Conversion {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    /// The page without its text; an HTML page's links already read from
    /// its HTML.
    page: FetchedPage,
    html: bool,
    base: PublicUrl,
}

impl Conversion {
    /// The page, with `markdown` as its text, and a document's links read
    /// from it.
    ///
    /// # Errors
    /// `web_unavailable` (gone out) when the page is an HTTP error whose
    /// conversion is empty: an error page with nothing to read.
    pub fn finish(mut self, markdown: String) -> Result<FetchedPage, WebError> {
        if self.page.status >= 400 && markdown.trim().is_empty() {
            return Err(http_error(&self.base, self.page.status));
        }
        if !self.html {
            self.page.links = links_of_markdown(&markdown, &self.base);
        }
        self.page.text = markdown;
        Ok(self.page)
    }
}

fn http_error(url: &PublicUrl, status: u16) -> WebError {
    let retryable = status >= 500 || status == 429 || status == 408;
    let next = if retryable {
        "Try again later, or use another source."
    } else {
        "Use another source."
    };
    unavailable(
        format!(
            "{} answered HTTP {status} without a readable body. {next}",
            url.normalized()
        ),
        retryable,
    )
    .went_out(true)
}

/// What the final response at `url` (`status`, its `Content-Type` header,
/// its whole `body`) makes in `format`, by the shared rules
/// ([`page::handling`], on [`page::media_type_of`]'s type: a missing or
/// `application/octet-stream` type is sniffed): HTML and documents to
/// convert ([`Body::Convert`]; an HTML page is given as UTF-8, decoded by
/// its charset first, its links read from the HTML), JSON pretty-printed
/// in `Auto`, other text as it came, its links read from it. A 4xx or 5xx
/// with a readable body is a page with its status.
///
/// # Errors
/// All gone out: `web_unsupported_content_type` for what
/// [`Handling::Unsupported`] covers; `web_unavailable` for a 4xx or 5xx
/// whose body is empty or not text.
pub fn read_body(
    url: &PublicUrl,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
    format: Format,
) -> Result<Body, WebError> {
    let media = page::media_type_of(content_type, &body);
    let how = page::handling(&media, format);
    if status >= 400 && (body.is_empty() || !how.is_textual()) {
        return Err(http_error(url, status));
    }
    let page = |text: String, links: Vec<String>| FetchedPage {
        final_url: url.normalized().to_string(),
        status,
        content_type: media.clone(),
        text,
        links,
        request_id: None,
    };
    let text = match how {
        Handling::Unsupported => {
            let raw = if page::handling(&media, Format::Auto) != Handling::Unsupported {
                " with format \"raw\"; fetch it without"
            } else {
                ""
            };
            let what = if media.is_empty() {
                "of no known type"
            } else {
                &media
            };
            return Err(WebError::new(
                CODE_WEB_UNSUPPORTED_CONTENT_TYPE,
                format!(
                    "{} is {what}, which cannot be read as text{raw}. Use another source.",
                    url.normalized()
                ),
                false,
            )
            .went_out(true));
        }
        Handling::Document(ext) => {
            return Ok(Body::Convert(Conversion {
                name: format!("document.{ext}"),
                mime: media.clone(),
                bytes: body,
                page: page(String::new(), Vec::new()),
                html: false,
                base: url.clone(),
            }));
        }
        _ => decode(&body, content_type),
    };
    Ok(match how {
        Handling::Html => {
            let links = links_of_html(&text, url);
            Body::Convert(Conversion {
                name: "page.html".into(),
                mime: HTML_UTF8.into(),
                bytes: text.into_bytes(),
                page: page(String::new(), links),
                html: true,
                base: url.clone(),
            })
        }
        Handling::RawHtml => {
            let links = links_of_html(&text, url);
            Body::Page(page(text, links))
        }
        Handling::Json => {
            let text = pretty_json(&text).unwrap_or(text);
            let links = links_of_markdown(&text, url);
            Body::Page(page(text, links))
        }
        _ => {
            let links = links_of_markdown(&text, url);
            Body::Page(page(text, links))
        }
    })
}

/// The type an HTML page is given to `toMarkdown()` as: it is decoded
/// first, whatever charset it came in.
pub const HTML_UTF8: &str = "text/html; charset=utf-8";

/// The test mode's source (`CLAX_TEST_ROUTES` and `CLAX_TEST_WEB`): a
/// [`FakeWebSource`] scripted by `POST /_worker/web-fixtures`
/// ([`FakeWebSource::from_fixtures`]'s shape), which a later load
/// replaces whole.
#[derive(Default)]
pub struct FixtureWeb {
    fake: Mutex<Arc<FakeWebSource>>,
}

impl FixtureWeb {
    fn current(&self) -> Arc<FakeWebSource> {
        self.fake.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Scripts the source from `v`, replacing what it had.
    ///
    /// # Errors
    /// What in `v` is not a fixture ([`FakeWebSource::from_fixtures`]);
    /// the source is unchanged then.
    pub fn load(&self, v: &Value) -> Result<(), String> {
        self.replace(FakeWebSource::from_fixtures(v)?);
        Ok(())
    }

    /// Makes `fake` the source, replacing what it had.
    pub fn replace(&self, fake: FakeWebSource) {
        *self.fake.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(fake);
    }

    /// The calls since the last load ([`FakeWebSource::calls`]).
    #[must_use]
    pub fn calls(&self) -> Vec<(String, String)> {
        self.current().calls()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl WebSource for FixtureWeb {
    async fn search(
        &self,
        q: &SearchQuery,
        cancel: &CancelToken,
    ) -> Result<SearchAnswer, WebError> {
        self.current().search(q, cancel).await
    }

    async fn fetch(
        &self,
        url: &PublicUrl,
        format: Format,
        limits: &FetchLimits,
        hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
        cancel: &CancelToken,
    ) -> Result<FetchedPage, WebError> {
        self.current().fetch(url, format, limits, hop, cancel).await
    }
}

#[cfg(target_arch = "wasm32")]
pub use binding::BindingSource;

#[cfg(target_arch = "wasm32")]
mod binding {
    use super::*;
    use clax_bot::web::CODE_WEB_TIMEOUT;
    use futures_util::future::{Either, select};
    use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &key.into()).unwrap_or(JsValue::UNDEFINED)
    }

    fn message(e: &JsValue) -> String {
        e.dyn_ref::<js_sys::Error>()
            .map(|e| String::from(e.message()))
            .or_else(|| e.as_string())
            .unwrap_or_else(|| format!("{e:?}"))
    }

    /// `o[method](...args)`, awaited. Through `Reflect.apply`, since the
    /// AI binding is an RPC stub (every property of its methods is another
    /// remote call), and through `Promise.resolve`, which adopts its
    /// answer's thenable (`src/repos/js.rs` `call`).
    async fn call(o: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = get(o, method)
            .dyn_into()
            .map_err(|_| JsValue::from_str(&format!("no {method}()")))?;
        let argv: Array = args.iter().collect();
        let v = Reflect::apply(&f, o, &argv)?;
        JsFuture::from(Promise::resolve(&v)).await
    }

    fn object(pairs: &[(&str, JsValue)]) -> Object {
        let o = Object::new();
        for (k, v) in pairs {
            let _ = Reflect::set(&o, &(*k).into(), v);
        }
        o
    }

    /// `new globalThis[name](...args)`.
    fn construct(name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = get(&js_sys::global(), name).dyn_into()?;
        let argv: Array = args.iter().collect();
        Reflect::construct(&f, &argv)
    }

    /// `v` as JSON, through `JSON.stringify` (`null` for what it cannot
    /// write).
    fn to_value(v: &JsValue) -> Value {
        js_sys::JSON::stringify(v)
            .ok()
            .and_then(|s| s.as_string())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null)
    }

    /// An `AbortController`, aborted when dropped: whatever a call left
    /// in flight stops with it.
    struct Abort(JsValue);

    impl Abort {
        fn new() -> Abort {
            Abort(construct("AbortController", &[]).unwrap_or(JsValue::UNDEFINED))
        }

        fn signal(&self) -> JsValue {
            get(&self.0, "signal")
        }
    }

    impl Drop for Abort {
        fn drop(&mut self) {
            if let Ok(f) = get(&self.0, "abort").dyn_into::<Function>() {
                let _ = f.call0(&self.0);
            }
        }
    }

    /// `fut`, unless `cancel` is cancelled or `deadline_ms` (by
    /// `Date.now()`) passes first.
    async fn within<T>(
        fut: impl std::future::Future<Output = T>,
        cancel: &CancelToken,
        deadline_ms: f64,
        what: &str,
    ) -> Result<T, WebError> {
        let left = (deadline_ms - js_sys::Date::now()).max(0.0);
        let delay = worker::Delay::from(Duration::from_millis(left as u64));
        let fut = std::pin::pin!(fut);
        let stopped = std::pin::pin!(cancel.cancelled());
        let stop = select(stopped, delay);
        match select(fut, stop).await {
            Either::Left((v, _)) => Ok(v),
            Either::Right((Either::Left(_), _)) => Err(cancelled()),
            Either::Right((Either::Right(_), _)) => Err(WebError::new(
                CODE_WEB_TIMEOUT,
                format!("{what} took longer than the deadline. Try again, or use another source."),
                true,
            )),
        }
    }

    /// ClaxBot's web source over the Workers AI binding `AI` (see the
    /// module docs).
    pub struct BindingSource {
        ai: JsValue,
        /// [`page::user_agent`] of the instance's public origin, if any.
        user_agent: String,
    }

    impl BindingSource {
        /// The source over `env`'s `AI` binding, when it has one. Under
        /// `wrangler dev --local` it has one whose every call throws
        /// ("needs to be run remotely"), which answers `web_unavailable`.
        /// Its fetches name the instance at `public_origin`, when it has
        /// one, in their `User-Agent` ([`page::user_agent`]).
        #[must_use]
        pub fn from_env(env: &worker::Env, public_origin: Option<&str>) -> Option<BindingSource> {
            let ai = Reflect::get(env.as_ref(), &JsValue::from_str("AI")).ok()?;
            ai.is_object().then(|| BindingSource {
                ai,
                user_agent: page::user_agent(public_origin),
            })
        }

        /// One `GET` of `url`, not following redirects; a redirect's body
        /// is dropped unread.
        async fn request(
            &self,
            url: PublicUrl,
            abort: &Abort,
            cancel: &CancelToken,
            deadline: f64,
        ) -> Result<Reply<JsValue>, WebError> {
            let headers = object(&[
                ("accept", ACCEPT.into()),
                ("user-agent", self.user_agent.as_str().into()),
            ]);
            let init = object(&[
                ("method", "GET".into()),
                ("redirect", "manual".into()),
                ("headers", headers.into()),
                ("signal", abort.signal()),
            ]);
            let global = js_sys::global();
            let fetch: Function = get(&global, "fetch")
                .dyn_into()
                .map_err(|_| unavailable("This Worker has no fetch.".into(), false))?;
            let failed = |e: JsValue| {
                unavailable(
                    format!(
                        "Fetching {} failed: {}. Try again, or use another source.",
                        url.normalized(),
                        message(&e)
                    ),
                    true,
                )
            };
            let started = Reflect::apply(
                &fetch,
                &global,
                &Array::of2(&url.as_str().into(), &init.into()),
            )
            .map_err(failed)?;
            let res = within(
                JsFuture::from(Promise::resolve(&started)),
                cancel,
                deadline,
                "The fetch",
            )
            .await?
            .map_err(failed)?;
            let status = get(&res, "status").as_f64().unwrap_or(0.0) as u16;
            let location = header(&res, "location");
            if is_redirect(status) && location.is_some() {
                discard(&res, cancel, deadline).await;
            }
            Ok(Reply {
                status,
                location,
                body: res,
            })
        }

        /// The page of `reply`, the final response at `url`.
        async fn page(
            &self,
            url: &PublicUrl,
            reply: Reply<JsValue>,
            format: Format,
            limits: &FetchLimits,
            cancel: &CancelToken,
            deadline: f64,
        ) -> Result<FetchedPage, WebError> {
            let res = reply.body;
            let content_type = header(&res, "content-type").unwrap_or_default();
            let length = header(&res, "content-length").and_then(|l| l.trim().parse::<u64>().ok());
            let mitigated = header(&res, "cf-mitigated");
            if let Err(e) = check_head(url, reply.status, length, mitigated.as_deref(), limits) {
                discard(&res, cancel, deadline).await;
                return Err(e);
            }
            let body = read_capped(&res, url, limits, cancel, deadline).await?;
            match read_body(url, reply.status, &content_type, body, format)? {
                Body::Page(p) => Ok(p),
                Body::Convert(c) => {
                    let markdown = self.to_markdown(&c, cancel, deadline).await?;
                    c.finish(markdown)
                }
            }
        }

        /// `toMarkdown([{name, blob}])` of `c`.
        async fn to_markdown(
            &self,
            c: &Conversion,
            cancel: &CancelToken,
            deadline: f64,
        ) -> Result<String, WebError> {
            let failed = |e: JsValue| {
                unavailable(
                    format!(
                        "The page could not be converted to Markdown: {}. Try again, or fetch it with format \"raw\".",
                        message(&e)
                    ),
                    true,
                )
            };
            let parts = Array::of1(&Uint8Array::from(c.bytes.as_slice()).into());
            let blob = construct(
                "Blob",
                &[
                    parts.into(),
                    object(&[("type", c.mime.as_str().into())]).into(),
                ],
            )
            .map_err(failed)?;
            let doc = object(&[("name", c.name.as_str().into()), ("blob", blob)]);
            let docs = Array::of1(&doc.into());
            let out = within(
                call(&self.ai, "toMarkdown", &[docs.into()]),
                cancel,
                deadline,
                "The conversion",
            )
            .await?
            .map_err(failed)?;
            parse_markdown(&to_value(&out))
        }
    }

    /// `res.headers.get(name)`.
    fn header(res: &JsValue, name: &str) -> Option<String> {
        let headers = get(res, "headers");
        let f: Function = get(&headers, "get").dyn_into().ok()?;
        f.call1(&headers, &name.into()).ok()?.as_string()
    }

    /// Cancels `res`'s body, unread, waiting for it no longer than the
    /// deadline or the run's cancellation.
    async fn discard(res: &JsValue, cancel: &CancelToken, deadline: f64) {
        let body = get(res, "body");
        if body.is_object() {
            let _ = within(
                call(&body, "cancel", &[]),
                cancel,
                deadline,
                "Dropping a body",
            )
            .await;
        }
    }

    /// `res`'s body, or `web_too_large` once it passes the byte limit (the
    /// stream cancelled there).
    async fn read_capped(
        res: &JsValue,
        url: &PublicUrl,
        limits: &FetchLimits,
        cancel: &CancelToken,
        deadline: f64,
    ) -> Result<Vec<u8>, WebError> {
        let failed = |e: JsValue| {
            unavailable(
                format!(
                    "Reading {} failed: {}. Try again, or use another source.",
                    url.normalized(),
                    message(&e)
                ),
                true,
            )
        };
        let stream = get(res, "body");
        if !stream.is_object() {
            return Ok(Vec::new());
        }
        let reader = call(&stream, "getReader", &[]).await.map_err(failed)?;
        let mut body = Vec::new();
        loop {
            let step = within(
                call(&reader, "read", &[]),
                cancel,
                deadline,
                "Reading the page",
            )
            .await?
            .map_err(failed)?;
            if get(&step, "done").as_bool().unwrap_or(false) {
                return Ok(body);
            }
            let chunk = Uint8Array::new(&get(&step, "value"));
            let len = chunk.length() as usize;
            if (body.len() + len) as u64 > limits.max_bytes {
                let _ = within(
                    call(&reader, "cancel", &[]),
                    cancel,
                    deadline,
                    "Dropping a body",
                )
                .await;
                return Err(too_large(url, limits));
            }
            // Straight from the chunk into the body, with no copy between.
            let at = body.len();
            body.resize(at + len, 0);
            chunk.copy_to(&mut body[at..]);
        }
    }

    #[async_trait::async_trait(?Send)]
    impl WebSource for BindingSource {
        async fn search(
            &self,
            q: &SearchQuery,
            cancel: &CancelToken,
        ) -> Result<SearchAnswer, WebError> {
            if cancel.is_cancelled() {
                return Err(cancelled());
            }
            if !get(&self.ai, "websearch").is_function() {
                return Err(unavailable(
                    "Web search is unavailable on this Worker: its runtime has no \
                     env.AI.websearch() (it needs workerd 1.20260924.1 or later). \
                     Continue without searching."
                        .into(),
                    false,
                ));
            }
            let mut opts = vec![
                ("gatewayId", JsValue::from_str(&q.gateway)),
                ("query", JsValue::from_str(&q.query)),
                ("provider", JsValue::from_str(q.provider.as_str())),
                ("limit", JsValue::from_f64(f64::from(q.limit))),
            ];
            if let Some(alias) = &q.byok_alias {
                opts.push(("byokAlias", JsValue::from_str(alias)));
            }
            // A thrown error may name the gateway's status or code.
            let failed = |e: JsValue| {
                let status = get(&e, "status").as_f64().map(|s| s as u16);
                let code = get(&e, "code").as_string();
                thrown_search_failure(status, code.as_deref(), &message(&e))
            };
            let deadline = js_sys::Date::now() + SEARCH_TIMEOUT.as_millis() as f64;
            let res = within(
                call(&self.ai, "websearch", &[object(&opts).into()]),
                cancel,
                deadline,
                "The search",
            )
            .await?
            .map_err(failed)?;
            let status = get(&res, "status").as_f64().unwrap_or(0.0) as u16;
            let text = within(call(&res, "text", &[]), cancel, deadline, "The search")
                .await?
                .map_err(failed)?
                .as_string()
                .unwrap_or_default();
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            match search_failure(status, &v) {
                Some(e) => Err(e),
                None => Ok(parse_search(&v)),
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
            let deadline = js_sys::Date::now() + limits.timeout.as_millis() as f64;
            let abort = Abort::new();
            let (at, reply) = follow(url, limits, hop, cancel, |u| {
                self.request(u, &abort, cancel, deadline)
            })
            .await?;
            self.page(&at, reply, format, limits, cancel, deadline)
                .await
                .map_err(|e| e.went_out(true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_bot::web::{CODE_URL_NOT_ALLOWED, Provider};
    use serde_json::json;

    #[test]
    fn parse_search_reads_items_and_request_id() {
        let v = json!({
            "items": [
                {"url": "https://a.example/", "title": "A", "description": "About A",
                 "lastModifiedDate": "2026-10-01"},
                {"url": " https://b.example/x ", "title": "  "}
            ],
            "metadata": {"query": "q", "requestId": "req-1", "latencyMs": 12}
        });
        let a = parse_search(&v);
        assert_eq!(a.request_id.as_deref(), Some("req-1"));
        assert_eq!(a.raw, v);
        assert_eq!(a.hits.len(), 2);
        assert_eq!(a.hits[0].url, "https://a.example/");
        assert_eq!(a.hits[0].title.as_deref(), Some("A"));
        assert_eq!(a.hits[0].description.as_deref(), Some("About A"));
        assert_eq!(a.hits[0].age.as_deref(), Some("2026-10-01"));
        // Trimmed; a blank title is none (the script's result falls back
        // to the URL).
        assert_eq!(a.hits[1].url, "https://b.example/x");
        assert_eq!(a.hits[1].title, None);
        assert_eq!(a.hits[1].description, None);
    }

    #[test]
    fn parse_search_drops_items_without_a_url() {
        let v = json!({"items": [
            {"title": "no url"},
            {"url": "", "title": "empty"},
            {"url": 7},
            "not an object",
            {"url": "https://kept.example/"}
        ]});
        let a = parse_search(&v);
        let urls: Vec<_> = a.hits.iter().map(|h| h.url.as_str()).collect();
        assert_eq!(urls, ["https://kept.example/"]);
        assert_eq!(a.request_id, None);
        // No items at all is no hits.
        assert!(parse_search(&json!({})).hits.is_empty());
    }

    #[test]
    fn a_search_failure_is_unavailable_and_says_whether_to_retry() {
        // An answer, empty or not.
        assert!(search_failure(200, &json!({"items": []})).is_none());
        assert!(search_failure(200, &json!({"items": [{"url": "https://a.example/"}]})).is_none());
        let cases = [
            // A 2xx that is no search answer may pass.
            (200, json!({"oops": true}), true),
            (200, Value::Null, true),
            (402, json!({}), false),
            (
                400,
                json!({"ok": false, "error": {"code": "web_search_byok_not_configured", "status": 400}}),
                false,
            ),
            (429, json!({}), true),
            (500, json!({}), true),
            (
                503,
                json!({"error": {"code": "web_search_unavailable", "retryable": false,
                                 "gatewayRequestId": "gw-9"}}),
                false,
            ),
            (
                401,
                json!({"success": false, "errors": [{"code": 10000}]}),
                false,
            ),
        ];
        for (status, body, retryable) in cases {
            let e = search_failure(status, &body).expect("a failure");
            assert_eq!(e.code, CODE_WEB_UNAVAILABLE, "{status} {body}");
            assert_eq!(e.retryable, retryable, "{status} {body}: {}", e.message);
            assert!(!e.went_out, "a search never goes out as charged");
        }
        let e = search_failure(
            402,
            &json!({"error": {"code": "web_search_payment_required"}}),
        )
        .unwrap();
        assert!(e.message.contains("credits"), "{}", e.message);
        let e = search_failure(
            503,
            &json!({"error": {"code": "x", "gatewayRequestId": "gw-9"}}),
        )
        .unwrap();
        assert_eq!(e.request_id.as_deref(), Some("gw-9"));
    }

    #[test]
    fn a_thrown_search_error_keeps_its_status_and_code() {
        // Payment required, a missing provider key: not retryable.
        let e = thrown_search_failure(Some(402), None, "Payment Required");
        assert_eq!((e.code, e.retryable), (CODE_WEB_UNAVAILABLE, false));
        let e = thrown_search_failure(None, Some("web_search_byok_not_configured"), "no key");
        assert!(
            !e.retryable && e.message.contains("provider key"),
            "{}",
            e.message
        );
        let e = thrown_search_failure(None, Some("web_search_payment_required"), "x");
        assert!(
            !e.retryable && e.message.contains("credits"),
            "{}",
            e.message
        );
        // A rate limit or a server error may pass.
        assert!(thrown_search_failure(None, Some("web_search_rate_limited"), "x").retryable);
        assert!(thrown_search_failure(Some(503), None, "x").retryable);
        // Nothing to go by: retryable, with the error's message.
        let e = thrown_search_failure(None, Some("ECONNRESET"), "socket hang up");
        assert!(
            e.retryable && e.message.contains("socket hang up"),
            "{}",
            e.message
        );
        assert!(!e.went_out);
    }

    #[test]
    fn parse_markdown_reads_the_first_result() {
        let v = json!([
            {"name": "page.html", "format": "markdown", "data": "# Hi"},
            {"name": "other", "format": "markdown", "data": "# Other"}
        ]);
        assert_eq!(parse_markdown(&v).unwrap(), "# Hi");
        // One result, not in a list.
        assert_eq!(
            parse_markdown(&json!({"format": "markdown", "data": "x"})).unwrap(),
            "x"
        );
        for bad in [
            json!([{"name": "page.html", "format": "error", "error": "bad file"}]),
            json!([]),
            json!([{"format": "markdown"}]),
            json!(null),
        ] {
            let e = parse_markdown(&bad).unwrap_err();
            assert_eq!(e.code, CODE_WEB_UNAVAILABLE, "{bad}");
            assert!(e.went_out, "a conversion runs after the request: {bad}");
        }
        let e = parse_markdown(&json!([{"format": "error", "error": "bad file"}])).unwrap_err();
        assert!(e.message.contains("bad file"), "{}", e.message);
    }

    fn url(s: &str) -> PublicUrl {
        PublicUrl::parse(s).unwrap()
    }

    fn any(_: &PublicUrl) -> Result<(), WebError> {
        Ok(())
    }

    fn redirect(to: &str) -> Reply<()> {
        Reply {
            status: 302,
            location: Some(to.into()),
            body: (),
        }
    }

    fn ok() -> Reply<()> {
        Reply {
            status: 200,
            location: None,
            body: (),
        }
    }

    /// Follows from `from`, answering each request with `answer`; the
    /// outcome and every URL requested.
    async fn run(
        from: &str,
        hop: &(dyn Fn(&PublicUrl) -> Result<(), WebError> + Sync),
        cancel: &CancelToken,
        answer: impl Fn(&PublicUrl) -> Result<Reply<()>, WebError>,
    ) -> (Result<String, WebError>, Vec<String>) {
        let sent = Mutex::new(Vec::new());
        let r = follow(&url(from), &FetchLimits::default(), hop, cancel, |u| {
            sent.lock().unwrap().push(u.normalized().to_string());
            let a = answer(&u);
            async move { a }
        })
        .await;
        let sent = sent.into_inner().unwrap();
        (r.map(|(at, _)| at.normalized().to_string()), sent)
    }

    #[tokio::test]
    async fn a_redirect_loop_ends_after_five() {
        // A page that redirects to itself: the request and five redirects
        // are sent, then the chain is cut, as gone out (charged once).
        let c = CancelToken::new();
        let (r, sent) = run("https://loop.example/", &any, &c, |_| {
            Ok(redirect("https://loop.example/"))
        })
        .await;
        let e = r.unwrap_err();
        assert_eq!(e.code, CODE_WEB_UNAVAILABLE);
        assert!(e.went_out);
        assert!(e.message.contains("more than 5"), "{}", e.message);
        assert_eq!(sent.len(), 6);
        // A → B → A ends the same way.
        let (r, sent) = run("https://a.example/", &any, &c, |u| {
            Ok(redirect(if u.host() == "a.example" {
                "https://b.example/"
            } else {
                "https://a.example/"
            }))
        })
        .await;
        assert_eq!(r.unwrap_err().code, CODE_WEB_UNAVAILABLE);
        assert_eq!(sent.len(), 6);
        // Five redirects are followed.
        let (r, sent) = run("https://h.example/0", &any, &c, |u| {
            let n: u32 = u.as_url().path()[1..].parse().unwrap();
            Ok(if n < 5 {
                redirect(&format!("/{}", n + 1))
            } else {
                ok()
            })
        })
        .await;
        assert_eq!(r.unwrap(), "https://h.example/5");
        assert_eq!(sent.len(), 6);
    }

    #[tokio::test]
    async fn a_redirect_is_resolved_checked_and_followed() {
        let c = CancelToken::new();
        let hops = Mutex::new(Vec::new());
        let record = |u: &PublicUrl| {
            hops.lock().unwrap().push(u.normalized().to_string());
            Ok(())
        };
        // A relative Location is resolved against the URL it came from.
        let (r, sent) = run("https://a.example/dir/page", &record, &c, |u| {
            Ok(if u.as_url().path() == "/dir/page" {
                redirect("next?x=1")
            } else {
                ok()
            })
        })
        .await;
        assert_eq!(r.unwrap(), "https://a.example/dir/next?x=1");
        assert_eq!(
            sent,
            [
                "https://a.example/dir/page",
                "https://a.example/dir/next?x=1"
            ]
        );
        assert_eq!(*hops.lock().unwrap(), ["https://a.example/dir/next?x=1"]);
        // A 3xx without a Location is the page.
        let (r, _) = run("https://a.example/", &any, &c, |_| {
            Ok(Reply {
                status: 304,
                location: None,
                body: (),
            })
        })
        .await;
        assert_eq!(r.unwrap(), "https://a.example/");
        // A Location on a status that is no redirect is not followed.
        let (r, sent) = run("https://a.example/", &any, &c, |_| {
            Ok(Reply {
                status: 201,
                location: Some("https://b.example/".into()),
                body: (),
            })
        })
        .await;
        assert_eq!(
            (r.unwrap(), sent.len()),
            ("https://a.example/".to_string(), 1)
        );
    }

    #[tokio::test]
    async fn a_refused_or_broken_redirect_went_out() {
        let c = CancelToken::new();
        // https to http.
        let (r, sent) = run("https://a.example/", &any, &c, |_| {
            Ok(redirect("http://a.example/"))
        })
        .await;
        let e = r.unwrap_err();
        assert_eq!(
            (e.code, e.went_out, sent.len()),
            (CODE_URL_NOT_ALLOWED, true, 1)
        );
        // A Location that is no URL.
        let (r, _) = run("https://a.example/", &any, &c, |_| {
            Ok(redirect("http://[::1"))
        })
        .await;
        let e = r.unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_UNAVAILABLE, true));
        // A private host, refused by the URL check itself.
        let (r, _) = run("https://a.example/", &any, &c, |_| {
            Ok(redirect("https://127.0.0.1/admin"))
        })
        .await;
        let e = r.unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_URL_NOT_ALLOWED, true));
        // The hop check's refusal.
        let refuse = |_: &PublicUrl| Err(WebError::new(CODE_URL_NOT_ALLOWED, "not here", false));
        let (r, sent) = run("https://a.example/", &refuse, &c, |_| {
            Ok(redirect("https://b.example/"))
        })
        .await;
        let e = r.unwrap_err();
        assert_eq!(
            (e.code, e.went_out, sent.len()),
            (CODE_URL_NOT_ALLOWED, true, 1)
        );
        assert_eq!(e.message, "not here");
        // A request that failed went out.
        let (r, _) = run("https://a.example/", &any, &c, |_| {
            Err(WebError::new(CODE_WEB_UNAVAILABLE, "reset", true))
        })
        .await;
        assert!(r.unwrap_err().went_out);
    }

    #[tokio::test]
    async fn a_cancelled_fetch_sends_nothing() {
        let c = CancelToken::new();
        c.cancel();
        let (r, sent) = run("https://a.example/", &any, &c, |_| Ok(ok())).await;
        let e = r.unwrap_err();
        assert_eq!((e.code, e.went_out, sent.len()), (CODE_CANCELLED, false, 0));
    }

    fn page_of(b: Body) -> FetchedPage {
        match b {
            Body::Page(p) => p,
            Body::Convert(c) => panic!("a conversion: {c:?}"),
        }
    }

    #[test]
    fn a_body_is_passed_pretty_printed_or_converted() {
        let u = url("https://a.example/docs/");
        // Markdown as it came, its links read from it.
        let p = page_of(
            read_body(
                &u,
                200,
                "text/markdown",
                b"# T\n[x](/x)".to_vec(),
                Format::Auto,
            )
            .unwrap(),
        );
        assert_eq!(p.text, "# T\n[x](/x)");
        assert_eq!(p.links, ["https://a.example/x"]);
        assert_eq!(
            (p.final_url.as_str(), p.content_type.as_str()),
            ("https://a.example/docs/", "text/markdown")
        );
        // JSON pretty-printed in Auto, as it came in Raw.
        let p = page_of(
            read_body(
                &u,
                200,
                "application/json",
                br#"{"a":1}"#.to_vec(),
                Format::Auto,
            )
            .unwrap(),
        );
        assert_eq!(p.text, "{\n  \"a\": 1\n}");
        let p = page_of(
            read_body(
                &u,
                200,
                "application/json",
                br#"{"a":1}"#.to_vec(),
                Format::Raw,
            )
            .unwrap(),
        );
        assert_eq!(p.text, r#"{"a":1}"#);
        // Text decoded by its charset.
        let p = page_of(
            read_body(
                &u,
                200,
                "text/plain; charset=iso-8859-1",
                vec![0x63, 0x61, 0x66, 0xe9],
                Format::Auto,
            )
            .unwrap(),
        );
        assert_eq!(p.text, "café");
        // HTML goes to toMarkdown as UTF-8, its links read from the HTML.
        let html = "<title>T</title><a href=\"page2\">next</a>";
        let Body::Convert(c) = read_body(
            &u,
            200,
            "text/html; charset=utf-8",
            html.as_bytes().to_vec(),
            Format::Auto,
        )
        .unwrap() else {
            panic!("HTML is converted")
        };
        assert_eq!(
            (c.name.as_str(), c.mime.as_str()),
            ("page.html", "text/html; charset=utf-8")
        );
        assert_eq!(c.bytes, html.as_bytes());
        let p = c
            .finish("# T\n\n[next](https://other.example/)".into())
            .unwrap();
        assert_eq!(p.text, "# T\n\n[next](https://other.example/)");
        assert_eq!(
            p.links,
            ["https://a.example/docs/page2"],
            "the HTML's links"
        );
        // HTML in Raw is the HTML.
        let p = page_of(
            read_body(&u, 200, "text/html", html.as_bytes().to_vec(), Format::Raw).unwrap(),
        );
        assert_eq!(p.text, html);
        // A PDF is converted under its extension, its links read from the
        // Markdown.
        let Body::Convert(c) = read_body(
            &u,
            200,
            "application/pdf",
            b"%PDF-1.7".to_vec(),
            Format::Auto,
        )
        .unwrap() else {
            panic!("a PDF is converted")
        };
        assert_eq!(
            (c.name.as_str(), c.mime.as_str()),
            ("document.pdf", "application/pdf")
        );
        let p = c.finish("see https://b.example/r".into()).unwrap();
        assert_eq!(p.links, ["https://b.example/r"]);
        // A body served as bytes is sniffed.
        let p = page_of(
            read_body(
                &u,
                200,
                "application/octet-stream",
                b"plain words".to_vec(),
                Format::Auto,
            )
            .unwrap(),
        );
        assert_eq!(
            (p.content_type.as_str(), p.text.as_str()),
            ("text/plain", "plain words")
        );
    }

    #[test]
    fn binary_bodies_and_unreadable_errors_are_refused() {
        let u = url("https://a.example/");
        for (ct, format) in [
            ("image/png", Format::Auto),
            ("audio/ogg", Format::Auto),
            ("video/webm", Format::Auto),
            ("application/zip", Format::Raw),
            ("application/pdf", Format::Raw),
        ] {
            let e = read_body(&u, 200, ct, b"\x89PNG".to_vec(), format).unwrap_err();
            assert_eq!(e.code, CODE_WEB_UNSUPPORTED_CONTENT_TYPE, "{ct}");
            assert!(e.went_out);
        }
        // A readable error page is a page with its status.
        let p =
            page_of(read_body(&u, 404, "text/plain", b"Not here".to_vec(), Format::Auto).unwrap());
        assert_eq!((p.status, p.text.as_str()), (404, "Not here"));
        // One with nothing to read is unavailable; a server error may pass.
        let e = read_body(&u, 503, "text/html", Vec::new(), Format::Auto).unwrap_err();
        assert_eq!(
            (e.code, e.retryable, e.went_out),
            (CODE_WEB_UNAVAILABLE, true, true)
        );
        let e = read_body(&u, 403, "image/png", b"x".to_vec(), Format::Auto).unwrap_err();
        assert_eq!((e.code, e.retryable), (CODE_WEB_UNAVAILABLE, false));
        let Body::Convert(c) =
            read_body(&u, 500, "text/html", b"<p></p>".to_vec(), Format::Auto).unwrap()
        else {
            panic!("converted")
        };
        assert_eq!(
            c.finish(" \n".into()).unwrap_err().code,
            CODE_WEB_UNAVAILABLE
        );
    }

    #[test]
    fn a_challenge_is_blocked_and_a_long_body_too_large() {
        let u = url("https://a.example/");
        let limits = FetchLimits::default();
        assert!(check_head(&u, 200, Some(5_000_000), None, &limits).is_ok());
        let e = check_head(&u, 403, None, Some("challenge"), &limits).unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_BLOCKED, true));
        let e = check_head(&u, 200, Some(5_000_001), None, &limits).unwrap_err();
        assert_eq!((e.code, e.went_out), (CODE_WEB_TOO_LARGE, true));
        assert!(check_head(&u, 200, None, Some("managed"), &limits).is_ok());
    }

    #[tokio::test]
    async fn fixtures_replace_the_test_source() {
        let w = FixtureWeb::default();
        let c = CancelToken::new();
        let q = SearchQuery {
            query: "rust".into(),
            limit: 5,
            provider: Provider::Ceramic,
            gateway: "default".into(),
            byok_alias: None,
        };
        assert_eq!(
            w.search(&q, &c).await.unwrap_err().code,
            CODE_WEB_UNAVAILABLE
        );
        w.load(&json!({
            "searches": {"rust": [{"url": "https://rust-lang.org/"}]},
            "pages": {"https://rust-lang.org/": {"status": 200, "content_type": "text/html", "text": "# Rust"}}
        }))
        .unwrap();
        assert_eq!(
            w.search(&q, &c).await.unwrap().hits[0].url,
            "https://rust-lang.org/"
        );
        let p = w
            .fetch(
                &url("https://rust-lang.org/"),
                Format::Auto,
                &FetchLimits::default(),
                &any,
                &c,
            )
            .await
            .unwrap();
        assert_eq!(p.text, "# Rust");
        assert_eq!(w.calls().len(), 2);
        // A bad fixture leaves the source as it was.
        assert!(w.load(&json!({"pages": []})).is_err());
        assert_eq!(w.calls().len(), 2);
        // A load replaces it whole.
        w.load(&json!({})).unwrap();
        assert!(w.calls().is_empty());
        assert_eq!(
            w.search(&q, &c).await.unwrap_err().code,
            CODE_WEB_UNAVAILABLE
        );
    }
}
