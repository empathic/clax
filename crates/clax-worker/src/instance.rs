//! [`InstanceDo`]: one Clax instance, the whole Clax server, in a Durable
//! Object.
//!
//! The object opens the instance's database in its own storage through the
//! `clax-do-vfs` VFS (`Store::open_on`, one connection, every store job
//! inline), keeps artifact content in git through the content store
//! `REPO_STORE` names (`crate::repos::worker::content_store`: versions in
//! `RepoDO`'s stand-in or Cloudflare Artifacts, assets in the `ASSETS`
//! bucket), made once per object and kept across the test route's
//! rebuilds, builds the server's state and router once
//! (`AppState::assemble`), and answers every request by calling the
//! router, its body read whole first (a body the router reads from the
//! runtime's stream would be read after the response was sent).
//!
//! # Hosted mode
//!
//! With `HOSTED` = `1` the instance is a hosted Clax
//! (`clax_server::hosted`): people sign in through the browser (`/setup`
//! with the `SETUP_SECRET` secret, invites, sign-in links, guests), each
//! person's browser and agents act within their rights, and agent tokens
//! reach `/mcp`. Its public origin is `PUBLIC_ORIGIN` (required), artifact
//! content is served on `CONTENT_ORIGIN` when set (and never on the public
//! origin), and clients are named by `CF-Connecting-IP`. The owner's token
//! (`CLAX_TOKEN`, else one the object made and keeps) still works for the
//! CLI. Without `HOSTED`, no request reaches the object over a connection
//! of the owner's machine (`clax_server::auth::conn_of` is `None` on
//! wasm32), so it serves with the daemon's rules for a request from another
//! machine: the owner is whoever presents the bearer token, and every
//! browser is a viewer.
//!
//! # Sockets
//!
//! `GET /api/stream`, `GET /api/artifacts/<id>/live` (the live layers) and
//! `GET /api/artifacts/<id>/room` (the `room` capability) upgrade to
//! WebSockets the object accepts with the hibernation API: the router
//! admits the request and opens the stream, live socket or room socket
//! (`routes::stream::open_socket`, `routes::live_layers::live_socket`,
//! `routes::room::room_socket`), taking live seats through the server's
//! seat functions, and the object holds it beside the socket. A stream's
//! subscriptions stay on `POST /api/stream/<id>`, through the router and
//! its admission rules. The object drains every socket (a stream's frames,
//! one SSE block a text message; a live socket's batches; a room's frames)
//! after every request, socket message and alarm, and from a task the
//! hubs wake (`Hub::on_queue`, `LiveLayers::on_queue`) whenever they queue
//! frames, so what spawned listeners and timers dispatch goes out too. The
//! runtime answers the shell's `ping` on the stream with `pong` itself, so
//! an idle socket never wakes the object.
//!
//! Each socket's attachment keeps its kind, its key, the credentials it
//! was opened with (`SocketCredentials`: the viewer cookie's ULID, a tag of
//! the token when it was presented, a hosted session by its cookie's hash,
//! never a token or a cookie) and what reopens it (the stream's ID; the
//! artifact; a room socket's peer label and named rooms). An object rebuilt
//! after hibernation or eviction has empty hubs: at its first event it
//! reopens each socket as the same caller, checked against the store again
//! (a hosted session must still be live and still read the page): a
//! stream gets a new stream and its `ready`, and the shell subscribes its
//! topics again; a live or room socket joins again and is sent its welcome
//! (and its rooms' peers).
//!
//! # Seats and presence across a wake
//!
//! The live seats held and the live presence reports are kept in the
//! object's storage after every event that changed them (a few hundred
//! bytes a seat or report), and taken back when the server is built: a
//! seated person stays seated, with their latest input, until their own
//! connection takes the seat again or it goes idle, so nobody who arrives
//! first after a wake takes it, and presence chips do not empty while tabs
//! report again.
//!
//! # Pushes
//!
//! `POST /_internal/pushed` at the host `crate::INSTANCE_INTERNAL`
//! (`{"artifact", "person", "session"}`; see the crate's Pushes) records
//! the artifact's pushed commits as versions published by that person and
//! session (`clax_server::pushes::reconcile`), answering `{"recorded":
//! [n, …]}`; streams hear of each as of a publish. A missing or deleted
//! artifact records nothing (200); a failure is 503.
//!
//! # Alarm
//!
//! The daemon's loops run from the object's alarm instead, set for when
//! the earliest of them next has something to do: `AppState::sweep` by
//! `AppState::next_sweep_in` (a working record or presence report lapsing,
//! a gone viewer dropping, a draft going idle, a detached stream's grace
//! ending, a live seat going idle, a question's grace ending: graces run
//! on no timer here, `questions::SweptGraces`), the grant expiry by
//! `Store::grants_next_due`, and the store's planner refresh an hour after
//! the last one once a request that may write was served. After every
//! request, socket event and alarm the object works that time out again and
//! moves the alarm earlier when it must; with nothing due it holds no
//! alarm, so an idle object, open sockets or not, sleeps until a request or
//! a message wakes it. An alarm is never set sooner than `SWEEP_INTERVAL`
//! ahead, the daemon's own sweep period. A server built anew starts a grace
//! for each open hook question no poll holds (its poll did not outlive the
//! old one).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use clax_core::content::ContentStore;
use clax_core::{Home, Store};
use clax_server::AppState;
use clax_server::identity::SocketCredentials;
use clax_server::routes::live_layers::{LiveOpened, LiveSocket, LiveStep};
use clax_server::routes::room::{RoomOpened, RoomSocket, RoomStep};
use clax_server::routes::stream::{SocketStream, open_stream};
use clax_server::stream::Conn;
use serde::{Deserialize, Serialize};
use tower_service::Service;
use wasm_bindgen::JsValue;
use worker::{
    DurableObject, Env, Request, Response, Result, State, WebSocket, WebSocketIncomingMessage,
    WebSocketPair, durable_object,
};

use crate::store::{DoPageStore, run};

/// The instance's content store (see the module docs).
#[cfg(target_arch = "wasm32")]
fn content_store(env: &Env) -> Result<Arc<dyn ContentStore>> {
    crate::repos::worker::content_store(env)
}

