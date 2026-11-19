//! One browser test, from an open browser to its close (spec §4.3, §5):
//! [`run_test`] drives a [`Cdp`] connection through the sequence below
//! and answers a [`TestRun`].
//!
//! # The sequence
//!
//! 1. `Target.createBrowserContext {disposeOnDetach: true}`: incognito.
//! 2. `Target.createTarget {url: "about:blank", browserContextId}`, then
//!    `Target.attachToTarget {targetId, flatten: true}`: every later page
//!    command carries the page's `sessionId`.
//! 3. On the page: `Fetch.enable` (every request paused at the Request
//!    stage), `Runtime.enable`, `Log.enable`, `Page.enable`,
//!    `Network.enable`, `Network.setBypassServiceWorker {bypass: true}`,
//!    `Emulation.setDeviceMetricsOverride` (and
//!    `Emulation.setTouchEmulationEnabled` for a mobile viewport), and
//!    `Page.addScriptToEvaluateOnNewDocument` with the preamble
//!    ([`preamble_js`]).
//! 4. `Page.navigate`, then `Page.loadEventFired`, then settle: every
//!    paused request is answered as it comes, so none is outstanding, and
//!    two animation frames pass ([`SETTLE_JS`]).
//! 5. The steps: an action that fails ends the list (the rest come back
//!    skipped); an assertion that fails is recorded and the list goes on.
//!    Then, with `tests`, the target's saved tests
//!    ([`RequestPolicy::saved_tests`], at most [`MAX_TEST_FILES`]) through
//!    the bridge's hook ([`saved_tests_js`]): `window.claxTest.run`, which
//!    the bridge defines in a top-level window the preamble named
//!    `clax-test` (`web/bridge/src/bridge.ts`), each test at most
//!    [`TEST_MS`] and all of them within the call's time left. A page with
//!    no hook (no Clax bridge) answers one failure saying so; any failed
//!    test fails the result.
//! 6. Collect: title, final URL, `html` and `text` when asked, and the
//!    content security policy refusals the preamble recorded (listed as
//!    blocked, why `csp`).
//! 7. `Target.disposeBrowserContext`, `Browser.close`, and the transport
//!    closed: on every path out, the deadline and cancellation included
//!    (sent without waiting for answers once the connection's waits have
//!    ended).
//!
//! While anything waits, every `Fetch.requestPaused` is answered by the
//! [`RequestPolicy`] (a test's: [`TestPolicy`], the synthetic origin of
//! [`super::serve`]), and the console (`Runtime.consoleAPICalled`,
//! `Log.entryAdded`), uncaught errors (`Runtime.exceptionThrown`) and
//! failed loads (`Network.loadingFailed`) are gathered.
//!
//! # Endings
//!
//! The deadline, a cancellation or a closed connection before the page
//! loaded is an error ([`TestRun::outcome`]): `browser_timeout`,
//! `cancelled` or `browser_unavailable`, each with `opened` set. After the
//! page loaded, none of them is: the result comes back with what was
//! gathered, the remaining steps skipped, `passed: false` and `"time"` in
//! `truncated` (a closed connection adds an error instead). A page that
//! threw or an assertion that failed is an answer, not an error.
//!
//! # Caps
//!
//! The console keeps [`MAX_CONSOLE`] entries of [`MAX_CONSOLE_TEXT`]
//! characters, errors [`MAX_ERRORS`], each request list
//! [`MAX_REQUESTS`], `html` [`MAX_HTML`] characters and `text`
//! [`MAX_TEXT_CHARS`]; what was cut is named in `truncated`. Each
//! screenshot is taken scaled so its longer side is at most the run's
//! `image_side` ([`SessionLimits`]): `Page.captureScreenshot`'s `clip`
//! carries the scale, so no image is decoded here.

use super::cdp::{Cdp, CdpEvent, CdpFail, Got};
use super::opts::{
    BrowserOpts, Expect, MAX_EVAL_BYTES, MAX_TEST_FILES, Mode, SETTLE_JS, Step, Tests, click_js,
    eval_js, expect_js, focus_js, scroll_js, wait_js,
};
use super::serve::{Decision, Origin, Paused, Site};
use super::{BrowserError, BrowserSource, CODE_BROWSER_TIMEOUT, CODE_BROWSER_UNAVAILABLE};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use clax_bot_sandbox::{CODE_CANCELLED, MaybeSendSync};
use clax_core::working::Clock;
use clax_store::Store;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The most console entries a result keeps.
pub const MAX_CONSOLE: usize = 100;
/// The most characters of one console entry.
pub const MAX_CONSOLE_TEXT: usize = 1_000;
/// The most uncaught errors a result keeps.
pub const MAX_ERRORS: usize = 50;
/// The most entries of each request list.
pub const MAX_REQUESTS: usize = 100;
/// The most characters of `html`.
pub const MAX_HTML: usize = 100_000;
/// The most characters of `text`.
pub const MAX_TEXT_CHARS: usize = 20_000;
/// The longest one saved test may take, in milliseconds.
pub const TEST_MS: u32 = 5_000;
/// The most failures a result's `tests` keeps (as the bridge's harness).
const MAX_TEST_FAILURES: usize = 50;
/// The most characters of a saved test failure's name or error, of the
/// page's title, and of its URL: each comes from the page's own code.
const MAX_TEST_TEXT: usize = 1_000;
const MAX_TITLE: usize = 1_000;
const MAX_URL: usize = 10_000;

/// What answers the requests a page makes.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RequestPolicy: MaybeSendSync {
    /// What to do with `req`.
    async fn decide(&self, req: &Paused) -> Decision;

    /// The saved test files `which` names, in the order they run: the
    /// paths a call named, as named; for `{tests: true}`, the target's
    /// own, which only a policy that knows the target can list (none by
    /// default).
    ///
    /// # Errors
    /// In words, when the target's files cannot be listed.
    async fn saved_tests(&self, which: &Tests) -> Result<Vec<String>, String> {
        Ok(match which {
            Tests::Paths(p) => p.clone(),
            Tests::None | Tests::All => Vec::new(),
        })
    }
}

/// A test's policy: the clax's synthetic origin ([`Site::answer`]).
pub struct TestPolicy<'a> {
    pub site: &'a Site,
    pub source: &'a dyn BrowserSource,
    pub store: &'a Store,
    pub person: &'a str,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl RequestPolicy for TestPolicy<'_> {
    async fn decide(&self, req: &Paused) -> Decision {
        self.site
            .answer(req, self.source, self.store, self.person)
            .await
    }

    /// `{tests: true}`: every `tests/<name>.test.js` of the target
    /// ([`super::serve::Pages::test_files`]).
    async fn saved_tests(&self, which: &Tests) -> Result<Vec<String>, String> {
        match which {
            Tests::None => Ok(Vec::new()),
            Tests::All => self.site.pages.test_files(self.store).await,
            Tests::Paths(p) => Ok(p.clone()),
        }
    }
}

/// Where a test starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// `test`: `path` of the clax at its synthetic origin; a `goto` is a
    /// path there too.
    Test { origin: Origin, path: String },
    /// `browse`: a URL; a `goto` is a URL.
    Browse { url: String },
}

impl Start {
    fn mode(&self) -> Mode {
        match self {
            Start::Test { .. } => Mode::Test,
            Start::Browse { .. } => Mode::Browse,
        }
    }

    fn url(&self) -> String {
        match self {
            Start::Test { origin, path } => origin.url(path),
            Start::Browse { url } => url.clone(),
        }
    }

    /// The URL a `goto` of `to` opens.
    fn goto(&self, to: &str) -> String {
        match self {
            Start::Test { origin, .. } => {
                let (path, rest) = match to.find(['?', '#']) {
                    Some(at) => to.split_at(at),
                    None => (to, ""),
                };
                format!("{}{rest}", origin.url(path))
            }
            Start::Browse { .. } => to.to_string(),
        }
    }
}

/// The run's limits a test is held to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionLimits {
    /// The longest side of a screenshot (`RunLimits::image_side`).
    pub image_side: u32,
}

/// What one test answered.
#[derive(Debug)]
pub struct TestRun {
    /// The result (spec §5.1 without `screenshots`, `cost` and `left`,
    /// which the caller adds), or the error that ended the test before
    /// its page loaded.
    pub outcome: Result<Value, BrowserError>,
    /// The screenshots taken, PNG, in order: step `screenshot`'s value is
    /// `{screenshot: <index>}` into it.
    pub shots: Vec<Vec<u8>>,
    pub opened_at: DateTime<Utc>,
    pub closed_at: DateTime<Utc>,
}

/// The script added to every document before its own (spec §6.4, §7.3):
/// for a test, `window.name = "clax-test"` in the top-level page; a
/// recorder of content security policy refusals; WebRTC removed, its
/// connectivity checks being outside both the policy and the Fetch
/// domain.
#[must_use]
pub fn preamble_js(mode: Mode) -> String {
    let name = match mode {
        Mode::Test => "if (window === window.top) window.name = 'clax-test';",
        Mode::Browse => "",
    };
    format!(
        "(() => {{ {name} const seen = []; try {{ Object.defineProperty(window, '__claxCsp', {{value: seen}}); }} catch (e) {{}} document.addEventListener('securitypolicyviolation', (e) => {{ if (seen.length < {MAX_REQUESTS}) seen.push({{url: String(e.blockedURI || ''), directive: String(e.effectiveDirective || '')}}); }}, true); for (const n of ['RTCPeerConnection', 'webkitRTCPeerConnection', 'RTCDataChannel', 'RTCSessionDescription', 'RTCIceCandidate', 'RTCRtpSender', 'RTCRtpReceiver']) {{ try {{ Object.defineProperty(window, n, {{value: undefined, writable: false, configurable: false}}); }} catch (e) {{}} }} }})();"
    )
}

