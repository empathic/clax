//! One open core in this process ([`Embed`]): its home opened on a runtime
//! of its own, and request frames answered through its router by who they
//! speak for (spec 2026-10-19 §2.3, §2.5).

use crate::frame::{self, As, FrameError, RequestHead, ResponseHead};
use axum::body::Body;
use bytes::Bytes;
use clax_http::state::EmbedOrigins;
use clax_server::core::{Core, CoreConfig};
use http_body_util::BodyExt;
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::watch;
use tower::ServiceExt;

/// The longest body read back (spec §2.3): a larger one answers 502
/// `response_too_large`.
pub const RESPONSE_LIMIT: usize = 96 << 20;

/// How long a dropped core's runtime gives its tasks to end.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(2);

/// A message to the port given at open (spec §2.2), as JSON.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum OpenEvent {
    /// The store is opening: the audit backfill's phase.
    Opening { phase: String },
    /// The core answers calls.
    Ready,
    /// The core did not open: `home_in_use`, `migration_failed` (with the
    /// newest backup's path) or `open_failed`.
    Failed {
        code: String,
        message: String,
        backup: Option<String>,
    },
    /// The core stopped after [`Embed::close`].
    Closed,
}

/// What [`Embed::open`] opens.
pub struct OpenOptions {
    /// The Clax home.
    pub home: PathBuf,
    /// The origins the app gives its WebViews.
    pub origins: EmbedOrigins,
    /// Where the rolling log goes (`<cache>/core.log`); none when `None`.
    pub log: Option<PathBuf>,
}

/// Why [`Embed::open`] refused.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// This process has that home open already.
    HomeInUse,
    /// The home's path is not UTF-8.
    NotUtf8,
    /// The core's runtime did not start (its threads could not be
    /// spawned).
    Runtime(String),
}

/// Where a core is.
#[derive(Clone)]
enum Phase {
    Opening,
    Ready { router: axum::Router, token: String },
    Failed,
    /// Closing: new calls are refused, calls in flight go on for up to
    /// the drain's bound.
    Draining,
    /// Closing past the drain: calls still in flight are cut.
    Stopping,
    Closed,
}

/// What says when a ready core stops being ready (it closes).
pub(crate) struct Readiness(watch::Receiver<Phase>);

impl Readiness {
    /// Resolves when the core is no longer ready.
    pub(crate) async fn closed(&mut self) {
        let _ = self.0.wait_for(|p| !matches!(p, Phase::Ready { .. })).await;
    }

    /// Whether the core is still ready.
    pub(crate) fn ready(&self) -> bool {
        matches!(*self.0.borrow(), Phase::Ready { .. })
    }
}

/// The homes this process has open, by canonical path.
static OPEN: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the core's tasks share; it never holds the runtime, so no task
/// keeps it alive. The home is released ([`Shared::release`]) by close, by
/// a failed open, or else when the last share is dropped.
struct Shared {
    /// The home's key in [`OPEN`].
    key: PathBuf,
    phase: watch::Sender<Phase>,
    core: tokio::sync::Mutex<Option<Core>>,
    on_event: Box<dyn Fn(OpenEvent) + Send + Sync>,
    /// `<home>/core.lock`, locked while the core holds the home.
    lock: Mutex<Option<std::fs::File>>,
    released: AtomicBool,
    closing: AtomicBool,
    /// The calls admitted and not yet answered.
    in_flight: watch::Sender<usize>,
    /// How long [`Embed::close`] lets calls in flight finish.
    drain: Duration,
}

/// One call counted in [`Shared::in_flight`] while it lives.
struct InFlight<'a>(&'a watch::Sender<usize>);

impl<'a> InFlight<'a> {
    fn enter(n: &'a watch::Sender<usize>) -> InFlight<'a> {
        n.send_modify(|n| *n += 1);
        InFlight(n)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n -= 1);
    }
}

impl Shared {
    /// Unlocks the home and forgets it in [`OPEN`]; once.
    fn release(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        drop(lock(&self.lock).take());
        lock(&OPEN).remove(&self.key);
    }

    fn emit(&self, e: OpenEvent) {
        (self.on_event)(e);
    }
}