/// The content store's bindings exist only in a Worker.
#[cfg(not(target_arch = "wasm32"))]
fn content_store(_env: &Env) -> Result<Arc<dyn ContentStore>> {
    Err(worker::Error::RustError(
        "an instance's content store is a Worker's (wasm32)".into(),
    ))
}

/// Installs the server's git remotes (`crate::repos::worker::git_remotes`):
/// its git token routes mint from `RepoDO`'s ledger. Without the `REPO`
/// binding they answer `not_available`, as on the daemon.
#[cfg(target_arch = "wasm32")]
fn install_git_remotes(app: &AppState, env: &Env) {
    match crate::repos::worker::git_remotes(env) {
        Ok(r) => app.git_remotes.set(r),
        Err(e) => worker::console_error!("the git remotes are off: {e}"),
    }
}

/// The repositories' bindings exist only in a Worker.
#[cfg(not(target_arch = "wasm32"))]
fn install_git_remotes(_app: &AppState, _env: &Env) {}

/// The database file's name in the object's storage.
const DB_FILE: &str = "clax.db";
/// The soonest an alarm is set for, from now: the daemon's sweep period.
const MIN_ALARM_MS: f64 = clax_server::working::SWEEP_INTERVAL.as_millis() as f64;
/// How long after the last planner refresh the next is due, once a request
/// that may write was served.
const OPTIMIZE_MS: f64 = clax_core::store::OPTIMIZE_INTERVAL.as_millis() as f64;
/// The most bytes of one room socket message (as the daemon's room route).
const MAX_ROOM_MESSAGE: usize = 64 * 1024;

/// What a socket carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    /// `/api/stream` (attachments written before kinds name none).
    #[default]
    Stream,
    /// `/api/artifacts/<id>/live`.
    Live,
    /// `/api/artifacts/<id>/room`.
    Room,
}

/// What a socket's attachment carries across hibernation (well under the
/// platform's 2,048 bytes: two hex IDs, a ULID, a 16-digit tag, a 64-digit
/// hash, an artifact ID, a peer label and at most a few room names).
#[derive(Serialize, Deserialize)]
struct Attachment {
    #[serde(default, skip_serializing_if = "is_stream")]
    kind: Kind,
    sock: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    stream: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    aid: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    peer: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rooms: Vec<String>,
    #[serde(flatten)]
    creds: SocketCredentials,
}

fn is_stream(k: &Kind) -> bool {
    *k == Kind::Stream
}

/// What a socket carries, live.
enum Carried {
    Stream(Box<Conn>),
    Live(Box<LiveSocket>),
    Room(Box<RoomSocket>),
}

/// A socket and what it carries.
struct Socket {
    ws: WebSocket,
    carried: Carried,
}

/// The server: built once per object, rebuilt only by the test route.
struct Instance {
    app: AppState,
    router: axum::Router,
    sockets: RefCell<HashMap<String, Socket>>,
    /// The seats and presence last written to storage, as written.
    kept: RefCell<String>,
    /// Whether the open hook questions' graces were started.
    graced: Cell<bool>,
    /// Kept so the streams' `shutdown` never closes.
    _shutdown: tokio::sync::watch::Sender<bool>,
    /// Dropped last: keeps the database file in the VFS.
    _db: clax_do_vfs::Registered,
}

/// One Clax instance (see the module docs).
#[durable_object]
pub struct InstanceDo {
    state: State,
    env: Env,
    /// The content store, made once per object.
    content: std::result::Result<Arc<dyn ContentStore>, String>,
    inner: RefCell<std::result::Result<Rc<Instance>, String>>,
    /// Whether the sockets the object found at its start were reopened.
    revived: Cell<bool>,
    /// The alarm's time (ms) as this object last read or set it: `None`
    /// before it has been read, `Some(None)` with none set.
    alarm: Cell<Option<Option<f64>>>,
    /// Whether a request that may write was served since the last planner
    /// refresh.
    wrote: Cell<bool>,
    last_optimize: Cell<f64>,
    /// The request bodies the object holds.
    bodies: BodyBudget,
}

fn var(env: &Env, name: &str) -> String {
    env.var(name).map(|v| v.to_string()).unwrap_or_default()
}

/// A secret, else a var of the name (`wrangler dev --var`), else empty.
fn secret(env: &Env, name: &str) -> String {
    env.secret(name)
        .map(|v| v.to_string())
        .unwrap_or_else(|_| var(env, name))
}

fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// `bytes` random bytes from the platform's cryptographic source
/// (`crypto.getRandomValues`), in hex.
fn random_hex(bytes: usize) -> Result<String> {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).map_err(rust_err)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn rust_err(e: impl std::fmt::Display) -> worker::Error {
    worker::Error::RustError(e.to_string())
}

/// The object's own key-value rows (`_clax_instance`).
fn kv_table(state: &State) -> Result<worker::SqlStorage> {
    let sql = state.storage().sql();
    run(
        &sql,
        "CREATE TABLE IF NOT EXISTS _clax_instance (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        vec![],
    )?;
    Ok(sql)
}

fn kv_get(state: &State, key: &str) -> Result<Option<String>> {
    let sql = kv_table(state)?;
    let row = sql
        .exec_raw(
            "SELECT value FROM _clax_instance WHERE key = ?",
            vec![JsValue::from(key)],
        )?
        .raw()
        .next()
        .transpose()?;
    Ok(match row.as_deref() {
        Some([worker::SqlStorageValue::String(t)]) => Some(t.clone()),
        _ => None,
    })
}

fn kv_put(state: &State, key: &str, value: &str) -> Result<()> {
    let sql = kv_table(state)?;
    run(
        &sql,
        "INSERT INTO _clax_instance (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        vec![JsValue::from(key), JsValue::from(value)],
    )
}

/// The fewest characters a set `CLAX_TOKEN` may have: the owner's
/// credential of a public Worker must not be guessable (the one the object
/// makes is 64 hex digits).
const MIN_TOKEN_CHARS: usize = 32;

/// `set`, a `CLAX_TOKEN`, when it is long enough to be the owner's token;
/// otherwise why not.
fn checked_token(set: &str) -> std::result::Result<(), String> {
    if set.chars().count() < MIN_TOKEN_CHARS {
        return Err(format!(
            "CLAX_TOKEN must be at least {MIN_TOKEN_CHARS} characters (it has {}); \
             set a random one, or unset it and the instance makes its own",
            set.chars().count()
        ));
    }
    Ok(())
}

