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
    /// Who waits for a command to be sent ([`ChromeLog::wait_sent`]).
    watchers: Vec<Waker>,
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

    /// Resolves once `n` commands of `method` have been sent: a test's
    /// wait for a session's task to act, on a condition, not a time. In
    /// this crate's own tests the wait carries the hang guard
    /// (`tests/common`'s `within`), so a command never sent fails the test
    /// instead of hanging it; an outside test bounds it itself.
    pub async fn wait_sent(&self, method: &str, n: usize) {
        let sent = std::future::poll_fn(|cx| {
            let mut s = self.lock();
            if s.commands.iter().filter(|c| c["method"] == method).count() >= n {
                return Poll::Ready(());
            }
            s.watchers.push(cx.waker().clone());
            Poll::Pending
        });
        #[cfg(test)]
        crate::script_scenarios::common::within(&format!("{n} {method} sent"), sent).await;
        #[cfg(not(test))]
        sent.await;
    }

    /// Has the browser send `frame` (an event: a crash, a paused request
    /// of a page's own, an attached target) without a command before it,
    /// as a browser does between commands.
    pub fn emit(&self, frame: Value) {
        let mut s = self.lock();
        s.outbox.push_back(frame.to_string());
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
        {
            let mut s = self.log.lock();
            s.commands.push(cmd.clone());
            for w in s.watchers.drain(..) {
                w.wake();
            }
        }
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
/// - `Page.navigate` (and `Page.reload`, of the URL last navigated to)
///   pauses the document's request
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
    /// The URL last navigated to, which `Page.reload` loads again.
    url: String,
    /// The page's document, when the test gives one ([`PageModel::dom`]).
    dom: Option<Dom>,
    /// How many waits in the page (`clax:frame`) are answered; the rest
    /// never are.
    frame_limit: Option<usize>,
    frames: usize,
    /// The URLs navigated to, and where in them the page is.
    history: Vec<String>,
    at: usize,
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
            url: String::new(),
            dom: None,
            frame_limit: None,
            frames: 0,
            history: Vec::new(),
            at: 0,
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

    /// The page's document ([`Dom`]): `DOM.*`, `Accessibility.queryAXTree`,
    /// `Runtime.callFunctionOn` on its elements and the input it takes are
    /// answered from it.
    #[must_use]
    pub fn dom(mut self, dom: Dom) -> PageModel {
        self.dom = Some(dom);
        self
    }

    /// Answers the first `n` waits in the page (`clax:frame`) and never
    /// the rest, so a driver that would wait for ever waits for its
    /// deadline.
    #[must_use]
    pub fn frame_limit(mut self, n: usize) -> PageModel {
        self.frame_limit = Some(n);
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
        if let Some(dom) = &mut self.dom
            && let Some(frames) = dom.answer(cmd)
        {
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
            "Page.getNavigationHistory" => {
                let entries: Vec<Value> = self
                    .history
                    .iter()
                    .enumerate()
                    .map(|(i, u)| json!({"id": i, "url": u, "userTypedURL": u, "title": "", "transitionType": "link"}))
                    .collect();
                vec![answer(
                    cmd,
                    json!({"currentIndex": self.at, "entries": entries}),
                )]
            }
            "Page.resetNavigationHistory" => {
                let current = self.history.get(self.at).cloned();
                self.history = current.into_iter().collect();
                self.at = 0;
                vec![answer(cmd, json!({}))]
            }
            "Page.navigate" | "Page.reload" | "Page.navigateToHistoryEntry" => {
                let url = match (params["url"].as_str(), params["entryId"].as_u64()) {
                    (Some(u), _) => {
                        self.history.truncate(self.at + 1);
                        if self.history.is_empty() {
                            self.history.push(u.to_string());
                        } else {
                            self.history.push(u.to_string());
                            self.at = self.history.len() - 1;
                        }
                        u.to_string()
                    }
                    (None, Some(id)) => {
                        let i = usize::try_from(id)
                            .unwrap_or(0)
                            .min(self.history.len().saturating_sub(1));
                        self.at = i;
                        self.history.get(i).cloned().unwrap_or_default()
                    }
                    (None, None) => self.url.clone(),
                };
                self.url = url.clone();
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
                let e = params["expression"].as_str().unwrap_or_default();
                if e.contains("clax:frame") {
                    self.frames += 1;
                    if self.frame_limit.is_some_and(|n| self.frames > n) {
                        return Vec::new();
                    }
                    return vec![answer(cmd, value(json!(true)))];
                }
                if e.contains("clax:regex") {
                    let args: Value = e
                        .rfind("})(")
                        .and_then(|at| serde_json::from_str(e[at + 3..].trim_end_matches(')')).ok())
                        .unwrap_or(Value::Null);
                    let (src, flags) = (
                        args[0].as_str().unwrap_or(""),
                        args[1].as_str().unwrap_or(""),
                    );
                    let hits: Vec<bool> = args[2]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|t| fake_regex(src, flags, t.as_str().unwrap_or("")))
                        .collect();
                    return vec![answer(cmd, value(json!(hits)))];
                }
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

/// One element (or text) of a [`Dom`]: its tag, attributes, children,
/// box, accessibility role and name, and its live state.
#[derive(Clone, Debug, Default)]
pub struct El {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    /// A text node's text (its `tag` is `#text`).
    pub text: Option<String>,
    pub children: Vec<El>,
    /// Its boxes `(x, y, width, height)`, one per `DOM.getBoxModel` asked
    /// of it, the last repeated; none: not rendered.
    pub rects: Vec<(f64, f64, f64, f64)>,
    /// Higher is on top at a point.
    pub z: i32,
    /// Its accessibility role and name, and whether the tree ignores it.
    pub role: Option<(String, String)>,
    pub ignored: bool,
    pub checked: Option<bool>,
    pub value: Option<String>,
    pub level: Option<u32>,
}

/// An element `tag`.
#[must_use]
pub fn el(tag: &str) -> El {
    El {
        tag: tag.to_string(),
        ..El::default()
    }
}

impl El {
    #[must_use]
    pub fn attr(mut self, k: &str, v: &str) -> El {
        self.attrs.push((k.to_string(), v.to_string()));
        self
    }
    /// A text child.
    #[must_use]
    pub fn text(mut self, t: &str) -> El {
        self.children.push(El {
            tag: "#text".into(),
            text: Some(t.to_string()),
            ..El::default()
        });
        self
    }
    #[must_use]
    pub fn child(mut self, c: El) -> El {
        self.children.push(c);
        self
    }
    #[must_use]
    pub fn rect(mut self, x: f64, y: f64, w: f64, h: f64) -> El {
        self.rects.push((x, y, w, h));
        self
    }
    #[must_use]
    pub fn z(mut self, z: i32) -> El {
        self.z = z;
        self
    }
    #[must_use]
    pub fn role(mut self, role: &str, name: &str) -> El {
        self.role = Some((role.to_string(), name.to_string()));
        self
    }
    #[must_use]
    pub fn ignored(mut self) -> El {
        self.ignored = true;
        self
    }
    #[must_use]
    pub fn checked(mut self, c: bool) -> El {
        self.checked = Some(c);
        self
    }
    #[must_use]
    pub fn value(mut self, v: &str) -> El {
        self.value = Some(v.to_string());
        self
    }
    #[must_use]
    pub fn level(mut self, l: u32) -> El {
        self.level = Some(l);
        self
    }
}

/// A flattened node of a [`Dom`]: its ID is its index plus one (as
/// `nodeId` and `backendNodeId` both).
#[derive(Clone, Debug)]
struct Flat {
    el: El,
    parent: Option<usize>,
    children: Vec<usize>,
    boxes_asked: usize,
}

/// A page's document for [`PageModel::dom`]: the documents its
/// successive `DOM.getDocument` calls see (the last repeated), so a test
/// says how the page changes between a driver's attempts; the focus, the
/// selection and the input `Input.insertText` types into.
#[derive(Clone, Debug, Default)]
pub struct Dom {
    states: Vec<El>,
    served: usize,
    flat: Vec<Flat>,
    focused: Option<usize>,
    /// Whether the focused element's text is selected (by `select()`), so
    /// typing replaces it.
    selected: bool,
}

impl Dom {
    /// A document whose `<body>` holds `body`, the same at every attempt.
    #[must_use]
    pub fn new(body: Vec<El>) -> Dom {
        Dom::states(vec![body])
    }

    /// A document whose `<body>` holds each of `states` in turn, one per
    /// `DOM.getDocument`, the last repeated.
    #[must_use]
    pub fn states(states: Vec<Vec<El>>) -> Dom {
        let docs = states
            .into_iter()
            .map(|body| {
                let mut b = el("body").rect(0.0, 0.0, 1280.0, 800.0);
                b.children = body;
                El {
                    tag: "#document".into(),
                    children: vec![el("html").rect(0.0, 0.0, 1280.0, 800.0).child(b)],
                    ..El::default()
                }
            })
            .collect();
        let mut d = Dom {
            states: docs,
            ..Dom::default()
        };
        d.flatten(0);
        d
    }

    fn flatten(&mut self, state: usize) {
        let keep: Vec<(Option<String>, Option<bool>, usize)> = self
            .flat
            .iter()
            .map(|f| (f.el.value.clone(), f.el.checked, f.boxes_asked))
            .collect();
        self.flat.clear();
        let root = self.states[state.min(self.states.len() - 1)].clone();
        self.push(root, None);
        // What the page changed (typed values, checks) and the boxes asked
        // live on across attempts of the same shape.
        if keep.len() == self.flat.len() {
            for (f, (v, c, b)) in self.flat.iter_mut().zip(keep) {
                f.el.value = v.or(f.el.value.take());
                f.el.checked = c.or(f.el.checked);
                f.boxes_asked = b;
            }
        }
    }

    fn push(&mut self, mut e: El, parent: Option<usize>) -> usize {
        let at = self.flat.len();
        let children = std::mem::take(&mut e.children);
        self.flat.push(Flat {
            el: e,
            parent,
            children: Vec::new(),
            boxes_asked: 0,
        });
        for c in children {
            let ci = self.push(c, Some(at));
            self.flat[at].children.push(ci);
        }
        at
    }

    fn node_json(&self, i: usize) -> Value {
        let f = &self.flat[i];
        let mut n = json!({
            "nodeId": i + 1,
            "backendNodeId": i + 1,
            "nodeName": f.el.tag.to_uppercase(),
            "nodeType": if f.el.tag == "#text" { 3 } else if f.el.tag == "#document" { 9 } else { 1 },
        });
        if let Some(t) = &f.el.text {
            n["nodeValue"] = json!(t);
        }
        if !f.el.attrs.is_empty() {
            n["attributes"] = json!(
                f.el.attrs
                    .iter()
                    .flat_map(|(k, v)| [k.clone(), v.clone()])
                    .collect::<Vec<_>>()
            );
        }
        n["children"] = json!(
            f.children
                .iter()
                .map(|&c| self.node_json(c))
                .collect::<Vec<_>>()
        );
        n
    }

    fn inside(&self, outer: usize, inner: usize) -> bool {
        let mut at = Some(inner);
        while let Some(i) = at {
            if i == outer {
                return true;
            }
            at = self.flat[i].parent;
        }
        false
    }

    /// Whether element `i` matches a simple selector: compounds of a tag,
    /// `#id`, `.class` and `[attr]` or `[attr="v"]`, joined by spaces.
    fn matches(&self, i: usize, selector: &str) -> bool {
        let parts: Vec<&str> = selector.split_whitespace().collect();
        let Some((last, before)) = parts.split_last() else {
            return false;
        };
        if !self.compound(i, last) {
            return false;
        }
        let mut at = self.flat[i].parent;
        for p in before.iter().rev() {
            loop {
                let Some(a) = at else { return false };
                at = self.flat[a].parent;
                if self.compound(a, p) {
                    break;
                }
            }
        }
        true
    }

    fn compound(&self, i: usize, c: &str) -> bool {
        let e = &self.flat[i].el;
        if e.tag.starts_with('#') {
            return false;
        }
        let attr = |k: &str| {
            e.attrs
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
        };
        let mut rest = c;
        let tag_end = rest.find(['#', '.', '[']).unwrap_or(rest.len());
        let (tag, tail) = rest.split_at(tag_end);
        if !tag.is_empty() && tag != "*" && tag != e.tag {
            return false;
        }
        rest = tail;
        while !rest.is_empty() {
            let (kind, body) = rest.split_at(1);
            let end = body.find(['#', '.', '[']).unwrap_or(body.len());
            let (part, tail) = if kind == "[" {
                let close = body.find(']').unwrap_or(body.len());
                (&body[..close], &body[(close + 1).min(body.len())..])
            } else {
                body.split_at(end)
            };
            let ok = match kind {
                "#" => attr("id") == Some(part),
                "." => attr("class").is_some_and(|c| c.split_whitespace().any(|x| x == part)),
                "[" => match part.split_once('=') {
                    Some((k, v)) => attr(k) == Some(v.trim_matches('"').trim_matches('\'')),
                    None => attr(part).is_some(),
                },
                _ => false,
            };
            if !ok {
                return false;
            }
            rest = tail;
        }
        true
    }

    fn by_id(&self, v: &Value) -> Option<usize> {
        let id = v.as_i64()?;
        usize::try_from(id - 1)
            .ok()
            .filter(|&i| i < self.flat.len())
    }

    fn rect_of(&mut self, i: usize) -> Option<(f64, f64, f64, f64)> {
        let f = &mut self.flat[i];
        let r =
            f.el.rects
                .get(f.boxes_asked.min(f.el.rects.len().saturating_sub(1)))
                .copied();
        f.boxes_asked += 1;
        r
    }

    fn texts(&self, i: usize) -> String {
        let f = &self.flat[i];
        if let Some(t) = &f.el.text {
            return t.clone();
        }
        f.children.iter().map(|&c| self.texts(c)).collect()
    }

    /// The answer to `cmd`, when it is a command of the document.
    fn answer(&mut self, cmd: &Value) -> Option<Vec<Value>> {
        let p = &cmd["params"];
        let method = cmd["method"].as_str().unwrap_or_default();
        let result = match method {
            "DOM.getDocument" => {
                self.flatten(self.served);
                self.served += 1;
                json!({"root": self.node_json(0)})
            }
            "DOM.querySelectorAll" => {
                let scope = self.by_id(&p["nodeId"]).unwrap_or(0);
                let sel = p["selector"].as_str().unwrap_or_default();
                let ids: Vec<usize> = (0..self.flat.len())
                    .filter(|&i| {
                        i != scope
                            && self.inside(scope, i)
                            && sel.split(',').any(|s| self.matches(i, s.trim()))
                    })
                    .map(|i| i + 1)
                    .collect();
                json!({"nodeIds": ids})
            }
            "Accessibility.queryAXTree" => {
                let scope = self.by_id(&p["backendNodeId"]).unwrap_or(0);
                let role = p["role"].as_str().unwrap_or_default();
                let nodes: Vec<Value> = (0..self.flat.len())
                    .filter(|&i| self.inside(scope, i))
                    .filter_map(|i| {
                        let e = &self.flat[i].el;
                        let (r, name) = e.role.as_ref()?;
                        (r == role).then(|| {
                            let mut props = Vec::new();
                            if let Some(c) = e.checked {
                                props.push(json!({"name": "checked", "value": {"type": "tristate", "value": c.to_string()}}));
                            }
                            if let Some(l) = e.level {
                                props.push(json!({"name": "level", "value": {"type": "integer", "value": l}}));
                            }
                            json!({"nodeId": format!("ax{i}"), "ignored": e.ignored, "role": {"type": "role", "value": r}, "name": {"type": "computedString", "value": name}, "properties": props, "backendDOMNodeId": i + 1})
                        })
                    })
                    .collect();
                json!({"nodes": nodes})
            }
            "DOM.getBoxModel" => {
                let i = self.by_id(&p["backendNodeId"])?;
                match self.rect_of(i) {
                    Some((x, y, w, h)) => {
                        let quad = json!([x, y, x + w, y, x + w, y + h, x, y + h]);
                        json!({"model": {"content": quad, "padding": quad, "border": quad, "margin": quad, "width": w, "height": h}})
                    }
                    None => return Some(vec![error(cmd, "Could not compute box model.")]),
                }
            }
            "DOM.getNodeForLocation" => {
                let (x, y) = (
                    p["x"].as_f64().unwrap_or(0.0),
                    p["y"].as_f64().unwrap_or(0.0),
                );
                let hit = (0..self.flat.len())
                    .filter(|&i| {
                        let e = &self.flat[i].el;
                        !e.tag.starts_with('#')
                            && e.rects.last().is_some_and(|&(rx, ry, w, h)| {
                                x >= rx && x <= rx + w && y >= ry && y <= ry + h
                            })
                    })
                    .max_by_key(|&i| (self.flat[i].el.z, i));
                match hit {
                    Some(i) => json!({"backendNodeId": i + 1, "nodeId": i + 1, "frameId": "F"}),
                    None => return Some(vec![error(cmd, "No node found at given location")]),
                }
            }
            "DOM.resolveNode" => {
                let i = self.by_id(&p["backendNodeId"])?;
                json!({"object": {"type": "object", "subtype": "node", "objectId": format!("node-{}", i + 1)}})
            }
            "DOM.focus" => {
                self.focused = self.by_id(&p["backendNodeId"]);
                self.selected = false;
                json!({})
            }
            "DOM.scrollIntoViewIfNeeded" => json!({}),
            "Input.insertText" => {
                if let Some(i) = self.focused {
                    let t = p["text"].as_str().unwrap_or_default();
                    let e = &mut self.flat[i].el;
                    let before = if self.selected {
                        String::new()
                    } else {
                        e.value.clone().unwrap_or_default()
                    };
                    e.value = Some(before + t);
                    self.selected = false;
                }
                json!({})
            }
            "Runtime.callFunctionOn" => {
                let i = p["objectId"]
                    .as_str()
                    .and_then(|o| o.strip_prefix("node-"))
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|n| n.checked_sub(1))?;
                let f = p["functionDeclaration"].as_str().unwrap_or_default();
                let thrown = |m: &str| json!({"result": {"type": "object", "subtype": "error"}, "exceptionDetails": {"text": "Uncaught", "exception": {"description": format!("Error: {m}")}}});
                if f.contains("clax:selectOption") {
                    let wanted = &p["arguments"][0]["value"];
                    let opts: Vec<(String, String)> = self.flat[i]
                        .children
                        .iter()
                        .filter(|&&c| self.flat[c].el.tag == "option")
                        .map(|&c| {
                            let v = self.flat[c]
                                .el
                                .attrs
                                .iter()
                                .find(|(k, _)| k == "value")
                                .map(|(_, v)| v.clone());
                            let label = self.texts(c);
                            (v.unwrap_or_else(|| label.clone()), label)
                        })
                        .collect();
                    let w = &wanted[0];
                    match opts.iter().find(|(v, l)| {
                        w["value"].as_str() == Some(v) || w["label"].as_str() == Some(l)
                    }) {
                        Some((v, _)) => {
                            self.flat[i].el.value = Some(v.clone());
                            value(json!([v]))
                        }
                        None => thrown("did not find some options"),
                    }
                } else if f.contains("this.checked") {
                    match self.flat[i].el.checked {
                        Some(c) => value(json!(c)),
                        None => value(json!(false)),
                    }
                } else if f.contains("this.value") {
                    match &self.flat[i].el.value {
                        Some(v) => value(json!(v)),
                        None => thrown("Not an input element"),
                    }
                } else if f.contains("this.innerText") {
                    value(json!(self.texts(i)))
                } else if f.contains("this.select") {
                    self.selected = true;
                    json!({"result": {"type": "undefined"}})
                } else if f.contains("document.activeElement") {
                    value(json!(self.focused == Some(i)))
                } else {
                    json!({"result": {"type": "undefined"}})
                }
            }
            "Input.dispatchMouseEvent" => {
                // A press and release on a checkbox toggles it.
                if p["type"] == "mouseReleased" {
                    let (x, y) = (
                        p["x"].as_f64().unwrap_or(0.0),
                        p["y"].as_f64().unwrap_or(0.0),
                    );
                    let hit = (0..self.flat.len())
                        .filter(|&i| {
                            self.flat[i].el.rects.last().is_some_and(|&(rx, ry, w, h)| {
                                x >= rx && x <= rx + w && y >= ry && y <= ry + h
                            })
                        })
                        .max_by_key(|&i| (self.flat[i].el.z, i));
                    if let Some(i) = hit
                        && let Some(c) = self.flat[i].el.checked
                    {
                        self.flat[i].el.checked = Some(!c);
                    }
                }
                json!({})
            }
            _ => return None,
        };
        Some(vec![answer(cmd, result)])
    }
}

/// Whether `text` matches the JavaScript pattern `source` with `flags`,
/// for the patterns a test sends: literal text, `^` and `$`, `|`, `.`
/// and `.*`; `i` folds case.
fn fake_regex(source: &str, flags: &str, text: &str) -> bool {
    let fold = flags.contains('i');
    let (src, text) = if fold {
        (source.to_lowercase(), text.to_lowercase())
    } else {
        (source.to_string(), text.to_string())
    };
    src.split('|').any(|alt| {
        let start = alt.starts_with('^');
        let end = alt.ends_with('$') && !alt.ends_with("\\$");
        let body = alt.trim_start_matches('^');
        let body = if end { &body[..body.len() - 1] } else { body };
        let at_from =
            |from: usize| -> Option<usize> { glob_at(body, &text[from..]).map(|n| from + n) };
        if start {
            at_from(0).is_some_and(|n| !end || n == text.len())
        } else {
            (0..=text.len())
                .filter(|&i| text.is_char_boundary(i))
                .any(|i| at_from(i).is_some_and(|n| !end || n == text.len()))
        }
    })
}

/// The length of `text`'s prefix `pat` matches (`.` any one character,
/// `.*` any run, `\` escapes), the longest.
fn glob_at(pat: &str, text: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn go(p: &[char], t: &[char], used: usize) -> Option<usize> {
        match p {
            [] => Some(used),
            ['.', '*', rest @ ..] => (0..=t.len())
                .rev()
                .find_map(|k| go(rest, &t[k..], used + k)),
            ['\\', c, rest @ ..] => (t.first() == Some(c))
                .then(|| go(rest, &t[1..], used + 1))
                .flatten(),
            ['.', rest @ ..] => (!t.is_empty())
                .then(|| go(rest, &t[1..], used + 1))
                .flatten(),
            [c, rest @ ..] => (t.first() == Some(c))
                .then(|| go(rest, &t[1..], used + 1))
                .flatten(),
        }
    }
    let n = go(&p, &t, 0)?;
    Some(t[..n].iter().collect::<String>().len())
}

type Opener = Box<dyn FnMut() -> Result<FakeChrome, BrowserError> + Send>;

/// The time [`FakeBrowserSource::fire_until`] reached, and the deadlines
/// not yet fired, each with what fires it.
type Pending = (Option<DateTime<Utc>>, Vec<(DateTime<Utc>, CancelToken)>);

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
    /// The deadlines not yet fired, each with what fires it, and the time
    /// [`FakeBrowserSource::fire_until`] reached.
    pending: Mutex<Pending>,
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
            pending: Mutex::new((None, Vec::new())),
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

    /// Fires every deadline it handed out at `at` or before, and every
    /// later one at or before `at` at once: a manual clock for deadlines,
    /// which a test moves with its [`clax_core::working::ManualClock`].
    pub fn fire_until(&self, at: DateTime<Utc>) {
        let mut p = lock(&self.pending);
        p.0 = Some(p.0.map_or(at, |t| t.max(at)));
        p.1.retain(|(when, fire)| {
            if *when <= at {
                fire.cancel();
                false
            } else {
                true
            }
        });
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
        let all = self.deadline.clone();
        let mine = CancelToken::new();
        {
            let mut p = lock(&self.pending);
            if p.0.is_some_and(|now| at <= now) {
                mine.cancel();
            } else {
                p.1.push((at, mine.clone()));
            }
        }
        let f = async move {
            let a = all.cancelled();
            let m = mine.cancelled();
            futures::pin_mut!(a, m);
            futures::future::select(a, m).await;
        };
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

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    /// A wait for a command never sent fails the test, past the hang
    /// guard, instead of hanging it (the paused clock reaches the guard
    /// at once).
    #[tokio::test(start_paused = true)]
    #[should_panic(expected = "hang guard")]
    async fn a_wait_for_a_command_never_sent_fails() {
        let log = FakeChrome::page(PageModel::new()).log();
        log.wait_sent("Page.navigate", 1).await;
    }
}
