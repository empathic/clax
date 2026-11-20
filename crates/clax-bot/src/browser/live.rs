//! Browser sessions that outlive a step (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md` §6.3,
//! §6.4, §7.2, §7.4): [`BrowserSessions`], one per host process, holds
//! each open session as a task that owns its [`Cdp`] connection.
//!
//! # A session's task
//!
//! Spawned by [`BrowserSessions::open`], it opens the page, answers the
//! open, and then, until the session ends:
//!
//! - answers every paused request by the session's [`RequestPolicy`], and
//!   takes in every other event (console, errors, attached iframes and
//!   workers held to the policy, popups closed), also between calls, so a
//!   page's late requests never hang;
//! - sends the heartbeat, `Browser.getVersion`, once no command has gone
//!   to the browser for [`HEARTBEAT_AFTER_MS`] (spec §7.2: Browser Run
//!   ends a browser that hears nothing for its `keep_alive`);
//! - ends the session `idle` once no call came for the idle time, and
//!   `lifetime` once it has been open for its lifetime, by
//!   [`BrowserSource::deadline`], so tests drive both; the lifetime also
//!   cuts a call that is running then;
//! - ends it `lost` once the page crashed or was detached, or the socket
//!   closed ([`CdpFail::Lost`], [`CdpFail::Closed`]);
//! - performs the calls it receives ([`SessionCall`]), one at a time,
//!   each bounded by its own deadline (at most the lifetime) and the
//!   run's cancellation.
//!
//! Each end disposes of the context, sends `Browser.close` and closes the
//! socket ([`Driver::close_page`]). Its timers wait in the browser
//! source's clock: nothing here sleeps.
//!
//! # Ends and charging
//!
//! Points are milliseconds since the session opened (the WebSocket's
//! upgrade, the `at_ms` of every answer), which a call's Spend charges
//! through. An end the host makes (idle, lifetime, a revoke, credits, the
//! run's end, a lost page) is queued as an [`Ended`] for the store to
//! charge ([`BrowserSessions::ended`],
//! `clax_store::Store::end_bot_browser_session`); a close the script asks
//! for (`SessionEnd::Closed`) is charged by its own call.
//!
//! # Sources and leaving an origin
//!
//! A session opens on a URL, the run's clax or supplied content
//! ([`Start`]); an artifact session of the draft is given the draft as
//! each call sees it ([`CallCtx::pages`]). A `goto` within the session's
//! synthetic origin needs nothing more; a `goto` elsewhere from an
//! artifact or content session needs the URL session's policy the caller
//! gives once `browser.open.url` and its rule admit the URL
//! ([`CallCtx::leave`], [`BrowserSessions::leaves`]), and makes the
//! session a URL session for the rest of its life, once (spec §8.5): the
//! synthetic origin is no longer served ([`super::serve::Left`]: every
//! request to it fails, why `left`), the preamble becomes a browse's, and
//! `runTests` answers `browser_disabled`. The call that converted it
//! answers the new kind ([`Answered::kind`]).
//!
//! # Who may reach a session
//!
//! A session's handle is a capability, a macaroon (spec §18,
//! [`super::cap`]): the open mints the session's root
//! ([`Opened::handle`]), held by the run that opened it, and a share mints
//! a narrowed child of a capability ([`BrowserSessions::share`]). Holding
//! one is the authority to drive the session as far as its caveats allow
//! ([`BrowserSessions::authorize`]): one that does not verify, is not held
//! by the calling run, or names a session this process does not hold (the
//! daemon restarted) answers `browser_session_lost`, whose message never
//! repeats it; one whose caveats refuse the call, or that was revoked
//! ([`BrowserSessions::revoke`]: it and every capability under it),
//! answers `browser_forbidden`; a session that ended answers
//! `browser_session_closed` with its reason (or `browser_session_lost`,
//! for a lost one), whichever capability names it. Everything else,
//! events, records and messages, names a session by its ID.

use super::cap::{Cap, Caveat, Class, Holder, Keys, Target, Use};
use super::cdp::{Cdp, CdpFail};
use super::opts::Tests;
use super::opts::{MAX_EVAL_BYTES, Viewport};
use super::serve::{Decision, Left, Pages, Paused};
use super::session::{Driver, RequestPolicy, SessionLimits, Start};
use super::{
    BrowserError, BrowserSource, CODE_BROWSER_FORBIDDEN, CODE_BROWSER_SESSION_CLOSED,
    CODE_BROWSER_SESSION_LOST, Deadline,
};
use chrono::{DateTime, Duration, Utc};
use clax_bot_sandbox::{CODE_CANCELLED, CancelToken};
use clax_core::working::Clock;
use clax_store::bot_web::{SessionEnd, SessionKind};
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, StreamExt};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

/// How long a session may send the browser no command before the
/// heartbeat (spec §7.2).
pub const HEARTBEAT_AFTER_MS: i64 = 20_000;

/// The most characters of `page.content()` (spec §6.5).
pub const MAX_CONTENT_CHARS: usize = 100_000;

/// A session's times (`clax::capability::BrowserTerms`): one call's cap,
/// the idle time and the lifetime, in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTimes {
    pub call_seconds: u32,
    pub idle_seconds: u32,
    pub max_seconds: u32,
}

impl From<crate::capability::BrowserTerms> for SessionTimes {
    fn from(t: crate::capability::BrowserTerms) -> SessionTimes {
        SessionTimes {
            call_seconds: t.call_seconds,
            idle_seconds: t.idle_seconds,
            max_seconds: t.max_seconds,
        }
    }
}

/// A new session's ID: 12 lowercase Crockford base32 characters, a DNS
/// label (a content session's origin is `<ID>.clax-content.invalid`).
#[must_use]
pub fn new_session_id() -> String {
    clax_core::ArtifactId::generate().as_str().to_string()
}

/// One call of a session (spec §6.9): a page action, a locator action
/// with its locator's steps, or an `expect` matcher on a locator or the
/// page.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionCall {
    Page {
        action: String,
        args: Value,
    },
    Act {
        locator: Value,
        action: String,
        args: Value,
    },
    Expect {
        locator: Option<Value>,
        matcher: String,
        args: Value,
    },
}

/// What a session call answered (spec §6.9's `BrowserAnswer` without
/// its cost): a value, or a failure on the page (`ok` false, with
/// Playwright's message, `timeout` when its time ran out), the page's URL
/// after it, and the point it ended at.
#[derive(Clone, Debug, PartialEq)]
pub struct Answered {
    pub ok: bool,
    pub value: Value,
    pub error: Option<String>,
    pub timeout: bool,
    pub url: String,
    /// Milliseconds since the session opened, at the call's end.
    pub at_ms: i64,
    /// The screenshots the call took, PNG, in order.
    pub shots: Vec<Vec<u8>>,
    /// The session's new kind, when this call changed it (a `goto` out of
    /// its origin made it a URL session).
    pub kind: Option<SessionKind>,
}

/// What a call is given beside itself: its deadline (at most the
/// session's lifetime) and the run's cancellation; for an artifact session
/// of the draft, the draft as the call sees it; and, when the run may
/// leave the session's origin by this call (`browser.open.url` live, the
/// URL admitted by its rule), the URL session's policy it would then
/// have.
#[derive(Clone)]
pub struct CallCtx {
    pub deadline: DateTime<Utc>,
    pub cancel: CancelToken,
    pub pages: Option<Pages>,
    pub leave: Option<Arc<dyn RequestPolicy>>,
}

impl CallCtx {
    /// A call by `deadline` and the run's `cancel`, with no draft and no
    /// leave to go out of the origin.
    #[must_use]
    pub fn new(deadline: DateTime<Utc>, cancel: CancelToken) -> CallCtx {
        CallCtx {
            deadline,
            cancel,
            pages: None,
            leave: None,
        }
    }
}

/// An open session, as [`BrowserSessions::open`] answers it.
#[derive(Clone, Debug, PartialEq)]
pub struct Opened {
    pub session: String,
    /// The session's root capability, serialized: shown nowhere but to
    /// the prelude that holds it in its `Page` (see the module docs).
    pub handle: String,
    pub kind: SessionKind,
    pub url: String,
    pub title: String,
    /// The document's status, 0 when it did not load.
    pub status: u16,
    /// Why the page did not load, when it did not: the session is open
    /// all the same.
    pub error: Option<String>,
    pub opened_at: DateTime<Utc>,
    /// Milliseconds since the WebSocket's upgrade, at the open's end.
    pub at_ms: i64,
}

/// A session the host ended, for the store to charge (see the module
/// docs): its last segment ends at `at_ms`; a lost session's at 0, which
/// charges nothing past what its calls charged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    pub run: String,
    pub session: String,
    pub kind: SessionKind,
    pub at_ms: i64,
    pub reason: SessionEnd,
}

/// What a session's task is sent.
enum Msg {
    Call {
        call: SessionCall,
        ctx: CallCtx,
        reply: oneshot::Sender<Result<Answered, BrowserError>>,
    },
    Close {
        reason: SessionEnd,
        reply: oneshot::Sender<i64>,
    },
}

/// A session the registry holds.
struct Entry {
    run: String,
    /// The capability IDs revoked (spec §18.6): a capability whose chain
    /// holds one is refused.
    revoked: BTreeSet<String>,
    kind: SessionKind,
    /// `None` while open; why it ended once it did.
    ended: Option<SessionEnd>,
    tx: mpsc::UnboundedSender<Msg>,
    /// The page's URL after the last call.
    url: String,
    /// The synthetic origin's host it is served from, until it leaves it.
    synthetic: Option<String>,
}

