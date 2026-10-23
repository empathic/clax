//! One open core in this process ([`Embed`]): its home opened on a runtime
//! of its own, and request frames answered through its router by who they
//! speak for (spec 2026-10-19 §2.3, §2.5).

use crate::frame::{self, As, FrameError, RequestHead, ResponseHead};
use axum::body::Body;
use bytes::Bytes;
use clax_http::state::EmbedOrigins;
use clax_server::core::{Core, CoreConfig};
use futures::FutureExt;
use http_body_util::BodyExt;
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::panic::AssertUnwindSafe;
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
    /// The store is opening: `backing up` (the copy before a migration),
    /// `migrating` (with the migrations `done` of `total`), then the
    /// audit backfill's phases.
    Opening {
        phase: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        done: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
    },
    /// The core answers calls.
    Ready,
    /// The core did not open: `home_in_use`, `migration_failed` (with the
    /// newest backup's path), `open_failed`, or `internal` (it panicked).
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
    /// Where the rolling log goes; none when `None`. The C functions pass
    /// `<home>/logs/core.log`.
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
    Ready {
        router: axum::Router,
        token: String,
    },
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

/// Tests: where a core's task panics.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanicAt {
    /// The opening task, once it holds `core.lock`.
    Opening,
    /// The close task, before it stops the core.
    Closing,
}

#[cfg(test)]
thread_local! {
    /// Tests: where the next core [`Embed::open`] makes on this thread
    /// panics.
    static PANIC_AT: std::cell::Cell<Option<PanicAt>> = const { std::cell::Cell::new(None) };
    /// Tests: what the next core [`Embed::open`] makes on this thread
    /// waits for, once it holds `core.lock`, before it opens the store.
    pub(crate) static HOLD_OPENING: std::cell::RefCell<Option<Arc<tokio::sync::Semaphore>>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests: panics when the core was opened to panic `here`.
#[cfg(test)]
fn panic_if(sh: &Shared, here: PanicAt) {
    if sh.panic_at == Some(here) {
        panic!("a panic injected at {here:?}");
    }
}

/// A home's key in [`OPEN`]: its canonical path, or the path as given
/// when it cannot be made canonical.
pub(crate) fn home_key(home: &Path) -> PathBuf {
    std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf())
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
    lock: Mutex<Option<clax_core::daemon::FileLock>>,
    released: AtomicBool,
    closing: AtomicBool,
    /// The calls admitted and not yet answered.
    in_flight: watch::Sender<usize>,
    /// How long [`Embed::close`] lets calls in flight finish.
    drain: Duration,
    /// The sites' cookies, kept for the content frames
    /// ([`crate::site_jar`]).
    jar: crate::site_jar::SiteJar,
    /// An artifact's content origin with `{id}` for its ID (the embed's
    /// `content`): the `Origin` of the site calls the shell relays.
    content: String,
    #[cfg(test)]
    panic_at: Option<PanicAt>,
    #[cfg(test)]
    hold_opening: Option<Arc<tokio::sync::Semaphore>>,
}

/// Aborts a task when dropped; once the task is done, that does nothing.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
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

