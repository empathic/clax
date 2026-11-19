//! A browser in memory, for tests: [`FakeChrome`], a [`CdpTransport`]
//! answering CDP commands by a handler, with [`PageModel`], a page it
//! answers as Chrome would; and [`FakeBrowserSource`], a
//! [`BrowserSource`] that opens them, beside
//! [`crate::web::FakeWebSource`].

use super::{BrowserError, BrowserSource, CdpTransport, Deadline, Served};
use crate::web::url::OwnHosts;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use clax_bot_sandbox::CancelToken;
use futures::FutureExt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Poll, Waker};

/// The answer to `cmd` (a command frame as sent): `result`, with the
/// command's `sessionId` when it had one.
#[must_use]
pub fn answer(cmd: &Value, result: Value) -> Value {
    let mut a = json!({"id": cmd["id"], "result": result});
    if let Some(s) = cmd.get("sessionId") {
        a["sessionId"] = s.clone();
    }
    a
}

/// An error answer to `cmd`, with `message`.
#[must_use]
pub fn error(cmd: &Value, message: &str) -> Value {
    let mut a = json!({"id": cmd["id"], "error": {"code": -32000, "message": message}});
    if let Some(s) = cmd.get("sessionId") {
        a["sessionId"] = s.clone();
    }
    a
}

/// An event frame: `method` with `params`, of `session` when given.
#[must_use]
pub fn event(method: &str, params: Value, session: Option<&str>) -> Value {
    let mut e = json!({"method": method, "params": params});
    if let Some(s) = session {
        e["sessionId"] = json!(s);
    }
    e
}

/// The frame a handler returns to have the socket closed after the frames
/// before it.
pub const CLOSE: Value = Value::Null;

#[derive(Debug, Default)]
struct Shared {
    commands: Vec<Value>,
    outbox: VecDeque<String>,
    closed: bool,
    closes: u32,
    waker: Option<Waker>,
}

/// What a test keeps of a [`FakeChrome`] once it is handed over: what it
/// was sent, and a way to close its socket.
#[derive(Clone, Debug, Default)]
pub struct ChromeLog(Arc<Mutex<Shared>>);

impl ChromeLog {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every command sent, in order, as sent.
    #[must_use]
    pub fn commands(&self) -> Vec<Value> {
        self.lock().commands.clone()
    }

    /// The methods of every command sent, in order.
    #[must_use]
    pub fn methods(&self) -> Vec<String> {
        self.lock()
            .commands
            .iter()
            .map(|c| c["method"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The commands of `method`, in order.
    #[must_use]
    pub fn sent(&self, method: &str) -> Vec<Value> {
        self.lock()
            .commands
            .iter()
            .filter(|c| c["method"] == method)
            .cloned()
            .collect()
    }

    /// Closes the browser's side of the socket: once the frames already
    /// queued are read, `recv` answers `None`.
    pub fn close_socket(&self) {
        let mut s = self.lock();
        s.closed = true;
        if let Some(w) = s.waker.take() {
            w.wake();
        }
    }

    /// How many times the transport was closed by its user.
    #[must_use]
    pub fn closes(&self) -> u32 {
        self.lock().closes
    }
}

type Handler = Box<dyn FnMut(&Value) -> Vec<Value> + Send>;

/// A [`CdpTransport`] in memory: each command sent is recorded and handed
/// to its handler, whose frames (events, answers, in order; [`CLOSE`] to
/// close the socket there) are what `recv` reads next. With nothing to
/// read and the socket open, `recv` waits, as a socket does.
pub struct FakeChrome {
    log: ChromeLog,
    handler: Handler,
}

impl std::fmt::Debug for FakeChrome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeChrome").finish_non_exhaustive()
    }
}

impl FakeChrome {
    /// A browser answering each command with `handler`'s frames.
    pub fn new(handler: impl FnMut(&Value) -> Vec<Value> + Send + 'static) -> FakeChrome {
        FakeChrome {
            log: ChromeLog::default(),
            handler: Box::new(handler),
        }
    }

    /// A browser with one page answering as `model` says.
    #[must_use]
    pub fn page(model: PageModel) -> FakeChrome {
        let mut model = model;
        FakeChrome::new(move |cmd| model.answer(cmd))
    }