/// What runs the saved tests `files` through the bridge's hook, each for
/// at most `test_ms`, all within `total_ms`: `window.claxTest.run`'s
/// promise of the harness's `TestResults`, or `null` on a page without
/// the hook. The bridge's side is pinned by
/// `web/bridge/test/bridge-clax-test.test.ts`.
#[must_use]
pub fn saved_tests_js(files: &[String], test_ms: u32, total_ms: u64) -> String {
    let order = json!({"files": files, "testMs": test_ms, "totalMs": total_ms});
    format!(
        "(() => {{ const t = window.claxTest; return t && typeof t.run === 'function' ? t.run({order}) : null; }})()"
    )
}

/// A saved tests' result of one failure, named `tests`, saying `why`.
fn tests_failed(why: &str) -> Value {
    json!({"passed": 0, "failed": [{"name": "tests", "error": why}], "skipped": 0})
}

/// The hook's answer `v` as the result's `tests`: whole numbers, and each
/// failure's name and error as text, at most [`MAX_TEST_FAILURES`] of
/// them.
fn tests_of(v: &Value) -> Value {
    let (Some(passed), Some(failed), Some(skipped)) = (
        v["passed"].as_u64(),
        v["failed"].as_array(),
        v["skipped"].as_u64(),
    ) else {
        return tests_failed("the bridge answered no test results");
    };
    let text = |x: &Value| {
        let s = x.as_str().map_or_else(|| x.to_string(), str::to_string);
        cut_chars(&s, MAX_TEST_TEXT).0
    };
    let failed: Vec<Value> = failed
        .iter()
        .take(MAX_TEST_FAILURES)
        .map(|f| json!({"name": text(&f["name"]), "error": text(&f["error"])}))
        .collect();
    json!({"passed": passed, "failed": failed, "skipped": skipped})
}

/// What the collect step evaluates: `html` and `text` cut in the page at
/// their caps in UTF-16 code units, never inside a surrogate pair, with
/// their whole lengths, which say whether they were cut.
fn collect_js(html: bool, text: bool) -> String {
    format!(
        "(() => {{ const cut = (s, n) => {{ const r = s.slice(0, n); return /[\\uD800-\\uDBFF]$/.test(r) ? r.slice(0, -1) : r; }}; const h = {html} ? document.documentElement.outerHTML : ''; const t = {text} && document.body ? document.body.innerText : ''; return {{title: document.title, url: location.href, csp: Array.isArray(window.__claxCsp) ? window.__claxCsp.slice() : [], html: {html} ? cut(h, {MAX_HTML}) : null, htmlLength: h.length, text: {text} && document.body ? cut(t, {MAX_TEXT_CHARS}) : null, textLength: t.length}}; }})()"
    )
}

/// Drives one test over `cdp` (see the module docs): opens `start`, runs
/// `opts`, answers each paused request by `policy`, and closes the
/// browser, measuring by `clock`.
pub async fn run_test(
    cdp: &mut Cdp,
    start: &Start,
    opts: &BrowserOpts,
    policy: &dyn RequestPolicy,
    clock: &dyn Clock,
    limits: SessionLimits,
) -> TestRun {
    let opened_at = clock.now();
    let mut d = Driver {
        cdp,
        policy,
        clock,
        opened_at,
        session: None,
        context: None,
        console: Vec::new(),
        console_errors: 0,
        errors: Vec::new(),
        uncaught: 0,
        failed: Vec::new(),
        blocked: Vec::new(),
        cut: Vec::new(),
        loaded: false,
        awaiting_document: false,
        status: 0,
        network_urls: BTreeMap::new(),
        shots: Vec::new(),
    };
    let gathered = d.drive(start, opts, limits).await;
    d.close().await;
    let closed_at = clock.now();
    let ms = (closed_at - opened_at).num_milliseconds().max(0);
    let outcome = gathered.map(|g| d.result(g, ms));
    TestRun {
        outcome,
        shots: std::mem::take(&mut d.shots),
        opened_at,
        closed_at,
    }
}

/// What a test gathered beyond the events, before the result is made.
struct Gathered {
    url: String,
    title: String,
    steps: Vec<Value>,
    passed: bool,
    html: Option<String>,
    text: Option<String>,
    /// The saved tests' results, when the call asked for them.
    tests: Option<Value>,
}

/// One test's state while it runs.
struct Driver<'a> {
    cdp: &'a mut Cdp,
    policy: &'a dyn RequestPolicy,
    clock: &'a dyn Clock,
    opened_at: DateTime<Utc>,
    session: Option<String>,
    context: Option<String>,
    console: Vec<Value>,
    console_errors: usize,
    errors: Vec<Value>,
    uncaught: usize,
    failed: Vec<Value>,
    blocked: Vec<Value>,
    /// What was cut, by name.
    cut: Vec<&'static str>,
    loaded: bool,
    /// Whether the next `Document` request is the navigation's.
    awaiting_document: bool,
    status: u16,
    /// The URLs of the network's requests, to name a failed load.
    network_urls: BTreeMap<String, String>,
    shots: Vec<Vec<u8>>,
}

/// The most network request URLs kept to name failed loads.
const MAX_NETWORK_URLS: usize = 10_000;

/// The order `truncated` names what was cut in.
const CUT_ORDER: [&str; 6] = ["console", "errors", "html", "text", "requests", "time"];

/// `s` cut to `max` characters, and whether it was.
fn cut_chars(s: &str, max: usize) -> (String, bool) {
    match s.char_indices().nth(max) {
        Some((at, _)) => (s[..at].to_string(), true),
        None => (s.to_string(), false),
    }
}

/// The text of one console argument: its value as written, else its
/// description.
fn arg_text(arg: &Value) -> String {
    match arg.get("value") {
        Some(Value::String(s)) => s.clone(),
        Some(v) if !v.is_null() => v.to_string(),
        _ => arg
            .get("description")
            .or_else(|| arg.get("unserializableValue"))
            .and_then(Value::as_str)
            .map_or_else(
                || {
                    arg.get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                },
                str::to_string,
            ),
    }
}

/// A failure before the page loaded, as the call's error.
fn setup_error(fail: CdpFail, timeout_s: u32) -> BrowserError {
    let e = match fail {
        CdpFail::Deadline => BrowserError::new(
            CODE_BROWSER_TIMEOUT,
            format!("the page did not load within {timeout_s} s"),
            false,
        ),
        CdpFail::Cancelled => BrowserError::new(CODE_CANCELLED, "the run was cancelled", false),
        CdpFail::Closed => BrowserError::new(
            CODE_BROWSER_UNAVAILABLE,
            "the browser closed its connection before the page loaded",
            true,
        ),
        CdpFail::Protocol(m) => BrowserError::new(
            CODE_BROWSER_UNAVAILABLE,
            format!("the browser refused a command before the page loaded: {m}"),
            false,
        ),
        CdpFail::Transport(e) => e,
    };
    e.opened(true)
}

/// What a step answered: its value, or why it failed.
type StepAnswer = Result<Option<Value>, String>;