/// The owner's token: `CLAX_TOKEN` when set (refused when shorter than
/// [`MIN_TOKEN_CHARS`]), else the one the object keeps in its storage, made
/// on first use.
fn owner_token(state: &State, env: &Env) -> Result<String> {
    let set = secret(env, "CLAX_TOKEN");
    if !set.is_empty() {
        checked_token(&set).map_err(worker::Error::RustError)?;
        return Ok(set);
    }
    if let Some(t) = kv_get(state, "token")? {
        return Ok(t);
    }
    let token = random_hex(32)?;
    kv_put(state, "token", &token)?;
    Ok(token)
}

/// Hosted mode as the Worker's vars and secrets set it (see the module
/// docs): `None` without `HOSTED` = `1`; an error for a hosted instance
/// whose origins hosted mode does not take.
fn hosted(env: &Env) -> Result<Option<Arc<clax_server::hosted::Hosted>>> {
    if var(env, "HOSTED") != "1" {
        return Ok(None);
    }
    let public = var(env, "PUBLIC_ORIGIN");
    let setup = secret(env, "SETUP_SECRET");
    let h = clax_server::hosted::Hosted::new(&public, Some(setup.as_str()))
        .ok_or_else(|| {
            rust_err(format!(
                "HOSTED is 1 and PUBLIC_ORIGIN must be an https:// origin (http:// on localhost), not {public:?}"
            ))
        })?
        .behind_cloudflare(true);
    let content = var(env, "CONTENT_ORIGIN");
    let h = if content.is_empty() {
        h
    } else {
        h.with_content_origin(&content).ok_or_else(|| {
            rust_err(format!(
                "CONTENT_ORIGIN must be an origin of its own host, not {content:?}"
            ))
        })?
    };
    // GitHub sign-in: on with `GITHUB_CLIENT_ID` and the secret
    // `GITHUB_CLIENT_SECRET`; `OWNER_GITHUB` names the owner's login;
    // `GITHUB_OAUTH_BASE` and `GITHUB_API_BASE` point a test at a fake.
    let opt = |name: &str| Some(var(env, name)).filter(|v| !v.is_empty());
    let github = clax_server::github::GitHub::new(
        &var(env, "GITHUB_CLIENT_ID"),
        &secret(env, "GITHUB_CLIENT_SECRET"),
        opt("OWNER_GITHUB").as_deref(),
        opt("GITHUB_OAUTH_BASE").as_deref(),
        opt("GITHUB_API_BASE").as_deref(),
    );
    let max_artifacts = opt("MEMBER_MAX_ARTIFACTS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(clax_core::store::github::DEFAULT_MAX_ARTIFACTS);
    let h = h.with_github(github, max_artifacts);
    Ok(Some(Arc::new(h)))
}

/// The seats and presence the object keeps across a wake.
#[derive(Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    seats: Vec<clax_core::seats::KeptSeat>,
    #[serde(default)]
    presence: Vec<clax_core::presence::KeptPresence>,
}