impl Drop for Shared {
    /// An Embed dropped without [`Embed::close`] still frees its home once
    /// its runtime has ended its tasks, the last holders of this share.
    fn drop(&mut self) {
        self.release();
    }
}

struct Inner {
    shared: Arc<Shared>,
    handle: tokio::runtime::Handle,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
}

impl Drop for Inner {
    /// The runtime ends on a thread of its own, so the last handle may go
    /// anywhere, a task of the runtime itself included.
    fn drop(&mut self) {
        if let Some(rt) = lock(&self.runtime).take() {
            let _ = std::thread::Builder::new()
                .name("clax-rt-end".into())
                .spawn(move || rt.shutdown_timeout(RUNTIME_SHUTDOWN));
        }
    }
}

/// One open core. Cloning shares it.
#[derive(Clone)]
pub struct Embed {
    inner: Arc<Inner>,
}

impl Embed {
    /// Starts opening `opts.home` on a runtime the Embed owns (two
    /// workers, `clax-rt`), and returns at once. `on_event` hears every
    /// [`OpenEvent`], from the core's threads: `Opening`s, then `Ready` or
    /// `Failed`, then `Closed` after [`Embed::close`].
    ///
    /// # Errors
    /// [`OpenError::HomeInUse`] when this process has the home open,
    /// [`OpenError::Runtime`] when the runtime's threads cannot be started.
    pub fn open(
        opts: OpenOptions,
        on_event: Box<dyn Fn(OpenEvent) + Send + Sync>,
    ) -> Result<Embed, OpenError> {
        Embed::open_draining(opts, on_event, clax_server::serve::DRAIN_TIMEOUT)
    }

    /// [`Embed::open`], with `drain` as how long a close lets calls in
    /// flight finish.
    fn open_draining(
        opts: OpenOptions,
        on_event: Box<dyn Fn(OpenEvent) + Send + Sync>,
        drain: Duration,
    ) -> Result<Embed, OpenError> {
        let _ = std::fs::create_dir_all(&opts.home);
        let key = std::fs::canonicalize(&opts.home).unwrap_or_else(|_| opts.home.clone());
        if !lock(&OPEN).insert(key.clone()) {
            return Err(OpenError::HomeInUse);
        }
        if let Some(log) = &opts.log {
            if let Some(dir) = log.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            crate::log::init(log.clone());
        }
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("clax-rt")
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                lock(&OPEN).remove(&key);
                return Err(OpenError::Runtime(e.to_string()));
            }
        };
        let shared = Arc::new(Shared {
            key,
            phase: watch::channel(Phase::Opening).0,
            core: tokio::sync::Mutex::new(None),
            on_event,
            lock: Mutex::new(None),
            released: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            in_flight: watch::channel(0).0,
            drain,
        });
        let handle = runtime.handle().clone();
        handle.spawn(opening(shared.clone(), opts));
        Ok(Embed {
            inner: Arc::new(Inner {
                shared,
                handle,
                runtime: Mutex::new(Some(runtime)),
            }),
        })
    }

    /// Answers one request frame with a response frame; waits for the core
    /// to be ready first. A refused path answers 404 `not_in_app`, a core
    /// that failed or closed 503, and a body over [`RESPONSE_LIMIT`] 502.
    ///
    /// # Errors
    /// A [`FrameError`] for a frame that does not parse.
    pub async fn call(&self, bytes: &[u8]) -> Result<Vec<u8>, FrameError> {
        let (head, body) = frame::decode_request(bytes)?;
        let body = Bytes::copy_from_slice(body);
        let task = self
            .inner
            .handle
            .spawn(answer(self.inner.shared.clone(), head, body));
        Ok(match task.await {
            Ok(answer) => answer,
            Err(e) if e.is_panic() => {
                error_frame(500, "internal", "the core panicked while it answered")
            }
            Err(_) => error_frame(503, "core_closed", "the core stopped before it answered"),
        })
    }

    /// Waits on the core's runtime until the core is ready: its router,
    /// the owner's token, and what says when it stops being ready; or,
    /// when it failed or closed first, `core_failed` or `core_closed`.
    pub(crate) fn ready(
        &self,
    ) -> impl std::future::Future<Output = Result<(axum::Router, String, Readiness), &'static str>>
    + Send
    + 'static {
        let mut rx = self.inner.shared.phase.subscribe();
        async move {
            let ready = rx
                .wait_for(|p| !matches!(p, Phase::Opening))
                .await
                .map(|p| p.clone());
            match ready {
                Ok(Phase::Ready { router, token }) => Ok((router, token, Readiness(rx))),
                Ok(Phase::Failed) => Err("core_failed"),
                _ => Err("core_closed"),
            }
        }
    }

    /// Spawns `fut` on the core's runtime.
    pub fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(&self, fut: F) {
        self.inner.handle.spawn(fut);
    }

    /// Stops the core as `clax serve` stops on SIGTERM: new calls answer
    /// 503 `core_closed`; streams and long polls are told to end, and calls
    /// in flight finish for up to [`clax_server::serve::DRAIN_TIMEOUT`]
    /// (one that committed a write answers as it would have), after which
    /// those still running answer 503 `core_closed`; then the tasks stop,
    /// the journal syncs, the store closes, and the home is released.
    /// Returns at once; `on_event` hears `Closed` when done. A second
    /// close does nothing.
    pub fn close(&self) {
        let sh = &self.inner.shared;
        if sh.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        // The task holds the Embed, so the runtime outlives the close.
        let embed = self.clone();
        self.inner.handle.spawn(async move {
            let sh = embed.inner.shared.clone();
            let mut rx = sh.phase.subscribe();
            let _ = rx.wait_for(|p| !matches!(p, Phase::Opening)).await;
            sh.phase.send_replace(Phase::Draining);
            if let Some(core) = sh.core.lock().await.as_ref() {
                let _ = core.shutdown_handle().send(true);
            }
            let mut in_flight = sh.in_flight.subscribe();
            let drained = tokio::time::timeout(sh.drain, in_flight.wait_for(|n| *n == 0))
                .await
                .is_ok();
            if !drained {
                tracing::warn!("calls in flight did not finish in time; cutting them");
            }
            sh.phase.send_replace(Phase::Stopping);
            if let Some(core) = sh.core.lock().await.take() {
                core.stop().await;
            }
            sh.release();
            sh.phase.send_replace(Phase::Closed);
            sh.emit(OpenEvent::Closed);
            drop(embed);
        });
    }
}