impl Driver<'_> {
    fn cut(&mut self, what: &'static str) {
        if !self.cut.contains(&what) {
            self.cut.push(what);
        }
    }

    fn at_ms(&self) -> i64 {
        (self.clock.now() - self.opened_at)
            .num_milliseconds()
            .max(0)
    }

    fn push_console(&mut self, level: &str, text: &str) {
        if self.console.len() >= MAX_CONSOLE {
            self.cut("console");
            return;
        }
        let (text, was_cut) = cut_chars(text, MAX_CONSOLE_TEXT);
        if was_cut {
            self.cut("console");
        }
        let at_ms = self.at_ms();
        self.console
            .push(json!({"level": level, "text": text, "at_ms": at_ms}));
    }

    fn push_request(&mut self, blocked: bool, entry: Value) {
        let list = if blocked {
            &mut self.blocked
        } else {
            &mut self.failed
        };
        if list.len() >= MAX_REQUESTS {
            self.cut("requests");
        } else {
            list.push(entry);
        }
    }

    fn push_error(&mut self, description: &str) {
        let message = description.lines().next().unwrap_or_default().to_string();
        let mut e = json!({"message": message});
        if description.contains('\n') {
            e["stack"] = json!(description);
        }
        if self.errors.len() >= MAX_ERRORS {
            self.cut("errors");
        } else {
            self.errors.push(e);
        }
    }

    /// Takes in one event (see the module docs).
    async fn handle(&mut self, ev: CdpEvent) -> Result<(), CdpFail> {
        let p = &ev.params;
        match ev.method.as_str() {
            "Fetch.requestPaused" => {
                let Some(req) = Paused::of(p) else {
                    return Ok(());
                };
                // The host's answer (its API, the store) is held to the
                // test's deadline and the run's cancellation.
                let policy = self.policy;
                let decision = self.cdp.bounded(policy.decide(&req)).await?;
                let document = req.resource_type == "Document" && self.awaiting_document;
                if document {
                    self.awaiting_document = false;
                }
                match decision {
                    Decision::Fulfill(served) => {
                        if document {
                            self.status = served.status;
                        }
                        if served.status >= 400 {
                            self.push_request(
                                false,
                                json!({"url": req.url, "status": served.status}),
                            );
                        }
                        let headers: Vec<Value> = served
                            .headers
                            .iter()
                            .map(|(k, v)| json!({"name": k, "value": v}))
                            .collect();
                        let session = self.session.clone();
                        self.cdp
                            .send_only(
                                session.as_deref(),
                                "Fetch.fulfillRequest",
                                json!({
                                    "requestId": req.request_id,
                                    "responseCode": served.status,
                                    "responseHeaders": headers,
                                    "body": base64::engine::general_purpose::STANDARD.encode(&served.body),
                                }),
                            )
                            .await?;
                    }
                    Decision::Fail { why } => {
                        if document {
                            self.status = 0;
                        }
                        self.push_request(true, json!({"url": req.url, "why": why}));
                        let session = self.session.clone();
                        self.cdp
                            .send_only(
                                session.as_deref(),
                                "Fetch.failRequest",
                                json!({"requestId": req.request_id, "errorReason": "BlockedByClient"}),
                            )
                            .await?;
                    }
                }
            }
            "Runtime.consoleAPICalled" => {
                let level = p["type"].as_str().unwrap_or("log").to_string();
                if level == "error" {
                    self.console_errors += 1;
                }
                let text = p["args"]
                    .as_array()
                    .map(|a| a.iter().map(arg_text).collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                self.push_console(&level, &text);
            }
            "Log.entryAdded" => {
                let level = p["entry"]["level"].as_str().unwrap_or("info").to_string();
                let text = p["entry"]["text"].as_str().unwrap_or_default().to_string();
                self.push_console(&level, &text);
            }
            "Runtime.exceptionThrown" => {
                self.uncaught += 1;
                let d = &p["exceptionDetails"];
                let description = d["exception"]["description"]
                    .as_str()
                    .or_else(|| d["text"].as_str())
                    .unwrap_or("an uncaught error")
                    .to_string();
                self.push_error(&description);
            }
            "Network.requestWillBeSent" => {
                if let (Some(id), Some(url)) =
                    (p["requestId"].as_str(), p["request"]["url"].as_str())
                    && self.network_urls.len() < MAX_NETWORK_URLS
                {
                    self.network_urls.insert(id.to_string(), url.to_string());
                }
            }
            "Network.loadingFailed" => {
                let error = p["errorText"].as_str().unwrap_or_default().to_string();
                // A request this test blocked is listed as blocked.
                if p.get("blockedReason").is_some() || error.contains("BLOCKED_BY_CLIENT") {
                    return Ok(());
                }
                let url = p["requestId"]
                    .as_str()
                    .and_then(|id| self.network_urls.get(id))
                    .cloned()
                    .unwrap_or_default();
                self.push_request(false, json!({"url": url, "error": error}));
            }
            "Page.loadEventFired" => self.loaded = true,
            // A dialog holds the page's main thread until it is answered:
            // it is accepted at once and noted in the console.
            "Page.javascriptDialogOpening" => {
                let kind = p["type"].as_str().unwrap_or("alert").to_string();
                let message = p["message"].as_str().unwrap_or_default().to_string();
                self.push_console("info", &format!("{kind} dialog, accepted: {message}"));
                let session = ev.session.clone().or_else(|| self.session.clone());
                self.cdp
                    .send_only(
                        session.as_deref(),
                        "Page.handleJavaScriptDialog",
                        json!({"accept": true, "promptText": ""}),
                    )
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// A command on `session`, answering each event that comes before
    /// its answer.
    async fn call_on(
        &mut self,
        session: Option<String>,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpFail> {
        let id = self.cdp.send(session.as_deref(), method, params).await?;
        loop {
            match self.cdp.answer_or_event(id).await? {
                Got::Answer(a) => return a,
                Got::Event(e) => self.handle(e).await?,
            }
        }
    }

    async fn browser_call(&mut self, method: &str, params: Value) -> Result<Value, CdpFail> {
        self.call_on(None, method, params).await
    }

    async fn page_call(&mut self, method: &str, params: Value) -> Result<Value, CdpFail> {
        let session = self.session.clone();
        self.call_on(session, method, params).await
    }

    /// Evaluates `expression` in the page, awaiting a promise: its value,
    /// or what it threw.
    async fn evaluate(&mut self, expression: &str) -> Result<Result<Value, String>, CdpFail> {
        let r = self
            .page_call(
                "Runtime.evaluate",
                json!({"expression": expression, "awaitPromise": true, "returnByValue": true}),
            )
            .await?;
        if let Some(d) = r.get("exceptionDetails") {
            let what = d["exception"]["description"]
                .as_str()
                .or_else(|| d["text"].as_str())
                .unwrap_or("the expression threw");
            return Ok(Err(what.to_string()));
        }
        Ok(Ok(r["result"].get("value").cloned().unwrap_or(Value::Null)))
    }

    /// Opens `url` and waits for its load: `Err` with the browser's
    /// error text when the navigation failed.
    async fn navigate(&mut self, url: &str) -> Result<Result<(), String>, CdpFail> {
        self.loaded = false;
        self.awaiting_document = true;
        let session = self.session.clone();
        let id = self
            .cdp
            .send(session.as_deref(), "Page.navigate", json!({"url": url}))
            .await?;
        loop {
            match self.cdp.answer_or_event(id).await? {
                Got::Answer(Ok(a)) => {
                    if let Some(e) = a.get("errorText").and_then(Value::as_str) {
                        self.awaiting_document = false;
                        return Ok(Err(e.to_string()));
                    }
                    // A same-document navigation (a new fragment) answers
                    // no loaderId and fires no load event.
                    if a.get("loaderId").is_none() {
                        self.awaiting_document = false;
                        self.loaded = true;
                    }
                    break;
                }
                Got::Answer(Err(CdpFail::Protocol(m))) => return Ok(Err(m)),
                Got::Answer(Err(e)) => return Err(e),
                Got::Event(e) => self.handle(e).await?,
            }
        }
        while !self.loaded {
            let e = self.cdp.next_event().await?;
            self.handle(e).await?;
        }
        Ok(Ok(()))
    }

    /// Two animation frames after the load: every paused request was
    /// answered as it came.
    async fn settle(&mut self) -> Result<(), CdpFail> {
        self.evaluate(SETTLE_JS).await.map(|_| ())
    }

    /// Steps 1 to 6 of the sequence: the result's parts, or the error
    /// that ended the test before its page loaded.
    async fn drive(
        &mut self,
        start: &Start,
        opts: &BrowserOpts,
        limits: SessionLimits,
    ) -> Result<Gathered, BrowserError> {
        let loaded = self
            .open(start, opts)
            .await
            .map_err(|f| setup_error(f, opts.timeout_s))?;
        let mut steps = Vec::with_capacity(opts.steps.len());
        let mut passed = loaded.is_ok();
        let mut stopped = false;
        if let Err(e) = &loaded {
            self.push_error(&format!("the page did not load: {e}"));
            stopped = true;
        } else if let Err(fail) = self.settle().await {
            // The page loaded: what ends the settle ends the test with
            // what was gathered, as during a step.
            passed = false;
            stopped = true;
            self.end(&fail);
        }
        for step in &opts.steps {
            if stopped {
                steps.push(json!({"ok": false, "skipped": true}));
                continue;
            }
            match self.step(step, start, opts, limits).await {
                Ok(Ok(v)) => {
                    let mut r = json!({"ok": true});
                    if let Some(v) = v {
                        r["value"] = v;
                    }
                    steps.push(r);
                }
                Ok(Err(error)) => {
                    passed = false;
                    steps.push(json!({"ok": false, "error": error}));
                    stopped = !step.is_assertion();
                }
                Err(fail) => {
                    passed = false;
                    stopped = true;
                    steps.push(json!({"ok": false, "error": fail.to_string()}));
                    self.end(&fail);
                }
            }
        }
        let mut g = Gathered {
            url: start.url(),
            title: String::new(),
            steps,
            passed,
            html: None,
            text: None,
            tests: None,
        };
        if opts.tests != Tests::None && self.cdp.ended().is_none() && loaded.is_ok() {
            match self.saved_tests(opts).await {
                Ok(t) => {
                    if t["failed"].as_array().is_some_and(|f| !f.is_empty()) {
                        g.passed = false;
                    }
                    g.tests = Some(t);
                }
                Err(fail) => {
                    g.passed = false;
                    self.end(&fail);
                }
            }
        }
        if self.cdp.ended().is_none() && loaded.is_ok() {
            match self.evaluate(&collect_js(opts.html, opts.text)).await {
                Ok(Ok(v)) => self.collected(&v, &mut g),
                Ok(Err(_)) => {}
                Err(fail) => {
                    g.passed = false;
                    self.end(&fail);
                }
            }
        }
        Ok(g)
    }

    /// Runs the saved tests `opts.tests` names (see the module docs): the
    /// result's `tests`, or the failure that ended the test.
    async fn saved_tests(&mut self, opts: &BrowserOpts) -> Result<Value, CdpFail> {
        let mut files = match self.policy.saved_tests(&opts.tests).await {
            Ok(f) => f,
            Err(e) => {
                return Ok(tests_failed(&format!(
                    "the saved tests could not be listed: {e}"
                )));
            }
        };
        let all = files.len();
        files.truncate(MAX_TEST_FILES);
        let spent = u64::try_from(self.at_ms()).unwrap_or(0);
        let total_ms = (u64::from(opts.timeout_s) * 1000)
            .saturating_sub(spent)
            .max(1);
        let mut t = match self
            .evaluate(&saved_tests_js(&files, TEST_MS, total_ms))
            .await?
        {
            Ok(Value::Null) => tests_failed("the page has no Clax bridge"),
            Ok(v) => tests_of(&v),
            Err(e) => tests_failed(&format!("the saved tests did not run: {e}")),
        };
        if all > MAX_TEST_FILES
            && let Some(failed) = t["failed"].as_array_mut()
        {
            failed.push(json!({
                "name": "tests",
                "error": format!("only the first {MAX_TEST_FILES} of {all} test files ran"),
            }));
        }
        Ok(t)
    }

    /// Notes a failure that ended the test after its page loaded.
    fn end(&mut self, fail: &CdpFail) {
        match fail {
            CdpFail::Deadline | CdpFail::Cancelled => self.cut("time"),
            other => self.push_error(&other.to_string()),
        }
    }

    /// Takes the collect step's value into `g`.
    fn collected(&mut self, v: &Value, g: &mut Gathered) {
        if let Some(t) = v["title"].as_str() {
            g.title = cut_chars(t, MAX_TITLE).0;
        }
        if let Some(u) = v["url"].as_str() {
            g.url = cut_chars(u, MAX_URL).0;
        }
        for c in v["csp"].as_array().into_iter().flatten() {
            let url = c["url"].as_str().unwrap_or_default().to_string();
            self.push_request(true, json!({"url": url, "why": "csp"}));
        }
        // The page's own lengths say whether it cut; the bound is kept
        // here too, the page being the artifact's code.
        let longer = |len: &Value, max: usize| {
            len.as_u64()
                .is_some_and(|n| usize::try_from(n).map_or(true, |n| n > max))
        };
        if let Some(h) = v["html"].as_str() {
            let (h, was_cut) = cut_chars(h, MAX_HTML);
            if was_cut || longer(&v["htmlLength"], MAX_HTML) {
                self.cut("html");
            }
            g.html = Some(h);
        }
        if let Some(t) = v["text"].as_str() {
            let (t, was_cut) = cut_chars(t, MAX_TEXT_CHARS);
            if was_cut || longer(&v["textLength"], MAX_TEXT_CHARS) {
                self.cut("text");
            }
            g.text = Some(t);
        }
    }

    /// Steps 1 to 4 up to the load: the page open, or the browser's error
    /// text when its navigation failed. The settle that follows is after
    /// the load ([`Driver::drive`]).
    async fn open(
        &mut self,
        start: &Start,
        opts: &BrowserOpts,
    ) -> Result<Result<(), String>, CdpFail> {
        let c = self
            .browser_call(
                "Target.createBrowserContext",
                json!({"disposeOnDetach": true}),
            )
            .await?;
        let ctx = c["browserContextId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        self.context = Some(ctx.clone());
        let t = self
            .browser_call(
                "Target.createTarget",
                json!({"url": "about:blank", "browserContextId": ctx}),
            )
            .await?;
        let target = t["targetId"].as_str().unwrap_or_default().to_string();
        let s = self
            .browser_call(
                "Target.attachToTarget",
                json!({"targetId": target, "flatten": true}),
            )
            .await?;
        self.session = s["sessionId"].as_str().map(str::to_string);
        let vp = &opts.viewport;
        let mut setup = vec![
            (
                "Fetch.enable",
                json!({"patterns": [{"urlPattern": "*", "requestStage": "Request"}]}),
            ),
            ("Runtime.enable", json!({})),
            ("Log.enable", json!({})),
            ("Page.enable", json!({})),
            ("Network.enable", json!({})),
            ("Network.setBypassServiceWorker", json!({"bypass": true})),
            (
                "Emulation.setDeviceMetricsOverride",
                json!({"width": vp.width, "height": vp.height, "deviceScaleFactor": vp.scale, "mobile": vp.mobile}),
            ),
        ];
        if vp.mobile {
            setup.push((
                "Emulation.setTouchEmulationEnabled",
                json!({"enabled": true, "maxTouchPoints": 5}),
            ));
        }
        setup.push((
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source": preamble_js(start.mode())}),
        ));
        for (method, params) in setup {
            self.page_call(method, params).await?;
        }
        self.navigate(&start.url()).await
    }

    /// One step (spec §5.2): its answer, or the failure that ends the
    /// test. A command the browser refused fails the step.
    async fn step(
        &mut self,
        step: &Step,
        start: &Start,
        opts: &BrowserOpts,
        limits: SessionLimits,
    ) -> Result<StepAnswer, CdpFail> {
        let r = self.step_inner(step, start, opts, limits).await;
        match r {
            Err(CdpFail::Protocol(m)) => Ok(Err(format!("the browser refused it: {m}"))),
            other => other,
        }
    }

    async fn step_inner(
        &mut self,
        step: &Step,
        start: &Start,
        opts: &BrowserOpts,
        limits: SessionLimits,
    ) -> Result<StepAnswer, CdpFail> {
        Ok(match step {
            Step::Goto(to) => match self.navigate(&start.goto(to)).await? {
                Ok(()) => {
                    self.settle().await?;
                    Ok(None)
                }
                Err(e) => Err(format!("{to} did not load: {e}")),
            },
            Step::Click(sel) => match self.evaluate(&click_js(sel)).await? {
                Err(e) => Err(e),
                Ok(v) if v["found"] != true => Err(format!("no element matches {sel}")),
                Ok(v) if v["ok"] != true => {
                    let top = v["top"].as_str().unwrap_or("something");
                    if v.get("x").is_some() {
                        Err(format!("{sel} is covered at its centre by {top}"))
                    } else {
                        Err(format!("{sel} cannot be clicked: {top}"))
                    }
                }
                Ok(v) => {
                    let (x, y) = (
                        v["x"].as_f64().unwrap_or(0.0),
                        v["y"].as_f64().unwrap_or(0.0),
                    );
                    for (ty, buttons) in
                        [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 0)]
                    {
                        let mut p = json!({"type": ty, "x": x, "y": y});
                        if ty != "mouseMoved" {
                            p["button"] = json!("left");
                            p["buttons"] = json!(buttons);
                            p["clickCount"] = json!(1);
                        }
                        self.page_call("Input.dispatchMouseEvent", p).await?;
                    }
                    Ok(None)
                }
            },
            Step::Type { selector, text } => match self.evaluate(&focus_js(selector)).await? {
                Err(e) => Err(e),
                Ok(v) if v["found"] != true => Err(format!("no element matches {selector}")),
                Ok(_) => {
                    self.page_call("Input.insertText", json!({"text": text}))
                        .await?;
                    Ok(None)
                }
            },
            Step::Press(k) => {
                let mut down = json!({
                    "type": "keyDown",
                    "key": k.key,
                    "code": k.code,
                    "windowsVirtualKeyCode": k.key_code,
                });
                if let Some(t) = &k.text {
                    down["text"] = json!(t);
                }
                self.page_call("Input.dispatchKeyEvent", down).await?;
                self.page_call(
                    "Input.dispatchKeyEvent",
                    json!({"type": "keyUp", "key": k.key, "code": k.code, "windowsVirtualKeyCode": k.key_code}),
                )
                .await?;
                Ok(None)
            }
            Step::Scroll { selector, y, to } => {
                match self
                    .evaluate(&scroll_js(selector.as_deref(), *y, *to))
                    .await?
                {
                    Err(e) => Err(e),
                    Ok(v) if v["found"] != true => Err(format!(
                        "no element matches {}",
                        selector.as_deref().unwrap_or("the page")
                    )),
                    Ok(_) => Ok(None),
                }
            }
            Step::Wait(w) => match self.evaluate(&wait_js(w)).await? {
                Err(e) => Err(e),
                Ok(v) if v["ok"] == true => Ok(None),
                Ok(v) => Err(v["error"]
                    .as_str()
                    .unwrap_or("the wait ran out")
                    .to_string()),
            },
            Step::Eval(expr) => match self.evaluate(&eval_js(expr)).await? {
                Err(e) => Err(e),
                // Measured here too: the page can replace what measures it.
                Ok(v)
                    if v["tooLong"] == true
                        || v["json"].as_str().is_some_and(|j| j.len() > MAX_EVAL_BYTES) =>
                {
                    Err(format!(
                        "the value is over {} KiB as JSON",
                        MAX_EVAL_BYTES / 1024
                    ))
                }
                Ok(v) => match v["json"].as_str().map(serde_json::from_str::<Value>) {
                    Some(Ok(value)) => Ok(Some(value)),
                    _ => Err("the value cannot be written as JSON".to_string()),
                },
            },
            Step::Expect(e) => self.expect(e).await?,
            Step::ExpectNoErrors => {
                if self.uncaught == 0 && self.console_errors == 0 {
                    Ok(None)
                } else {
                    Err(format!(
                        "{} uncaught errors and {} console errors so far",
                        self.uncaught, self.console_errors
                    ))
                }
            }
            Step::Screenshot {
                full_page,
                selector,
            } => {
                self.screenshot(*full_page, selector.as_deref(), opts, limits)
                    .await?
            }
        })
    }

    async fn expect(&mut self, e: &Expect) -> Result<StepAnswer, CdpFail> {
        Ok(match self.evaluate(&expect_js(e)).await? {
            Err(e) => Err(e),
            Ok(v) if v["ok"] == true => Ok(None),
            Ok(v) => Err(v["error"]
                .as_str()
                .unwrap_or("the expectation failed")
                .to_string()),
        })
    }

    /// A screenshot of the viewport, the whole page or one element,
    /// scaled so its longer side is at most `limits.image_side`.
    async fn screenshot(
        &mut self,
        full_page: bool,
        selector: Option<&str>,
        opts: &BrowserOpts,
        limits: SessionLimits,
    ) -> Result<StepAnswer, CdpFail> {
        let m = self.page_call("Page.getLayoutMetrics", json!({})).await?;
        let num = |v: &Value| v.as_f64().unwrap_or(0.0);
        let (x, y, w, h) = if full_page {
            let c = &m["cssContentSize"];
            (0.0, 0.0, num(&c["width"]), num(&c["height"]))
        } else if let Some(sel) = selector {
            let js = format!(
                "(() => {{ const el = document.querySelector({}); if (!el) return null; el.scrollIntoView({{block: 'center', behavior: 'instant'}}); const r = el.getBoundingClientRect(); return {{x: r.left + scrollX, y: r.top + scrollY, width: r.width, height: r.height}}; }})()",
                Value::String(sel.to_string())
            );
            match self.evaluate(&js).await? {
                Err(e) => return Ok(Err(e)),
                Ok(Value::Null) => return Ok(Err(format!("no element matches {sel}"))),
                Ok(r) => (
                    num(&r["x"]),
                    num(&r["y"]),
                    num(&r["width"]),
                    num(&r["height"]),
                ),
            }
        } else {
            let v = &m["cssLayoutViewport"];
            (
                num(&v["pageX"]),
                num(&v["pageY"]),
                num(&v["clientWidth"]),
                num(&v["clientHeight"]),
            )
        };
        if w <= 0.0 || h <= 0.0 {
            return Ok(Err("there is nothing to capture (no size)".to_string()));
        }
        let longest = w.max(h) * opts.viewport.scale;
        let scale = (f64::from(limits.image_side) / longest).min(1.0);
        let scale = (scale * 10_000.0).floor() / 10_000.0;
        let shot = self
            .page_call(
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "clip": {"x": x, "y": y, "width": w, "height": h, "scale": scale},
                    "captureBeyondViewport": full_page || selector.is_some(),
                }),
            )
            .await?;
        let Some(bytes) = shot["data"]
            .as_str()
            .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
        else {
            return Ok(Err("the browser answered no image".to_string()));
        };
        self.shots.push(bytes);
        Ok(Ok(Some(json!({"screenshot": self.shots.len() - 1}))))
    }

    /// Step 7: the context disposed, the browser closed, the transport
    /// closed (see the module docs).
    async fn close(&mut self) {
        let context = self.context.take();
        if self.cdp.ended().is_none() {
            if let Some(ctx) = context {
                let _ = self
                    .browser_call(
                        "Target.disposeBrowserContext",
                        json!({"browserContextId": ctx}),
                    )
                    .await;
            }
            let _ = self.browser_call("Browser.close", json!({})).await;
        } else {
            if let Some(ctx) = context {
                let _ = self
                    .cdp
                    .send_only(
                        None,
                        "Target.disposeBrowserContext",
                        json!({"browserContextId": ctx}),
                    )
                    .await;
            }
            let _ = self.cdp.send_only(None, "Browser.close", json!({})).await;
        }
        self.cdp.close().await;
    }

    /// The result (spec §5.1 without `screenshots`, `cost` and `left`).
    fn result(&mut self, g: Gathered, ms: i64) -> Value {
        let truncated: Vec<&str> = CUT_ORDER
            .iter()
            .copied()
            .filter(|c| self.cut.contains(c))
            .collect();
        let passed = g.passed && !self.cut.contains(&"time");
        let mut r = json!({
            "passed": passed,
            "url": g.url,
            "title": g.title,
            "status": self.status,
            "steps": g.steps,
            "console": std::mem::take(&mut self.console),
            "errors": std::mem::take(&mut self.errors),
            "requests": {
                "failed": std::mem::take(&mut self.failed),
                "blocked": std::mem::take(&mut self.blocked),
            },
        });
        if let Some(h) = g.html {
            r["html"] = json!(h);
        }
        if let Some(t) = g.text {
            r["text"] = json!(t);
        }
        if let Some(t) = g.tests {
            r["tests"] = t;
        }
        r["truncated"] = json!(truncated);
        r["ms"] = json!(ms);
        r
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::browser::fake::{
        ChromeLog, FakeBrowserSource, FakeChrome, ONE_PIXEL_PNG, PageModel, event, thrown, value,
    };
    use crate::browser::opts::{Tests, parse_opts};
    use crate::browser::serve::{Pages, WHY_ORIGIN};
    use crate::browser::{Deadline, Served};
    use crate::draft::DraftEntry;
    use clax_bot_sandbox::CancelToken;
    use clax_core::ArtifactId;
    use clax_core::working::ManualClock;
    use futures::FutureExt;
    use std::sync::Arc;

    const AID: &str = "abcdefghjkmn";

    /// A policy that serves every request of the origin a 200 page and
    /// blocks every other.
    struct Serve;

    #[async_trait::async_trait]
    impl RequestPolicy for Serve {
        async fn decide(&self, req: &Paused) -> Decision {
            if req.url.contains(".clax-test.invalid/") {
                Decision::Fulfill(Served {
                    status: 200,
                    headers: vec![("Content-Type".into(), "text/html".into())],
                    body: b"<p>hi</p>".to_vec(),
                })
            } else {
                Decision::Fail { why: WHY_ORIGIN }
            }
        }
    }

    fn start() -> Start {
        Start::Test {
            origin: Origin::new(&ArtifactId::parse(AID).unwrap()),
            path: "index.html".into(),
        }
    }

    fn never() -> Deadline {
        futures::future::pending().boxed()
    }

    fn opts(v: Value) -> BrowserOpts {
        parse_opts(&v, Mode::Test, 60).unwrap()
    }

    const LIMITS: SessionLimits = SessionLimits { image_side: 8000 };

    fn clock() -> ManualClock {
        ManualClock::at("2026-11-19T10:00:00Z")
    }

    /// Runs `opts` against `model` with the [`Serve`] policy.
    async fn run(model: PageModel, o: Value) -> (TestRun, ChromeLog) {
        let chrome = FakeChrome::page(model);
        let log = chrome.log();
        let mut cdp = Cdp::new(Box::new(chrome), never(), CancelToken::new());
        let run = run_test(&mut cdp, &start(), &opts(o), &Serve, &clock(), LIMITS).await;
        (run, log)
    }

    /// The `Runtime.evaluate` answer of a page whose step scripts all
    /// succeed: a click's element is on top at (10, 20), a wait and an
    /// expect hold, the collect step answers a title.
    fn fine(params: &Value) -> Value {
        let e = params["expression"].as_str().unwrap_or_default();
        if e.contains("elementFromPoint") {
            value(json!({"found": true, "ok": true, "x": 10, "y": 20, "top": "button#go"}))
        } else if e.contains("document.activeElement") || e.contains("scrollTo(") {
            value(json!({"found": true, "focused": true}))
        } else if e.contains("__claxCsp") && e.contains("title") {
            value(
                json!({"title": "Hello", "url": "https://abcdefghjkmn.clax-test.invalid/", "csp": [], "html": null, "htmlLength": 0, "text": null, "textLength": 0}),
            )
        } else if e.contains("JSON.stringify(v)") {
            value(json!({"json": "\"Hello\""}))
        } else if e.contains("MutationObserver")
            || e.contains("return {ok: true}")
            || e.contains("setTimeout")
        {
            value(json!({"ok": true}))
        } else {
            value(json!(true))
        }
    }

    fn result(run: &TestRun) -> &Value {
        run.outcome.as_ref().expect("an answer")
    }

    #[tokio::test]
    async fn the_command_sequence_is_the_specs() {
        // The real synthetic origin: the draft's index, wrapped.
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        let src = FakeBrowserSource::new();
        let mut files = BTreeMap::new();
        files.insert(
            "index.html".to_string(),
            DraftEntry {
                content: b"<h1>Hi</h1>".to_vec(),
                content_type: "text/html".into(),
            },
        );
        let aid = ArtifactId::parse(AID).unwrap();
        let site = Site::new(&aid, Pages::Draft(files, 2));
        let policy = TestPolicy {
            site: &site,
            source: &src,
            store: &st,
            person: "p",
        };
        let chrome = FakeChrome::page(
            PageModel::new()
                .evaluate(fine)
                .requests(&[("https://cdn.example/x.js", "Script")]),
        );
        let log = chrome.log();
        let mut cdp = Cdp::new(Box::new(chrome), never(), CancelToken::new());
        let run = run_test(
            &mut cdp,
            &start(),
            &opts(json!({"viewport": "phone"})),
            &policy,
            &clock(),
            LIMITS,
        )
        .await;
        let r = result(&run);
        assert_eq!(r["passed"], true, "{r}");
        assert_eq!(r["status"], 200);
        assert_eq!(r["title"], "Hello");
        assert_eq!(
            r["requests"]["blocked"],
            json!([{"url": "https://cdn.example/x.js", "why": "origin"}])
        );
        let methods = log.methods();
        let evaluate = |m: &str| m == "Runtime.evaluate";
        let without_evaluate: Vec<&str> = methods
            .iter()
            .map(String::as_str)
            .filter(|m| !evaluate(m))
            .collect();
        assert_eq!(
            without_evaluate,
            [
                "Target.createBrowserContext",
                "Target.createTarget",
                "Target.attachToTarget",
                "Fetch.enable",
                "Runtime.enable",
                "Log.enable",
                "Page.enable",
                "Network.enable",
                "Network.setBypassServiceWorker",
                "Emulation.setDeviceMetricsOverride",
                "Emulation.setTouchEmulationEnabled",
                "Page.addScriptToEvaluateOnNewDocument",
                "Page.navigate",
                "Fetch.fulfillRequest",
                "Fetch.failRequest",
                "Target.disposeBrowserContext",
                "Browser.close",
            ]
        );
        let sent = log.commands();
        let page_cmds = sent.iter().filter(|c| c.get("sessionId").is_some()).count();
        for c in &sent {
            let m = c["method"].as_str().unwrap();
            let on_page = c.get("sessionId").is_some();
            assert_eq!(
                on_page,
                !(m.starts_with("Target.") || m.starts_with("Browser.")),
                "{m}"
            );
        }
        assert!(page_cmds > 10);
        assert_eq!(
            log.sent("Target.createBrowserContext")[0]["params"],
            json!({"disposeOnDetach": true})
        );
        assert_eq!(
            log.sent("Target.createTarget")[0]["params"],
            json!({"url": "about:blank", "browserContextId": "CTX"})
        );
        assert_eq!(
            log.sent("Target.attachToTarget")[0]["params"],
            json!({"targetId": "T", "flatten": true})
        );
        assert_eq!(
            log.sent("Fetch.enable")[0]["params"],
            json!({"patterns": [{"urlPattern": "*", "requestStage": "Request"}]})
        );
        assert_eq!(
            log.sent("Emulation.setDeviceMetricsOverride")[0]["params"],
            json!({"width": 390, "height": 844, "deviceScaleFactor": 3.0, "mobile": true})
        );
        let preamble = log.sent("Page.addScriptToEvaluateOnNewDocument")[0]["params"]["source"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(preamble.contains("window.name = 'clax-test'"));
        assert!(preamble.contains("securitypolicyviolation"));
        assert!(preamble.contains("RTCPeerConnection"));
        assert_eq!(
            log.sent("Page.navigate")[0]["params"]["url"],
            "https://abcdefghjkmn.clax-test.invalid/"
        );
        let fulfilled = &log.sent("Fetch.fulfillRequest")[0]["params"];
        assert_eq!(fulfilled["requestId"], "R1");
        assert_eq!(fulfilled["responseCode"], 200);
        let body = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            fulfilled["body"].as_str().unwrap(),
        )
        .unwrap();
        assert!(
            String::from_utf8(body)
                .unwrap()
                .contains("data-artifact=\"abcdefghjkmn\"")
        );
        assert!(
            fulfilled["responseHeaders"]
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["name"] == "Content-Security-Policy")
        );
        assert_eq!(
            log.sent("Fetch.failRequest")[0]["params"],
            json!({"requestId": "R2", "errorReason": "BlockedByClient"})
        );
        // Settling: two animation frames after the load.
        let evals: Vec<String> = log
            .sent("Runtime.evaluate")
            .iter()
            .map(|c| c["params"]["expression"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(evals[0], SETTLE_JS);
        assert_eq!(log.closes(), 1);
    }

    #[tokio::test]
    async fn a_failed_click_skips_the_rest() {
        let (run, log) = run(
            PageModel::new().evaluate(|p: &Value| {
                if p["expression"]
                    .as_str()
                    .unwrap()
                    .contains("elementFromPoint")
                {
                    value(json!({"found": false}))
                } else {
                    fine(p)
                }
            }),
            json!({"steps": [
                {"eval": "document.title"},
                {"click": "#missing"},
                {"expect": {"selector": "h1"}},
                {"press": "Enter"},
            ]}),
        )
        .await;
        let r = result(&run);
        assert_eq!(r["passed"], false);
        let steps = r["steps"].as_array().unwrap();
        assert_eq!(steps[0], json!({"ok": true, "value": "Hello"}));
        assert_eq!(steps[1]["ok"], false);
        assert!(
            steps[1]["error"].as_str().unwrap().contains("#missing"),
            "{}",
            steps[1]
        );
        for s in &steps[2..] {
            assert_eq!(s, &json!({"ok": false, "skipped": true}));
        }
        assert!(log.sent("Input.dispatchKeyEvent").is_empty());
        assert_eq!(log.sent("Browser.close").len(), 1);
    }

    #[tokio::test]
    async fn a_failed_expect_does_not_stop_the_list() {
        let (run, log) = run(
            PageModel::new().evaluate(|p: &Value| {
                let e = p["expression"].as_str().unwrap();
                if e.contains("querySelectorAll(\"li\")") {
                    value(json!({"ok": false, "error": "\"li\" matched 2, not 3"}))
                } else {
                    fine(p)
                }
            }),
            json!({"steps": [
                {"expect": {"selector": "li", "count": 3}},
                {"click": "#go"},
                {"expectNoErrors": true},
            ]}),
        )
        .await;
        let r = result(&run);
        assert_eq!(r["passed"], false);
        let steps = r["steps"].as_array().unwrap();
        assert_eq!(
            steps[0],
            json!({"ok": false, "error": "\"li\" matched 2, not 3"})
        );
        assert_eq!(steps[1], json!({"ok": true}));
        assert_eq!(steps[2], json!({"ok": true}));
        let mouse: Vec<String> = log
            .sent("Input.dispatchMouseEvent")
            .iter()
            .map(|c| c["params"]["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(mouse, ["mouseMoved", "mousePressed", "mouseReleased"]);
        let pressed = &log.sent("Input.dispatchMouseEvent")[1]["params"];
        assert_eq!(
            (pressed["x"].as_f64(), pressed["y"].as_f64()),
            (Some(10.0), Some(20.0))
        );
        assert_eq!(pressed["button"], "left");
        assert_eq!(pressed["clickCount"], 1);
    }

    #[tokio::test]
    async fn console_and_errors_are_gathered_and_capped() {
        let mut frames = Vec::new();
        frames.push(event(
            "Runtime.consoleAPICalled",
            json!({"type": "log", "args": [{"type": "string", "value": "x".repeat(1500)}]}),
            None,
        ));
        for i in 0..120 {
            frames.push(event(
                "Runtime.consoleAPICalled",
                json!({"type": "log", "args": [{"type": "string", "value": "n"}, {"type": "number", "value": i}]}),
                None,
            ));
        }
        for i in 0..60 {
            frames.push(event(
                "Runtime.exceptionThrown",
                json!({"exceptionDetails": {"text": "Uncaught", "exception": {"description": format!("TypeError: boom {i}\n    at app.js:1:1")}}}),
                None,
            ));
        }
        frames.push(event(
            "Fetch.requestPaused",
            json!({"requestId": "AWAY", "request": {"url": "https://elsewhere.example/?d=secret", "method": "GET", "headers": {}}, "resourceType": "Document", "frameId": "F"}),
            None,
        ));
        let (run, log) = run(
            PageModel::new().evaluate(fine).after_load(frames),
            json!({"steps": [{"expectNoErrors": true}]}),
        )
        .await;
        let r = result(&run);
        let console = r["console"].as_array().unwrap();
        assert_eq!(console.len(), MAX_CONSOLE);
        assert_eq!(
            console[0]["text"].as_str().unwrap().chars().count(),
            MAX_CONSOLE_TEXT
        );
        assert_eq!(console[0]["level"], "log");
        assert_eq!(console[0]["at_ms"], 0);
        assert_eq!(console[1]["text"], "n 0");
        let errors = r["errors"].as_array().unwrap();
        assert_eq!(errors.len(), MAX_ERRORS);
        assert_eq!(errors[0]["message"], "TypeError: boom 0");
        assert_eq!(errors[0]["stack"], "TypeError: boom 0\n    at app.js:1:1");
        let truncated: Vec<&str> = r["truncated"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(truncated, ["console", "errors"]);
        // An uncaught error fails expectNoErrors, and so the test.
        assert_eq!(r["steps"][0]["ok"], false);
        assert_eq!(r["passed"], false);
        // The page navigating itself away is blocked and listed; the test
        // still answered.
        assert_eq!(
            r["requests"]["blocked"],
            json!([{"url": "https://elsewhere.example/?d=secret", "why": "origin"}])
        );
        assert_eq!(
            log.sent("Fetch.failRequest")[0]["params"]["requestId"],
            "AWAY"
        );
    }

    #[tokio::test]
    async fn a_covered_button_fails_its_click_naming_what_covers_it() {
        let (run, log) = run(
            PageModel::new().evaluate(|p: &Value| {
                if p["expression"].as_str().unwrap().contains("elementFromPoint") {
                    value(json!({"found": true, "ok": false, "x": 5, "y": 5, "top": "div#cookie-banner"}))
                } else {
                    fine(p)
                }
            }),
            json!({"steps": [{"click": "#submit"}, {"eval": "1"}]}),
        )
        .await;
        let r = result(&run);
        let err = r["steps"][0]["error"].as_str().unwrap();
        assert!(
            err.contains("#submit") && err.contains("div#cookie-banner"),
            "{err}"
        );
        assert_eq!(r["steps"][1]["skipped"], true);
        assert!(log.sent("Input.dispatchMouseEvent").is_empty());
    }

    #[tokio::test]
    async fn a_full_page_screenshot_is_scaled_to_the_image_side() {
        let (run, log) = run(
            PageModel::new()
                .evaluate(fine)
                .content_size(1280.0, 20_000.0),
            json!({"steps": [{"screenshot": {"fullPage": true}}, {"screenshot": {}}]}),
        )
        .await;
        let r = result(&run);
        assert_eq!(
            r["steps"][0],
            json!({"ok": true, "value": {"screenshot": 0}})
        );
        assert_eq!(
            r["steps"][1],
            json!({"ok": true, "value": {"screenshot": 1}})
        );
        assert_eq!(run.shots, [ONE_PIXEL_PNG.to_vec(), ONE_PIXEL_PNG.to_vec()]);
        let shots = log.sent("Page.captureScreenshot");
        let full = &shots[0]["params"];
        assert_eq!(full["format"], "png");
        assert_eq!(full["captureBeyondViewport"], true);
        assert_eq!(
            full["clip"],
            json!({"x": 0.0, "y": 0.0, "width": 1280.0, "height": 20000.0, "scale": 0.4})
        );
        // The viewport fits: not scaled.
        assert_eq!(shots[1]["params"]["clip"]["scale"], 1.0);
        assert_eq!(shots[1]["params"]["clip"]["width"], 1280.0);
    }

    #[tokio::test]
    async fn the_deadline_mid_step_answers_what_was_gathered() {
        // The deadline fires while the page waits on the wait step.
        let deadline = CancelToken::new();
        let fire = deadline.clone();
        let chrome = FakeChrome::page(
            PageModel::new()
                .evaluate(fine)
                .on(move |cmd: &Value| {
                    let e = cmd["params"]["expression"].as_str().unwrap_or_default();
                    (e.contains("MutationObserver")).then(|| {
                        fire.cancel();
                        vec![]
                    })
                })
                .after_load(vec![event(
                    "Runtime.consoleAPICalled",
                    json!({"type": "error", "args": [{"type": "string", "value": "early"}]}),
                    None,
                )]),
        );
        let log = chrome.log();
        let d = deadline.clone();
        let mut cdp = Cdp::new(
            Box::new(chrome),
            async move { d.cancelled().await }.boxed(),
            CancelToken::new(),
        );
        let clock = clock();
        let run = run_test(
            &mut cdp,
            &start(),
            &opts(json!({"steps": [
                {"eval": "document.title"},
                {"wait": {"selector": ".never"}},
                {"click": "#go"},
            ]})),
            &Serve,
            &clock,
            LIMITS,
        )
        .await;
        let r = result(&run);
        assert_eq!(r["passed"], false);
        assert_eq!(r["steps"][0], json!({"ok": true, "value": "Hello"}));
        assert_eq!(r["steps"][1]["ok"], false);
        assert_eq!(r["steps"][2], json!({"ok": false, "skipped": true}));
        assert!(r["truncated"].as_array().unwrap().contains(&json!("time")));
        assert_eq!(r["console"][0]["text"], "early");
        assert_eq!(log.sent("Browser.close").len(), 1);
        assert_eq!(log.sent("Target.disposeBrowserContext").len(), 1);
        assert_eq!(log.closes(), 1);
    }

    #[tokio::test]
    async fn the_deadline_before_the_load_is_a_timeout() {
        let deadline = CancelToken::new();
        let fire = deadline.clone();
        // The page never answers its navigation.
        let chrome = FakeChrome::page(PageModel::new().on(move |cmd: &Value| {
            (cmd["method"] == "Page.navigate").then(|| {
                fire.cancel();
                vec![]
            })
        }));
        let log = chrome.log();
        let d = deadline.clone();
        let mut cdp = Cdp::new(
            Box::new(chrome),
            async move { d.cancelled().await }.boxed(),
            CancelToken::new(),
        );
        let run = run_test(
            &mut cdp,
            &start(),
            &opts(json!({})),
            &Serve,
            &clock(),
            LIMITS,
        )
        .await;
        let e = run.outcome.unwrap_err();
        assert_eq!(e.code, CODE_BROWSER_TIMEOUT);
        assert!(e.opened);
        assert_eq!(log.sent("Browser.close").len(), 1);
        assert_eq!(log.closes(), 1);
    }

    #[tokio::test]
    async fn cancel_closes_the_browser() {
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let chrome = FakeChrome::page(PageModel::new().evaluate(fine).on(move |cmd: &Value| {
            let e = cmd["params"]["expression"].as_str().unwrap_or_default();
            e.contains("setTimeout").then(|| {
                c.cancel();
                vec![]
            })
        }));
        let log = chrome.log();
        let mut cdp = Cdp::new(Box::new(chrome), never(), cancel.clone());
        let run = run_test(
            &mut cdp,
            &start(),
            &opts(json!({"steps": [{"wait": 500}, {"eval": "1"}]})),
            &Serve,
            &clock(),
            LIMITS,
        )
        .await;
        let r = result(&run);
        assert_eq!(r["passed"], false);
        assert_eq!(r["steps"][1]["skipped"], true);
        assert!(r["truncated"].as_array().unwrap().contains(&json!("time")));
        assert_eq!(log.sent("Browser.close").len(), 1);
        assert_eq!(log.closes(), 1);
        // Cancelled before the browser opened anything: an error, closed.
        let cancel = CancelToken::new();
        cancel.cancel();
        let chrome = FakeChrome::page(PageModel::new());
        let log = chrome.log();
        let mut cdp = Cdp::new(Box::new(chrome), never(), cancel);
        let run = run_test(
            &mut cdp,
            &start(),
            &opts(json!({})),
            &Serve,
            &clock(),
            LIMITS,
        )
        .await;
        assert_eq!(run.outcome.unwrap_err().code, CODE_CANCELLED);
        assert_eq!(log.sent("Browser.close").len(), 1);
        assert_eq!(log.closes(), 1);
    }

    #[tokio::test]
    async fn browser_close_is_sent_on_every_path() {
        // An error answer while setting up, a closed socket mid-test, a
        // page that throws on eval, and a clean run.
        let models: Vec<(&str, PageModel)> = vec![
            (
                "an error while setting up",
                PageModel::new().on(|cmd: &Value| {
                    (cmd["method"] == "Network.enable")
                        .then(|| vec![crate::browser::fake::error(cmd, "no network")])
                }),
            ),
            (
                "a closed socket",
                PageModel::new().evaluate(fine).on(|cmd: &Value| {
                    let e = cmd["params"]["expression"].as_str().unwrap_or_default();
                    e.contains("JSON.stringify(v)")
                        .then(|| vec![crate::browser::fake::CLOSE])
                }),
            ),
            (
                "a page that throws",
                PageModel::new().evaluate(|p: &Value| {
                    if p["expression"]
                        .as_str()
                        .unwrap()
                        .contains("JSON.stringify(v)")
                    {
                        thrown("ReferenceError: nope is not defined")
                    } else {
                        fine(p)
                    }
                }),
            ),
            ("a clean run", PageModel::new().evaluate(fine)),
        ];
        for (what, model) in models {
            let (run, log) = run(model, json!({"steps": [{"eval": "nope"}]})).await;
            // Over a socket the browser closed, nothing more can be sent.
            let closes = usize::from(what != "a closed socket");
            assert_eq!(log.sent("Browser.close").len(), closes, "{what}");
            assert_eq!(log.closes(), 1, "{what}");
            match what {
                "an error while setting up" => {
                    let e = run.outcome.unwrap_err();
                    assert_eq!(e.code, CODE_BROWSER_UNAVAILABLE, "{what}");
                    assert!(e.opened);
                }
                "a closed socket" => {
                    let r = run.outcome.unwrap();
                    assert_eq!(r["passed"], false);
                    assert!(!r["errors"].as_array().unwrap().is_empty(), "{r}");
                }
                "a page that throws" => {
                    let r = run.outcome.unwrap();
                    assert!(
                        r["steps"][0]["error"]
                            .as_str()
                            .unwrap()
                            .contains("ReferenceError"),
                        "{r}"
                    );
                }
                _ => assert_eq!(run.outcome.unwrap()["passed"], true, "{what}"),
            }
        }
    }

    /// [`Serve`]'s answers, with `files` as the target's saved tests.
    struct WithTests(Vec<String>);

    #[async_trait::async_trait]
    impl RequestPolicy for WithTests {
        async fn decide(&self, req: &Paused) -> Decision {
            Serve.decide(req).await
        }

        async fn saved_tests(&self, which: &Tests) -> Result<Vec<String>, String> {
            Ok(match which {
                Tests::None => Vec::new(),
                Tests::All => self.0.clone(),
                Tests::Paths(p) => p.clone(),
            })
        }
    }

    /// Runs `o` against a page whose step scripts succeed and whose bridge
    /// hook answers `hook` (the `Runtime.evaluate` result of the saved
    /// tests' expression), with `tests/a.test.js` and `tests/b.test.js`
    /// saved.
    async fn with_saved(o: Value, hook: Value) -> (TestRun, ChromeLog) {
        let model = PageModel::new().evaluate(move |p| {
            if p["expression"]
                .as_str()
                .unwrap_or_default()
                .contains("claxTest")
            {
                hook.clone()
            } else {
                fine(p)
            }
        });
        let chrome = FakeChrome::page(model);
        let log = chrome.log();
        let mut cdp = Cdp::new(Box::new(chrome), never(), CancelToken::new());
        let policy = WithTests(vec!["tests/a.test.js".into(), "tests/b.test.js".into()]);
        let run = run_test(&mut cdp, &start(), &opts(o), &policy, &clock(), LIMITS).await;
        (run, log)
    }

    /// The expressions the page was asked to evaluate, in order.
    fn evaluated(log: &ChromeLog) -> Vec<String> {
        log.sent("Runtime.evaluate")
            .iter()
            .map(|c| c["params"]["expression"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn saved_tests_run_through_the_bridge_hook() {
        let (run, log) = with_saved(
            json!({"steps": [{"click": "#go"}], "tests": true}),
            value(json!({"passed": 2, "failed": [], "skipped": 1})),
        )
        .await;
        let r = result(&run);
        assert_eq!(r["tests"], json!({"passed": 2, "failed": [], "skipped": 1}));
        assert_eq!(r["passed"], true, "{r}");
        // The hook is called with the target's files, 5 s a test and the
        // call's time left (the clock did not move: all 60 s of it), its
        // promise awaited and its value returned.
        let files = ["tests/a.test.js".to_string(), "tests/b.test.js".to_string()];
        let expected = saved_tests_js(&files, 5_000, 60_000);
        assert!(
            expected.contains(r#"window.claxTest"#)
                && expected.contains(
                    r#"{"files":["tests/a.test.js","tests/b.test.js"],"testMs":5000,"totalMs":60000}"#
                ),
            "{expected}"
        );
        let all = log.sent("Runtime.evaluate");
        let call = all
            .iter()
            .find(|c| c["params"]["expression"] == expected.as_str())
            .expect("the hook was called");
        assert_eq!(call["params"]["awaitPromise"], true);
        assert_eq!(call["params"]["returnByValue"], true);
        assert_eq!(call["sessionId"], "S");
        // After the steps, before the collect.
        let e = evaluated(&log);
        let at = |needle: &str| e.iter().position(|x| x.contains(needle)).unwrap();
        assert!(at("elementFromPoint") < at("claxTest"), "{e:?}");
        assert!(at("claxTest") < at("__claxCsp"), "{e:?}");
        // Named files run as named.
        let (run, log) = with_saved(
            json!({"tests": ["tests/b.test.js"]}),
            value(json!({"passed": 1, "failed": [], "skipped": 0})),
        )
        .await;
        assert_eq!(result(&run)["tests"]["passed"], 1);
        assert!(
            evaluated(&log)
                .iter()
                .any(|x| x.contains(r#""files":["tests/b.test.js"]"#))
        );
        // Without `tests`, the hook is not called and there is no `tests`.
        let (run, log) = with_saved(json!({}), value(json!(null))).await;
        assert!(result(&run).get("tests").is_none());
        assert!(!evaluated(&log).iter().any(|x| x.contains("claxTest")));
    }

    #[tokio::test]
    async fn a_failed_saved_test_fails_the_result() {
        let failed = json!({"passed": 1, "failed": [{"name": "adds a row", "error": "expected 2, got 1"}], "skipped": 0});
        let (run, _) = with_saved(json!({"tests": true}), value(failed.clone())).await;
        let r = result(&run);
        assert_eq!(r["tests"], failed);
        assert_eq!(r["passed"], false);
        // A page without the bridge has no hook: one failure says so.
        let (run, _) = with_saved(json!({"tests": true}), value(Value::Null)).await;
        let r = result(&run);
        assert_eq!(
            r["tests"],
            json!({"passed": 0, "failed": [{"name": "tests", "error": "the page has no Clax bridge"}], "skipped": 0})
        );
        assert_eq!(r["passed"], false);
        // A hook that threw: one failure with what it threw.
        let (run, _) = with_saved(json!({"tests": true}), thrown("Error: boom")).await;
        let r = result(&run);
        assert_eq!(r["tests"]["failed"][0]["name"], "tests");
        assert!(
            r["tests"]["failed"][0]["error"]
                .as_str()
                .unwrap()
                .contains("boom"),
            "{r}"
        );
        assert_eq!(r["passed"], false);
    }

    #[tokio::test]
    async fn the_deadline_during_the_settle_answers_what_was_gathered() {
        // The page loaded and logged; its settle never answers.
        let deadline = CancelToken::new();
        let fire = deadline.clone();
        let chrome = FakeChrome::page(
            PageModel::new()
                .evaluate(fine)
                .on(move |cmd: &Value| {
                    (cmd["params"]["expression"] == SETTLE_JS).then(|| {
                        fire.cancel();
                        vec![]
                    })
                })
                .after_load(vec![event(
                    "Runtime.consoleAPICalled",
                    json!({"type": "log", "args": [{"type": "string", "value": "loaded"}]}),
                    None,
                )]),
        );
        let log = chrome.log();
        let d = deadline.clone();
        let mut cdp = Cdp::new(
            Box::new(chrome),
            async move { d.cancelled().await }.boxed(),
            CancelToken::new(),
        );
        let run = run_test(
            &mut cdp,
            &start(),
            &opts(json!({"steps": [{"eval": "document.title"}]})),
            &Serve,
            &clock(),
            LIMITS,
        )
        .await;
        let r = result(&run);
        assert_eq!(r["passed"], false);
        assert_eq!(r["console"][0]["text"], "loaded");
        assert_eq!(r["steps"][0], json!({"ok": false, "skipped": true}));
        assert!(r["truncated"].as_array().unwrap().contains(&json!("time")));
        assert_eq!(log.sent("Browser.close").len(), 1);
    }

    /// A policy whose host never answers, after firing the deadline.
    struct Stalls(CancelToken);

    #[async_trait::async_trait]
    impl RequestPolicy for Stalls {
        async fn decide(&self, _req: &Paused) -> Decision {
            self.0.cancel();
            futures::future::pending().await
        }
    }

    #[tokio::test]
    async fn a_stalled_host_answer_ends_at_the_deadline() {
        let deadline = CancelToken::new();
        let chrome = FakeChrome::page(PageModel::new().evaluate(fine));
        let log = chrome.log();
        let d = deadline.clone();
        let mut cdp = Cdp::new(
            Box::new(chrome),
            async move { d.cancelled().await }.boxed(),
            CancelToken::new(),
        );
        let run = run_test(
            &mut cdp,
            &start(),
            &opts(json!({})),
            &Stalls(deadline.clone()),
            &clock(),
            LIMITS,
        )
        .await;
        let e = run.outcome.unwrap_err();
        assert_eq!(e.code, CODE_BROWSER_TIMEOUT);
        assert!(e.opened);
        assert_eq!(log.sent("Browser.close").len(), 1);
        assert_eq!(log.closes(), 1);
    }

    #[tokio::test]
    async fn what_the_page_answers_is_capped_here_too() {
        // A page that replaced what measures in it: an eval value over
        // the cap, a long title, HTML it says it cut by code units (60,000
        // emoji: 120,000 units, under 100,000 characters once cut).
        let emoji = "\u{1f600}".repeat(50_000);
        let (run, _log) = run(
            PageModel::new().evaluate(move |p: &Value| {
                let e = p["expression"].as_str().unwrap_or_default();
                if e.contains("JSON.stringify(v)") {
                    let big = format!("\"{}\"", "x".repeat(MAX_EVAL_BYTES + 1));
                    value(json!({"json": big}))
                } else if e.contains("__claxCsp") && e.contains("title") {
                    value(json!({
                        "title": "t".repeat(5_000), "url": "https://abcdefghjkmn.clax-test.invalid/",
                        "csp": [], "html": emoji, "htmlLength": 120_000, "text": null, "textLength": 0,
                    }))
                } else {
                    fine(p)
                }
            }),
            json!({"steps": [{"eval": "big"}], "html": true}),
        )
        .await;
        let r = result(&run);
        assert_eq!(r["steps"][0]["ok"], false);
        assert!(
            r["steps"][0]["error"].as_str().unwrap().contains("16 KiB"),
            "{}",
            r["steps"][0]
        );
        assert_eq!(r["title"].as_str().unwrap().chars().count(), MAX_TITLE);
        assert!(r["truncated"].as_array().unwrap().contains(&json!("html")));
    }

    #[test]
    fn a_saved_test_failure_is_cut() {
        let t = tests_of(&json!({"passed": 0, "skipped": 0, "failed": [{"name": "n".repeat(5_000), "error": "e"}]}));
        assert_eq!(
            t["failed"][0]["name"].as_str().unwrap().len(),
            MAX_TEST_TEXT
        );
    }

    #[tokio::test]
    async fn a_dialog_is_accepted_and_noted() {
        // The click's script opens a confirm(), which holds its answer
        // until the dialog is answered.
        let held = Arc::new(std::sync::Mutex::new(None::<Value>));
        let h = held.clone();
        let (run, log) = run(
            PageModel::new().evaluate(fine).on(move |cmd: &Value| {
                let e = cmd["params"]["expression"].as_str().unwrap_or_default();
                if e.contains("elementFromPoint") {
                    *h.lock().unwrap() = Some(cmd.clone());
                    return Some(vec![event(
                        "Page.javascriptDialogOpening",
                        json!({"type": "confirm", "message": "Delete?", "url": "x"}),
                        Some("S"),
                    )]);
                }
                if cmd["method"] == "Page.handleJavaScriptDialog" {
                    let click = h.lock().unwrap().take().expect("a held click");
                    return Some(vec![
                        crate::browser::fake::answer(cmd, json!({})),
                        crate::browser::fake::answer(&click, fine(&click["params"])),
                    ]);
                }
                None
            }),
            json!({"steps": [{"click": "#delete"}]}),
        )
        .await;
        let r = result(&run);
        assert_eq!(r["steps"][0], json!({"ok": true}), "{r}");
        assert_eq!(r["passed"], true);
        let sent = log.sent("Page.handleJavaScriptDialog");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["sessionId"], "S");
        assert_eq!(sent[0]["params"]["accept"], true);
        assert!(
            r["console"][0]["text"]
                .as_str()
                .unwrap()
                .contains("confirm dialog, accepted: Delete?"),
            "{r}"
        );
    }

    #[tokio::test]
    async fn a_goto_to_a_fragment_waits_for_no_load() {
        // A same-document navigation: no loaderId, no load event.
        let (run, _log) = run(
            PageModel::new().evaluate(fine).on(|cmd: &Value| {
                let url = cmd["params"]["url"].as_str().unwrap_or_default();
                (cmd["method"] == "Page.navigate" && url.contains('#')).then(|| {
                    vec![crate::browser::fake::answer(cmd, json!({"frameId": "F"}))]
                })
            }),
            json!({"steps": [{"goto": "index.html#settings"}, {"eval": "document.title"}]}),
        )
        .await;
        let r = result(&run);
        assert_eq!(r["steps"][0], json!({"ok": true}), "{r}");
        assert_eq!(r["steps"][1], json!({"ok": true, "value": "Hello"}));
        assert_eq!(r["passed"], true);
    }

    #[test]
    fn the_preamble_names_the_window_only_for_a_test() {
        assert!(preamble_js(Mode::Test).contains("window.name = 'clax-test'"));
        assert!(!preamble_js(Mode::Browse).contains("clax-test"));
        assert!(preamble_js(Mode::Browse).contains("RTCPeerConnection"));
    }
}