/// A call counted in [`Shared::in_flight`] until it is dropped, once its
/// answer has been delivered ([`Embed::count_call`]).
pub(crate) struct Counted(Arc<Shared>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.in_flight.send_modify(|n| *n -= 1);
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
        let key = home_key(&opts.home);
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
            jar: crate::site_jar::SiteJar::default(),
            content: opts.origins.content.clone(),
            #[cfg(test)]
            panic_at: PANIC_AT.take(),
            #[cfg(test)]
            hold_opening: HOLD_OPENING.take(),
        });
        let handle = runtime.handle().clone();
        // A panic while opening fails the open as any failure does: the
        // home is released and `failed` posted, so no call or
        // subscription waits on it.
        let sh = shared.clone();
        handle.spawn(async move {
            let opened = AssertUnwindSafe(opening(sh.clone(), opts))
                .catch_unwind()
                .await;
            if opened.is_err() {
                failed(
                    &sh,
                    "internal",
                    "the core panicked while it opened".into(),
                    None,
                );
            }
        });
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
        Ok(self
            .answered(head, Bytes::copy_from_slice(body), Vec::new())
            .await)
    }

    /// [`Embed::call`] on a frame the caller owns, its body taken as a
    /// slice of it, the response frame written after `prefix` (a call's
    /// tag) in the one buffer that is posted.
    ///
    /// # Errors
    /// A [`FrameError`] for a frame that does not parse.
    pub(crate) async fn call_owned(
        &self,
        bytes: Bytes,
        prefix: Vec<u8>,
    ) -> Result<Vec<u8>, FrameError> {
        let (head, body) = frame::decode_request(&bytes)?;
        let body = bytes.slice_ref(body);
        Ok(self.answered(head, body, prefix).await)
    }

    /// The answer, on a task of its own so a panic answers 500; dropping
    /// this future (a cancelled call) aborts that task too.
    async fn answered(&self, head: RequestHead, body: Bytes, prefix: Vec<u8>) -> Vec<u8> {
        let task = self.inner.handle.spawn(answer(
            self.inner.shared.clone(),
            head,
            body,
            prefix.clone(),
        ));
        let _abort = AbortOnDrop(task.abort_handle());
        match task.await {
            Ok(answer) => answer,
            Err(e) if e.is_panic() => error_frame_after(
                &prefix,
                500,
                "internal",
                "the core panicked while it answered",
            ),
            Err(_) => error_frame_after(
                &prefix,
                503,
                "core_closed",
                "the core stopped before it answered",
            ),
        }
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

    /// Counts a call in flight until the returned guard is dropped: a host
    /// that delivers the answer after [`Embed::call`] returns holds it
    /// until then, so a close, which waits for the calls in flight, posts
    /// `Closed` only after every answer it drained was delivered.
    pub(crate) fn count_call(&self) -> Counted {
        let sh = self.inner.shared.clone();
        sh.in_flight.send_modify(|n| *n += 1);
        Counted(sh)
    }

    /// The key of the home this core holds ([`home_key`]).
    pub(crate) fn home(&self) -> &Path {
        &self.inner.shared.key
    }

    /// Whether the core still holds its home: false once a failed open or
    /// a close released it.
    pub(crate) fn holds_home(&self) -> bool {
        !self.inner.shared.released.load(Ordering::SeqCst)
    }

    /// Whether [`Embed::close`] was called.
    pub(crate) fn closing(&self) -> bool {
        self.inner.shared.closing.load(Ordering::SeqCst)
    }

    /// Tests: the calls counted in flight.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        *self.inner.shared.in_flight.borrow()
    }

    /// Spawns `fut` on the core's runtime.
    pub fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(
        &self,
        fut: F,
    ) -> tokio::task::JoinHandle<()> {
        self.inner.handle.spawn(fut)
    }

    /// Stops the core as `clax serve` stops on SIGTERM: new calls answer
    /// 503 `core_closed`; streams and long polls are told to end, and calls
    /// in flight finish for up to [`clax_server::serve::DRAIN_TIMEOUT`]
    /// (one that committed a write answers as it would have), after which
    /// those still running answer 503 `core_closed`; then the tasks stop,
    /// the journal syncs, the store closes, and the home is released.
    /// Returns at once; `on_event` hears `Closed` when done, a panic on
    /// the way included. A second close does nothing.
    pub fn close(&self) {
        let sh = &self.inner.shared;
        if sh.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        // The task holds the Embed, so the runtime outlives the close.
        let embed = self.clone();
        self.inner.handle.spawn(async move {
            let sh = embed.inner.shared.clone();
            let closed = AssertUnwindSafe(async {
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
                #[cfg(test)]
                panic_if(&sh, PanicAt::Closing);
                if let Some(core) = sh.core.lock().await.take() {
                    core.stop().await;
                }
            })
            .catch_unwind()
            .await;
            // A panic on the way still closes: calls are cut, a core left
            // is stopped, and the home is released.
            if closed.is_err() {
                tracing::error!("the core panicked while it closed");
                sh.phase.send_replace(Phase::Stopping);
                let left = sh.core.lock().await.take();
                if let Some(core) = left
                    && AssertUnwindSafe(core.stop()).catch_unwind().await.is_err()
                {
                    tracing::error!("the core panicked while it stopped");
                }
            }
            sh.release();
            sh.phase.send_replace(Phase::Closed);
            sh.emit(OpenEvent::Closed);
            drop(embed);
        });
    }
}

/// The core's router as the embed serves it: a route that panics answers
/// 500 `internal`, so the panic ends neither the call's task nor a
/// subscription's.
pub(crate) fn embedded_router(router: axum::Router) -> axum::Router {
    router.layer(axum::middleware::from_fn(catch_panic))
}

/// The route's response, or 500 `internal` when the route panics.
async fn catch_panic(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    match AssertUnwindSafe(next.run(req)).catch_unwind().await {
        Ok(res) => res,
        Err(_) => {
            tracing::error!("a route of the core panicked");
            let body = serde_json::json!({"error": {
                "code": "internal",
                "message": "the core panicked while it answered",
            }});
            axum::response::Response::builder()
                .status(500)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("a response with a valid status and header")
        }
    }
}

/// How long an open waits for an earlier app's `clax serve` to end after
/// its SIGTERM: the daemon drains calls for up to 5 s, then closes its
/// store.
#[cfg(not(target_os = "ios"))]
const OLD_DAEMON_WAIT: Duration = Duration::from_secs(10);