/// Opens the home and starts the core (spec §2.5): takes `core.lock`,
/// retires an old daemon's `daemon.json`, opens the store and starts the
/// core with the embed's origins.
async fn opening(sh: Arc<Shared>, opts: OpenOptions) {
    let home = opts.home;
    let file = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join("core.lock"));
    let held = file.and_then(|f| match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    });
    match held {
        Ok(Some(f)) => *lock(&sh.lock) = Some(f),
        Ok(None) => {
            return failed(
                &sh,
                "home_in_use",
                "another process has this home open".into(),
                None,
            );
        }
        Err(e) => return failed(&sh, "open_failed", format!("core.lock: {e}"), None),
    }
    retire_daemon_json(&home);
    let cfg = CoreConfig {
        home: clax_core::Home::at(home.clone()),
        version: env!("CARGO_PKG_VERSION"),
        reap_interval: Duration::from_secs(60),
        codex: Default::default(),
        sample: Arc::new(clax_sample::Sampler::from_home_with_slot(
            &home,
            clax_sample::KeySlot::new(),
        )),
        web: crate::web::web_ui(),
        hosted: None,
        require_credential: true,
        bind: Ipv4Addr::LOCALHOST.into(),
        port: 0,
        embed: Some(opts.origins),
    };
    let mut last = String::new();
    let on_event = &sh.on_event;
    let started = Core::start(cfg, &mut |p| {
        let phase = p.phase.label();
        if phase != last {
            last = phase.to_string();
            on_event(OpenEvent::Opening {
                phase: phase.to_string(),
            });
        }
    })
    .await;
    match started {
        Ok(core) => {
            let ready = Phase::Ready {
                router: core.router(),
                token: core.state().token.clone(),
            };
            *sh.core.lock().await = Some(core);
            sh.phase.send_replace(ready);
            sh.emit(OpenEvent::Ready);
        }
        Err(e) => {
            let migration =
                clax_core::exit::status_of(e.as_ref()) == clax_core::exit::MIGRATION_FAILED;
            if migration {
                let backup = newest_backup(&home).map(|p| p.display().to_string());
                failed(&sh, "migration_failed", format!("{e:#}"), backup);
            } else {
                failed(&sh, "open_failed", format!("{e:#}"), None);
            }
        }
    }
}