#[derive(Default)]
struct Registry {
    sessions: BTreeMap<String, Entry>,
    ended: Vec<Ended>,
    /// What charges each end as it is made, when the host gave one
    /// ([`BrowserSessions::charge_with`]); else ends wait in `ended`.
    charger: Option<Charger>,
}

/// What a host charges the ends its registry makes with.
pub type Charger = Arc<dyn Fn(Vec<Ended>) + Send + Sync>;

/// The host's browser sessions (see the module docs).
pub struct BrowserSessions {
    source: Arc<dyn BrowserSource>,
    clock: Arc<dyn Clock>,
    limits: SessionLimits,
    reg: Arc<Mutex<Registry>>,
    /// What its capabilities are made and checked with (spec §18.2).
    keys: Keys,
}

/// Where a call goes, for a capability's `origin` caveat (spec §18.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Nav {
    /// Not a `navigate` call.
    None,
    /// A `goto` of this URL, resolved against the page's.
    Goto(String),
    /// A `reload`: to the page's own URL.
    Reload,
    /// A `goBack` or `goForward`: not known before the call.
    History,
}

impl std::fmt::Debug for BrowserSessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserSessions").finish_non_exhaustive()
    }
}

fn lock(reg: &Mutex<Registry>) -> MutexGuard<'_, Registry> {
    reg.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `browser_session_lost`, not retryable.
fn lost(session: &str, why: &str) -> BrowserError {
    BrowserError::new(
        CODE_BROWSER_SESSION_LOST,
        format!("browser session {session} is lost ({why}); open a new one"),
        false,
    )
}

/// What a call of a session that ended answers.
fn ended_error(session: &str, reason: SessionEnd) -> BrowserError {
    if reason == SessionEnd::Lost {
        return lost(session, "its browser is gone");
    }
    BrowserError::new(
        CODE_BROWSER_SESSION_CLOSED,
        format!(
            "browser session {session} was closed ({}); open a new one if you need it",
            reason.as_str()
        ),
        false,
    )
}

impl BrowserSessions {
    /// A registry opening browsers from `source`, measuring by `clock`,
    /// its screenshots held to `limits`.
    pub fn new(
        source: Arc<dyn BrowserSource>,
        clock: Arc<dyn Clock>,
        limits: SessionLimits,
    ) -> BrowserSessions {
        BrowserSessions {
            source,
            clock,
            limits,
            reg: Arc::new(Mutex::new(Registry::default())),
            keys: Keys::random(),
        }
    }

    /// This registry, making and checking its capabilities with `keys`
    /// (the instance's, spec §18.2) instead of a random key of its own.
    #[must_use]
    pub fn with_keys(mut self, keys: Keys) -> BrowserSessions {
        self.keys = keys;
        self
    }

    /// Opens session `id` (from [`new_session_id`]) of `kind` for run
    /// `run` on `start` at `viewport`, its requests answered by `policy`,
    /// within `times`; the open waits until `deadline` (and at most the
    /// lifetime) and the run's `cancel`.
    ///
    /// # Errors
    /// The source's refusal (`browser_busy`, `browser_unavailable`,
    /// `cancelled`, none opened); a failure before the page loaded
    /// (`browser_timeout`, `cancelled`, `browser_unavailable`, with
    /// `opened` set), the browser closed.
    #[allow(clippy::too_many_arguments)]
    pub async fn open(
        &self,
        run: &str,
        id: String,
        kind: SessionKind,
        start: Start,
        policy: Arc<dyn RequestPolicy>,
        viewport: Viewport,
        times: SessionTimes,
        deadline: DateTime<Utc>,
        cancel: &CancelToken,
    ) -> Result<Opened, BrowserError> {
        let transport = self.source.open(cancel).await?;
        let opened_at = self.clock.now();
        let lifetime = opened_at + Duration::seconds(i64::from(times.max_seconds));
        let cdp = Cdp::new(
            transport,
            self.source.deadline(deadline.min(lifetime)),
            cancel.clone(),
        );
        let (tx, rx) = mpsc::unbounded();
        let (open_tx, open_rx) = oneshot::channel();
        let synthetic = start.synthetic_host();
        let handle = self.keys.root(&id, run).encode();
        let task = Task {
            run: run.to_string(),
            id: id.clone(),
            kind,
            source: self.source.clone(),
            clock: self.clock.clone(),
            limits: self.limits,
            times,
            opened_at,
            lifetime,
            reg: self.reg.clone(),
        };
        lock(&self.reg).sessions.insert(
            id.clone(),
            Entry {
                run: run.to_string(),
                revoked: BTreeSet::new(),
                kind,
                ended: None,
                tx,
                url: start.url(),
                synthetic,
            },
        );
        drop(clax_rt::spawn(
            task.run(cdp, start, viewport, policy, open_tx, rx),
        ));
        match open_rx.await {
            Ok(r) => r.map(|o| Opened { handle, ..o }),
            Err(_) => Err(lost(&id, "its task ended before the page opened").opened(true)),
        }
    }

    /// Performs `call` on session `session` of run `run`, within `ctx`
    /// (see [`CallCtx`]).
    ///
    /// # Errors
    /// `browser_session_lost` for a session this process does not hold
    /// for `run`, or whose browser is gone; `browser_session_closed` for
    /// one that ended (its lifetime included, when it cut this call);
    /// `browser_disabled` for a `goto` out of the session's origin with
    /// no [`CallCtx::leave`], and for `runTests` in a URL session;
    /// `cancelled`.
    pub async fn call(
        &self,
        run: &str,
        session: &str,
        call: SessionCall,
        ctx: CallCtx,
    ) -> Result<Answered, BrowserError> {
        let tx = self.sender(run, session)?;
        let (reply, answer) = oneshot::channel();
        let msg = Msg::Call { call, ctx, reply };
        if tx.unbounded_send(msg).is_err() {
            return Err(self.gone(session));
        }
        answer.await.unwrap_or_else(|_| Err(self.gone(session)))
    }

    /// Closes session `session` of run `run` for `reason`, and answers
    /// the point its browser closed at. A close the script asked for
    /// (`SessionEnd::Closed`) is charged by its own call; any other is
    /// queued for the store ([`BrowserSessions::ended`]).
    ///
    /// # Errors
    /// As [`BrowserSessions::call`], for a session not open.
    pub async fn close(
        &self,
        run: &str,
        session: &str,
        reason: SessionEnd,
    ) -> Result<i64, BrowserError> {
        let tx = self.sender(run, session)?;
        let (reply, answer) = oneshot::channel();
        if tx.unbounded_send(Msg::Close { reason, reply }).is_err() {
            return Err(self.gone(session));
        }
        answer.await.map_err(|_| self.gone(session))
    }

    /// Closes every open session of run `run`, `run_ended`, and forgets
    /// them all once each has closed.
    pub async fn end_run(&self, run: &str) {
        for session in self.open_of(|r, _| r == run) {
            let _ = self.close(run, &session, SessionEnd::RunEnded).await;
        }
        lock(&self.reg).sessions.retain(|_, e| e.run != run);
    }