impl Instance {
    fn build(
        state: &State,
        env: &Env,
        content: &std::result::Result<Arc<dyn ContentStore>, String>,
    ) -> Result<Rc<Instance>> {
        let content = content.clone().map_err(worker::Error::RustError)?;
        let hosted = hosted(env)?;
        let pages = Rc::new(DoPageStore::new(state.storage())?);
        let (conn, db) = clax_do_vfs::open(DB_FILE, pages)
            .map_err(rust_err)?
            .into_parts();
        let home = Home::at("/clax".into());
        let clock: Arc<dyn clax_core::working::Clock> = Arc::new(clax_core::working::SystemClock);
        let store =
            Arc::new(Store::open_on(&home, conn, clock.clone(), content).map_err(rust_err)?);
        let token = owner_token(state, env)?;
        let public = var(env, "PUBLIC_ORIGIN");
        let base = if public.is_empty() {
            "http://localhost".to_string()
        } else {
            public.trim_end_matches('/').to_string()
        };
        let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let app = AppState::assemble(clax_server::state::StateParts {
            started_at: store.now(),
            store,
            home,
            token,
            version: clax_server::VERSION,
            shutdown,
            self_base: base.clone(),
            browser_base: base,
            codex: clax_server::push::CodexPush::default(),
            sample: Arc::new(clax_server::sample::Sampler::disabled()),
            clock,
            // Graces wait for the alarm, not a timer that keeps the object up.
            question_sleeper: Arc::new(clax_server::questions::SweptGraces),
            agent_timer: Arc::new(clax_server::agent::TokioTimer::default()),
            extension_id: String::new(),
            terminal_after_s: clax_core::config::TERMINAL_AFTER_S,
            // Loopback: the most restrictive bind (invites need a LAN one).
            bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 0,
            hosted,
        })
        .map_err(rust_err)?;
        app.seats
            .set_limits(clax_core::seats::SeatLimits::from_env(|k| {
                Some(var(env, k)).filter(|v| !v.is_empty())
            }));
        // Before anyone can take a seat or report: what the last server held.
        let kept_json = kv_get(state, "live")?.unwrap_or_default();
        if let Ok(k) = serde_json::from_str::<Kept>(&kept_json) {
            app.seats.restore(&k.seats);
            app.presence.restore(&k.presence);
        }
        install_git_remotes(&app, env);
        app.start_listeners();
        let router = clax_server::build_router(app.clone());
        let inst = Rc::new(Instance {
            app,
            router,
            sockets: RefCell::default(),
            kept: RefCell::new(kept_json),
            graced: Cell::new(false),
            _shutdown: shutdown_tx,
            _db: db,
        });
        // The hubs wake this task whenever they queue frames, from whatever
        // task dispatched them.
        let wake = Arc::new(tokio::sync::Notify::new());
        let tell = wake.clone();
        inst.app
            .stream
            .on_queue(Arc::new(move || tell.notify_one()));
        let tell = wake.clone();
        inst.app
            .live_layers
            .on_queue(Arc::new(move || tell.notify_one()));
        let weak: Weak<Instance> = Rc::downgrade(&inst);
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                wake.notified().await;
                let Some(inst) = weak.upgrade() else { return };
                inst.settle().await;
            }
        });
        Ok(inst)
    }

    /// Sends every socket what it has; closes those that ended; checks
    /// again the room sockets of artifacts with a new version.
    async fn settle(&self) {
        let recheck = self.drain_all();
        for (key, aid) in recheck {
            if !clax_server::routes::room::still_declared(&self.app, &aid).await {
                self.close(&key, 4403, "revoked");
            }
        }
    }

    /// [`Instance::drain`] for every socket with something pending: the
    /// room sockets to check again, with their artifacts.
    fn drain_all(&self) -> Vec<(String, String)> {
        let keys: Vec<String> = self
            .sockets
            .borrow()
            .iter()
            .filter(|(_, s)| match &s.carried {
                Carried::Stream(c) => c.pending(),
                Carried::Live(_) | Carried::Room(_) => true,
            })
            .map(|(k, _)| k.clone())
            .collect();
        keys.into_iter()
            .filter_map(|k| self.drain(&k).map(|aid| (k, aid)))
            .collect()
    }

    fn close(&self, key: &str, code: u16, reason: &str) {
        if let Some(s) = self.sockets.borrow_mut().remove(key) {
            let _ = s.ws.close(Some(code), Some(reason));
        }
    }

    /// Sends socket `key` what it carries has; closes it once that has
    /// ended. A room socket of an artifact with a new version answers its
    /// artifact, to be checked again.
    fn drain(&self, key: &str) -> Option<String> {
        let mut sockets = self.sockets.borrow_mut();
        let s = sockets.get_mut(key)?;
        let mut end: Option<(u16, &'static str)> = None;
        let mut recheck = None;
        match &mut s.carried {
            Carried::Stream(conn) => match conn.drain() {
                Some(frames) => {
                    for f in frames {
                        if s.ws.send_with_str(String::from_utf8_lossy(&f)).is_err() {
                            break;
                        }
                    }
                }
                None => end = Some((1000, "the stream ended")),
            },
            Carried::Live(live) => {
                for step in live.drain() {
                    match step {
                        LiveStep::Send(t) => {
                            let _ = s.ws.send_with_str(t);
                        }
                        LiveStep::Close(c, r) => end = Some((c, r)),
                    }
                }
            }
            Carried::Room(room) => {
                let aid = room_aid(&s.ws);
                for step in room.drain() {
                    match step {
                        RoomStep::Send(t) => {
                            let _ = s.ws.send_with_str(t);
                        }
                        RoomStep::Close(c, r) => end = Some((c, r)),
                        RoomStep::Recheck => recheck = aid.clone(),
                    }
                }
            }
        }
        if let Some((code, reason)) = end {
            let _ = s.ws.close(Some(code), Some(reason));
            sockets.remove(key);
            return None;
        }
        recheck
    }

    /// Writes the seats held and the presence reports to the object's
    /// storage when they changed since the last write.
    fn keep(&self, state: &State) {
        let k = Kept {
            seats: self.app.seats.kept(),
            presence: self.app.presence.kept(),
        };
        let Ok(json) = serde_json::to_string(&k) else {
            return;
        };
        if *self.kept.borrow() == json {
            return;
        }
        if kv_put(state, "live", &json).is_ok() {
            *self.kept.borrow_mut() = json;
        }
    }
}

/// A room socket's artifact, from its attachment.
fn room_aid(ws: &WebSocket) -> Option<String> {
    ws.deserialize_attachment::<Attachment>()
        .ok()
        .flatten()
        .map(|a| a.aid)
}

/// The most bytes of one request's body the object reads. The daemon's
/// routes take more (a publish up to 96 MiB), but on the Worker the body,
/// its parse and what is made of it share one isolate's 128 MB with every
/// object in it: here a request body is at most 16 MiB, and a push to the
/// stand-in at most 12 MiB.
const MAX_BODY: usize = 16 << 20;
/// The most bytes of the body of a request that carries no credential (no
/// `Authorization` and no `Cookie`): what setup, sign-in and naming a guest
/// send, with room to spare.
const MAX_ANONYMOUS_BODY: usize = 256 << 10;
/// The most bytes of request bodies the object holds at once, across every
/// request it is reading or serving: past it a request is refused 429
/// before more is read, so concurrent bodies cannot exhaust the isolate.
const BODY_BUDGET: usize = 32 << 20;

/// 413 `body_too_large`, as the router answers a body over a route's limit.
fn too_large() -> Result<Response> {
    refused(
        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        "body_too_large",
        "the request body is too large",
    )
}

/// 429 `limit_reached`: the body would take the object past [`BODY_BUDGET`].
fn busy() -> Result<Response> {
    refused(
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "limit_reached",
        "the instance is reading too many request bodies; try again shortly",
    )
}

fn refused(status: axum::http::StatusCode, code: &'static str, message: &str) -> Result<Response> {
    use axum::response::IntoResponse;
    Response::try_from(clax_server::error::ApiError::new(status, code, message).into_response())
}

/// The bytes of request bodies the object holds (see [`BODY_BUDGET`]).
#[derive(Default)]
struct BodyBudget {
    held: Cell<usize>,
}

/// The bytes of a [`BodyBudget`] one request holds, given back when it is
/// dropped: once the request was answered, or when its future is dropped.
struct BodyHold<'a> {
    budget: &'a BodyBudget,
    held: usize,
}

impl BodyBudget {
    fn hold(&self) -> BodyHold<'_> {
        BodyHold {
            budget: self,
            held: 0,
        }
    }
}

impl BodyHold<'_> {
    /// Holds `to` bytes in all (never fewer than it holds); `false`, with
    /// nothing more held, when that would pass the budget.
    fn grow_to(&mut self, to: usize) -> bool {
        if to <= self.held {
            return true;
        }
        let total = self.budget.held.get() + (to - self.held);
        if total > BODY_BUDGET {
            return false;
        }
        self.budget.held.set(total);
        self.held = to;
        true
    }
}

impl Drop for BodyHold<'_> {
    fn drop(&mut self) {
        let total = self.budget.held.get();
        self.budget.held.set(total.saturating_sub(self.held));
    }
}