/// Records a failed open: the home released, the phase `Failed`, and the
/// event posted.
fn failed(sh: &Shared, code: &str, message: String, backup: Option<String>) {
    tracing::error!(code, message, "the core did not open");
    sh.release();
    sh.phase.send_replace(Phase::Failed);
    sh.emit(OpenEvent::Failed {
        code: code.into(),
        message,
        backup,
    });
}

/// Renames an old app's `daemon.json` to `daemon.json.retired`: a `clax
/// serve` it started that still runs finds its file gone on two stale
/// checks and exits.
fn retire_daemon_json(home: &Path) {
    let file = home.join("daemon.json");
    if file.exists()
        && let Err(e) = std::fs::rename(&file, home.join("daemon.json.retired"))
    {
        tracing::warn!(error = %e, "could not retire daemon.json");
    }
}

/// The newest copy in the home's backups, by name (they are named by
/// their time).
fn newest_backup(home: &Path) -> Option<PathBuf> {
    let dir = clax_store::backup::dir(&clax_core::Home::at(home.to_path_buf()));
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "db"))
        .max_by(|a, b| {
            let t = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            t(a).cmp(&t(b)).then_with(|| a.cmp(b))
        })
}

/// Whether a frame speaking as `head.as_` may reach `head.path` (spec
/// §2.3); `Err` is the refusal's code.
pub fn admit(head: &RequestHead) -> Result<(), &'static str> {
    const REFUSED: &str = "not_in_app";
    let path = head.path.split(['?', '#']).next().unwrap_or("");
    if !path.starts_with('/') {
        return Err(REFUSED);
    }
    let read = matches!(head.method.as_str(), "GET" | "HEAD");
    let ok = match head.as_ {
        // A call's response is read whole, so a route that holds its
        // connection open never answers; events go by `subscribe`.
        As::Owner => {
            !clax_server::router::held_open::matches(path) && path != "/api/admin/shutdown"
        }
        As::ShellPage => {
            read && (["/a/", "/assets/", "/_clax/", "/c/", "/_blob/"]
                .iter()
                .any(|p| path.starts_with(p))
                || path == "/favicon.ico")
        }
        As::Content => read && head.host.as_deref().is_some_and(content_host),
    };
    if ok { Ok(()) } else { Err(REFUSED) }
}

/// Whether `id` names an artifact's content host: an artifact ID, as
/// `clax_http::host::artifact_host` reads `<id>.localhost`.
fn content_host(id: &str) -> bool {
    id.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && clax_http::host::artifact_host(&format!("{id}.localhost")).is_some()
}

/// The response to one frame, on the core's runtime.
async fn answer(sh: Arc<Shared>, head: RequestHead, body: Bytes) -> Vec<u8> {
    if admit(&head).is_err() {
        let path = head.path.split('?').next().unwrap_or("");
        return error_frame(
            404,
            "not_in_app",
            &format!("{path} is not served in the app"),
        );
    }
    // Counted before the phase is read, so a close that sees no call in
    // flight sees no call admitted after it began.
    let _counted = InFlight::enter(&sh.in_flight);
    let mut rx = sh.phase.subscribe();
    let ready = rx
        .wait_for(|p| !matches!(p, Phase::Opening))
        .await
        .map(|p| p.clone());
    let (router, token) = match ready {
        Ok(Phase::Ready { router, token }) => (router, token),
        Ok(Phase::Failed) => return error_frame(503, "core_failed", "the core did not open"),
        _ => return error_frame(503, "core_closed", "the core is closed"),
    };
    let req = match request(&head, body, &token) {
        Ok(r) => r,
        Err(e) => return error_frame(400, "bad_frame", &e),
    };
    let answered = async move {
        let res = router
            .oneshot(req)
            .await
            .unwrap_or_else(|never| match never {});
        respond(res, RESPONSE_LIMIT).await
    };
    tokio::select! {
        r = answered => r,
        _ = rx.wait_for(|p| matches!(p, Phase::Stopping | Phase::Closed)) => {
            error_frame(503, "core_closed", "the core closed before it answered")
        }
    }
}

