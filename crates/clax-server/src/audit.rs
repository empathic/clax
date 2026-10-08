//! The audit journal's daemon side (spec 2026-10-06-toolpath-audit-design
//! §4, §7): the [`AuditCtx`] each request that acts resolves, the wake-up
//! the store sends the journal appender after a commit that recorded events,
//! and the [`Appender`], the thread that writes the journal.
//!
//! The context is built from what the daemon already trusts. The actor comes
//! from [`Identity`] ([`Identity::audit_actor`]), never from a second check
//! of credentials. The agent side's own claims (`x-clax-via`,
//! `x-clax-session`, `x-clax-git`, `x-clax-call`) count only on a request
//! that carries the token; a browser's are ignored. A header that does not
//! decode never fails the request: a bad `x-clax-git` is the capture
//! outcome `invalid`, a bad `x-clax-call` is no call, and a bad `x-clax-via`
//! or `x-clax-session` is inferred as though absent. Header values are never
//! logged.

use crate::identity::Identity;
use crate::routes::artifacts::{SESSION_HEADER, VIA_HEADER};
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::request::Parts;
use clax_core::audit::{Actor, AuditCtx, CallHeader, Via, decode_call_header};
use clax_core::gitctx::{self, GitField};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

/// Header carrying the agent side's git context (base64url JSON, §9).
pub const GIT_HEADER: &str = "x-clax-git";

/// Header carrying the identity of the tool call a request is made under
/// (base64url JSON, §6.7).
pub const CALL_HEADER: &str = "x-clax-call";

/// The journal appender's wake-up: a channel of capacity one. A nudge is a
/// `try_send`, so it never blocks; nudges while one is pending coalesce,
/// because the appender reads every row past its cursor once woken.
pub struct AuditWake {
    tx: SyncSender<()>,
    rx: Mutex<Option<Receiver<()>>>,
}

impl AuditWake {
    pub fn new() -> Arc<AuditWake> {
        let (tx, rx) = sync_channel(1);
        Arc::new(AuditWake {
            tx,
            rx: Mutex::new(Some(rx)),
        })
    }

    /// Tells the appender there are new rows. Returns at once: a pending
    /// wake-up already covers them, and without an appender there is no one
    /// to tell.
    pub fn nudge(&self) {
        let _ = self.tx.try_send(());
    }