/// The most bytes of the body of a request: [`MAX_ANONYMOUS_BODY`] for one
/// that presents no credential, else [`MAX_BODY`].
fn body_limit(authorization: bool, cookie: bool) -> usize {
    if authorization || cookie {
        MAX_BODY
    } else {
        MAX_ANONYMOUS_BODY
    }
}

/// `req`'s body limit ([`body_limit`]) and its declared length, when it
/// declares one; `None` for a request without a body (`GET`, `HEAD`).
fn declared_body(req: &Request) -> Result<Option<(usize, Option<u64>)>> {
    if matches!(req.method(), worker::Method::Get | worker::Method::Head) {
        return Ok(None);
    }
    let headers = req.headers();
    let limit = body_limit(headers.has("authorization")?, headers.has("cookie")?);
    let declared = headers
        .get("content-length")?
        .and_then(|v| v.trim().parse::<u64>().ok());
    Ok(Some((limit, declared)))
}

/// 413 for a request whose declared body is over its limit, which the
/// Worker answers before handing it to the object (an object's answer to a
/// body it never read breaks the next request on that connection under
/// `wrangler dev`); `None` otherwise.
pub(crate) fn refuse_declared(req: &Request) -> Result<Option<Response>> {
    match declared_body(req)? {
        Some((limit, Some(n))) if n > limit as u64 => Ok(Some(too_large()?)),
        _ => Ok(None),
    }
}

/// The runtime's request as the router takes it, its body read whole
/// first: the router may read a body after the response it answers with
/// is built, and the runtime's stream cannot be read once the response
/// has been sent. A body over its limit ([`body_limit`]) is not read past
/// it: the answer is 413 (`Err`), before anything else sees the request;
/// one that would take the object past [`BODY_BUDGET`] is 429. The body's
/// bytes stay held in `hold` until it is dropped.
async fn http_request(
    mut req: Request,
    hold: &mut BodyHold<'_>,
) -> Result<std::result::Result<axum::http::Request<axum::body::Body>, Response>> {
    use futures_util::StreamExt;
    let method = req.method().to_string();
    let url = req.url()?;
    let mut body = Vec::new();
    if let Some((limit, declared)) = declared_body(&req)? {
        if declared.is_some_and(|n| n > limit as u64) {
            return Ok(Err(too_large()?));
        }
        // The declared length is held before any of it is read, so a body
        // that would not fit is refused unread, and the buffer never grows.
        let declared = declared.unwrap_or(0) as usize;
        if !hold.grow_to(declared) {
            return Ok(Err(busy()?));
        }
        body.reserve_exact(declared);
        // No stream: the request has no body.
        if let Ok(mut chunks) = req.stream() {
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk?;
                let len = body.len() + chunk.len();
                if len > limit {
                    return Ok(Err(too_large()?));
                }
                // The buffer grows as a Vec would, within the limit, and
                // what it takes is held first.
                if len > body.capacity() {
                    let grown = len.max(body.capacity() * 2).min(limit);
                    if !hold.grow_to(grown) {
                        return Ok(Err(busy()?));
                    }
                    body.reserve_exact(grown - body.len());
                }
                body.extend_from_slice(&chunk);
            }
        }
    }
    let mut b = axum::http::Request::builder()
        .method(method.as_str())
        .uri(url.as_str());
    for (k, v) in req.headers().entries() {
        b = b.header(k, v);
    }
    b.body(axum::body::Body::from(body))
        .map(Ok)
        .map_err(rust_err)
}

impl InstanceDo {
    /// The server; one whose build failed is built again first, so a
    /// failure at a start (storage the runtime could not read then) lasts
    /// only until the next event, not until the object is evicted.
    fn instance(&self) -> std::result::Result<Rc<Instance>, String> {
        let built = self.inner.borrow().clone();
        if built.is_ok() {
            return built;
        }
        let again =
            Instance::build(&self.state, &self.env, &self.content).map_err(|e| e.to_string());
        if again.is_ok() {
            self.revived.set(false);
        }
        let _ = self.inner.replace(again.clone());
        again
    }

    /// What every event starts with: the sockets the object holds but has
    /// nothing for (accepted before it hibernated or was evicted) reopened,
    /// and, once per server, a grace for each open hook question.
    async fn revive(&self, inst: &Rc<Instance>) {
        if !inst.graced.replace(true) {
            let _ = clax_server::questions::arm_open_hooks(&inst.app).await;
        }
        if self.revived.replace(true) {
            return;
        }
        for ws in self.state.get_websockets() {
            let Ok(Some(att)) = ws.deserialize_attachment::<Attachment>() else {
                let _ = ws.close(Some(1011), Some("no attachment"));
                continue;
            };
            if inst.sockets.borrow().contains_key(&att.sock) {
                continue;
            }
            self.reopen(inst, ws, att).await;
        }
    }

    /// Reopens socket `ws` as its attachment says (see the module docs);
    /// closes it when it may no longer open, and the client reconnects.
    async fn reopen(&self, inst: &Rc<Instance>, ws: WebSocket, mut att: Attachment) {
        let key = att.sock.clone();
        let carried = match att.kind {
            Kind::Stream => {
                let (creds, token) = (att.creds.clone(), inst.app.token.clone());
                let reopened = inst
                    .app
                    .store_call(move |st| creds.reopen(st, &token))
                    .await;
                let who = match reopened {
                    Ok((who, session)) if att.creds.session.is_none() || session.is_some() => who,
                    _ => {
                        let _ = ws.close(Some(1008), Some("the session ended"));
                        return;
                    }
                };
                match open_stream(&inst.app, who, false, None, None, att.creds.guest).await {
                    Ok(conn) => {
                        // Admitted once already: counted again, not refused.
                        let client =
                            clax_server::socket_caps::SocketClient::of_credentials(&att.creds);
                        let conn = conn.holding(Some(inst.app.sockets.hold(&client)));
                        att.stream = conn.stream_id().to_string();
                        Carried::Stream(Box::new(conn))
                    }
                    Err(_) => {
                        let _ = ws.close(Some(1008), Some("the stream could not be reopened"));
                        return;
                    }
                }
            }
            Kind::Live => {
                match clax_server::routes::live_layers::reopen(&inst.app, &att.aid, &att.creds)
                    .await
                {
                    Some(Ok((live, welcome))) => {
                        let _ = ws.send_with_str(welcome);
                        Carried::Live(Box::new(live))
                    }
                    Some(Err(LiveStep::Close(c, r))) => {
                        let _ = ws.close(Some(c), Some(r));
                        return;
                    }
                    _ => {
                        let _ = ws.close(Some(1008), Some("the live socket could not be reopened"));
                        return;
                    }
                }
            }
            Kind::Room => {
                match clax_server::routes::room::reopen(
                    &inst.app, &att.aid, &att.peer, &att.rooms, &att.creds,
                )
                .await
                {
                    Some(Ok((room, first))) => {
                        for f in first {
                            let _ = ws.send_with_str(f);
                        }
                        att.rooms = room.joined();
                        Carried::Room(Box::new(room))
                    }
                    Some(Err(RoomStep::Close(c, r))) => {
                        let _ = ws.close(Some(c), Some(r));
                        return;
                    }
                    _ => {
                        let _ = ws.close(Some(1008), Some("the room could not be reopened"));
                        return;
                    }
                }
            }
        };
        let _ = ws.serialize_attachment(&att);
        inst.sockets
            .borrow_mut()
            .insert(key.clone(), Socket { ws, carried });
        inst.drain(&key);
    }