    /// Closes, `dropped`, every open session of run `run` that no
    /// capability among `held` keeps open, each closed before this answers
    /// (spec §18.7): one keeps its session open when it verifies, names
    /// it, is held by `run` and has no revoked ID in its chain; its
    /// expiry and classes are not read, so the decision is the same
    /// whenever it is made from the same `held`.
    pub async fn close_unheld(&self, run: &str, held: &[String]) {
        let holder = Holder::Run(run.to_string());
        let caps: Vec<Cap> = held.iter().filter_map(|h| self.keys.verify(h)).collect();
        let doomed: Vec<String> = {
            let reg = lock(&self.reg);
            reg.sessions
                .iter()
                .filter(|(id, e)| {
                    e.run == run
                        && e.ended.is_none()
                        && !caps.iter().any(|c| {
                            c.session() == id.as_str()
                                && c.run() == e.run
                                && c.admits(&holder)
                                && !c.revoked_in(&e.revoked)
                        })
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        for session in doomed {
            let _ = self.close(run, &session, SessionEnd::Dropped).await;
        }
    }

    /// Closes, `revoked`, every open session whose run no longer holds
    /// its kind's capability (`holds` false), each closed before this
    /// answers (spec §4.4).
    pub async fn reevaluate(&self, holds: &(dyn Fn(&str, SessionKind) -> bool + Sync)) {
        let doomed: Vec<(String, String)> = {
            let reg = lock(&self.reg);
            reg.sessions
                .iter()
                .filter(|(_, e)| e.ended.is_none() && !holds(&e.run, e.kind))
                .map(|(id, e)| (e.run.clone(), id.clone()))
                .collect()
        };
        for (run, session) in doomed {
            let _ = self.close(&run, &session, SessionEnd::Revoked).await;
        }
    }

    /// Whether a `goto` of `url` (resolved against the page's URL after
    /// the last call) leaves session `session` of run `run`'s synthetic
    /// origin: what needs `browser.open.url` and makes it a URL session.
    /// A URL session leaves nothing.
    ///
    /// # Errors
    /// As [`BrowserSessions::call`], for a session not open.
    pub fn leaves(&self, run: &str, session: &str, url: &str) -> Result<bool, BrowserError> {
        self.sender(run, session)?;
        let reg = lock(&self.reg);
        let Some(e) = reg.sessions.get(session) else {
            return Err(lost(
                session,
                "this process holds no such session for this run",
            ));
        };
        Ok(match &e.synthetic {
            None => false,
            Some(host) => {
                resolve_url(&e.url, url).as_deref().and_then(host_of) != Some(host.as_str())
            }
        })
    }

    /// The ID of the session `handle` is a capability of, for a call of
    /// run `run`, open or ended (see the module docs); nothing of its
    /// caveats but its holder is read.
    ///
    /// # Errors
    /// `browser_session_lost` for a handle that does not verify, is not
    /// held by `run`, or names no session this process holds of the run
    /// that opened it; its message does not repeat the handle.
    pub fn resolve(&self, run: &str, handle: &str) -> Result<String, BrowserError> {
        self.verified(run, handle).map(|c| c.session().to_string())
    }

    fn verified(&self, run: &str, handle: &str) -> Result<Cap, BrowserError> {
        let lost = || {
            BrowserError::new(
                CODE_BROWSER_SESSION_LOST,
                "this browser page's session is lost: this process holds no session of \
                 this run for it (the host may have restarted); open a new one",
                false,
            )
        };
        let cap = self.keys.verify(handle).ok_or_else(lost)?;
        let reg = lock(&self.reg);
        match reg.sessions.get(cap.session()) {
            Some(e) if e.run == cap.run() && cap.admits(&Holder::Run(run.to_string())) => Ok(cap),
            _ => Err(lost()),
        }
    }

    /// The session `handle` is a capability of, and the capability, when
    /// it allows a call of class `class` (none for `share` and `revoke`)
    /// going to `nav`, by run `run`, now (see the module docs).
    ///
    /// # Errors
    /// `browser_session_lost` as [`BrowserSessions::resolve`];
    /// `browser_session_closed` (or `_lost`) for a session that ended;
    /// `browser_forbidden` for a capability revoked, or whose caveats
    /// refuse the call.
    pub fn authorize(
        &self,
        run: &str,
        handle: &str,
        class: Option<Class>,
        nav: &Nav,
    ) -> Result<(String, Cap), BrowserError> {
        let cap = self.verified(run, handle)?;
        let session = cap.session().to_string();
        self.sender(cap.run(), &session)?;
        let forbidden = |why: String| BrowserError::new(CODE_BROWSER_FORBIDDEN, why, false);
        let target = {
            let reg = lock(&self.reg);
            let Some(e) = reg.sessions.get(&session) else {
                return Err(lost(
                    &session,
                    "this process holds no such session for this run",
                ));
            };
            if cap.revoked_in(&e.revoked) {
                return Err(forbidden(super::cap::Refusal::Revoked.message()));
            }
            match nav {
                Nav::None => Target::None,
                Nav::Goto(url) => {
                    Target::Url(resolve_url(&e.url, url).unwrap_or_else(|| url.clone()))
                }
                Nav::Reload => Target::Url(e.url.clone()),
                Nav::History => Target::Unknown,
            }
        };
        let holder = Holder::Run(run.to_string());
        let u = Use {
            holder: &holder,
            class,
            target,
            now_ms: self.clock.now().timestamp_millis(),
        };
        cap.check(&u).map_err(|r| forbidden(r.message()))?;
        Ok((session, cap))
    }

    /// The registry's clock, in Unix milliseconds: what `expires` caveats
    /// are held to.
    #[must_use]
    pub fn now_ms(&self) -> i64 {
        self.clock.now().timestamp_millis()
    }

    /// A new capability of the session `handle` is one of, its caveats and
    /// `more` (spec §18.4), for a call of run `run`.
    ///
    /// # Errors
    /// As [`BrowserSessions::authorize`] for a call of no class.
    pub fn share(&self, run: &str, handle: &str, more: &[Caveat]) -> Result<Cap, BrowserError> {
        let (_, parent) = self.authorize(run, handle, None, &Nav::None)?;
        Ok(self.keys.child(&parent, more))
    }

    /// Revokes the capability `handle` and every capability shared from
    /// it (spec §18.6), for a call of run `run`, and answers it.
    ///
    /// # Errors
    /// As [`BrowserSessions::authorize`] for a call of no class;
    /// `invalid_args` for a session's root, which is closed instead.
    pub fn revoke(&self, run: &str, handle: &str) -> Result<Cap, BrowserError> {
        let (session, cap) = self.authorize(run, handle, None, &Nav::None)?;
        if cap.is_root() {
            return Err(BrowserError::new(
                crate::api::CODE_BAD_ARGS,
                "this page opened its browser session: close it instead of revoking it",
                false,
            ));
        }
        if let Some(e) = lock(&self.reg).sessions.get_mut(&session) {
            e.revoked.insert(cap.id().to_string());
        }
        Ok(cap)
    }

    /// Whether session `session` of run `run` is open in this process.
    ///
    /// # Errors
    /// As [`BrowserSessions::call`], for a session not open.
    pub fn check(&self, run: &str, session: &str) -> Result<(), BrowserError> {
        self.sender(run, session).map(|_| ())
    }

    /// The sessions of run `run` open now, with their kinds.
    #[must_use]
    pub fn open_sessions(&self, run: &str) -> Vec<(String, SessionKind)> {
        let reg = lock(&self.reg);
        reg.sessions
            .iter()
            .filter(|(_, e)| e.run == run && e.ended.is_none())
            .map(|(id, e)| (id.clone(), e.kind))
            .collect()
    }

    /// The ends the host made since the last call, for the store to
    /// charge, in order.
    #[must_use]
    pub fn ended(&self) -> Vec<Ended> {
        std::mem::take(&mut lock(&self.reg).ended)
    }

    /// Has `charge` charge each end the host makes from now on as it is
    /// made, in the session's own task before anything waiting on the end
    /// is answered ([`BrowserSessions::reevaluate`] included), instead of
    /// queueing it for [`BrowserSessions::ended`]: so an end no call of its
    /// run will come to see (an idle end while the model thinks, a lost
    /// page) is charged then. Ends queued before are handed over first.
    pub fn charge_with(&self, charge: Charger) {
        let queued = {
            let mut reg = lock(&self.reg);
            reg.charger = Some(charge.clone());
            std::mem::take(&mut reg.ended)
        };
        if !queued.is_empty() {
            charge(queued);
        }
    }

    /// The runs with a session open now, each once.
    #[must_use]
    pub fn open_runs(&self) -> Vec<String> {
        let reg = lock(&self.reg);
        let mut runs: Vec<String> = reg
            .sessions
            .values()
            .filter(|e| e.ended.is_none())
            .map(|e| e.run.clone())
            .collect();
        runs.sort();
        runs.dedup();
        runs
    }

    fn open_of(&self, pick: impl Fn(&str, SessionKind) -> bool) -> Vec<String> {
        let reg = lock(&self.reg);
        reg.sessions
            .iter()
            .filter(|(_, e)| e.ended.is_none() && pick(&e.run, e.kind))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The channel of open session `session` of run `run`.
    fn sender(&self, run: &str, session: &str) -> Result<mpsc::UnboundedSender<Msg>, BrowserError> {
        let reg = lock(&self.reg);
        match reg.sessions.get(session) {
            Some(e) if e.run == run => match e.ended {
                Some(reason) => Err(ended_error(session, reason)),
                None => Ok(e.tx.clone()),
            },
            _ => Err(lost(
                session,
                "this process holds no such session for this run",
            )),
        }
    }

    /// What a call of `session` answers once its task stopped listening.
    fn gone(&self, session: &str) -> BrowserError {
        match lock(&self.reg).sessions.get(session).and_then(|e| e.ended) {
            Some(reason) => ended_error(session, reason),
            None => lost(session, "its task stopped"),
        }
    }
}

/// What woke a session's task between calls.
enum Woke {
    Timer,
    Msg(Option<Msg>),
    Event(Result<super::cdp::CdpEvent, CdpFail>),
}

/// A session's task (see the module docs).
struct Task {
    run: String,
    id: String,
    kind: SessionKind,
    source: Arc<dyn BrowserSource>,
    clock: Arc<dyn Clock>,
    limits: SessionLimits,
    times: SessionTimes,
    opened_at: DateTime<Utc>,
    lifetime: DateTime<Utc>,
    reg: Arc<Mutex<Registry>>,
}

/// A deadline that never fires.
fn never() -> Deadline {
    #[cfg(not(target_arch = "wasm32"))]
    return futures::future::pending().boxed();
    #[cfg(target_arch = "wasm32")]
    return futures::future::pending().boxed_local();
}

/// `to` resolved against `base` (a relative `goto`), as a URL.
fn resolve_url(base: &str, to: &str) -> Option<String> {
    url::Url::parse(base)
        .and_then(|b| b.join(to))
        .or_else(|_| url::Url::parse(to))
        .ok()
        .map(|u| u.to_string())
}

/// The host of `url`.
fn host_of(url: &str) -> Option<&str> {
    let after = url.split_once("://")?.1;
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..end];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    Some(host.split(':').next().unwrap_or(host))
}

/// The session's policy, which a `goto` out replaces once.
struct Swap(Mutex<Arc<dyn RequestPolicy>>);

impl Swap {
    fn now(&self) -> Arc<dyn RequestPolicy> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn replace(&self, p: Arc<dyn RequestPolicy>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = p;
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl RequestPolicy for Swap {
    async fn decide(&self, req: &Paused) -> Decision {
        self.now().decide(req).await
    }

    async fn saved_tests(&self, which: &Tests) -> Result<Vec<String>, String> {
        self.now().saved_tests(which).await
    }

    fn set_pages(&self, pages: Pages) {
        self.now().set_pages(pages);
    }
}

/// `browser_disabled`, saying what needs which capability.
fn disabled(message: String) -> BrowserError {
    BrowserError::new(super::CODE_BROWSER_DISABLED, message, false)
}

impl Task {
    fn at_ms(&self) -> i64 {
        (self.clock.now() - self.opened_at)
            .num_milliseconds()
            .max(0)
    }

    /// Marks the session ended for `reason` at `at_ms`, queueing a host's
    /// end for the store.
    fn mark(&self, reason: SessionEnd, at_ms: i64) {
        let mut reg = lock(&self.reg);
        if let Some(e) = reg.sessions.get_mut(&self.id) {
            e.ended = Some(reason);
        }
        if reason != SessionEnd::Closed {
            let at_ms = if reason == SessionEnd::Lost { 0 } else { at_ms };
            reg.ended.push(Ended {
                run: self.run.clone(),
                session: self.id.clone(),
                kind: self.kind,
                at_ms,
                reason,
            });
            if let Some(charge) = reg.charger.clone() {
                let ended = std::mem::take(&mut reg.ended);
                drop(reg);
                charge(ended);
            }
        }
    }

    async fn run(
        self,
        mut cdp: Cdp,
        start: Start,
        viewport: Viewport,
        policy: Arc<dyn RequestPolicy>,
        open_reply: oneshot::Sender<Result<Opened, BrowserError>>,
        mut rx: mpsc::UnboundedReceiver<Msg>,
    ) {
        let policy = Swap(Mutex::new(policy));
        let mut synthetic = start.synthetic_host();
        let mut d = Driver::new(
            &mut cdp,
            &policy,
            &*self.clock,
            start.mode(),
            self.opened_at,
        );
        let opened = self.open(&mut d, &start, &viewport).await;
        let ok = opened.is_ok();
        if !ok {
            d.close_page().await;
            lock(&self.reg).sessions.remove(&self.id);
        }
        let _ = open_reply.send(opened);
        if !ok {
            return;
        }
        let mut last_call = self.clock.now();
        let mut last_sent = last_call;
        // A timer that fired with nothing due (a test's deadline fired at
        // once) is not armed again until something else happens.
        let mut timer_spent = false;
        loop {
            let idle_at = last_call + Duration::seconds(i64::from(self.times.idle_seconds));
            let beat_at = last_sent + Duration::milliseconds(HEARTBEAT_AFTER_MS);
            let next = idle_at.min(self.lifetime).min(beat_at);
            d.cdp.rebound(never(), CancelToken::new());
            let sent_before = d.cdp.sent();
            let woke = {
                let timer = if timer_spent {
                    never()
                } else {
                    self.source.deadline(next)
                };
                let timer = timer.fuse();
                let msg = rx.next().fuse();
                let ev = d.cdp.next_event().fuse();
                futures::pin_mut!(timer, msg, ev);
                futures::select_biased! {
                    () = timer => Woke::Timer,
                    m = msg => Woke::Msg(m),
                    e = ev => Woke::Event(e),
                }
            };
            match woke {
                Woke::Timer => {
                    let now = self.clock.now();
                    if now >= self.lifetime {
                        self.end(&mut d, SessionEnd::Lifetime).await;
                        return;
                    }
                    if now >= idle_at {
                        self.end(&mut d, SessionEnd::Idle).await;
                        return;
                    }
                    if now >= beat_at {
                        let _ = d.cdp.send_only(None, "Browser.getVersion", json!({})).await;
                    } else {
                        timer_spent = true;
                        continue;
                    }
                }
                // The registry is gone (the host stops): the browser is
                // closed, as any end closes it.
                Woke::Msg(None) => {
                    let at = self.at_ms();
                    d.close_page().await;
                    self.mark(SessionEnd::Lost, at);
                    return;
                }
                Woke::Msg(Some(Msg::Close { reason, reply })) => {
                    let at = self.end(&mut d, reason).await;
                    let _ = reply.send(at);
                    return;
                }
                Woke::Msg(Some(Msg::Call { call, ctx, reply })) => {
                    let until = ctx.deadline.min(self.lifetime);
                    let started = self.clock.now();
                    d.cdp
                        .rebound(self.source.deadline(until), ctx.cancel.clone());
                    if let Some(pages) = ctx.pages.clone() {
                        policy.set_pages(pages);
                    }
                    // What the call may not do here, answered before it
                    // reaches the page.
                    let mut converted = None;
                    if let SessionCall::Page { action, args } = &call {
                        if action == "runTests" && synthetic.is_none() {
                            let _ = reply.send(Err(disabled(
                                "page.runTests: this session is on the web, not on the run's clax; open the clax to run its tests"
                                    .into(),
                            )));
                            last_call = self.clock.now();
                            continue;
                        }
                        if action == "goto"
                            && let Some(host) = synthetic.clone()
                        {
                            let to = args["url"].as_str().unwrap_or_default();
                            let target = resolve_url(&d.url, to).unwrap_or_else(|| to.to_string());
                            if host_of(&target) != Some(host.as_str()) {
                                let Some(leave) = ctx.leave.clone() else {
                                    let _ = reply.send(Err(disabled(format!(
                                        "page.goto: {target} is outside this session's origin; going there needs browser.open.url"
                                    ))));
                                    last_call = self.clock.now();
                                    continue;
                                };
                                policy.replace(Arc::new(Left { host, then: leave }));
                                synthetic = None;
                                converted = Some(SessionKind::Url);
                                {
                                    let mut reg = lock(&self.reg);
                                    if let Some(e) = reg.sessions.get_mut(&self.id) {
                                        e.kind = SessionKind::Url;
                                        e.synthetic = None;
                                    }
                                }
                            }
                        }
                    }
                    let timing = super::act::Timing {
                        clock: &*self.clock,
                        source: &*self.source,
                        call_until: until,
                        times: self.times,
                        limits: self.limits,
                    };
                    let r = match converted {
                        Some(_) => match d.become_browse().await {
                            Ok(()) => perform(&mut d, &call, &timing).await,
                            Err(e) => Err(e),
                        },
                        None => perform(&mut d, &call, &timing).await,
                    };
                    // The lifetime ends the session, whatever the call
                    // answered as it ran out.
                    let r = match r {
                        Ok(_) if self.clock.now() >= self.lifetime => Err(CdpFail::Deadline),
                        other => other,
                    };
                    let at_ms = self.at_ms();
                    if let Some(e) = lock(&self.reg).sessions.get_mut(&self.id) {
                        e.url.clone_from(&d.url);
                    }
                    let answer = match r {
                        Ok(mut a) => {
                            a.at_ms = at_ms;
                            a.url.clone_from(&d.url);
                            a.kind = converted;
                            Ok(a)
                        }
                        Err(CdpFail::Deadline) if self.clock.now() >= self.lifetime => {
                            self.end(&mut d, SessionEnd::Lifetime).await;
                            let _ = reply.send(Err(ended_error(&self.id, SessionEnd::Lifetime)));
                            return;
                        }
                        Err(CdpFail::Deadline) => {
                            let ms = (until - started).num_milliseconds().max(0);
                            Ok(Answered {
                                ok: false,
                                value: Value::Null,
                                error: Some(format!(
                                    "{}: Timeout {ms}ms exceeded.",
                                    call_name(&call)
                                )),
                                timeout: true,
                                url: d.url.clone(),
                                at_ms,
                                shots: std::mem::take(&mut d.shots),
                                kind: converted,
                            })
                        }
                        Err(CdpFail::Cancelled) => {
                            Err(
                                BrowserError::new(CODE_CANCELLED, "the run was cancelled", false)
                                    .opened(true),
                            )
                        }
                        Err(CdpFail::Protocol(m)) => Ok(Answered {
                            ok: false,
                            value: Value::Null,
                            error: Some(format!(
                                "{}: the browser refused it: {m}",
                                call_name(&call)
                            )),
                            timeout: false,
                            url: d.url.clone(),
                            at_ms,
                            shots: std::mem::take(&mut d.shots),
                            kind: converted,
                        }),
                        Err(
                            fail @ (CdpFail::Lost(_) | CdpFail::Closed | CdpFail::Transport(_)),
                        ) => {
                            d.close_page().await;
                            self.mark(SessionEnd::Lost, at_ms);
                            let _ = reply.send(Err(lost(&self.id, &fail.to_string())));
                            return;
                        }
                    };
                    let _ = reply.send(answer);
                    last_call = self.clock.now();
                }
                Woke::Event(Ok(e)) => {
                    // The host's answer to a paused request is held to a
                    // call's time.
                    let by =
                        self.clock.now() + Duration::seconds(i64::from(self.times.call_seconds));
                    d.cdp.rebound(self.source.deadline(by), CancelToken::new());
                    if let Err(fail) = d.handle(e).await
                        && let Some(reason) = self.ended_by(&fail)
                    {
                        let at = self.at_ms();
                        d.close_page().await;
                        self.mark(reason, at);
                        return;
                    }
                }
                Woke::Event(Err(fail)) => {
                    if let Some(reason) = self.ended_by(&fail) {
                        let at = self.at_ms();
                        d.close_page().await;
                        self.mark(reason, at);
                        return;
                    }
                }
            }
            timer_spent = false;
            if d.cdp.sent() != sent_before {
                last_sent = self.clock.now();
            }
        }
    }

    /// Whether `fail`, met between calls, ends the session, and why: a
    /// page gone or a socket closed is lost. (A deadline between calls is
    /// none: the timers are the task's own.)
    fn ended_by(&self, fail: &CdpFail) -> Option<SessionEnd> {
        match fail {
            CdpFail::Lost(_) | CdpFail::Closed | CdpFail::Transport(_) => Some(SessionEnd::Lost),
            CdpFail::Deadline | CdpFail::Cancelled | CdpFail::Protocol(_) => None,
        }
    }

    /// Closes the page for `reason` and marks the session: the point its
    /// browser closed at.
    async fn end(&self, d: &mut Driver<'_>, reason: SessionEnd) -> i64 {
        let at = self.at_ms();
        d.close_page().await;
        self.mark(reason, at);
        at
    }

    /// Opens the page and settles it: what the open answers.
    async fn open(
        &self,
        d: &mut Driver<'_>,
        start: &Start,
        viewport: &Viewport,
    ) -> Result<Opened, BrowserError> {
        let timeout_s = self.times.call_seconds;
        let loaded = d
            .open_page(start, viewport)
            .await
            .map_err(|f| super::session::setup_error(f, timeout_s))?;
        let error = match loaded {
            Ok(()) => {
                d.settle()
                    .await
                    .map_err(|f| super::session::setup_error(f, timeout_s))?;
                None
            }
            Err(e) => Some(format!("the page did not load: {e}")),
        };
        let title = if error.is_none() {
            match d.evaluate("document.title").await {
                Ok(Ok(Value::String(t))) => t,
                Ok(_) => String::new(),
                Err(f) => return Err(super::session::setup_error(f, timeout_s)),
            }
        } else {
            String::new()
        };
        Ok(Opened {
            session: self.id.clone(),
            // The registry gives the open its handle once the task answers.
            handle: String::new(),
            kind: self.kind,
            url: d.url.clone(),
            title: title.chars().take(1_000).collect(),
            status: d.status,
            error,
            opened_at: self.opened_at,
            at_ms: self.at_ms(),
        })
    }
}

/// How a call is named in its failure: `page.goto`, `locator.click`,
/// `expect.toBeVisible`.
fn call_name(call: &SessionCall) -> String {
    match call {
        SessionCall::Page { action, .. } => format!("page.{action}"),
        SessionCall::Act { action, .. } => format!("locator.{action}"),
        SessionCall::Expect { matcher, .. } => format!("expect.{matcher}"),
    }
}

/// A value answered.
fn answered(value: Value) -> Answered {
    Answered {
        ok: true,
        value,
        error: None,
        timeout: false,
        url: String::new(),
        at_ms: 0,
        shots: Vec::new(),
        kind: None,
    }
}

/// A failure on the page, answered with `error`.
fn failed(error: String) -> Answered {
    Answered {
        ok: false,
        value: Value::Null,
        error: Some(error),
        timeout: false,
        url: String::new(),
        at_ms: 0,
        shots: Vec::new(),
        kind: None,
    }
}

/// Performs `call` on the open page `d` (see [`SessionCall`]).
///
/// # Errors
/// What ended the wait: the deadline, the run's cancellation, the page
/// lost; the browser's refusal of a command ([`CdpFail::Protocol`]).
async fn perform(
    d: &mut Driver<'_>,
    call: &SessionCall,
    timing: &super::act::Timing<'_>,
) -> Result<Answered, CdpFail> {
    let mut a = match call {
        SessionCall::Page { action, args } => {
            match super::act::page(d, timing, action, args).await? {
                Some(a) => a,
                None => page(d, action, args, timing.times).await?,
            }
        }
        SessionCall::Act {
            locator,
            action,
            args,
        } => super::act::perform(d, timing, locator, action, args).await?,
        SessionCall::Expect {
            locator,
            matcher,
            args,
        } => super::act::expect(d, timing, locator.as_ref(), matcher, args).await?,
    };
    a.shots = std::mem::take(&mut d.shots);
    Ok(a)
}

/// A page action (spec §6.1): `goto`, `reload`, `title`, `content`,
/// `evaluate`, `runTests` (the saved tests, `args.paths` or all of them,
/// within the call's cap).
async fn page(
    d: &mut Driver<'_>,
    action: &str,
    args: &Value,
    times: SessionTimes,
) -> Result<Answered, CdpFail> {
    Ok(match action {
        "goto" => {
            let to = args["url"].as_str().unwrap_or_default();
            let url = match url::Url::parse(&d.url).and_then(|base| base.join(to)) {
                Ok(u) => u.to_string(),
                Err(_) => to.to_string(),
            };
            match d.navigate(&url).await? {
                Ok(()) => {
                    d.settle().await?;
                    answered(response(d))
                }
                Err(e) => failed(format!("page.goto: {e} at {url}")),
            }
        }
        "reload" => match d.reload().await? {
            Ok(()) => {
                d.settle().await?;
                answered(response(d))
            }
            Err(e) => failed(format!("page.reload: {e}")),
        },
        "title" => match d.evaluate("document.title").await? {
            Ok(Value::String(t)) => answered(json!(t.chars().take(1_000).collect::<String>())),
            Ok(_) => answered(json!("")),
            Err(e) => failed(format!("page.title: {e}")),
        },
        "content" => match d
            .evaluate("document.documentElement ? document.documentElement.outerHTML : ''")
            .await?
        {
            Ok(Value::String(h)) => answered(json!(cut(&h, MAX_CONTENT_CHARS))),
            Ok(_) => answered(json!("")),
            Err(e) => failed(format!("page.content: {e}")),
        },
        "evaluate" => evaluate(d, args).await?,
        "runTests" => {
            let which = match args.get("paths").and_then(Value::as_array) {
                Some(p) => Tests::Paths(
                    p.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                ),
                None => Tests::All,
            };
            answered(
                d.run_saved(&which, u64::from(times.call_seconds) * 1_000)
                    .await?,
            )
        }
        other => failed(format!("page.{other} is not an action")),
    })
}

/// `s` cut to `max` characters, ending with `…` when it was.
fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

/// What a navigation answers: Playwright's `Response`, as data.
fn response(d: &Driver<'_>) -> Value {
    json!({"url": d.url, "status": d.status, "ok": (200..300).contains(&d.status)})
}

/// `page.evaluate` (spec §6.5): `args.source`, a function's source text,
/// called with `args.arg`, or `args.expression` evaluated; either awaited,
/// its value as JSON of at most [`MAX_EVAL_BYTES`].
async fn evaluate(d: &mut Driver<'_>, args: &Value) -> Result<Answered, CdpFail> {
    let expression = match (args["source"].as_str(), args["expression"].as_str()) {
        (Some(src), _) => format!("({src})({})", args.get("arg").unwrap_or(&Value::Null)),
        (None, Some(e)) => e.to_string(),
        (None, None) => return Ok(failed("page.evaluate: nothing to evaluate".into())),
    };
    Ok(match d.evaluate(&expression).await? {
        Ok(v) if serde_json::to_string(&v).map_or(0, |s| s.len()) > MAX_EVAL_BYTES => {
            failed(format!(
                "page.evaluate: the value is over {} KiB as JSON",
                MAX_EVAL_BYTES / 1024
            ))
        }
        Ok(v) => answered(v),
        Err(e) => failed(format!("page.evaluate: {e}")),
    })
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::browser::Served;
    use crate::browser::fake::{ChromeLog, FakeBrowserSource, FakeChrome, PageModel, event, value};
    use crate::browser::serve::{Decision, Origin, Paused, WHY_ORIGIN};
    use crate::browser::session::{WHY_POPUP, WHY_TARGET, WORKER_PREAMBLE_JS};
    use clax_core::ArtifactId;
    use clax_core::working::ManualClock;

    const AID: &str = "abcdefghjkmn";
    const ORIGIN: &str = "https://abcdefghjkmn.clax-test.invalid/";
    const TIMES: SessionTimes = SessionTimes {
        call_seconds: 60,
        idle_seconds: 300,
        max_seconds: 1_800,
    };

    /// Serves every request of the origin a 200 page, blocks every other.
    struct Serve;

    #[async_trait::async_trait]
    impl RequestPolicy for Serve {
        async fn decide(&self, req: &Paused) -> Decision {
            if req.url.starts_with(ORIGIN) {
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

    /// A page whose title is "Hello" and whose `hang()` never answers.
    fn model() -> PageModel {
        PageModel::new()
            .evaluate(|p| {
                if p["expression"] == "document.title" {
                    value(json!("Hello"))
                } else {
                    value(json!(true))
                }
            })
            .on(|cmd| {
                (cmd["method"] == "Runtime.evaluate" && cmd["params"]["expression"] == "hang()")
                    .then(Vec::new)
            })
    }

    struct World {
        src: Arc<FakeBrowserSource>,
        clock: Arc<ManualClock>,
        sessions: BrowserSessions,
    }

    impl World {
        fn new() -> World {
            let src = Arc::new(FakeBrowserSource::new());
            src.opens(|| Ok(FakeChrome::page(model())));
            let clock = Arc::new(ManualClock::at("2026-11-19T10:00:00Z"));
            let sessions = BrowserSessions::new(
                src.clone(),
                clock.clone(),
                SessionLimits { image_side: 2_000 },
            );
            World {
                src,
                clock,
                sessions,
            }
        }

        async fn open(&self, times: SessionTimes) -> (Opened, ChromeLog) {
            let start = Start::Test {
                origin: Origin::new(&ArtifactId::parse(AID).unwrap()),
                path: "index.html".into(),
            };
            let o = self
                .sessions
                .open(
                    "run-1",
                    "s1".into(),
                    SessionKind::Artifact,
                    start,
                    Arc::new(Serve),
                    Viewport::desktop(),
                    times,
                    self.clock.now() + Duration::seconds(60),
                    &CancelToken::new(),
                )
                .await
                .expect("opened");
            (o, self.src.logs()[0].clone())
        }

        /// Moves the clock `secs` on and fires the deadlines due by then.
        fn advance(&self, secs: i64) {
            self.clock.advance(secs);
            self.src.fire_until(self.clock.now());
        }

        async fn page(&self, action: &str, args: Value) -> Result<Answered, BrowserError> {
            self.call_with(action, args, None, None).await
        }

        async fn call_with(
            &self,
            action: &str,
            args: Value,
            pages: Option<Pages>,
            leave: Option<Arc<dyn RequestPolicy>>,
        ) -> Result<Answered, BrowserError> {
            let mut ctx =
                CallCtx::new(self.clock.now() + Duration::seconds(60), CancelToken::new());
            ctx.pages = pages;
            ctx.leave = leave;
            self.sessions
                .call(
                    "run-1",
                    "s1",
                    SessionCall::Page {
                        action: action.into(),
                        args,
                    },
                    ctx,
                )
                .await
        }
    }

    fn code(r: Result<Answered, BrowserError>) -> &'static str {
        r.expect_err("an error").code
    }

    #[tokio::test]
    async fn a_session_opens_and_answers_its_page_calls() {
        let w = World::new();
        let (o, log) = w.open(TIMES).await;
        assert_eq!(
            (
                o.session.as_str(),
                o.url.as_str(),
                o.title.as_str(),
                o.status,
                o.error
            ),
            ("s1", ORIGIN, "Hello", 200, None)
        );
        assert_eq!(
            w.sessions.open_sessions("run-1"),
            [("s1".to_string(), SessionKind::Artifact)]
        );
        w.clock.advance_millis(1_500);
        let t = w.page("title", json!({})).await.unwrap();
        assert_eq!(
            (t.ok, &t.value, t.at_ms, t.url.as_str()),
            (true, &json!("Hello"), 1_500, ORIGIN)
        );
        let g = w.page("goto", json!({"url": "/b.html"})).await.unwrap();
        let b = format!("{ORIGIN}b.html");
        assert_eq!(g.value, json!({"url": b, "status": 200, "ok": true}));
        assert_eq!(g.url, b);
        let r = w.page("reload", json!({})).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(log.sent("Page.reload").len(), 1);
        let e = w
            .page(
                "evaluate",
                json!({"source": "(a) => a.n + 1", "arg": {"n": 1}}),
            )
            .await
            .unwrap();
        assert_eq!(
            log.sent("Runtime.evaluate").last().unwrap()["params"]["expression"],
            "((a) => a.n + 1)({\"n\":1})"
        );
        assert!(e.ok);
        // A session is its run's alone.
        let theirs = w
            .sessions
            .call(
                "run-2",
                "s1",
                SessionCall::Page {
                    action: "title".into(),
                    args: json!({}),
                },
                CallCtx::new(w.clock.now() + Duration::seconds(60), CancelToken::new()),
            )
            .await;
        assert_eq!(code(theirs), CODE_BROWSER_SESSION_LOST);
        assert_eq!(
            w.sessions.close("run-1", "s1", SessionEnd::Closed).await,
            Ok(1_500)
        );
        assert_eq!(
            code(w.page("title", json!({})).await),
            CODE_BROWSER_SESSION_CLOSED
        );
        assert!(
            w.sessions.ended().is_empty(),
            "the script's close is charged by its call"
        );
        assert_eq!(log.sent("Browser.close").len(), 1);
        assert_eq!(log.closes(), 1);
    }

    #[tokio::test]
    async fn requests_are_served_between_calls() {
        let w = World::new();
        let (_, log) = w.open(TIMES).await;
        let paused = |id: &str, url: &str| {
            event(
                "Fetch.requestPaused",
                json!({"requestId": id, "request": {"url": url, "method": "GET", "headers": {}}, "frameId": "F", "resourceType": "Script"}),
                Some("S"),
            )
        };
        let before = log.sent("Fetch.fulfillRequest").len();
        log.emit(paused("late", &format!("{ORIGIN}late.js")));
        log.wait_sent("Fetch.fulfillRequest", before + 1).await;
        log.emit(paused("away", "https://evil.example/?d=1"));
        log.wait_sent("Fetch.failRequest", 1).await;
        let failed = log.sent("Fetch.failRequest");
        assert_eq!(
            (&failed[0]["params"]["requestId"], &failed[0]["sessionId"]),
            (&json!("away"), &json!("S"))
        );
        assert_eq!(
            log.sent("Fetch.fulfillRequest").last().unwrap()["params"]["requestId"],
            "late"
        );
    }

    #[tokio::test]
    async fn the_heartbeat_is_sent_after_twenty_idle_seconds() {
        let w = World::new();
        let (_, log) = w.open(TIMES).await;
        w.advance(19);
        // A call after the timer fired is taken after it.
        w.page("title", json!({})).await.unwrap();
        assert!(log.sent("Browser.getVersion").is_empty());
        w.advance(20);
        log.wait_sent("Browser.getVersion", 1).await;
        w.advance(19);
        w.page("title", json!({})).await.unwrap();
        assert_eq!(
            log.sent("Browser.getVersion").len(),
            1,
            "the call was heard"
        );
        w.advance(20);
        log.wait_sent("Browser.getVersion", 2).await;
        w.advance(20);
        log.wait_sent("Browser.getVersion", 3).await;
        assert!(w.sessions.ended().is_empty());
    }

    #[tokio::test]
    async fn the_idle_end_fires_by_the_deadline() {
        let w = World::new();
        let (_, log) = w.open(TIMES).await;
        w.clock.advance(100);
        w.page("title", json!({})).await.unwrap();
        // Heartbeats keep the browser; the idle time counts from the call.
        for _ in 0..14 {
            w.advance(20);
        }
        w.advance(19);
        // A second short of the idle time, the task beats and stays.
        log.wait_sent("Browser.getVersion", 1).await;
        assert!(w.sessions.ended().is_empty());
        w.advance(300);
        assert_eq!(
            code(w.page("title", json!({})).await),
            CODE_BROWSER_SESSION_CLOSED
        );
        let ended = w.sessions.ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(
            (
                ended[0].reason,
                ended[0].kind,
                ended[0].session.as_str(),
                ended[0].run.as_str()
            ),
            (SessionEnd::Idle, SessionKind::Artifact, "s1", "run-1")
        );
        assert!(ended[0].at_ms >= 400_000, "{:?}", ended[0]);
        assert_eq!(log.sent("Browser.close").len(), 1);
        assert_eq!(log.sent("Target.disposeBrowserContext").len(), 1);
        assert!(w.sessions.open_sessions("run-1").is_empty());
        assert!(w.sessions.ended().is_empty(), "drained");
    }

    /// A host's charger is handed each host end before the call that made
    /// it answers, and nothing is left queued; a close the script asked
    /// for is charged by its call and handed over to none.
    #[tokio::test]
    async fn a_host_end_is_handed_to_the_charger() {
        let w = World::new();
        let charged = Arc::new(Mutex::new(Vec::<Ended>::new()));
        let into = charged.clone();
        w.sessions
            .charge_with(Arc::new(move |e| into.lock().unwrap().extend(e)));
        let (opened, _log) = w.open(TIMES).await;
        assert_eq!(w.sessions.open_runs(), ["run-1"]);
        w.sessions.reevaluate(&|_, _| false).await;
        assert_eq!(
            charged
                .lock()
                .unwrap()
                .iter()
                .map(|e| (e.session.clone(), e.reason))
                .collect::<Vec<_>>(),
            [(opened.session.clone(), SessionEnd::Revoked)]
        );
        assert!(w.sessions.ended().is_empty());
        assert!(w.sessions.open_runs().is_empty());
    }

    #[tokio::test]
    async fn the_lifetime_end_cuts_a_running_call() {
        let w = World::new();
        let times = SessionTimes {
            call_seconds: 120,
            idle_seconds: 1_200,
            max_seconds: 60,
        };
        let (_, log) = w.open(times).await;
        let evaluated = log.sent("Runtime.evaluate").len();
        let call = w.page("evaluate", json!({"expression": "hang()"}));
        futures::pin_mut!(call);
        tokio::select! {
            r = &mut call => panic!("answered: {r:?}"),
            () = log.wait_sent("Runtime.evaluate", evaluated + 1) => {}
        }
        w.advance(60);
        assert_eq!(code(call.await), CODE_BROWSER_SESSION_CLOSED);
        let ended = w.sessions.ended();
        assert_eq!(
            ended
                .iter()
                .map(|e| (e.reason, e.at_ms))
                .collect::<Vec<_>>(),
            [(SessionEnd::Lifetime, 60_000)]
        );
        assert_eq!(log.sent("Browser.close").len(), 1);
    }

    #[tokio::test]
    async fn a_call_past_its_own_deadline_answers_a_timeout_and_the_session_stays() {
        let w = World::new();
        let (_, _log) = w.open(TIMES).await;
        let call = w.page("evaluate", json!({"expression": "hang()"}));
        futures::pin_mut!(call);
        tokio::select! {
            r = &mut call => panic!("answered: {r:?}"),
            () = _log.wait_sent("Runtime.evaluate", 3) => {}
        }
        w.advance(60);
        let a = call.await.unwrap();
        assert_eq!(
            (a.ok, a.timeout, a.error.as_deref()),
            (
                false,
                true,
                Some("page.evaluate: Timeout 60000ms exceeded.")
            )
        );
        assert!(w.page("title", json!({})).await.unwrap().ok);
    }

    #[tokio::test]
    async fn a_crashed_page_ends_the_session_as_lost() {
        for frame in [
            event("Inspector.targetCrashed", json!({}), Some("S")),
            event(
                "Target.detachedFromTarget",
                json!({"sessionId": "S", "targetId": "T"}),
                None,
            ),
        ] {
            let w = World::new();
            let (_, log) = w.open(TIMES).await;
            w.clock.advance(5);
            log.emit(frame);
            assert_eq!(
                code(w.page("title", json!({})).await),
                CODE_BROWSER_SESSION_LOST
            );
            assert_eq!(
                code(w.page("title", json!({})).await),
                CODE_BROWSER_SESSION_LOST
            );
            let ended = w.sessions.ended();
            assert_eq!(
                ended
                    .iter()
                    .map(|e| (e.reason, e.at_ms))
                    .collect::<Vec<_>>(),
                [(SessionEnd::Lost, 0)],
                "a lost session charges nothing past its calls"
            );
            assert_eq!(log.closes(), 1);
        }
    }

    #[tokio::test]
    async fn an_iframe_and_a_worker_are_held_to_the_policy() {
        let w = World::new();
        let (_, log) = w.open(TIMES).await;
        let attached = |session: &str, target: &str, ty: &str| {
            event(
                "Target.attachedToTarget",
                json!({"sessionId": session, "targetInfo": {"targetId": target, "type": ty, "url": "https://evil.example/"}, "waitingForDebugger": true}),
                Some("S"),
            )
        };
        log.emit(attached("IF", "F1", "iframe"));
        log.emit(attached("W", "W1", "worker"));
        log.wait_sent("Runtime.runIfWaitingForDebugger", 2).await;
        let on = |s: &str| -> Vec<String> {
            log.commands()
                .iter()
                .filter(|c| c["sessionId"] == s)
                .map(|c| c["method"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            on("IF"),
            [
                "Fetch.enable",
                "Target.setAutoAttach",
                "Runtime.runIfWaitingForDebugger"
            ]
        );
        assert_eq!(
            on("W"),
            [
                "Fetch.enable",
                "Target.setAutoAttach",
                "Runtime.evaluate",
                "Runtime.runIfWaitingForDebugger"
            ]
        );
        let worker_eval = log
            .sent("Runtime.evaluate")
            .into_iter()
            .find(|c| c["sessionId"] == "W")
            .unwrap();
        assert_eq!(worker_eval["params"]["expression"], WORKER_PREAMBLE_JS);
        assert!(WORKER_PREAMBLE_JS.contains("'WebSocket', 'WebTransport'"));
        // Their requests are answered by the policy, on their own sessions.
        log.emit(event(
            "Fetch.requestPaused",
            json!({"requestId": "i1", "request": {"url": "https://evil.example/x", "method": "GET", "headers": {}}, "frameId": "F1", "resourceType": "Document"}),
            Some("IF"),
        ));
        log.emit(event(
            "Fetch.requestPaused",
            json!({"requestId": "w1", "request": {"url": format!("{ORIGIN}data.json"), "method": "GET", "headers": {}}, "resourceType": "Fetch"}),
            Some("W"),
        ));
        log.wait_sent("Fetch.failRequest", 1).await;
        // The document's, then the worker's.
        log.wait_sent("Fetch.fulfillRequest", 2).await;
        assert_eq!(log.sent("Fetch.failRequest")[0]["sessionId"], "IF");
        let to_worker = log
            .sent("Fetch.fulfillRequest")
            .into_iter()
            .find(|c| c["params"]["requestId"] == "w1")
            .unwrap();
        assert_eq!(to_worker["sessionId"], "W");
        assert!(log.sent("Target.closeTarget").is_empty());
    }

    #[tokio::test]
    async fn a_popup_is_closed_after_auto_attach() {
        let w = World::new();
        let (_, log) = w.open(TIMES).await;
        for (session, target, ty) in [("P", "P2", "page"), ("SW", "SW1", "service_worker")] {
            log.emit(event(
                "Target.attachedToTarget",
                json!({"sessionId": session, "targetInfo": {"targetId": target, "type": ty, "url": "https://evil.example/"}, "waitingForDebugger": true}),
                None,
            ));
        }
        log.wait_sent("Target.closeTarget", 2).await;
        let closed: Vec<Value> = log
            .sent("Target.closeTarget")
            .iter()
            .map(|c| c["params"]["targetId"].clone())
            .collect();
        assert_eq!(closed, [json!("P2"), json!("SW1")]);
        // Held before they ran, they never ran.
        assert!(
            log.commands()
                .iter()
                .all(|c| c["sessionId"] != "P" && c["sessionId"] != "SW"),
            "nothing resumed the popup or the service worker"
        );
        assert_eq!((WHY_POPUP, WHY_TARGET), ("popup", "target"));
    }

    /// A session's handle is its root capability, held by the run that
    /// opened it; a forged, tampered, foreign or empty one, and one
    /// presented by another run, are lost, and no message repeats it.
    #[tokio::test]
    async fn a_handle_reaches_its_session_for_its_run_alone() {
        let w = World::new();
        let (o, _) = w.open(TIMES).await;
        assert!(
            o.handle.starts_with(super::super::cap::PREFIX),
            "{}",
            o.handle
        );
        assert_eq!(w.sessions.resolve("run-1", &o.handle).unwrap(), o.session);
        let mut tampered = o.handle.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == 'A' { 'B' } else { 'A' });
        let foreign = Keys::new([9; 32]).root(&o.session, "run-1").encode();
        for (run, handle) in [
            ("run-2", o.handle.as_str()),
            ("run-1", tampered.as_str()),
            ("run-1", foreign.as_str()),
            ("run-1", "00".repeat(32).as_str()),
            ("run-1", o.session.as_str()),
            ("run-1", ""),
        ] {
            let e = w.sessions.resolve(run, handle).unwrap_err();
            assert_eq!(e.code, CODE_BROWSER_SESSION_LOST);
            assert!(!e.message.contains(&o.handle), "{}", e.message);
            let e = w
                .sessions
                .authorize(run, handle, Some(Class::Read), &Nav::None)
                .unwrap_err();
            assert_eq!(e.code, CODE_BROWSER_SESSION_LOST);
        }
        let (other, _) = w.open(TIMES).await;
        assert_ne!(other.handle, o.handle);
    }

    fn ops(cs: &[Class]) -> Caveat {
        Caveat::Ops(cs.iter().copied().collect())
    }

    /// A read-only share reads and cannot act, navigate, record or close;
    /// one expired on the registry's clock allows nothing; an origin
    /// caveat holds a goto to its origins.
    #[tokio::test]
    async fn a_shared_capability_allows_no_more_than_its_caveats() {
        let w = World::new();
        let (o, _) = w.open(TIMES).await;
        let read = w
            .sessions
            .share("run-1", &o.handle, &[ops(&[Class::Read])])
            .unwrap();
        let read = read.encode();
        let auth = |h: &str, c: Option<Class>, nav: Nav| {
            w.sessions.authorize("run-1", h, c, &nav).map(|(s, _)| s)
        };
        assert_eq!(
            auth(&read, Some(Class::Read), Nav::None).unwrap(),
            o.session
        );
        for c in [Class::Act, Class::Navigate, Class::Record, Class::Close] {
            let e = auth(&read, Some(c), Nav::None).unwrap_err();
            assert_eq!(e.code, CODE_BROWSER_FORBIDDEN, "{c:?}");
            assert!(!e.message.contains(&read));
        }
        // Narrowing a narrowed capability never widens it.
        let wider = w
            .sessions
            .share("run-1", &read, &[ops(&Class::ALL)])
            .unwrap()
            .encode();
        assert_eq!(
            auth(&wider, Some(Class::Act), Nav::None).unwrap_err().code,
            CODE_BROWSER_FORBIDDEN
        );
        let now = w.clock.now().timestamp_millis();
        let soon = w
            .sessions
            .share("run-1", &o.handle, &[Caveat::Expires(now + 5_000)])
            .unwrap()
            .encode();
        assert!(auth(&soon, Some(Class::Act), Nav::None).is_ok());
        w.clock.advance(5);
        assert_eq!(
            auth(&soon, Some(Class::Read), Nav::None).unwrap_err().code,
            CODE_BROWSER_FORBIDDEN
        );
        assert!(auth(&o.handle, Some(Class::Act), Nav::None).is_ok());
        let only_a = w
            .sessions
            .share(
                "run-1",
                &o.handle,
                &[Caveat::Origin(["https://a.example".to_string()].into())],
            )
            .unwrap()
            .encode();
        assert!(
            auth(
                &only_a,
                Some(Class::Navigate),
                Nav::Goto("https://a.example/x".into())
            )
            .is_ok()
        );
        for nav in [
            Nav::Goto("https://b.example/".into()),
            Nav::Goto("/x".into()),
            Nav::History,
            Nav::Reload,
        ] {
            assert_eq!(
                auth(&only_a, Some(Class::Navigate), nav.clone())
                    .unwrap_err()
                    .code,
                CODE_BROWSER_FORBIDDEN,
                "{nav:?}"
            );
        }
    }

    /// Revoking a capability refuses it and every one shared from it, and
    /// nothing else; the root is closed, not revoked; closing the session
    /// ends every capability of it.
    #[tokio::test]
    async fn revoking_a_parent_revokes_its_children_and_closing_ends_all() {
        let w = World::new();
        let (o, _) = w.open(TIMES).await;
        let share = |h: &str| w.sessions.share("run-1", h, &[]).unwrap().encode();
        let parent = share(&o.handle);
        let child = share(&parent);
        let grandchild = share(&child);
        let sibling = share(&o.handle);
        let ok = |h: &str| {
            w.sessions
                .authorize("run-1", h, Some(Class::Read), &Nav::None)
        };
        assert_eq!(
            w.sessions.revoke("run-1", &o.handle).unwrap_err().code,
            crate::api::CODE_BAD_ARGS
        );
        let revoked = w.sessions.revoke("run-1", &parent).unwrap();
        assert_eq!(revoked.encode(), parent);
        for h in [&parent, &child, &grandchild] {
            let e = ok(h).unwrap_err();
            assert_eq!(e.code, CODE_BROWSER_FORBIDDEN);
            assert!(e.message.contains("revoked"), "{}", e.message);
            assert_eq!(
                w.sessions.share("run-1", h, &[]).unwrap_err().code,
                CODE_BROWSER_FORBIDDEN
            );
        }
        assert!(ok(&o.handle).is_ok() && ok(&sibling).is_ok());
        w.sessions
            .close("run-1", "s1", SessionEnd::Closed)
            .await
            .unwrap();
        for h in [&o.handle, &sibling, &child] {
            assert_eq!(ok(h).unwrap_err().code, CODE_BROWSER_SESSION_CLOSED);
        }
    }

    /// A session stays open while a capability of it that is held by its
    /// run and not revoked is held, whatever its classes or expiry; it
    /// closes `dropped`, queued for the store, once none is.
    #[tokio::test]
    async fn a_session_not_held_closes_dropped() {
        let w = World::new();
        let (o, log) = w.open(TIMES).await;
        let now = w.clock.now().timestamp_millis();
        let read = w
            .sessions
            .share(
                "run-1",
                &o.handle,
                &[ops(&[Class::Read]), Caveat::Expires(now + 1_000)],
            )
            .unwrap()
            .encode();
        w.sessions
            .close_unheld("run-1", std::slice::from_ref(&o.handle))
            .await;
        w.clock.advance(3);
        w.sessions
            .close_unheld("run-1", std::slice::from_ref(&read))
            .await;
        assert!(
            w.sessions.ended().is_empty(),
            "an expired read-only page holds it"
        );
        w.sessions.close_unheld("run-2", &[]).await;
        assert!(
            w.sessions.ended().is_empty(),
            "another run's call ends nothing"
        );
        let foreign = Keys::new([9; 32]).root("s1", "run-1").encode();
        let mut tampered = read.clone();
        tampered.insert(10, 'x');
        let revoked = w.sessions.share("run-1", &o.handle, &[]).unwrap().encode();
        w.sessions.revoke("run-1", &revoked).unwrap();
        w.sessions
            .close_unheld("run-1", &[foreign, tampered, revoked, "x".into()])
            .await;
        assert_eq!(
            w.sessions
                .ended()
                .iter()
                .map(|e| (e.reason, e.at_ms))
                .collect::<Vec<_>>(),
            [(SessionEnd::Dropped, 3_000)]
        );
        assert_eq!(log.sent("Browser.close").len(), 1);
        assert_eq!(
            code(w.page("title", json!({})).await),
            CODE_BROWSER_SESSION_CLOSED
        );
    }

    #[tokio::test]
    async fn the_run_end_and_a_revoke_close_and_queue_their_ends() {
        let w = World::new();
        let (_, _) = w.open(TIMES).await;
        w.clock.advance(7);
        w.sessions
            .reevaluate(&|_, k| k != SessionKind::Artifact)
            .await;
        assert_eq!(
            code(w.page("title", json!({})).await),
            CODE_BROWSER_SESSION_CLOSED
        );
        let ended = w.sessions.ended();
        assert_eq!(
            ended
                .iter()
                .map(|e| (e.reason, e.at_ms))
                .collect::<Vec<_>>(),
            [(SessionEnd::Revoked, 7_000)]
        );
        let w = World::new();
        let (_, log) = w.open(TIMES).await;
        w.sessions.reevaluate(&|_, _| true).await;
        assert!(
            w.sessions.ended().is_empty(),
            "a session still covered stays"
        );
        w.sessions.end_run("run-1").await;
        assert_eq!(
            w.sessions
                .ended()
                .iter()
                .map(|e| e.reason)
                .collect::<Vec<_>>(),
            [SessionEnd::RunEnded]
        );
        assert_eq!(
            code(w.page("title", json!({})).await),
            CODE_BROWSER_SESSION_LOST
        );
        assert_eq!(log.closes(), 1);
    }

    /// A URL session's policy for the tests: continues every request.
    struct Web;

    #[async_trait::async_trait]
    impl RequestPolicy for Web {
        async fn decide(&self, _req: &Paused) -> Decision {
            Decision::Continue
        }
    }

    #[tokio::test]
    async fn a_goto_within_the_origin_needs_nothing() {
        let w = World::new();
        let (_, log) = w.open(TIMES).await;
        assert_eq!(w.sessions.leaves("run-1", "s1", "/about.html"), Ok(false));
        assert_eq!(
            w.sessions.leaves("run-1", "s1", &format!("{ORIGIN}a?b#c")),
            Ok(false)
        );
        let g = w.page("goto", json!({"url": "about.html"})).await.unwrap();
        assert!(g.ok, "{g:?}");
        assert_eq!(
            (g.url.as_str(), g.kind),
            (format!("{ORIGIN}about.html").as_str(), None)
        );
        assert!(log.sent("Fetch.continueRequest").is_empty());
    }

    #[tokio::test]
    async fn a_goto_out_converts_the_session_once() {
        let w = World::new();
        let (_, log) = w.open(TIMES).await;
        let out = "https://example.com/page";
        assert_eq!(w.sessions.leaves("run-1", "s1", out), Ok(true));
        // Without the URL policy browser.open.url gives, it is refused.
        let refused = w.page("goto", json!({"url": out})).await;
        assert_eq!(
            refused.unwrap_err().code,
            crate::browser::CODE_BROWSER_DISABLED
        );
        let g = w
            .call_with("goto", json!({"url": out}), None, Some(Arc::new(Web)))
            .await
            .unwrap();
        assert!(g.ok, "{g:?}");
        assert_eq!((g.url.as_str(), g.kind), (out, Some(SessionKind::Url)));
        assert_eq!(
            w.sessions.open_sessions("run-1"),
            [("s1".to_string(), SessionKind::Url)]
        );
        // The synthetic origin is no longer served: a request to it fails,
        // why left; the web is the URL policy's.
        log.emit(event(
            "Fetch.requestPaused",
            json!({"requestId": "back", "request": {"url": format!("{ORIGIN}_clax/api/docs"), "method": "GET", "headers": {}}, "resourceType": "Fetch"}),
            Some("S"),
        ));
        log.wait_sent("Fetch.failRequest", 1).await;
        assert_eq!(
            log.sent("Fetch.failRequest")[0]["params"]["requestId"],
            "back"
        );
        assert!(
            !log.sent("Fetch.continueRequest").is_empty(),
            "the page out went to the web"
        );
        // Converted once: a second goto elsewhere changes no kind.
        let again = w
            .call_with(
                "goto",
                json!({"url": "https://example.org/"}),
                None,
                Some(Arc::new(Web)),
            )
            .await
            .unwrap();
        assert_eq!(again.kind, None);
        assert_eq!(
            w.sessions.leaves("run-1", "s1", "https://example.org/x"),
            Ok(false)
        );
        // A URL session runs no saved tests.
        let t = w.page("runTests", json!({})).await;
        assert_eq!(t.unwrap_err().code, crate::browser::CODE_BROWSER_DISABLED);
    }

    #[tokio::test]
    async fn an_artifact_session_serves_the_draft_as_the_last_call_saw_it() {
        use crate::browser::serve::Site;
        use crate::browser::session::ArtifactPolicy;
        use crate::draft::DraftEntry;
        use base64::Engine as _;
        let dir = tempfile::tempdir().unwrap();
        let store =
            Arc::new(clax_store::Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap());
        let draft = |body: &str| {
            let mut files = std::collections::BTreeMap::new();
            files.insert(
                "index.html".to_string(),
                DraftEntry {
                    content: body.as_bytes().to_vec(),
                    content_type: "text/html".into(),
                },
            );
            Pages::Draft(files, 1)
        };
        let w = World::new();
        let aid = ArtifactId::parse(AID).unwrap();
        let policy = ArtifactPolicy::new(
            Site::new(&aid, draft("<h1>One</h1>")),
            w.src.clone(),
            store,
            "owner",
        );
        let start = Start::Test {
            origin: Origin::new(&aid),
            path: "index.html".into(),
        };
        w.sessions
            .open(
                "run-1",
                "s1".into(),
                SessionKind::Artifact,
                start,
                Arc::new(policy),
                Viewport::desktop(),
                TIMES,
                w.clock.now() + Duration::seconds(60),
                &CancelToken::new(),
            )
            .await
            .unwrap();
        let log = w.src.logs()[0].clone();
        let bodies = || -> Vec<String> {
            log.sent("Fetch.fulfillRequest")
                .iter()
                .map(|c| {
                    let b = base64::engine::general_purpose::STANDARD
                        .decode(c["params"]["body"].as_str().unwrap())
                        .unwrap();
                    String::from_utf8(b).unwrap()
                })
                .collect()
        };
        assert!(bodies()[0].contains("<h1>One</h1>"));
        // The draft changed between the calls: the reload is served what
        // the call saw.
        let r = w
            .call_with("reload", json!({}), Some(draft("<h1>Two</h1>")), None)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert!(bodies()[1].contains("<h1>Two</h1>"), "{:?}", bodies());
        // A call that brings no draft keeps the last one.
        w.page("reload", json!({})).await.unwrap();
        assert!(bodies()[2].contains("<h1>Two</h1>"));
    }

    #[tokio::test]
    async fn content_opens_at_its_session_origin() {
        use crate::browser::serve::{ContentFile, ContentSite};
        use crate::browser::session::ContentPolicy;
        let w = World::new();
        let site = ContentSite::new(
            "s0123456789a",
            b"<h1>Mine</h1>".to_vec(),
            std::collections::BTreeMap::<String, ContentFile>::new(),
        );
        let o = w
            .sessions
            .open(
                "run-1",
                "s1".into(),
                SessionKind::Content,
                Start::Content {
                    host: site.host.clone(),
                },
                Arc::new(ContentPolicy(site)),
                Viewport::desktop(),
                TIMES,
                w.clock.now() + Duration::seconds(60),
                &CancelToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            (o.url.as_str(), o.status),
            ("https://s0123456789a.clax-content.invalid/", 200)
        );
        assert_eq!(
            w.sessions
                .leaves("run-1", "s1", "https://abcdefghjkmn.clax-test.invalid/"),
            Ok(true)
        );
        assert_eq!(w.sessions.leaves("run-1", "s1", "/b.css"), Ok(false));
    }
}