    /// The receiving end, for the one appender; `None` once taken.
    pub fn take_receiver(&self) -> Option<Receiver<()>> {
        self.rx.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Makes `store` nudge this wake-up after each commit that recorded an
    /// audit event.
    pub fn install(self: &Arc<Self>, store: &clax_core::Store) {
        let wake = Arc::clone(self);
        store.set_audit_nudge(move || wake.nudge());
    }
}

/// The text value of header `name`; `None` when absent or not visible ASCII.
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The channel an agent side may name for itself in `x-clax-via`. The
/// browser channels (`shell`, `extension`, `lan`) follow from the
/// credentials, and `daemon` is internal, so naming them counts for nothing.
fn named_channel(headers: &HeaderMap) -> Option<Via> {
    header(headers, VIA_HEADER)
        .and_then(Via::parse)
        .filter(|v| matches!(v, Via::Mcp | Via::Hook | Via::Pi | Via::Cli))
}

/// The channel of a request with credentials `id`: what the token holder
/// names, else the extension, a browser of the owner's (the shell), the
/// token (the MCP shim when it acts for a session, else the CLI), or a LAN
/// viewer.
pub fn channel(id: &Identity, named: Option<Via>, agent_session: bool) -> Via {
    match named {
        Some(v) if id.token => v,
        _ if id.extension => Via::Extension,
        _ if id.owner_browser() => Via::Shell,
        _ if id.token && agent_session => Via::Mcp,
        _ if id.token => Via::Cli,
        _ => Via::Lan,
    }
}

/// The git half of the context: the decoded `x-clax-git` of a token holder.
fn git_field(id: &Identity, headers: &HeaderMap) -> GitField {
    if !id.token {
        return GitField::Absent;
    }
    match headers.get(GIT_HEADER) {
        None => GitField::Absent,
        Some(v) => match v.to_str() {
            Ok(s) => gitctx::decode_header(s),
            Err(_) => GitField::Capture("invalid"),
        },
    }
}

/// The decoded `x-clax-call` of a token holder; `None` without one, or when
/// it does not decode (the reason, never the value, is logged).
fn call_field(id: &Identity, headers: &HeaderMap) -> Option<CallHeader> {
    if !id.token {
        return None;
    }
    let value = headers.get(CALL_HEADER)?;
    let decoded = value
        .to_str()
        .map_err(|_| "header is not visible ASCII".to_string())
        .and_then(decode_call_header);
    match decoded {
        Ok(call) => Some(call),
        Err(why) => {
            tracing::debug!("ignoring {CALL_HEADER}: {why}");
            None
        }
    }
}

/// The actor and channel of a request with credentials `id` that names
/// channel `named` and session `session`, reading the rows they name.
fn actor_and_channel(
    st: &clax_core::Store,
    id: &Identity,
    named: Option<Via>,
    session: Option<&str>,
) -> clax_core::Result<(Actor, Via)> {
    let actor = id.audit_actor(st, session, named)?;
    let agent_session = matches!(&actor, Actor::Agent(a) if a.session_id.is_some());
    Ok((actor, channel(id, named, agent_session)))
}

/// The audit context of a token holder's `GET` poll that hands session
/// `sid` its feedback: that session's agent, through the channel the
/// request names (else `mcp`), with its git state and tool call. Unlike
/// the [`AuditCtx`] extractor it writes no row, so a `GET` route may use
/// it. Call it only behind the token.
pub fn session_ctx(
    st: &clax_core::Store,
    headers: &HeaderMap,
    sid: &str,
) -> clax_core::Result<AuditCtx> {
    let id = Identity {
        token: true,
        ..Identity::default()
    };
    let agent = st
        .session_actor(sid)?
        .unwrap_or_else(|| clax_core::audit::AgentActor {
            session_id: Some(sid.to_string()),
            ..Default::default()
        });
    Ok(AuditCtx {
        actor: Actor::Agent(agent),
        via: named_channel(headers).unwrap_or(Via::Mcp),
        git: git_field(&id, headers),
        call: call_field(&id, headers),
    })
}

/// What a request's audit context is resolved from, read from its headers
/// without touching the store. A handler that refuses some requests after
/// reading its body takes this instead of [`AuditCtx`] and calls
/// [`DeferredAudit::resolve`] once the request is accepted, so a refused
/// request makes no row.
pub struct DeferredAudit {
    id: Identity,
    named: Option<Via>,
    session: Option<String>,
    git: GitField,
    call: Option<CallHeader>,
}

impl DeferredAudit {
    fn from_parts(parts: &Parts, state: &AppState) -> DeferredAudit {
        let headers = &parts.headers;
        let id = Identity::from_parts(headers, &parts.extensions, &state.token);
        DeferredAudit {
            named: named_channel(headers),
            session: header(headers, SESSION_HEADER).map(str::to_string),
            git: git_field(&id, headers),
            call: call_field(&id, headers),
            id,
        }
    }

    /// The audit context of a token holder acting for session `sid`, as
    /// the routes under `/api/sessions/<sid>` do: that session's agent
    /// (`session_id` alone when no such session exists), through the
    /// channel the request names, else `mcp`. It reads the session row and
    /// writes nothing. Without the token it is [`DeferredAudit::resolve`].
    pub fn for_session(self, st: &clax_core::Store, sid: &str) -> clax_core::Result<AuditCtx> {
        if !self.id.token {
            return self.resolve(st);
        }
        let agent = st
            .session_actor(sid)?
            .unwrap_or_else(|| clax_core::audit::AgentActor {
                session_id: Some(sid.to_string()),
                ..Default::default()
            });
        Ok(AuditCtx {
            actor: Actor::Agent(agent),
            via: self.named.unwrap_or(Via::Mcp),
            git: self.git,
            call: self.call,
        })
    }

    /// The context of a token holder registering or joining a session: the
    /// channel it names (else `mcp`), its git state and its tool call. The
    /// store records those events as the session's own agent, which it
    /// names itself, so the actor here is the sessionless agent and no row
    /// is read or written.
    pub fn agent_side(self) -> AuditCtx {
        AuditCtx {
            actor: Actor::Agent(clax_core::audit::AgentActor::default()),
            via: self.named.filter(|_| self.id.token).unwrap_or(Via::Mcp),
            git: self.git,
            call: self.call,
        }
    }