/// Opens the home and starts the core (spec §2.5): takes `core.lock`,
/// stops the `clax serve` an old app's `daemon.json` names (off iOS,
/// `home_in_use` when it does not stop), retires the file, opens the
/// store and starts the core with the embed's origins.
async fn opening(sh: Arc<Shared>, opts: OpenOptions) {
    let home = opts.home;
    let file = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join("core.lock"));
    match file.and_then(clax_core::daemon::FileLock::try_exclusive) {
        Ok(Some(f)) => {
            *lock(&sh.lock) = Some(f);
            #[cfg(test)]
            panic_if(&sh, PanicAt::Opening);
            #[cfg(test)]
            if let Some(hold) = &sh.hold_opening {
                let _ = hold.acquire().await;
            }
        }
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
    // iOS never ran a daemon of its own; elsewhere an earlier app's `clax
    // serve` may still serve this home, and must be gone before the store
    // opens beside it.
    #[cfg(not(target_os = "ios"))]
    {
        use clax_core::daemon::OldDaemon;
        let h = clax_core::Home::at(home.clone());
        let old = tokio::task::spawn_blocking(move || {
            clax_core::daemon::stop_old_daemon(&h, OLD_DAEMON_WAIT)
        })
        .await;
        match old {
            Ok(OldDaemon::Running(pid)) => {
                return failed(
                    &sh,
                    "home_in_use",
                    format!(
                        "an earlier app's clax serve (PID {pid}) serves this home and did not stop"
                    ),
                    None,
                );
            }
            Ok(OldDaemon::Stopped(pid)) => {
                tracing::info!(pid, "stopped an earlier app's clax serve on this home");
            }
            Ok(OldDaemon::Absent) => {}
            Err(e) => return failed(&sh, "open_failed", format!("daemon.json: {e}"), None),
        }
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
    // One event per phase, and per migration.
    let mut last = None;
    let on_event = &sh.on_event;
    let started = Core::start(cfg, &mut |p| {
        let step = (p.phase == clax_store::backfill::Phase::Migrating).then_some((p.done, p.total));
        let at = (p.phase, step);
        if last != Some(at) {
            last = Some(at);
            on_event(OpenEvent::Opening {
                phase: p.phase.label().to_string(),
                done: step.map(|s| s.0),
                total: step.map(|s| s.1),
            });
        }
    })
    .await;
    match started {
        Ok(core) => {
            let ready = Phase::Ready {
                router: embedded_router(core.router()),
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

/// Renames an old app's `daemon.json` to `daemon.json.retired`, once the
/// `clax serve` it names is stopped (or, on iOS, which never ran one, at
/// once).
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
        As::Owner | As::OwnerBrowser => {
            !clax_server::router::held_open::matches(path) && path != "/api/admin/shutdown"
        }
        // The shell document and what the core serves it: the web UI's
        // files under `/_clax/`. Content and assets are each site's.
        As::ShellPage => read && ["/a/", "/_clax/"].iter().any(|p| path.starts_with(p)),
        As::Content => read && head.host.as_deref().is_some_and(content_host),
        // A content frame's own site API, any method, as the shell relays
        // it; never the room, a socket that holds its connection open.
        As::Site => {
            head.host.as_deref().is_some_and(content_host)
                && path.starts_with(SITE_API)
                && path != SITE_ROOM
                && !path.split('/').any(|seg| seg == "." || seg == "..")
        }
    };
    if ok { Ok(()) } else { Err(REFUSED) }
}

/// Where a site serves its API, which an `as: site` frame alone reaches.
const SITE_API: &str = "/_clax/api/";

/// A site's room, a WebSocket: no call.
const SITE_ROOM: &str = "/_clax/api/room";

/// Whether `id` names an artifact's content host: an artifact ID, as
/// `clax_http::host::artifact_host` reads `<id>.localhost`.
fn content_host(id: &str) -> bool {
    id.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && clax_http::host::artifact_host(&format!("{id}.localhost")).is_some()
}

/// The response to one frame, after `prefix`, on the core's runtime.
async fn answer(sh: Arc<Shared>, head: RequestHead, body: Bytes, prefix: Vec<u8>) -> Vec<u8> {
    let error_frame =
        |status, code, message: &str| error_frame_after(&prefix, status, code, message);
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
    let mut req = match request(&head, body, &token) {
        Ok(r) => r,
        Err(e) => return error_frame(400, "bad_frame", &e),
    };
    // A content frame's site cookies are the core's to keep: no WebView
    // keeps a cookie its scheme's answer sets (`crate::site_jar`). A site
    // call the shell relays for the frame carries the same cookies, and the
    // site's own origin, which the site API asks of every write.
    let site = match (&head.as_, &head.host) {
        (As::Content | As::Site, Some(id)) => Some(id.clone()),
        _ => None,
    };
    if let Some(cookie) = site.as_deref().and_then(|id| sh.jar.cookie_header(id))
        && let Ok(v) = axum::http::HeaderValue::from_str(&cookie)
    {
        req.headers_mut().insert(axum::http::header::COOKIE, v);
    }
    if let (As::Site, Some(id)) = (&head.as_, &site)
        && let Ok(v) = axum::http::HeaderValue::from_str(&sh.content.replace("{id}", id))
    {
        req.headers_mut().insert(axum::http::header::ORIGIN, v);
    }
    let answered = async {
        let mut res = router
            .oneshot(req)
            .await
            .unwrap_or_else(|never| match never {});
        if let Some(id) = &site {
            sh.jar.take(id, res.headers());
            // The jar has them: none reaches the WebView, or the app.
            res.headers_mut().remove(axum::http::header::SET_COOKIE);
        }
        respond(res, RESPONSE_LIMIT, &prefix).await
    };
    tokio::select! {
        r = answered => r,
        _ = rx.wait_for(|p| matches!(p, Phase::Stopping | Phase::Closed)) => {
            error_frame(503, "core_closed", "the core closed before it answered")
        }
    }
}

/// The headers a page's frame keeps (spec §2.3, §4.2): `Range` and
/// `Accept`, which the answer depends on, and `Service-Worker`, which the
/// kill switch reads. A page's frame drops every other, `If-None-Match` and
/// `If-Modified-Since` included: a conditional request would be answered
/// 304, which no WebView here takes from its host, and an answer made in
/// the app's own process costs nothing to send whole.
const PAGE_HEADERS: [&str; 3] = ["range", "accept", "service-worker"];

/// The headers a site call the shell relays keeps: `Content-Type`, which
/// the site reads a write's body by (JSON, or an upload's multipart form),
/// and `Accept`. Its `Origin` and `Cookie` are the core's to set.
const SITE_HEADERS: [&str; 2] = ["content-type", "accept"];

/// The headers a content frame keeps besides [`PAGE_HEADERS`]: where the
/// request came from, as the WebView says (`Origin`, `Sec-Fetch-Site`,
/// `-Mode`, `-Dest`), so the site's own checks hold in the app as in a
/// browser: a site's API refuses a read another site's page makes
/// (`Origin` or `Sec-Fetch-Site` naming elsewhere), whatever the content
/// policy also stops.
const CONTENT_HEADERS: [&str; 4] = [
    "origin",
    "sec-fetch-site",
    "sec-fetch-mode",
    "sec-fetch-dest",
];

/// Whether `path` asks for the shell document, `/a/…`: the one path a
/// shell-page frame is asked for as the owner.
fn is_shell_document(path: &str) -> bool {
    path.split(['?', '#'])
        .next()
        .is_some_and(|p| p.starts_with("/a/"))
}

/// The router's request for `head`: its headers but those the core sets
/// (an owner or owner-browser frame's all but an owner-browser frame's
/// `Cookie`; a site frame's only [`SITE_HEADERS`]; any other page frame's
/// only [`PAGE_HEADERS`], a content frame's with [`CONTENT_HEADERS`]), the
/// `Host` (a content or site frame's its
/// site's) and
/// credential for who it speaks as (an owner frame the token, an
/// owner-browser frame the token and the owner cookie, a shell-page frame
/// for the shell document the owner cookie, any other page frame none),
/// and a loopback peer.
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
    let browser = head.as_ == As::OwnerBrowser;
    let owner = browser || head.as_ == As::Owner;
    let mut authorized = false;
    let mut length = false;
    let kept: &[&str] = if head.as_ == As::Site {
        &SITE_HEADERS
    } else {
        &PAGE_HEADERS
    };
    for (k, v) in &head.headers {
        let name = k.to_ascii_lowercase();
        let from = head.as_ == As::Content && CONTENT_HEADERS.contains(&name.as_str());
        if !owner && !kept.contains(&name.as_str()) && !from {
            continue;
        }
        match name.as_str() {
            "host" => continue,
            "cookie" if browser => continue,
            "authorization" => authorized = true,
            "content-length" => length = true,
            _ => {}
        }
        b = b.header(k.as_str(), v.as_str());
    }
    if owner && !authorized {
        b = b.header("authorization", format!("Bearer {token}"));
    }
    // Any frame of the WebView can make a request to the shell's host (a
    // page's `<script src>`, `<img src>` or fetch included), so a
    // shell-page frame is the owner's browser only for the shell document,
    // `/a/…`, which the host answers to the main frame's navigation alone
    // (spec §4.2, §4.3). The rest it reaches (`/_clax/`) answers without a
    // credential and is asked for with none. A content frame carries no
    // credential of the owner's: only its own site's cookies, which the
    // core adds from its jar (`crate::site_jar`). The shell's API calls its host relays are
    // the owner's browser's, as a browser's shell is with the owner cookie
    // its token request set.
    if browser || (head.as_ == As::ShellPage && is_shell_document(&head.path)) {
        b = b.header(
            "cookie",
            format!(
                "{}={}",
                clax_http::identity::owner_cookie_name("localhost"),
                clax_http::identity::owner_cookie_value(token)
            ),
        );
    }
    if !length && !body.is_empty() {
        b = b.header("content-length", body.len());
    }
    let host = match (&head.as_, &head.host) {
        (As::Content | As::Site, Some(id)) => format!("{id}.localhost"),
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

/// `prefix`, then the response frame of `res`, its body read whole: 502
/// `response_too_large` past `limit` bytes (at once, unread, when its size
/// hint already says so), 502 `response_failed` when the body fails while
/// it is read. The frame is one buffer, sized from the body's size hint,
/// and each chunk of the body is copied into it once; a body whose size is
/// not known ahead may make the buffer grow.
async fn respond(res: axum::response::Response, limit: usize, prefix: &[u8]) -> Vec<u8> {
    let too_large = || {
        error_frame_after(
            prefix,
            502,
            "response_too_large",
            "the response is larger than the app reads",
        )
    };
    let hint = axum::body::HttpBody::size_hint(res.body());
    if hint.lower() > limit as u64 {
        return too_large();
    }
    let (parts, mut body) = res.into_parts();
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
    let head = ResponseHead {
        status: parts.status.as_u16(),
        headers,
    };
    let expected = hint.exact().unwrap_or(hint.lower()).min(limit as u64);
    let mut out = frame::begin(prefix, &head, expected as usize);
    let start = out.len();
    while let Some(next) = body.frame().await {
        match next {
            Ok(f) => {
                let Ok(data) = f.into_data() else { continue };
                if out.len() - start + data.len() > limit {
                    return too_large();
                }
                out.extend_from_slice(&data);
            }
            Err(e) => return error_frame_after(prefix, 502, "response_failed", &e.to_string()),
        }
    }
    out
}

/// A response frame with an API error body.
pub fn error_frame(status: u16, code: &str, message: &str) -> Vec<u8> {
    error_frame_after(&[], status, code, message)
}

/// `prefix`, then [`error_frame`]'s frame.
pub fn error_frame_after(prefix: &[u8], status: u16, code: &str, message: &str) -> Vec<u8> {
    let body = serde_json::json!({"error": {"code": code, "message": message}});
    frame::encode_after(
        prefix,
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
        tokio::task::block_in_place(|| {
            loop {
                match rx.recv().expect("an open event") {
                    OpenEvent::Opening { .. } => continue,
                    e => return e,
                }
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
        let _ = phase.wait_for(|p| !matches!(p, Phase::Ready { .. })).await;
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_closed_core_leaves_no_task_beside_a_reopen_of_its_home() {
        let dir = tempfile::tempdir().unwrap();
        let (on_event, rx) = events();
        let old = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        old.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
        // The same home opens again in this process while the old core's
        // runtime lives on (`old` is kept): nothing of the old core runs
        // there but its close task, which ends once it posted `closed`.
        let (on_event, again_rx) = events();
        let again = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&again_rx).await, OpenEvent::Ready);
        let alive = old.inner.handle.metrics().num_alive_tasks();
        assert!(alive <= 1, "{alive} tasks of the closed core still run");
        again.close();
        assert_eq!(settled(&again_rx).await, OpenEvent::Closed);
    }

    /// Opens a core on `$CLAX_TEST_HOLD_HOME` and holds it while the test
    /// that starts it asks, when run as a child of
    /// [`a_home_a_core_in_another_process_holds_is_in_use_until_it_closes`]:
    /// it prints `ready` once its core is ready, then closes the core when
    /// its stdin closes and prints `closed`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "a helper process, run by a_home_a_core_in_another_process_holds_is_in_use_until_it_closes"]
    async fn hold_home_open() {
        let Ok(dir) = std::env::var("CLAX_TEST_HOLD_HOME") else {
            return;
        };
        let (on_event, rx) = events();
        let embed = Embed::open(options(Path::new(&dir)), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        println!("ready");
        tokio::task::block_in_place(|| {
            let mut rest = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut rest);
        });
        embed.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
        println!("closed");
    }

    /// `core.lock` keeps a home to one core across processes: while a core
    /// in another process holds it, an open here fails `home_in_use`; once
    /// that core closes, the home opens here.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_home_a_core_in_another_process_holds_is_in_use_until_it_closes() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().unwrap();
        let mut holder = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "core::tests::hold_home_open",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("CLAX_TEST_HOLD_HOME", dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = std::io::BufReader::new(holder.stdout.take().unwrap()).lines();
        // Whether the holder prints `line` before its stdout ends.
        let mut hears =
            move |line: &str| tokio::task::block_in_place(|| lines.any(|l| l.unwrap() == line));
        assert!(hears("ready"), "the other process's core is ready");

        let (on_event, rx) = events();
        let refused = Embed::open(options(dir.path()), on_event).unwrap();
        match settled(&rx).await {
            OpenEvent::Failed { code, .. } => assert_eq!(code, "home_in_use"),
            e => panic!("{e:?}"),
        }
        refused.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);

        drop(holder.stdin.take());
        assert!(hears("closed"), "the other process's core closed");
        let (on_event, rx) = events();
        let again = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        again.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
        assert!(
            tokio::task::block_in_place(|| holder.wait())
                .unwrap()
                .success()
        );
    }

    /// Opens a core whose task panics at `at`.
    fn open_panicking(dir: &Path, at: PanicAt) -> (Embed, std::sync::mpsc::Receiver<OpenEvent>) {
        let (on_event, rx) = events();
        PANIC_AT.set(Some(at));
        let embed = Embed::open(options(dir), on_event).unwrap();
        (embed, rx)
    }

    /// The end block a subscription to `embed` hears last.
    async fn subscription_end(embed: &Embed) -> Option<Bytes> {
        let (tx, mut blocks) = tokio::sync::mpsc::unbounded_channel();
        embed.subscribe(Box::new(move |b| tx.send(b).is_ok()));
        let mut last = None;
        while let Some(b) = blocks.recv().await {
            last = Some(b);
        }
        last
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panic_while_opening_fails_the_open_and_frees_the_home() {
        let dir = tempfile::tempdir().unwrap();
        let (embed, rx) = open_panicking(dir.path(), PanicAt::Opening);
        match settled(&rx).await {
            OpenEvent::Failed { code, .. } => assert_eq!(code, "internal"),
            e => panic!("{e:?}"),
        }
        let (status, body) = send(&embed, "GET", "/api/artifacts").await.unwrap();
        assert_eq!(status, 503);
        assert!(String::from_utf8_lossy(&body).contains("core_failed"));
        assert_eq!(
            subscription_end(&embed).await,
            Some(crate::events::end("core_failed"))
        );
        embed.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
        let (on_event, rx) = events();
        let again = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        again.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panic_while_closing_still_closes_and_frees_the_home() {
        let dir = tempfile::tempdir().unwrap();
        let (embed, rx) = open_panicking(dir.path(), PanicAt::Closing);
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        let (tx, mut blocks) = tokio::sync::mpsc::unbounded_channel();
        embed.subscribe(Box::new(move |b| tx.send(b).is_ok()));
        assert!(blocks.recv().await.unwrap().starts_with(b"event: ready\n"));
        embed.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
        let mut last = None;
        while let Some(b) = blocks.recv().await {
            last = Some(b);
        }
        assert_eq!(last, Some(crate::events::end("core_closed")));
        let (status, body) = send(&embed, "GET", "/api/artifacts").await.unwrap();
        assert_eq!(status, 503);
        assert!(String::from_utf8_lossy(&body).contains("core_closed"));
        let (on_event, rx) = events();
        let again = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        again.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stream_refused_because_the_core_is_closing_ends_core_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (on_event, rx) = events();
        let embed = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        // The stream reaches a router that is closing: it leaves Ready
        // before it answers, and refuses.
        let phase = embed.inner.shared.phase.clone();
        let router = axum::Router::new().route(
            "/api/stream",
            axum::routing::get(move || {
                let phase = phase.clone();
                async move {
                    phase.send_replace(Phase::Draining);
                    axum::http::StatusCode::SERVICE_UNAVAILABLE
                }
            }),
        );
        embed.inner.shared.phase.send_replace(Phase::Ready {
            router,
            token: "t".into(),
        });
        let (tx, blocks) = tokio::sync::mpsc::unbounded_channel();
        embed.subscribe(Box::new(move |b| tx.send(b).is_ok()));
        let mut blocks = blocks;
        let mut last = None;
        while let Some(b) = blocks.recv().await {
            last = Some(b);
        }
        assert_eq!(last, Some(crate::events::end("core_closed")));
        embed.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stream_a_ready_core_refuses_ends_with_the_refusals_status() {
        let dir = tempfile::tempdir().unwrap();
        let (on_event, rx) = events();
        let embed = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        for status in [503u16, 403] {
            let router = axum::Router::new().route(
                "/api/stream",
                axum::routing::get(move || async move {
                    axum::http::StatusCode::from_u16(status).unwrap()
                }),
            );
            embed.inner.shared.phase.send_replace(Phase::Ready {
                router,
                token: "t".into(),
            });
            let end = subscription_end(&embed).await.unwrap();
            assert_eq!(
                String::from_utf8_lossy(&end),
                format!(
                    "event: end\ndata: {{\"reason\":\"stream_refused\",\"status\":{status}}}\n\n"
                )
            );
        }
        embed.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_route_that_panics_answers_500_internal_and_the_stream_ends_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (on_event, rx) = events();
        let embed = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        let boom = || async {
            panic!("a panic injected in a route");
            #[allow(unreachable_code)]
            ""
        };
        let router = axum::Router::new()
            .route("/api/boom", axum::routing::get(boom))
            .route("/api/stream", axum::routing::get(boom));
        embed.inner.shared.phase.send_replace(Phase::Ready {
            router: embedded_router(router),
            token: "t".into(),
        });
        let (status, body) = send(&embed, "GET", "/api/boom").await.unwrap();
        assert_eq!(status, 500);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "internal", "{v}");
        assert_eq!(
            subscription_end(&embed).await,
            Some(crate::events::refused(500))
        );
        embed.close();
        assert_eq!(settled(&rx).await, OpenEvent::Closed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stream_whose_body_panics_ends_internal() {
        let dir = tempfile::tempdir().unwrap();
        let (on_event, rx) = events();
        let embed = Embed::open(options(dir.path()), on_event).unwrap();
        assert_eq!(settled(&rx).await, OpenEvent::Ready);
        let router = axum::Router::new().route(
            "/api/stream",
            axum::routing::get(|| async {
                let first = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(
                    b"event: ready\ndata: {}\n\n",
                ))]);
                let then = futures::stream::once(async {
                    panic!("a panic injected in the stream's body");
                    #[allow(unreachable_code)]
                    Ok(Bytes::new())
                });
                Body::from_stream(futures::StreamExt::chain(first, then))
            }),
        );
        embed.inner.shared.phase.send_replace(Phase::Ready {
            router: embedded_router(router),
            token: "t".into(),
        });
        let (tx, mut blocks) = tokio::sync::mpsc::unbounded_channel();
        embed.subscribe(Box::new(move |b| tx.send(b).is_ok()));
        let mut got = Vec::new();
        while let Some(b) = blocks.recv().await {
            got.push(b);
        }
        assert_eq!(
            got,
            [
                Bytes::from_static(b"event: ready\ndata: {}\n\n"),
                crate::events::end("internal")
            ]
        );
        embed.close();
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
        let (status, body) = status_and_body(&respond(res(9), 8, &[]).await);
        assert_eq!(status, 502);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "response_too_large", "{v}");
        assert_eq!(
            status_and_body(&respond(res(8), 8, &[]).await),
            (200, vec![7u8; 8])
        );
    }

    #[tokio::test]
    async fn a_body_whose_size_is_known_past_the_limit_is_refused_unread() {
        let read = Arc::new(AtomicBool::new(false));
        let seen = read.clone();
        // Its size hint is exact, 9 bytes, past a limit of 8; read, it
        // fails (20 bytes past its own 9), and the failure is seen.
        let body =
            http_body_util::Limited::new(http_body_util::Full::new(Bytes::from(vec![7u8; 20])), 9)
                .map_err(move |e| {
                    seen.store(true, Ordering::SeqCst);
                    e
                });
        let res = axum::response::Response::new(Body::new(body));
        let (status, body) = status_and_body(&respond(res, 8, &[]).await);
        assert_eq!(status, 502);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "response_too_large", "{v}");
        assert!(!read.load(Ordering::SeqCst), "the body is never read");
    }

    #[tokio::test]
    async fn a_body_that_fails_while_read_answers_502_response_failed() {
        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(b"half")),
            Err(std::io::Error::other("cut")),
        ];
        let res = axum::response::Response::new(Body::from_stream(futures::stream::iter(chunks)));
        let (status, body) = status_and_body(&respond(res, RESPONSE_LIMIT, &[]).await);
        assert_eq!(status, 502);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "response_failed", "{v}");
    }

    #[tokio::test]
    async fn a_response_is_framed_after_its_tag_in_one_buffer_sized_once() {
        let res = axum::response::Response::new(Body::from(vec![7u8; 1000]));
        let out = respond(res, RESPONSE_LIMIT, &frame::tag(5)).await;
        assert_eq!(&out[..8], &5i64.to_le_bytes());
        assert_eq!(status_and_body(&out[8..]), (200, vec![7u8; 1000]));
        assert_eq!(out.capacity(), out.len(), "no copy to grow it");
    }

    #[test]
    fn a_page_frame_keeps_few_headers_and_a_content_frame_where_it_came_from() {
        let sent = [
            ("Range", "bytes=0-9"),
            ("If-None-Match", "\"v1\""),
            ("If-Modified-Since", "Mon, 19 Oct 2026 00:00:00 GMT"),
            ("Accept", "text/html"),
            ("Service-Worker", "script"),
            ("x-clax-session", "s1"),
            ("x-clax-via", "page"),
            ("origin", "clax://app"),
            ("sec-fetch-site", "same-origin"),
            ("cookie", "clax_owner=x"),
            ("authorization", "Bearer forged"),
            ("host", "elsewhere"),
        ];
        for (as_, path) in [
            (As::Content, "/_clax/v/1/"),
            (As::ShellPage, "/a/7q3k9mzx2b4t"),
        ] {
            let mut h = head("GET", path, as_, Some("7q3k9mzx2b4t"));
            h.headers = sent
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let req = request(&h, Bytes::new(), "t").unwrap();
            let mut names: Vec<&str> = req.headers().keys().map(|k| k.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            let mut want = vec!["accept", "host", "range", "service-worker"];
            if as_ == As::Content {
                want.extend(["origin", "sec-fetch-site"]);
                want.sort_unstable();
            }
            if as_ == As::ShellPage {
                want.push("cookie");
                want.sort_unstable();
                let owner = format!(
                    "{}={}",
                    clax_http::identity::owner_cookie_name("localhost"),
                    clax_http::identity::owner_cookie_value("t")
                );
                assert_eq!(req.headers()["cookie"], owner.as_str());
            }
            assert_eq!(names, want, "{as_:?}");
            assert_ne!(req.headers()["host"], "elsewhere");
        }
    }

    #[test]
    fn a_site_frame_keeps_only_its_content_type_and_accept_and_names_its_site() {
        let sent = [
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
            ("Range", "bytes=0-9"),
            ("If-None-Match", "\"v1\""),
            ("origin", "clax://elsewhere.c"),
            ("cookie", "__Host-clax_site=planted"),
            ("authorization", "Bearer forged"),
            ("x-clax-session", "s1"),
            ("host", "elsewhere"),
        ];
        let mut h = head("PUT", "/_clax/api/docs/a/b", As::Site, Some("7q3k9mzx2b4t"));
        h.headers = sent
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let req = request(&h, Bytes::from_static(b"{}"), "t").unwrap();
        assert_eq!(req.method(), "PUT");
        let mut names: Vec<&str> = req.headers().keys().map(|k| k.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["accept", "content-length", "content-type", "host"]);
        assert_eq!(req.headers()["host"], "7q3k9mzx2b4t.localhost");
        assert_eq!(req.headers()["content-type"], "application/json");
        assert_eq!(req.headers()["content-length"], "2");
    }

    #[test]
    fn only_the_shell_document_is_asked_for_as_the_owner() {
        // What a page can make the WebView ask of the shell's host, by
        // `<script src>`, `<img src>` or a fetch, reaches the core as a
        // shell-page frame too: none of it carries a credential.
        for path in ["/_clax/shell/app.js", "/_clax/bridge.js"] {
            let req = request(&head("GET", path, As::ShellPage, None), Bytes::new(), "t").unwrap();
            assert!(req.headers().get("cookie").is_none(), "{path}");
            assert!(req.headers().get("authorization").is_none(), "{path}");
        }
        let req = request(
            &head("GET", "/a/7q3k9mzx2b4t?app=1", As::ShellPage, None),
            Bytes::new(),
            "t",
        )
        .unwrap();
        assert!(
            req.headers().get("cookie").is_some(),
            "the shell document's"
        );
    }

    #[test]
    fn an_owner_browser_frame_carries_the_owner_cookie_and_no_cookie_of_its_own() {
        let cookie = format!(
            "{}={}",
            clax_http::identity::owner_cookie_name("localhost"),
            clax_http::identity::owner_cookie_value("t")
        );
        let mut h = head("POST", "/api/artifacts/x/sample", As::OwnerBrowser, None);
        h.headers = vec![
            ("authorization".into(), "Bearer shell".into()),
            ("cookie".into(), "clax_viewer=planted".into()),
        ];
        let req = request(&h, Bytes::new(), "t").unwrap();
        let cookies: Vec<_> = req.headers().get_all("cookie").iter().collect();
        assert_eq!(cookies, [cookie.as_str()]);
        assert_eq!(req.headers()["authorization"], "Bearer shell");
        h.headers.clear();
        let req = request(&h, Bytes::new(), "t").unwrap();
        assert_eq!(req.headers()["authorization"], "Bearer t");
        // The app's own code is no browser.
        h.as_ = As::Owner;
        let req = request(&h, Bytes::new(), "t").unwrap();
        assert!(req.headers().get("cookie").is_none());
    }

    #[test]
    fn each_speaker_reaches_only_its_paths() {
        let id = Some("7q3k9mzx2b4t");
        for (m, p, a, h, ok) in [
            ("GET", "/api/artifacts", As::Owner, None, true),
            ("POST", "/api/stream/x", As::Owner, None, true),
            (
                "POST",
                "/api/artifacts/x/sample",
                As::OwnerBrowser,
                None,
                true,
            ),
            ("GET", "/api/stream", As::OwnerBrowser, None, false),
            ("POST", "/api/admin/shutdown", As::OwnerBrowser, None, false),
            ("GET", "/api/stream", As::Owner, None, false),
            ("GET", "/api/stream?x=1", As::Owner, None, false),
            ("GET", "/api/events", As::Owner, None, false),
            ("GET", "/api/events?since=3", As::Owner, None, false),
            (
                "GET",
                "/api/artifacts/7q3k9mzx2b4t/room",
                As::Owner,
                None,
                false,
            ),
            (
                "GET",
                "/api/artifacts/7q3k9mzx2b4t/live",
                As::Owner,
                None,
                false,
            ),
            ("GET", "/mcp", As::Owner, None, false),
            ("POST", "/mcp", As::Owner, None, false),
            (
                "GET",
                "/api/artifacts/7q3k9mzx2b4t/live/x",
                As::Owner,
                None,
                true,
            ),
            (
                "GET",
                "/api/artifacts/7q3k9mzx2b4t/threads",
                As::Owner,
                None,
                true,
            ),
            ("POST", "/api/admin/shutdown", As::Owner, None, false),
            ("GET", "api/artifacts", As::Owner, None, false),
            ("GET", "/a/7q3k9mzx2b4t?app=1", As::ShellPage, None, true),
            ("HEAD", "/assets/x.js", As::ShellPage, None, false),
            ("GET", "/favicon.ico", As::ShellPage, None, false),
            ("GET", "/api/token", As::ShellPage, None, false),
            ("GET", "/", As::ShellPage, None, false),
            ("POST", "/a/x", As::ShellPage, None, false),
            ("GET", "/c/7q3k9mzx2b4t/v/1/", As::ShellPage, None, false),
            (
                "GET",
                "/_blob/01ARZ3NDEKTSV4RRFFQ69G5FAV",
                As::ShellPage,
                None,
                false,
            ),
            ("GET", "/", As::Content, id, true),
            ("GET", "/_clax/v/1/", As::Content, id, true),
            (
                "GET",
                "/_clax/session?code=c&next=%2F",
                As::Content,
                id,
                true,
            ),
            ("GET", "/api/artifacts", As::Content, id, true),
            ("PUT", "/_clax/v/1/", As::Content, id, false),
            ("GET", "/_clax/v/1/", As::Content, None, false),
            (
                "GET",
                "/_clax/v/1/",
                As::Content,
                Some("7Q3K9MZX2B4T"),
                false,
            ),
            (
                "GET",
                "/_clax/v/1/",
                As::Content,
                Some("x.localhost:1"),
                false,
            ),
            ("POST", "/_clax/api/docs/a/b", As::Content, id, false),
            ("PUT", "/_clax/api/docs/a/b", As::Site, id, true),
            ("PATCH", "/_clax/api/docs/a/b", As::Site, id, true),
            ("DELETE", "/_clax/api/docs/a/b?lww=true", As::Site, id, true),
            ("POST", "/_clax/api/assets", As::Site, id, true),
            ("GET", "/_clax/api/whoami", As::Site, id, true),
            ("POST", "/_clax/api/docs/a/b", As::Site, None, false),
            (
                "POST",
                "/_clax/api/docs/a/b",
                As::Site,
                Some("not-an-id"),
                false,
            ),
            ("GET", "/_clax/api/room", As::Site, id, false),
            ("GET", "/_clax/api/room?peer=x", As::Site, id, false),
            ("POST", "/_clax/session", As::Site, id, false),
            ("GET", "/", As::Site, id, false),
            ("POST", "/api/artifacts", As::Site, id, false),
            ("POST", "/_clax/api/../api/artifacts", As::Site, id, false),
            ("POST", "/_clax/api/./docs/a/b", As::Site, id, false),
        ] {
            assert_eq!(admit(&head(m, p, a, h)).is_ok(), ok, "{m} {p} {a:?} {h:?}");
        }
    }
}