    /// When the earliest of the object's timed work is next due (ms), as
    /// the module docs list it; `None` with nothing due.
    async fn next_due(&self, inst: &Instance) -> Option<f64> {
        let now = now_ms();
        let sweep = inst.app.next_sweep_in().map(|d| now + d.as_millis() as f64);
        let grants = match inst.app.store.call(|st| st.grants_next_due()).await {
            Ok(t) => t.map(|t| t.timestamp_millis() as f64),
            // Unknown: look again a sweep period from now.
            Err(_) => Some(now),
        };
        let optimize = self
            .wrote
            .get()
            .then(|| self.last_optimize.get() + OPTIMIZE_MS);
        [sweep, grants, optimize]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    /// What every event ends with: the seats and presence kept, and the
    /// alarm set for the next due work, no sooner than [`MIN_ALARM_MS`]
    /// from now, unless one already set fires first; deleted when nothing
    /// is due.
    async fn schedule(&self, inst: &Instance) {
        inst.keep(&self.state);
        let storage = self.state.storage();
        let set = match self.alarm.get() {
            Some(at) => at,
            None => match storage.get_alarm().await {
                Ok(at) => at.map(|ms| ms as f64),
                Err(_) => return,
            },
        };
        self.alarm.set(Some(set));
        let now = now_ms();
        let want = self
            .next_due(inst)
            .await
            .map(|at| at.max(now + MIN_ALARM_MS));
        match (set, want) {
            (None, None) => {}
            (Some(_), None) => {
                if storage.delete_alarm().await.is_ok() {
                    self.alarm.set(Some(None));
                }
            }
            (Some(at), Some(w)) if at <= w && at > now => {}
            (_, Some(w)) => {
                let delay = std::time::Duration::from_millis((w - now).max(0.0) as u64);
                if storage.set_alarm(delay).await.is_ok() {
                    self.alarm.set(Some(Some(now + delay.as_millis() as f64)));
                } else {
                    self.alarm.set(None);
                }
            }
        }
    }

    /// Accepts the socket for what the router opened, its first frames
    /// sent. A socket whose attachment cannot be kept is closed at once,
    /// never left accepted without one; one opened only to be closed (no
    /// live seat, no `room` declared) is accepted and closed with its code.
    fn accept(
        &self,
        inst: &Rc<Instance>,
        carried: std::result::Result<(Carried, Vec<String>), (u16, &'static str)>,
        mut att: Attachment,
    ) -> Result<Response> {
        let key = random_hex(16)?;
        let pair = WebSocketPair::new()?;
        self.state.accept_web_socket(&pair.server);
        let (carried, first) = match carried {
            Ok(c) => c,
            Err((code, reason)) => {
                let _ = pair.server.close(Some(code), Some(reason));
                return Response::from_websocket(pair.client);
            }
        };
        att.sock = key.clone();
        if let Carried::Stream(conn) = &carried {
            att.stream = conn.stream_id().to_string();
        }
        if let Err(e) = pair.server.serialize_attachment(&att) {
            let _ = pair.server.close(Some(1011), Some("no attachment"));
            return Err(e);
        }
        for f in first {
            let _ = pair.server.send_with_str(f);
        }
        inst.sockets.borrow_mut().insert(
            key.clone(),
            Socket {
                ws: pair.server,
                carried,
            },
        );
        inst.drain(&key);
        Response::from_websocket(pair.client)
    }

    /// The socket a response of the router opened, if it opened one: what
    /// it carries (or the close it opened with) and its attachment.
    #[allow(clippy::type_complexity)]
    fn opened(
        res: &mut axum::http::Response<axum::body::Body>,
    ) -> Option<(
        std::result::Result<(Carried, Vec<String>), (u16, &'static str)>,
        Attachment,
    )> {
        let att = |kind, aid: String, peer: String, creds| Attachment {
            kind,
            sock: String::new(),
            stream: String::new(),
            aid,
            peer,
            rooms: Vec::new(),
            creds,
        };
        let ext = res.extensions_mut();
        if let Some((conn, creds)) = ext
            .remove::<SocketStream>()
            .and_then(|s| s.0.lock().unwrap_or_else(|e| e.into_inner()).take())
        {
            return Some((
                Ok((Carried::Stream(Box::new(conn)), Vec::new())),
                att(Kind::Stream, String::new(), String::new(), creds),
            ));
        }
        if let Some((opened, creds)) = ext
            .remove::<LiveOpened>()
            .and_then(|s| s.0.lock().unwrap_or_else(|e| e.into_inner()).take())
        {
            let aid = match &opened {
                Ok((l, _)) => l.artifact().to_string(),
                Err(_) => String::new(),
            };
            let carried = match opened {
                Ok((l, welcome)) => Ok((Carried::Live(Box::new(l)), vec![welcome])),
                Err(LiveStep::Close(c, r)) => Err((c, r)),
                Err(LiveStep::Send(_)) => Err((1011, "no socket")),
            };
            return Some((carried, att(Kind::Live, aid, String::new(), creds)));
        }
        if let Some((opened, peer, creds)) = ext
            .remove::<RoomOpened>()
            .and_then(|s| s.0.lock().unwrap_or_else(|e| e.into_inner()).take())
        {
            let aid = match &opened {
                Ok((r, _)) => r.artifact().to_string(),
                Err(_) => String::new(),
            };
            let carried = match opened {
                Ok((r, first)) => Ok((Carried::Room(Box::new(r)), first)),
                Err(RoomStep::Close(c, r)) => Err((c, r)),
                Err(_) => Err((1011, "no socket")),
            };
            return Some((carried, att(Kind::Room, aid, peer, creds)));
        }
        None
    }

    /// The test routes, with `CLAX_TEST_ROUTES` = `1` and the token (else
    /// `None`: the router answers the path, a 404):
    ///
    /// - `POST /_worker/hibernate` drops the server and builds it again on
    ///   the same storage, as a wake from hibernation does, keeping the
    ///   sockets;
    /// - `GET /_worker/alarm`: `{alarm, now}`, the time the object's alarm
    ///   is set for (ms, `null` with none) and the object's time.
    async fn test_route(
        &self,
        inst: &Rc<Instance>,
        req: &axum::http::Request<axum::body::Body>,
    ) -> Option<Result<Response>> {
        let path = req.uri().path();
        if !matches!(path, "/_worker/hibernate" | "/_worker/alarm")
            || var(&self.env, "CLAX_TEST_ROUTES") != "1"
        {
            return None;
        }
        if !clax_server::auth::has_token(req.headers(), &inst.app.token) {
            return Some(Response::error("the token is required", 401));
        }
        if path == "/_worker/alarm" {
            let alarm = match self.state.storage().get_alarm().await {
                Ok(at) => at,
                Err(e) => return Some(Err(e)),
            };
            return Some(Response::from_json(
                &serde_json::json!({ "alarm": alarm, "now": now_ms() as i64 }),
            ));
        }
        // What a wake finds: only what storage kept.
        inst.keep(&self.state);
        let old = self.inner.replace(Err("rebuilding".into()));
        // The old sockets' carriers go with the old server, sockets kept.
        if let Ok(old) = old {
            old.sockets.borrow_mut().clear();
        }
        let fresh = match Instance::build(&self.state, &self.env, &self.content) {
            Ok(i) => i,
            Err(e) => return Some(Response::error(e.to_string(), 500)),
        };
        let _ = self.inner.replace(Ok(fresh));
        self.revived.set(false);
        Some(Response::ok("rebuilt"))
    }
}

impl DurableObject for InstanceDo {
    fn new(state: State, env: Env) -> Self {
        console_error_panic_hook::set_once();
        // The runtime answers the shell's keep-alive without waking us.
        if let Ok(pair) = worker::worker_sys::WebSocketRequestResponsePair::new("ping", "pong") {
            state.set_websocket_auto_response(&pair);
        }
        let content = content_store(&env).map_err(|e| e.to_string());
        let inner = Instance::build(&state, &env, &content).map_err(|e| e.to_string());
        InstanceDo {
            state,
            env,
            content,
            inner: RefCell::new(inner),
            revived: Cell::new(false),
            alarm: Cell::new(None),
            wrote: Cell::new(false),
            last_optimize: Cell::new(now_ms()),
            bodies: BodyBudget::default(),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let inst = match self.instance() {
            Ok(i) => i,
            Err(e) => return Response::error(format!("the instance could not start: {e}"), 500),
        };
        let url = req.url()?;
        if url.host_str() == Some(crate::INSTANCE_INTERNAL) {
            if url.path() != "/_internal/pushed" {
                return Response::error("Not found", 404);
            }
            let call: PushedCall = match req.json().await {
                Ok(c) => c,
                Err(e) => return Response::error(format!("{e}"), 400),
            };
            let out = self.record_pushes(&inst, call).await;
            self.schedule(&inst).await;
            return out;
        }
        let mut hold = self.bodies.hold();
        let req = match http_request(req, &mut hold).await? {
            Ok(r) => r,
            Err(refused) => return Ok(refused),
        };
        if let Some(res) = self.test_route(&inst, &req).await {
            return res;
        }
        self.revive(&inst).await;
        if !matches!(
            *req.method(),
            axum::http::Method::GET | axum::http::Method::HEAD
        ) {
            self.wrote.set(true);
        }
        let mut res = inst.router.clone().call(req).await.map_err(rust_err)?;
        let out = match Self::opened(&mut res) {
            Some((carried, att)) => self.accept(&inst, carried, att),
            None => {
                inst.settle().await;
                Response::try_from(res)
            }
        };
        self.schedule(&inst).await;
        out
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        let Ok(inst) = self.instance() else {
            return Ok(());
        };
        self.revive(&inst).await;
        let text = match &message {
            WebSocketIncomingMessage::String(t) => Some(t.as_str()),
            WebSocketIncomingMessage::Binary(_) => None,
        };
        let att = ws.deserialize_attachment::<Attachment>().ok().flatten();
        if let (Some(t), Some(mut att)) = (text, att) {
            self.on_text(&inst, &ws, &mut att, t);
        }
        inst.settle().await;
        self.schedule(&inst).await;
        Ok(())
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        _code: usize,
        _reason: String,
        _clean: bool,
    ) -> Result<()> {
        self.forget(&ws).await;
        let _ = ws.close(Some(1000), Some("closed"));
        Ok(())
    }

    async fn websocket_error(&self, ws: WebSocket, _error: worker::Error) -> Result<()> {
        self.forget(&ws).await;
        Ok(())
    }

    async fn alarm(&self) -> Result<Response> {
        // The alarm that woke us is spent.
        self.alarm.set(Some(None));
        let Ok(inst) = self.instance() else {
            return Response::ok("");
        };
        self.revive(&inst).await;
        inst.app.sweep().await;
        let now = now_ms();
        let grants_due = inst
            .app
            .store
            .call(|st| st.grants_next_due())
            .await
            .map_or(true, |t| {
                t.is_some_and(|t| t.timestamp_millis() as f64 <= now)
            });
        if grants_due {
            let _ = clax_server::routes::grants::sweep(&inst.app).await;
            // The sessions of grants that expired: their git tokens go
            // before the alarm ends, not from a task an eviction could drop.
            clax_server::git_remotes::settle(&inst.app).await;
        }
        if self.wrote.get() && now - self.last_optimize.get() >= OPTIMIZE_MS {
            self.last_optimize.set(now);
            self.wrote.set(false);
            let _ = inst.app.store.call(|st| st.optimize()).await;
        }
        inst.settle().await;
        self.schedule(&inst).await;
        Response::ok("")
    }
}

/// A call to record an artifact's pushed commits (see Pushes).
#[derive(Deserialize)]
struct PushedCall {
    artifact: String,
    person: Option<String>,
    session: Option<String>,
}

impl InstanceDo {
    /// Records `call`'s pushed commits and sends the streams their events.
    async fn record_pushes(&self, inst: &Rc<Instance>, call: PushedCall) -> Result<Response> {
        let Ok(id) = clax_core::ArtifactId::parse(&call.artifact) else {
            return Response::error("not an artifact ID", 400);
        };
        let pusher = clax_core::store::pushes::Pusher {
            person: call.person,
            session_id: call.session,
        };
        let out = match clax_server::pushes::reconcile(&inst.app, id, pusher).await {
            Ok(recorded) => Response::from_json(&serde_json::json!({ "recorded": recorded })),
            Err(e) => Response::error(format!("{e:?}"), 503),
        };
        inst.settle().await;
        out
    }

    /// A text message on socket `ws`: a stream's `ping` (the runtime
    /// answers it unless it arrives with other text) is answered `pong`;
    /// a live or room socket's goes to what it carries, and a room
    /// socket's named rooms are kept in its attachment.
    fn on_text(&self, inst: &Instance, ws: &WebSocket, att: &mut Attachment, t: &str) {
        let mut sockets = inst.sockets.borrow_mut();
        let Some(s) = sockets.get_mut(&att.sock) else {
            return;
        };
        let mut close: Option<(u16, &'static str)> = None;
        match &mut s.carried {
            Carried::Stream(_) => {
                if t == "ping" {
                    let _ = ws.send_with_str("pong");
                }
            }
            Carried::Live(live) => match live.message(t) {
                Some(LiveStep::Send(r)) => {
                    let _ = ws.send_with_str(r);
                }
                Some(LiveStep::Close(c, r)) => close = Some((c, r)),
                None => {}
            },
            Carried::Room(room) => {
                if t.len() > MAX_ROOM_MESSAGE {
                    close = Some((1009, "message too big"));
                } else {
                    for f in room.message(t) {
                        let _ = ws.send_with_str(f);
                    }
                    let joined = room.joined();
                    if joined != att.rooms {
                        att.rooms = joined;
                        let _ = ws.serialize_attachment(&*att);
                    }
                }
            }
        }
        if let Some((c, r)) = close {
            let _ = ws.close(Some(c), Some(r));
            sockets.remove(&att.sock);
        }
    }

    /// Drops what a closed socket carried: a stream detaches and is dropped
    /// after its grace, freeing its seats; a live or room socket leaves its
    /// hub and its seat at once. The alarm is set for what that leaves due.
    async fn forget(&self, ws: &WebSocket) {
        let Ok(inst) = self.instance() else { return };
        if let Ok(Some(att)) = ws.deserialize_attachment::<Attachment>() {
            inst.sockets.borrow_mut().remove(&att.sock);
        }
        self.schedule(&inst).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_owner_token_must_be_long_enough_to_be_unguessable() {
        assert!(checked_token("hunter2").is_err());
        assert!(checked_token(&"a".repeat(MIN_TOKEN_CHARS - 1)).is_err());
        let e = checked_token("short").unwrap_err();
        assert!(e.contains("CLAX_TOKEN") && e.contains("32"), "{e}");
        assert!(checked_token(&"a".repeat(MIN_TOKEN_CHARS)).is_ok());
    }

    #[test]
    fn request_bodies_share_one_budget_and_give_it_back_when_dropped() {
        let budget = BodyBudget::default();
        let mut a = budget.hold();
        assert!(a.grow_to(MAX_BODY));
        let mut b = budget.hold();
        assert!(b.grow_to(MAX_BODY));
        // A third body of any size waits for one of them.
        let mut c = budget.hold();
        assert!(!c.grow_to(1));
        assert_eq!(budget.held.get(), BODY_BUDGET);
        // Holding what it holds already, or less, takes nothing more.
        assert!(a.grow_to(MAX_BODY / 2));
        drop(a);
        assert!(c.grow_to(MAX_BODY));
        drop((b, c));
        assert_eq!(budget.held.get(), 0);
    }

    #[test]
    fn a_body_without_a_credential_is_small() {
        assert_eq!(body_limit(false, false), MAX_ANONYMOUS_BODY);
        assert_eq!(body_limit(true, false), MAX_BODY);
        assert_eq!(body_limit(false, true), MAX_BODY);
        // Within the isolate's 128 MB, beside what the body is made into.
        const { assert!(BODY_BUDGET <= 32 << 20 && MAX_BODY * 2 <= BODY_BUDGET) };
    }

    #[test]
    fn a_stream_attachment_written_before_kinds_reads_as_a_stream() {
        let old = r#"{"sock":"ab","stream":"cd","viewer":"01J00000000000000000000000"}"#;
        let a: Attachment = serde_json::from_str(old).unwrap();
        assert_eq!((a.kind, a.stream.as_str()), (Kind::Stream, "cd"));
        let again = serde_json::to_string(&a).unwrap();
        assert!(!again.contains("kind"), "{again}");
        let room = Attachment {
            kind: Kind::Room,
            sock: "ab".into(),
            stream: String::new(),
            aid: "7q3k9mzx2b4t".into(),
            peer: "0123456789abcdef".into(),
            rooms: vec!["chat".into()],
            creds: SocketCredentials {
                session: Some("f".repeat(64)),
                ..SocketCredentials::default()
            },
        };
        let json = serde_json::to_string(&room).unwrap();
        assert!(json.len() < 2048, "{json}");
        let back: Attachment = serde_json::from_str(&json).unwrap();
        assert_eq!(
            (back.kind, back.rooms),
            (Kind::Room, vec!["chat".to_string()])
        );
    }
}