/// The router's request for `head`: its headers but those the core sets,
/// the `Host` and credential for who it speaks as, and a loopback peer.
pub(crate) fn request(
    head: &RequestHead,
    body: Bytes,
    token: &str,
) -> Result<axum::extract::Request, String> {
    let method = axum::http::Method::from_bytes(head.method.as_bytes())
        .map_err(|_| format!("bad method {:?}", head.method))?;
    let mut b = axum::http::Request::builder()
        .method(method)
        .uri(head.path.as_str());
    let owner = head.as_ == As::Owner;
    let mut authorized = false;
    let mut length = false;
    for (k, v) in &head.headers {
        let name = k.to_ascii_lowercase();
        match name.as_str() {
            "host" => continue,
            "authorization" | "cookie" if !owner => continue,
            "authorization" => authorized = true,
            "content-length" => length = true,
            _ => {}
        }
        b = b.header(k.as_str(), v.as_str());
    }
    if owner && !authorized {
        b = b.header("authorization", format!("Bearer {token}"));
    }
    if !length && !body.is_empty() {
        b = b.header("content-length", body.len());
    }
    let host = match (&head.as_, &head.host) {
        (As::Content, Some(id)) => format!("{id}.localhost"),
        _ => "localhost".to_string(),
    };
    let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 1));
    b.header("host", host)
        .extension(axum::extract::ConnectInfo(clax_http::auth::Conn {
            peer,
            local: peer,
        }))
        .body(Body::from(body))
        .map_err(|e| e.to_string())
}

/// The response frame of `res`, its body read whole: 502
/// `response_too_large` past `limit` bytes, 502 `response_failed` when the
/// body fails while it is read.
async fn respond(res: axum::response::Response, limit: usize) -> Vec<u8> {
    let (parts, body) = res.into_parts();
    let body = match read_limited(body, limit).await {
        Ok(b) => b,
        Err(ReadError::TooLarge) => {
            return error_frame(
                502,
                "response_too_large",
                "the response is larger than the app reads",
            );
        }
        Err(ReadError::Failed(e)) => return error_frame(502, "response_failed", &e),
    };
    let headers = parts
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    frame::encode(
        &ResponseHead {
            status: parts.status.as_u16(),
            headers,
        },
        &body,
    )
}

/// Why [`read_limited`] read no body.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The body is longer than the limit.
    TooLarge,
    /// The body failed while it was read.
    Failed(String),
}

/// `body` read whole, if it is at most `limit` bytes.
///
/// # Errors
/// [`ReadError::TooLarge`] past the limit, else what the body failed with.
pub async fn read_limited(body: Body, limit: usize) -> Result<Bytes, ReadError> {
    match http_body_util::Limited::new(body, limit).collect().await {
        Ok(c) => Ok(c.to_bytes()),
        Err(e) if e.is::<http_body_util::LengthLimitError>() => Err(ReadError::TooLarge),
        Err(e) => Err(ReadError::Failed(e.to_string())),
    }
}