    /// The log of what it is sent.
    #[must_use]
    pub fn log(&self) -> ChromeLog {
        self.log.clone()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl CdpTransport for FakeChrome {
    async fn send(&mut self, frame: String) -> Result<(), BrowserError> {
        let cmd: Value = serde_json::from_str(&frame).map_err(|e| {
            BrowserError::new(
                super::CODE_BROWSER_UNAVAILABLE,
                format!("sent a frame that is not JSON: {e}"),
                false,
            )
        })?;
        if self.log.lock().closed {
            return Err(BrowserError::new(
                super::CODE_BROWSER_UNAVAILABLE,
                "the socket is closed",
                false,
            ));
        }
        self.log.lock().commands.push(cmd.clone());
        let frames = (self.handler)(&cmd);
        let mut s = self.log.lock();
        for f in frames {
            if f.is_null() {
                s.closed = true;
                break;
            }
            s.outbox.push_back(f.to_string());
        }
        if let Some(w) = s.waker.take() {
            w.wake();
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<String>, BrowserError> {
        let log = self.log.clone();
        std::future::poll_fn(move |cx| {
            let mut s = log.lock();
            if let Some(f) = s.outbox.pop_front() {
                return Poll::Ready(Ok(Some(f)));
            }
            if s.closed {
                return Poll::Ready(Ok(None));
            }
            s.waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    async fn close(&mut self) {
        let mut s = self.log.lock();
        s.closes += 1;
        s.closed = true;
    }
}

/// The answer to `Runtime.evaluate` of a value: `{"result": {"type",
/// "value"}}`.
#[must_use]
pub fn value(v: Value) -> Value {
    let ty = match &v {
        Value::Null => "object",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) | Value::Object(_) => "object",
    };
    json!({"result": {"type": ty, "value": v}})
}

/// The answer to `Runtime.evaluate` of an expression that threw `message`.
#[must_use]
pub fn thrown(message: &str) -> Value {
    json!({
        "result": {"type": "object", "subtype": "error", "description": message},
        "exceptionDetails": {"text": "Uncaught", "exception": {"description": message}}
    })
}

/// The bytes of the 1x1 PNG a [`PageModel`] screenshots by default.
pub const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

type Evaluate = Box<dyn FnMut(&Value) -> Value + Send>;
type Override = Box<dyn FnMut(&Value) -> Option<Vec<Value>> + Send>;

/// One page in a [`FakeChrome`] ([`FakeChrome::page`]), answering as
/// Chrome does:
///
/// - `Target.createBrowserContext`, `Target.createTarget` and
///   `Target.attachToTarget` with the context `CTX`, the target `T` and
///   the session `S`; `Browser.close` with `{}`, then the socket closes;
///   every other command not named here with `{}`.
/// - `Page.navigate` pauses the document's request
///   (`Fetch.requestPaused`, request IDs `R1`, `R2`, …) and answers once
///   it is fulfilled, failed or continued. A fulfilled document pauses
///   each of [`PageModel::requests`]; once all are answered, the events of
///   [`PageModel::after_load`] come, then `Page.loadEventFired`. A failed
///   document answers `errorText` `net::ERR_BLOCKED_BY_CLIENT` and does
///   not load.
/// - `Runtime.evaluate` by [`PageModel::evaluate`] (`undefined` by
///   default); `Page.getLayoutMetrics` with
///   [`PageModel::content_size`]; `Page.captureScreenshot` with
///   [`PageModel::screenshot`]'s bytes.
/// - [`PageModel::on`] first, for any command it answers.
pub struct PageModel {
    evaluate: Evaluate,
    on: Override,
    requests: Vec<(String, String)>,
    after_load: Vec<Value>,
    content: (f64, f64),
    screenshot: Vec<u8>,
    next_request: u32,
    /// The navigation waiting for its document: the command and the
    /// request ID.
    navigating: Option<(Value, String)>,
    /// Subresource requests paused and not yet answered.
    outstanding: Vec<String>,
}

impl Default for PageModel {
    fn default() -> PageModel {
        PageModel {
            evaluate: Box::new(|_| json!({"result": {"type": "undefined"}})),
            on: Box::new(|_| None),
            requests: Vec::new(),
            after_load: Vec::new(),
            content: (1280.0, 800.0),
            screenshot: ONE_PIXEL_PNG.to_vec(),
            next_request: 0,
            navigating: None,
            outstanding: Vec::new(),
        }
    }
}

impl PageModel {
    #[must_use]
    pub fn new() -> PageModel {
        PageModel::default()
    }

    /// Answers `Runtime.evaluate` with `f` of its parameters: the
    /// answer's whole result ([`value`], [`thrown`]).
    #[must_use]
    pub fn evaluate(mut self, f: impl FnMut(&Value) -> Value + Send + 'static) -> PageModel {
        self.evaluate = Box::new(f);
        self
    }

    /// Answers any command `f` answers (its frames), before the model.
    #[must_use]
    pub fn on(mut self, f: impl FnMut(&Value) -> Option<Vec<Value>> + Send + 'static) -> PageModel {
        self.on = Box::new(f);
        self
    }

    /// The subresources the document requests: `(URL, resource type)`.
    #[must_use]
    pub fn requests(mut self, r: &[(&str, &str)]) -> PageModel {
        self.requests = r
            .iter()
            .map(|(u, t)| ((*u).to_string(), (*t).to_string()))
            .collect();
        self
    }

    /// Events sent once the page loaded, before `Page.loadEventFired`
    /// (console messages, errors, a navigation away: their `sessionId` is
    /// set to the page's).
    #[must_use]
    pub fn after_load(mut self, frames: Vec<Value>) -> PageModel {
        self.after_load = frames;
        self
    }

    /// The page's content size in CSS pixels.
    #[must_use]
    pub fn content_size(mut self, width: f64, height: f64) -> PageModel {
        self.content = (width, height);
        self
    }

    /// The bytes every screenshot answers.
    #[must_use]
    pub fn screenshot(mut self, png: Vec<u8>) -> PageModel {
        self.screenshot = png;
        self
    }

    fn paused(&mut self, url: &str, resource_type: &str) -> (String, Value) {
        self.next_request += 1;
        let id = format!("R{}", self.next_request);
        let e = event(
            "Fetch.requestPaused",
            json!({
                "requestId": id,
                "request": {"url": url, "method": "GET", "headers": {}},
                "frameId": "F",
                "resourceType": resource_type,
            }),
            Some("S"),
        );
        (id, e)
    }

    fn loaded(&mut self) -> Vec<Value> {
        let mut frames: Vec<Value> = self
            .after_load
            .iter()
            .map(|f| {
                let mut f = f.clone();
                if f.get("method").is_some() {
                    f["sessionId"] = json!("S");
                }
                f
            })
            .collect();
        frames.push(event(
            "Page.loadEventFired",
            json!({"timestamp": 1.0}),
            Some("S"),
        ));
        frames
    }

    fn answer(&mut self, cmd: &Value) -> Vec<Value> {
        if let Some(frames) = (self.on)(cmd) {
            return frames;
        }
        let params = &cmd["params"];
        match cmd["method"].as_str().unwrap_or_default() {
            "Target.createBrowserContext" => vec![answer(cmd, json!({"browserContextId": "CTX"}))],
            "Target.createTarget" => vec![answer(cmd, json!({"targetId": "T"}))],
            // As Chrome does, the attach is announced before its answer.
            "Target.attachToTarget" => vec![
                event(
                    "Target.attachedToTarget",
                    json!({"sessionId": "S", "targetInfo": {"targetId": "T", "type": "page", "url": "about:blank"}, "waitingForDebugger": false}),
                    None,
                ),
                answer(cmd, json!({"sessionId": "S"})),
            ],
            "Browser.close" => vec![answer(cmd, json!({})), CLOSE],
            "Page.navigate" => {
                let url = params["url"].as_str().unwrap_or_default().to_string();
                let (id, e) = self.paused(&url, "Document");
                self.navigating = Some((cmd.clone(), id));
                vec![e]
            }
            "Fetch.fulfillRequest" | "Fetch.failRequest" | "Fetch.continueRequest" => {
                let rid = params["requestId"].as_str().unwrap_or_default().to_string();
                let mut frames = vec![answer(cmd, json!({}))];
                let failed = cmd["method"] == "Fetch.failRequest";
                if let Some((nav, id)) = self.navigating.take() {
                    if id == rid {
                        if failed {
                            frames.push(answer(
                                &nav,
                                json!({"frameId": "F", "errorText": "net::ERR_BLOCKED_BY_CLIENT"}),
                            ));
                            return frames;
                        }
                        frames.push(answer(&nav, json!({"frameId": "F", "loaderId": "L"})));
                        let requests = self.requests.clone();
                        for (url, ty) in &requests {
                            let (id, e) = self.paused(url, ty);
                            self.outstanding.push(id);
                            frames.push(e);
                        }
                        if self.outstanding.is_empty() {
                            frames.extend(self.loaded());
                        }
                        return frames;
                    }
                    self.navigating = Some((nav, id));
                }
                let before = self.outstanding.len();
                self.outstanding.retain(|r| *r != rid);
                if before > 0 && self.outstanding.is_empty() {
                    frames.extend(self.loaded());
                }
                frames
            }
            "Runtime.evaluate" => {
                let result = (self.evaluate)(params);
                vec![answer(cmd, result)]
            }
            "Page.getLayoutMetrics" => {
                let (w, h) = self.content;
                vec![answer(
                    cmd,
                    json!({
                        "cssContentSize": {"x": 0, "y": 0, "width": w, "height": h},
                        "cssLayoutViewport": {"pageX": 0, "pageY": 0, "clientWidth": 1280, "clientHeight": 800},
                    }),
                )]
            }
            "Page.captureScreenshot" => vec![answer(
                cmd,
                json!({"data": base64::engine::general_purpose::STANDARD.encode(&self.screenshot)}),
            )],
            _ => vec![answer(cmd, json!({}))],
        }
    }
}

type Opener = Box<dyn FnMut() -> Result<FakeChrome, BrowserError> + Send>;

/// A [`BrowserSource`] for tests: opens what its opener makes (a
/// [`FakeChrome`] with one [`PageModel`] page by default), counting each
/// browser it opened; its deadlines fire together when the test says
/// ([`FakeBrowserSource::fire_deadline`]); its UI files, site headers and
/// API answers are tables a test fills.
pub struct FakeBrowserSource {
    opened: AtomicU32,
    configured: AtomicBool,
    opener: Mutex<Opener>,
    logs: Mutex<Vec<ChromeLog>>,
    deadline: CancelToken,
    deadlines: Mutex<Vec<DateTime<Utc>>>,
    clax_files: Mutex<BTreeMap<String, Served>>,
    site_headers: Mutex<Vec<(String, String)>>,
    site_api: Mutex<BTreeMap<String, Served>>,
    api_calls: Mutex<Vec<(String, String, String)>>,
    own_hosts: Mutex<Result<OwnHosts, String>>,
    private_names: Mutex<BTreeSet<String>>,
    hosts_asked: Mutex<Vec<String>>,
    hangs: AtomicBool,
}

impl std::fmt::Debug for FakeBrowserSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeBrowserSource")
            .field("opened", &self.opened())
            .finish_non_exhaustive()
    }
}

impl Default for FakeBrowserSource {
    fn default() -> FakeBrowserSource {
        FakeBrowserSource::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeBrowserSource {
    /// A configured source opening one default page each time, with the
    /// site's headers `Content-Security-Policy: default-src 'self'`.
    #[must_use]
    pub fn new() -> FakeBrowserSource {
        FakeBrowserSource {
            opened: AtomicU32::new(0),
            configured: AtomicBool::new(true),
            opener: Mutex::new(Box::new(|| Ok(FakeChrome::page(PageModel::new())))),
            logs: Mutex::new(Vec::new()),
            deadline: CancelToken::new(),
            deadlines: Mutex::new(Vec::new()),
            clax_files: Mutex::new(BTreeMap::new()),
            site_headers: Mutex::new(vec![(
                "Content-Security-Policy".into(),
                "default-src 'self'".into(),
            )]),
            site_api: Mutex::new(BTreeMap::new()),
            api_calls: Mutex::new(Vec::new()),
            own_hosts: Mutex::new(Ok(OwnHosts::default())),
            private_names: Mutex::new(BTreeSet::new()),
            hosts_asked: Mutex::new(Vec::new()),
            hangs: AtomicBool::new(false),
        }
    }

    /// Makes each later open count as reaching Browser Run and then never
    /// answer, as an upgrade that stalls.
    pub fn hang_on_open(&self) {
        self.hangs.store(true, Ordering::SeqCst);
    }

    /// Opens what `f` makes, each time: a browser, or the refusal.
    pub fn opens(&self, f: impl FnMut() -> Result<FakeChrome, BrowserError> + Send + 'static) {
        *lock(&self.opener) = Box::new(f);
    }

    /// Whether the source says it is configured.
    pub fn set_configured(&self, on: bool) {
        self.configured.store(on, Ordering::SeqCst);
    }

    /// How many browsers it opened.
    #[must_use]
    pub fn opened(&self) -> u32 {
        self.opened.load(Ordering::SeqCst)
    }

    /// The logs of the browsers it opened, in order.
    #[must_use]
    pub fn logs(&self) -> Vec<ChromeLog> {
        lock(&self.logs).clone()
    }

    /// Fires every deadline it handed out, and every later one at once.
    pub fn fire_deadline(&self) {
        self.deadline.cancel();
    }

    /// The times of the deadlines it handed out, in order.
    #[must_use]
    pub fn deadlines(&self) -> Vec<DateTime<Utc>> {
        lock(&self.deadlines).clone()
    }

    /// Serves `path` (`/_clax/bridge.js`, …) as `served`.
    pub fn clax_file(&self, path: &str, served: Served) {
        lock(&self.clax_files).insert(path.to_string(), served);
    }

    /// The site's headers for every artifact.
    pub fn set_site_headers(&self, headers: Vec<(String, String)>) {
        *lock(&self.site_headers) = headers;
    }

    /// Answers a `GET` of `path_and_query` on the API with `served`.
    pub fn site_api(&self, path_and_query: &str, served: Served) {
        lock(&self.site_api).insert(path_and_query.to_string(), served);
    }

    /// Every API `GET` asked: `(person, artifact, path and query)`.
    #[must_use]
    pub fn api_calls(&self) -> Vec<(String, String, String)> {
        lock(&self.api_calls).clone()
    }

    /// The instance's own hosts it answers ([`BrowserSource::own_hosts`]),
    /// or why they cannot be named; none by default.
    pub fn set_own_hosts(&self, own: Result<OwnHosts, String>) {
        *lock(&self.own_hosts) = own;
    }

    /// Answers the name `host` as resolving to a private address
    /// ([`BrowserSource::host_public`]); every other name is public.
    pub fn resolves_private(&self, host: &str) {
        lock(&self.private_names).insert(host.to_string());
    }

    /// Every name its resolver was asked, in order.
    #[must_use]
    pub fn hosts_asked(&self) -> Vec<String> {
        lock(&self.hosts_asked).clone()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl BrowserSource for FakeBrowserSource {
    async fn open(&self, cancel: &CancelToken) -> Result<Box<dyn CdpTransport>, BrowserError> {
        if cancel.is_cancelled() {
            return Err(BrowserError::new(
                clax_bot_sandbox::CODE_CANCELLED,
                "the run was cancelled",
                false,
            ));
        }
        if self.hangs.load(Ordering::SeqCst) {
            self.opened.fetch_add(1, Ordering::SeqCst);
            return std::future::pending().await;
        }
        let chrome = {
            let mut opener = lock(&self.opener);
            (*opener)()?
        };
        self.opened.fetch_add(1, Ordering::SeqCst);
        lock(&self.logs).push(chrome.log());
        Ok(Box::new(chrome))
    }

    fn configured(&self) -> bool {
        self.configured.load(Ordering::SeqCst)
    }

    fn deadline(&self, at: DateTime<Utc>) -> Deadline {
        lock(&self.deadlines).push(at);
        let fired = self.deadline.clone();
        let f = async move { fired.cancelled().await };
        #[cfg(not(target_arch = "wasm32"))]
        return f.boxed();
        #[cfg(target_arch = "wasm32")]
        return f.boxed_local();
    }

    fn clax_file(&self, path: &str) -> Option<Served> {
        lock(&self.clax_files).get(path).cloned()
    }

    fn site_headers(&self, _artifact: &str) -> Vec<(String, String)> {
        lock(&self.site_headers).clone()
    }

    async fn site_api(&self, person: &str, artifact: &str, path_and_query: &str) -> Served {
        lock(&self.api_calls).push((
            person.to_string(),
            artifact.to_string(),
            path_and_query.to_string(),
        ));
        lock(&self.site_api)
            .get(path_and_query)
            .cloned()
            .unwrap_or_else(|| Served {
                status: 404,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body: br#"{"error":{"code":"not_found"}}"#.to_vec(),
            })
    }

    fn own_hosts(&self) -> Result<OwnHosts, String> {
        lock(&self.own_hosts).clone()
    }

    async fn host_public(&self, host: &str) -> Result<(), String> {
        lock(&self.hosts_asked).push(host.to_string());
        if lock(&self.private_names).contains(host) {
            return Err(format!("{host} resolves to 10.0.0.1, a private address"));
        }
        Ok(())
    }
}