    /// The audit context: reads the session and viewer rows, and makes the
    /// viewer row of a first-time viewer or the owner as
    /// [`Identity::ensure_viewer`] does.
    pub fn resolve(self, st: &clax_core::Store) -> clax_core::Result<AuditCtx> {
        let (actor, via) = actor_and_channel(st, &self.id, self.named, self.session.as_deref())?;
        Ok(AuditCtx {
            actor,
            via,
            git: self.git,
            call: self.call,
        })
    }
}

impl FromRequestParts<AppState> for DeferredAudit {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(DeferredAudit::from_parts(parts, state))
    }
}

/// The audit context of a request that acts, resolved during extraction
/// ([`DeferredAudit::resolve`]), so it belongs only on routes that record
/// events and refuse nothing after it.
impl FromRequestParts<AppState> for AuditCtx {
    type Rejection = crate::error::ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let deferred = DeferredAudit::from_parts(parts, state);
        state.store_call(move |st| deferred.resolve(st)).await
    }
}

/// The journal's state, as `GET /api/toolpath/status` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalState {
    /// The appender runs.
    pub journal: bool,
    /// The open segment's file name, else the newest one's.
    pub segment: Option<String>,
    /// The last `seq` journalled; `None` until the journal has been read.
    pub cursor: Option<i64>,
    /// Why the journal is behind, until a drain succeeds again.
    pub last_error: Option<String>,
    /// A problem that does not hold the journal back: retention could not
    /// remove a segment.
    pub warning: Option<String>,
    /// When the appender found events to write while caught up; `None`
    /// once it has caught up again.
    pub behind_since: Option<chrono::DateTime<chrono::Utc>>,
}

/// The journal's state, shared by the appender and the status route.
#[derive(Debug, Default)]
pub struct JournalStatus(Mutex<JournalState>);

impl JournalStatus {
    /// A status saying the journal does not run, and why, when there is a
    /// reason to give.
    pub fn off(reason: Option<String>) -> Arc<JournalStatus> {
        Arc::new(JournalStatus(Mutex::new(JournalState {
            last_error: reason,
            ..JournalState::default()
        })))
    }