/// A response frame with an API error body.
pub fn error_frame(status: u16, code: &str, message: &str) -> Vec<u8> {
    let body = serde_json::json!({"error": {"code": code, "message": message}});
    frame::encode(
        &ResponseHead {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
        },
        body.to_string().as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(method: &str, path: &str, as_: As, host: Option<&str>) -> RequestHead {
        RequestHead {
            method: method.into(),
            path: path.into(),
            headers: vec![],
            as_,
            host: host.map(str::to_string),
        }
    }

    /// The origins the tests' cores give their WebViews.
    fn origins() -> EmbedOrigins {
        EmbedOrigins {
            shell: "clax://app".into(),
            content: "clax://{id}.c".into(),
        }
    }

    fn options(dir: &Path) -> OpenOptions {
        OpenOptions {
            home: dir.join("home"),
            origins: origins(),
            log: None,
        }
    }

    /// The next open event that is not an `Opening`, waited for off the
    /// test's runtime.
    async fn settled(rx: &std::sync::mpsc::Receiver<OpenEvent>) -> OpenEvent {
        tokio::task::block_in_place(|| loop {
            match rx.recv().expect("an open event") {
                OpenEvent::Opening { .. } => continue,
                e => return e,
            }
        })
    }

    fn events() -> (
        Box<dyn Fn(OpenEvent) + Send + Sync>,
        std::sync::mpsc::Receiver<OpenEvent>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        (
            Box::new(move |e| {
                let _ = lock(&tx).send(e);
            }),
            rx,
        )
    }

    /// A router whose `POST /hold` says when a request entered it, then
    /// answers 201 `made` once the gate opens.
    fn hold_router() -> (
        axum::Router,
        tokio::sync::mpsc::UnboundedReceiver<()>,
        Arc<tokio::sync::Notify>,
    ) {
        let (entered, entering) = tokio::sync::mpsc::unbounded_channel();
        let gate = Arc::new(tokio::sync::Notify::new());
        let open = gate.clone();
        let router = axum::Router::new().route(
            "/hold",
            axum::routing::post(move || {
                let (entered, open) = (entered.clone(), open.clone());
                async move {
                    let _ = entered.send(());
                    open.notified().await;
                    (axum::http::StatusCode::CREATED, "made")
                }
            }),
        );
        (router, entering, gate)
    }

    /// Opens a core, then has its calls answered by [`hold_router`].
    async fn holding(
        embed: &Embed,
        rx: &std::sync::mpsc::Receiver<OpenEvent>,
    ) -> (
        tokio::sync::mpsc::UnboundedReceiver<()>,
        Arc<tokio::sync::Notify>,
    ) {
        assert_eq!(settled(rx).await, OpenEvent::Ready);
        let (router, entering, gate) = hold_router();
        embed.inner.shared.phase.send_replace(Phase::Ready {
            router,
            token: "t".into(),
        });
        (entering, gate)
    }

    /// Sends `method path` as the owner on a task of the test's runtime.
    fn send(embed: &Embed, method: &str, path: &str) -> tokio::task::JoinHandle<(u16, Vec<u8>)> {
        let embed = embed.clone();
        let req = frame::encode(&head(method, path, As::Owner, None), b"");
        tokio::spawn(async move {
            let res = embed.call(&req).await.expect("a frame");
            let (h, body) = frame::split(&res).unwrap();
            let h: ResponseHead = serde_json::from_slice(h).unwrap();
            (h.status, body.to_vec())
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_call_in_flight_at_close_answers_with_its_own_status() {
        let dir = tempfile::tempdir().unwrap();
        let (on_event, rx) = events();
        let embed = Embed::open(options(dir.path()), on_event).unwrap();
        let (mut entering, gate) = holding(&embed, &rx).await;
        let held = send(&embed, "POST", "/hold");
        entering.recv().await.unwrap();

        embed.close();
        let mut phase = embed.inner.shared.phase.subscribe();
        let _ = phase
            .wait_for(|p| !matches!(p, Phase::Ready { .. }))
            .await;
        let (status, body) = send(&embed, "GET", "/api/artifacts").await.unwrap();
        assert_eq!(status, 503, "a call made while closing is refused");
        assert!(String::from_utf8_lossy(&body).contains("core_closed"));

        gate.notify_one();
        let (status, body) = held.await.unwrap();
        assert_eq!((status, &body[..]), (201, &b"made"[..]));
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_call_still_running_when_the_drain_ends_answers_core_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (on_event, rx) = events();
        let embed = Embed::open_draining(options(dir.path()), on_event, Duration::ZERO).unwrap();
        let (mut entering, gate) = holding(&embed, &rx).await;
        let held = send(&embed, "POST", "/hold");
        entering.recv().await.unwrap();

        embed.close();
        let (status, body) = held.await.unwrap();
        assert_eq!(status, 503);
        assert!(String::from_utf8_lossy(&body).contains("core_closed"));
        gate.notify_one();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_long_poll_in_flight_at_close_is_told_to_end_and_answers_200() {
        let dir = tempfile::tempdir().unwrap();
        let (on_event, rx) = events();
        let embed = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        let body = br#"{"harness":"app","harness_session_id":"install-1","cwd":"/w"}"#;
        let mut h = head("POST", "/api/sessions", As::Owner, None);
        h.headers
            .push(("content-type".into(), "application/json".into()));
        let res = embed.call(&frame::encode(&h, body)).await.unwrap();
        let (_, created) = frame::split(&res).unwrap();
        let created: serde_json::Value = serde_json::from_slice(created).unwrap();
        let sid = created["session"]["id"].as_str().unwrap();

        let mut in_flight = embed.inner.shared.in_flight.subscribe();
        let poll = send(
            &embed,
            "GET",
            &format!("/api/sessions/{sid}/feedback?tier=wait&wait=600"),
        );
        let _ = in_flight.wait_for(|n| *n == 1).await;
        embed.close();
        let (status, body) = poll.await.unwrap();
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
    }

    /// A response frame's status and body.
    fn status_and_body(frame: &[u8]) -> (u16, Vec<u8>) {
        let (h, body) = frame::split(frame).unwrap();
        let h: ResponseHead = serde_json::from_slice(h).unwrap();
        (h.status, body.to_vec())
    }

    #[tokio::test]
    async fn a_body_past_the_limit_answers_502_response_too_large() {
        let res = |n| axum::response::Response::new(Body::from(vec![7u8; n]));
        let (status, body) = status_and_body(&respond(res(9), 8).await);
        assert_eq!(status, 502);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "response_too_large", "{v}");
        assert_eq!(status_and_body(&respond(res(8), 8).await), (200, vec![7u8; 8]));
    }

    #[tokio::test]
    async fn a_body_that_fails_while_read_answers_502_response_failed() {
        let chunks: Vec<Result<Bytes, std::io::Error>> =
            vec![Ok(Bytes::from_static(b"half")), Err(std::io::Error::other("cut"))];
        let res = axum::response::Response::new(Body::from_stream(futures::stream::iter(chunks)));
        let (status, body) = status_and_body(&respond(res, RESPONSE_LIMIT).await);
        assert_eq!(status, 502);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "response_failed", "{v}");
    }

    #[test]
    fn each_speaker_reaches_only_its_paths() {
        let id = Some("7q3k9mzx2b4t");
        for (m, p, a, h, ok) in [
            ("GET", "/api/artifacts", As::Owner, None, true),
            ("POST", "/api/stream/x", As::Owner, None, true),
            ("GET", "/api/stream", As::Owner, None, false),
            ("GET", "/api/stream?x=1", As::Owner, None, false),
            ("GET", "/api/events", As::Owner, None, false),
            ("GET", "/api/events?since=3", As::Owner, None, false),
            ("GET", "/api/artifacts/7q3k9mzx2b4t/room", As::Owner, None, false),
            ("GET", "/api/artifacts/7q3k9mzx2b4t/live", As::Owner, None, false),
            ("GET", "/mcp", As::Owner, None, false),
            ("POST", "/mcp", As::Owner, None, false),
            ("GET", "/api/artifacts/7q3k9mzx2b4t/live/x", As::Owner, None, true),
            ("GET", "/api/artifacts/7q3k9mzx2b4t/threads", As::Owner, None, true),
            ("POST", "/api/admin/shutdown", As::Owner, None, false),
            ("GET", "api/artifacts", As::Owner, None, false),
            ("GET", "/a/7q3k9mzx2b4t?app=1", As::ShellPage, None, true),
            ("HEAD", "/assets/x.js", As::ShellPage, None, true),
            ("GET", "/favicon.ico", As::ShellPage, None, true),
            ("GET", "/api/token", As::ShellPage, None, false),
            ("GET", "/", As::ShellPage, None, false),
            ("POST", "/a/x", As::ShellPage, None, false),
            ("GET", "/v/1/", As::Content, id, true),
            ("GET", "/api/artifacts", As::Content, id, true),
            ("PUT", "/v/1/", As::Content, id, false),
            ("GET", "/v/1/", As::Content, None, false),
            ("GET", "/v/1/", As::Content, Some("7Q3K9MZX2B4T"), false),
            ("GET", "/v/1/", As::Content, Some("x.localhost:1"), false),
        ] {
            assert_eq!(admit(&head(m, p, a, h)).is_ok(), ok, "{m} {p} {a:?} {h:?}");
        }
    }
}