    pub fn get(&self) -> JournalState {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn update(&self, f: impl FnOnce(&mut JournalState)) {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// How long the journal has trailed the table, in milliseconds, given
    /// the newest recorded `seq`, at `now`: 0 when caught up, `None` when
    /// the journal does not run or has not been read yet.
    pub fn lag_ms(&self, newest: i64, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
        let s = self.get();
        let cursor = s.cursor.filter(|_| s.journal)?;
        if cursor >= newest {
            return Some(0);
        }
        Some(
            s.behind_since
                .map_or(0, |t| (now - t).num_milliseconds().max(0)),
        )
    }
}

/// The longest the appender waits before retrying after a failure.
const MAX_BACKOFF_S: i64 = 60;

/// How long a graceful shutdown lets the appender catch up before it closes
/// the segment. What it does not write, the next start writes from the
/// table (§7.4).
const FINAL_DRAIN: std::time::Duration = std::time::Duration::from_millis(500);

/// The journal appender (spec §7.3): reads the events past its cursor from
/// the table, at most [`BATCH_ROWS`](clax_core::toolpath::segment::BATCH_ROWS)
/// at a time on a reader connection, and appends them to the segments. It
/// runs on its own thread and never takes the writer, so recording never
/// waits on it; the table is its queue, so a slow or failing disk only
/// leaves the journal behind. A failure is logged and kept in the status,
/// and the same events are tried again after 1 s, doubling to 60 s.
pub struct Appender {
    store: Arc<clax_core::Store>,
    writer: clax_core::toolpath::segment::SegmentWriter,
    status: Arc<JournalStatus>,
    clock: Arc<dyn clax_core::working::Clock>,
    backoff_s: i64,
    retry_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Appender {
    /// An appender of `store`'s events to the journal in `dir`, through
    /// `fs`, reporting to `status`. It reads the journal back on its first
    /// drain.
    pub fn new(
        store: Arc<clax_core::Store>,
        dir: impl Into<std::path::PathBuf>,
        cfg: clax_core::toolpath::segment::SegmentConfig,
        clock: Arc<dyn clax_core::working::Clock>,
        fs: Box<dyn clax_core::toolpath::segment::JournalFs>,
        status: Arc<JournalStatus>,
    ) -> Appender {
        status.update(|s| {
            s.journal = true;
            s.last_error = None;
        });
        Appender {
            store,
            writer: clax_core::toolpath::segment::SegmentWriter::new(dir, cfg, clock.clone(), fs),
            status,
            clock,
            backoff_s: 0,
            retry_at: None,
        }
    }

    /// Writes every event past the cursor, then syncs if a sync is due.
    /// While backing off after a failure it writes nothing and returns the
    /// failure. Tests drive the appender with this alone.
    ///
    /// # Errors
    /// Why the journal is behind (also in the status).
    pub fn drain_now(&mut self) -> Result<(), String> {
        self.drain_for(None).map(|_| ())
    }

    /// The 1 s timer's work when no nudge came in that second: drains (so a
    /// failed write is retried), then syncs what is unsynced at once
    /// (§7.4).
    ///
    /// # Errors
    /// Why the journal is behind (also in the status).
    pub fn quiet_tick(&mut self) -> Result<(), String> {
        self.quiet_tick_for(None).map(|_| ())
    }

    /// [`quiet_tick`](Self::quiet_tick) with its drain stopped between
    /// batches once `budget` has passed. Returns whether it caught up.
    fn quiet_tick_for(&mut self, budget: Option<std::time::Duration>) -> Result<bool, String> {
        let caught_up = self.drain_for(budget)?;
        let r = self
            .writer
            .sync()
            .map_err(|e| format!("syncing the journal: {e}"));
        if let Err(e) = &r {
            self.failed(self.clock.now(), e.clone());
        }
        r.map(|()| caught_up)
    }

    /// [`drain_now`](Self::drain_now), stopping between batches once
    /// `budget` has passed. Returns whether it caught up.
    fn drain_for(&mut self, budget: Option<std::time::Duration>) -> Result<bool, String> {
        let now = self.clock.now();
        if let Some(t) = self.retry_at
            && now < t
        {
            self.status.update(|s| {
                s.behind_since.get_or_insert(now);
            });
            return Err(self.status.get().last_error.unwrap_or_default());
        }
        match self.drain(budget) {
            Ok(caught_up) => {
                self.backoff_s = 0;
                self.retry_at = None;
                self.status.update(|s| s.last_error = None);
                Ok(caught_up)
            }
            Err(e) => {
                self.failed(now, e.clone());
                Err(e)
            }
        }
    }

    /// Records failure `e` at `now`, and backs off.
    fn failed(&mut self, now: chrono::DateTime<chrono::Utc>, e: String) {
        self.backoff_s = (self.backoff_s * 2).clamp(1, MAX_BACKOFF_S);
        self.retry_at = Some(now + chrono::Duration::seconds(self.backoff_s));
        tracing::warn!(error = %e, retry_in_s = self.backoff_s,
            "the audit journal is behind; events are still recorded, and the journal catches up once it can write");
        self.status.update(|s| {
            s.last_error = Some(e);
            s.behind_since.get_or_insert(now);
        });
    }

    fn drain(&mut self, budget: Option<std::time::Duration>) -> Result<bool, String> {
        use clax_core::toolpath::segment::BATCH_ROWS;
        let start = std::time::Instant::now();
        let io = |what: &str, e: std::io::Error| format!("{what}: {e}");
        let caught_up = loop {
            if budget.is_some_and(|b| start.elapsed() >= b) {
                break false;
            }
            let cursor = match self.writer.cursor() {
                Some(c) => c,
                None => {
                    self.writer
                        .recover()
                        .map_err(|e| io("reading the journal back", e))?;
                    self.publish();
                    self.writer.cursor().unwrap_or(0)
                }
            };
            let rows = self
                .store
                .events_after(cursor, BATCH_ROWS)
                .map_err(|e| format!("reading audit events: {e}"))?;
            if rows.is_empty() {
                break true;
            }
            let now = self.clock.now();
            self.status.update(|s| {
                s.behind_since.get_or_insert(now);
            });
            let r = self.writer.append_batch(&rows);
            self.publish();
            r.map_err(|e| io("writing the journal", e))?;
            if rows.len() < BATCH_ROWS as usize {
                break true;
            }
        };
        if caught_up {
            self.status.update(|s| s.behind_since = None);
        }
        self.writer
            .sync_if_due()
            .map_err(|e| io("syncing the journal", e))?;
        Ok(caught_up)
    }

    /// Copies the writer's cursor, segment and warning into the status.
    fn publish(&self) {
        let (cursor, segment) = (
            self.writer.cursor(),
            self.writer.segment().map(str::to_string),
        );
        let warning = self.writer.warning().map(str::to_string);
        self.status.update(|s| {
            s.cursor = cursor.or(s.cursor);
            s.segment = segment.or(s.segment.take());
            s.warning = warning;
        });
    }

    /// Runs the appender until `stop` is set or the wake-up's sender is
    /// gone: recovers and catches up, then drains on each nudge, syncing
    /// only once bytes have waited 5 s, and on each quiet second of the 1 s
    /// timer, which syncs at once and retries after a failure. It works a
    /// second at a time, so a long catch-up still sees `stop`, and at the
    /// end catches up for at most 0.5 s, syncs, and closes the segment. If
    /// it panics, the status says the journal stopped.
    pub fn run(mut self, rx: Receiver<()>, stop: Arc<std::sync::atomic::AtomicBool>) {
        use std::sync::atomic::Ordering;
        use std::sync::mpsc::RecvTimeoutError;
        const SLICE: std::time::Duration = std::time::Duration::from_secs(1);
        let _guard = DiesLoudly(self.status.clone());
        let mut behind = !matches!(self.drain_for(Some(SLICE)), Ok(true));
        loop {
            let woke = if behind && self.retry_at.is_none() {
                match rx.try_recv() {
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Err(RecvTimeoutError::Disconnected)
                    }
                    _ => Ok(()),
                }
            } else {
                rx.recv_timeout(SLICE)
            };
            if matches!(woke, Err(RecvTimeoutError::Disconnected)) || stop.load(Ordering::SeqCst) {
                break;
            }
            behind = if matches!(woke, Err(RecvTimeoutError::Timeout)) {
                !matches!(self.quiet_tick_for(Some(SLICE)), Ok(true))
            } else {
                !matches!(self.drain_for(Some(SLICE)), Ok(true))
            };
        }
        self.finish();
    }

    /// The graceful end: catches up (for up to 5 s), syncs, and writes the
    /// open segment's `Head` and `PathClose`.
    pub fn finish(&mut self) {
        self.retry_at = None;
        if let Err(e) = self.drain_for(Some(FINAL_DRAIN)) {
            tracing::warn!(error = %e, "the audit journal could not catch up at shutdown");
        }
        if let Err(e) = self.writer.sync() {
            tracing::warn!(error = %e, "syncing the audit journal at shutdown failed");
        }
        if let Err(e) = self.writer.close() {
            tracing::warn!(error = %e, "closing the audit journal segment at shutdown failed");
        }
        self.publish();
    }
}

/// The journal directory of `home` (spec §7.1).
pub fn journal_dir(home: &clax_core::Home) -> std::path::PathBuf {
    home.root().join("toolpath").join("journal")
}

/// Marks the journal stopped when the appender's thread unwinds from a
/// panic, so the status and `clax doctor` say so.
struct DiesLoudly(Arc<JournalStatus>);

impl Drop for DiesLoudly {
    fn drop(&mut self) {
        if std::thread::panicking() {
            tracing::error!(
                "the audit journal appender stopped unexpectedly; events are still recorded"
            );
            self.0.update(|s| {
                s.journal = false;
                s.last_error = Some("the journal appender stopped unexpectedly".into());
            });
        }
    }
}

/// The running appender thread.
pub struct JournalHandle {
    stop: Arc<std::sync::atomic::AtomicBool>,
    wake: Arc<AuditWake>,
    thread: std::thread::JoinHandle<()>,
    /// Fires when the thread has ended (closed or panicked).
    ended: tokio::sync::oneshot::Receiver<()>,
}

impl JournalHandle {
    /// Stops the appender and waits for it to close its segment, however
    /// long that takes (tests).
    pub fn stop(self) {
        self.request_stop();
        if self.thread.join().is_err() {
            tracing::error!("the audit journal appender panicked");
        }
    }

    fn request_stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.wake.nudge();
    }

    /// Stops the appender and waits at most `limit` for it to close its
    /// segment. Returns whether it did. A wedged appender (a disk that
    /// never answers) is left behind on its own thread, never on the
    /// runtime's blocking pool, so it cannot hold the daemon's exit; the
    /// process exit ends it, as a crash would, and the next start recovers.
    pub async fn stop_within(self, limit: std::time::Duration) -> bool {
        self.request_stop();
        let ended = tokio::time::timeout(limit, self.ended).await.is_ok();
        if ended && self.thread.join().is_err() {
            tracing::error!("the audit journal appender panicked");
        }
        ended
    }
}

/// Runs `appender` on its own thread, woken by `wake`'s receiver `rx`.
///
/// # Errors
/// The thread could not be spawned.
pub fn spawn_appender(
    appender: Appender,
    rx: Receiver<()>,
    wake: Arc<AuditWake>,
) -> std::io::Result<JournalHandle> {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop = stop.clone();
    let (ended_tx, ended) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("clax-journal".into())
        .spawn(move || {
            // Sent as the thread ends, a panic included.
            struct Ended(Option<tokio::sync::oneshot::Sender<()>>);
            impl Drop for Ended {
                fn drop(&mut self) {
                    if let Some(tx) = self.0.take() {
                        let _ = tx.send(());
                    }
                }
            }
            let _ended = Ended(Some(ended_tx));
            appender.run(rx, thread_stop)
        })?;
    Ok(JournalHandle {
        stop,
        wake,
        thread,
        ended,
    })
}

/// What starting the journal needs from the daemon.
pub struct JournalStart {
    pub store: Arc<clax_core::Store>,
    /// The journal directory (`<home>/toolpath/journal`).
    pub dir: std::path::PathBuf,
    pub wake: Arc<AuditWake>,
    pub status: Arc<JournalStatus>,
    /// The daemon's version, named in each segment's `PathOpen`.
    pub version: String,
}

/// Starts the appender thread per the home's `[toolpath]` (spec §7): none
/// when the journal is off or the table is unreadable, or when `config` is
/// invalid (the status then says why). Recording goes on either way, and a
/// later start catches the journal up.
pub fn start_journal(
    start: JournalStart,
    config: clax_core::Result<clax_core::config::ToolpathConfig>,
) -> Option<JournalHandle> {
    let set_off = |why: Option<String>| {
        start.status.update(|s| {
            *s = JournalState {
                last_error: why,
                ..JournalState::default()
            }
        })
    };
    let cfg = match config {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "the audit journal is off: [toolpath] is invalid");
            set_off(Some(format!("journal off: {e}")));
            return None;
        }
    };
    if !cfg.journal {
        set_off(None);
        return None;
    }
    let install = match start.store.install_id() {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(error = %e, "the audit journal is off: the install ID is unreadable");
            set_off(Some(format!("journal off: {e}")));
            return None;
        }
    };
    let Some(rx) = start.wake.take_receiver() else {
        set_off(Some(
            "journal off: another appender holds the wake-up".into(),
        ));
        return None;
    };
    let mut seg = clax_core::toolpath::segment::SegmentConfig::new(
        install,
        start.version,
        clax_core::build_commit(),
    );
    seg.max_bytes = cfg.segment_max_mb << 20;
    seg.retain_days = cfg.journal_retain_days;
    seg.redaction.no_text = !cfg.journal_text;
    let status = start.status.clone();
    let appender = Appender::new(
        start.store,
        start.dir,
        seg,
        Arc::new(clax_core::working::SystemClock),
        Box::new(clax_core::toolpath::segment::StdFs::new()),
        start.status,
    );
    match spawn_appender(appender, rx, start.wake) {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::error!(error = %e, "the audit journal thread could not start");
            status.update(|s| {
                *s = JournalState {
                    last_error: Some(format!("journal off: its thread could not start: {e}")),
                    ..JournalState::default()
                }
            });
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_follows_the_credentials() {
        let token = Identity {
            token: true,
            ..Identity::default()
        };
        let extension = Identity {
            extension: true,
            ..Identity::default()
        };
        let shell = Identity {
            owner_cookie: true,
            ..Identity::default()
        };
        let viewer = Identity {
            cookie: Some("01J9Z3K4M5N6P7Q8R9S0T1V2W3".into()),
            ..Identity::default()
        };
        assert_eq!(channel(&token, Some(Via::Hook), true), Via::Hook);
        assert_eq!(channel(&token, None, true), Via::Mcp);
        assert_eq!(channel(&token, None, false), Via::Cli);
        assert_eq!(channel(&extension, Some(Via::Mcp), false), Via::Extension);
        assert_eq!(channel(&shell, Some(Via::Cli), false), Via::Shell);
        assert_eq!(channel(&viewer, Some(Via::Mcp), true), Via::Lan);
        assert_eq!(channel(&Identity::default(), None, false), Via::Lan);
    }
}
