//! [`InstanceDo`]: one Clax instance, the whole Clax server, in a Durable
//! Object.
//!
//! The object opens the instance's database in its own storage through the
//! `clax-do-vfs` VFS (`Store::open_on`, one connection, every store job
//! inline), keeps artifact content where [`crate::content`] says, builds the
//! server's state and router once (`AppState::assemble`), and answers every
//! request by calling the router. No request reaches it over a connection
//! of the owner's machine: `clax_server::auth::conn_of` is `None` on wasm32,
//! so it serves with the daemon's rules for a request from another machine
//! (no `GET /api/token`, no owner cookie, no live pages, invites refused as
//! on a loopback bind). The owner is whoever presents the bearer token
//! (`CLAX_TOKEN`, else one the object made and keeps).
//!
//! # Streams
//!
//! `GET /api/stream` upgrades to a WebSocket the object accepts with the
//! hibernation API: the router opens the stream
//! (`clax_server::routes::stream::open_socket`), and the object holds its
//! `Conn` beside the socket. Subscriptions stay on `POST
//! /api/stream/<id>`, through the router and its admission rules. Frames
//! are taken with `Conn::drain`, one SSE block a text message: after every
//! request, socket message and alarm, and from a task the hub wakes
//! (`Hub::on_queue`) whenever it queues frames, so events dispatched by
//! spawned listeners go out too; only sockets whose `Conn::pending` says so
//! are drained. The runtime answers the shell's `ping` with `pong` itself,
//! so an idle socket never wakes the object.
//!
//! Each socket's attachment keeps its key, the credentials its stream was
//! opened with (`SocketCredentials`: the viewer cookie's ULID and, when the
//! token was presented, a tag of it, never the token) and the stream's ID.
//! A tag of a token that is no longer the instance's reopens no owner's
//! stream. An object
//! rebuilt after hibernation or eviction has an empty hub: at its first
//! event it opens each socket a new stream as the same caller, resolved
//! against the store again, and sends its `ready`, which names a new
//! stream; the shell then subscribes its topics again over `POST`, with
//! every admission rule, and refetches them, as after a daemon restart.
//!
//! # Alarm
//!
//! The daemon's loops run from the object's alarm instead, set for when
//! the earliest of them next has something to do: `AppState::sweep` by
//! `AppState::next_sweep_in` (a working record or presence report lapsing,
//! a gone viewer dropping, a draft going idle, a detached stream's grace
//! ending, a live seat going idle), the grant expiry by `Store::grants_next_due`, and the
//! store's planner refresh an hour after the last one once a request that
//! may write was served. After every request, socket event and alarm the
//! object works that time out again and moves the alarm earlier when it
//! must; with nothing due it holds no alarm, so an idle object, open
//! sockets or not, sleeps until a request or a message wakes it. An alarm
//! is never set sooner than `SWEEP_INTERVAL` ahead, the daemon's own sweep
//! period.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use clax_core::{Home, Store};
use clax_server::AppState;
use clax_server::identity::SocketCredentials;
use clax_server::routes::stream::{SocketStream, open_stream};
use clax_server::stream::Conn;
use serde::{Deserialize, Serialize};
use tower_service::Service;
use wasm_bindgen::JsValue;
use worker::{
    DurableObject, Env, HttpRequest, Request, Response, Result, State, WebSocket,
    WebSocketIncomingMessage, WebSocketPair, durable_object,
};

use crate::store::{DoPageStore, run};

/// The database file's name in the object's storage.
const DB_FILE: &str = "clax.db";
/// The soonest an alarm is set for, from now: the daemon's sweep period.
const MIN_ALARM_MS: f64 = clax_server::working::SWEEP_INTERVAL.as_millis() as f64;
/// How long after the last planner refresh the next is due, once a request
/// that may write was served.
const OPTIMIZE_MS: f64 = clax_core::store::OPTIMIZE_INTERVAL.as_millis() as f64;

/// What a socket's attachment carries across hibernation (well under the
/// platform's 2,048 bytes: two hex IDs, a ULID and a 16-digit tag).
#[derive(Serialize, Deserialize)]
struct Attachment {
    sock: String,
    stream: String,
    #[serde(flatten)]
    creds: SocketCredentials,
}

/// A socket and the stream connection it carries.
struct Socket {
    ws: WebSocket,
    conn: Conn,
}

/// The server: built once per object, rebuilt only by the test route.
struct Instance {
    app: AppState,
    router: axum::Router,
    sockets: RefCell<HashMap<String, Socket>>,
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
    inner: RefCell<std::result::Result<Rc<Instance>, String>>,
    /// Whether the sockets the object found at its start have streams.
    revived: Cell<bool>,
    /// The alarm's time (ms) as this object last read or set it: `None`
    /// before it has been read, `Some(None)` with none set.
    alarm: Cell<Option<Option<f64>>>,
    /// Whether a request that may write was served since the last planner
    /// refresh.
    wrote: Cell<bool>,
    last_optimize: Cell<f64>,
}

fn var(env: &Env, name: &str) -> String {
    env.var(name).map(|v| v.to_string()).unwrap_or_default()
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

/// The owner's token: `CLAX_TOKEN` when set, else the one the object keeps
/// in its storage, made on first use.
fn owner_token(state: &State, env: &Env) -> Result<String> {
    let set = var(env, "CLAX_TOKEN");
    if !set.is_empty() {
        return Ok(set);
    }
    let sql = state.storage().sql();
    run(
        &sql,
        "CREATE TABLE IF NOT EXISTS _clax_instance (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        vec![],
    )?;
    let row = sql
        .exec(
            "SELECT value FROM _clax_instance WHERE key = 'token'",
            vec![],
        )?
        .raw()
        .next()
        .transpose()?;
    if let Some([worker::SqlStorageValue::String(t)]) = row.as_deref() {
        return Ok(t.clone());
    }
    let token = random_hex(32)?;
    run(
        &sql,
        "INSERT INTO _clax_instance (key, value) VALUES ('token', ?)",
        vec![JsValue::from(token.as_str())],
    )?;
    Ok(token)
}

impl Instance {
    fn build(state: &State, env: &Env) -> Result<Rc<Instance>> {
        let pages = Rc::new(DoPageStore::new(state.storage())?);
        let (conn, db) = clax_do_vfs::open(DB_FILE, pages)
            .map_err(rust_err)?
            .into_parts();
        let content = crate::content::content_store(state, &var(env, "CLAX_CONTENT"))?;
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
            question_sleeper: Arc::new(clax_server::questions::TokioSleeper),
            agent_timer: Arc::new(clax_server::agent::TokioTimer::default()),
            extension_id: String::new(),
            terminal_after_s: clax_core::config::TERMINAL_AFTER_S,
            // Loopback: the most restrictive bind (invites need a LAN one).
            bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 0,
            // Not a hosted daemon (yet): the owner's token and viewer
            // cookies are its only callers.
            hosted: None,
        })
        .map_err(rust_err)?;
        app.start_listeners();
        let router = clax_server::build_router(app.clone());
        let inst = Rc::new(Instance {
            app,
            router,
            sockets: RefCell::default(),
            _shutdown: shutdown_tx,
            _db: db,
        });
        // The hub wakes this task whenever it queues frames, from whatever
        // task dispatched them.
        let wake = Arc::new(tokio::sync::Notify::new());
        let tell = wake.clone();
        inst.app
            .stream
            .on_queue(Arc::new(move || tell.notify_one()));
        let weak: Weak<Instance> = Rc::downgrade(&inst);
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                wake.notified().await;
                let Some(inst) = weak.upgrade() else { return };
                inst.drain_all();
            }
        });
        Ok(inst)
    }

    /// Sends every socket with frames pending what its stream has.
    fn drain_all(&self) {
        let keys: Vec<String> = self
            .sockets
            .borrow()
            .iter()
            .filter(|(_, s)| s.conn.pending())
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            self.drain(&k);
        }
    }

    /// Sends socket `key` its stream's frames; closes it once the stream
    /// has ended.
    fn drain(&self, key: &str) {
        let mut sockets = self.sockets.borrow_mut();
        let Some(s) = sockets.get_mut(key) else {
            return;
        };
        match s.conn.drain() {
            Some(frames) => {
                for f in frames {
                    if s.ws.send_with_str(String::from_utf8_lossy(&f)).is_err() {
                        break;
                    }
                }
            }
            None => {
                let _ = s.ws.close(Some(1000), Some("the stream ended"));
                sockets.remove(key);
            }
        }
    }
}

impl InstanceDo {
    fn instance(&self) -> std::result::Result<Rc<Instance>, String> {
        self.inner.borrow().clone()
    }

    /// Opens a new stream for each socket the object holds but has none
    /// for (sockets accepted before it hibernated or was evicted), and
    /// sends its `ready`. A socket whose caller no longer resolves is
    /// closed; the shell reconnects.
    async fn revive(&self, inst: &Rc<Instance>) {
        if self.revived.replace(true) {
            return;
        }
        for ws in self.state.get_websockets() {
            let Ok(Some(mut att)) = ws.deserialize_attachment::<Attachment>() else {
                let _ = ws.close(Some(1011), Some("no attachment"));
                continue;
            };
            if inst.sockets.borrow().contains_key(&att.sock) {
                continue;
            }
            let who = att.creds.identity(&inst.app.token);
            match open_stream(&inst.app, who, false, None, None, false).await {
                Ok(conn) => {
                    att.stream = conn.stream_id().to_string();
                    let _ = ws.serialize_attachment(&att);
                    let key = att.sock.clone();
                    inst.sockets
                        .borrow_mut()
                        .insert(key.clone(), Socket { ws, conn });
                    inst.drain(&key);
                }
                Err(_) => {
                    let _ = ws.close(Some(1008), Some("the stream could not be reopened"));
                }
            }
        }
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

    /// Sets the alarm for the next due work, no sooner than
    /// [`MIN_ALARM_MS`] from now, unless one already set fires first;
    /// deletes it when nothing is due.
    async fn schedule(&self, inst: &Instance) {
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

    /// Accepts the socket for a stream the router opened. A socket whose
    /// attachment cannot be kept is closed at once, never left accepted
    /// without one.
    fn accept(
        &self,
        inst: &Rc<Instance>,
        conn: Conn,
        creds: SocketCredentials,
    ) -> Result<Response> {
        let key = random_hex(16)?;
        let pair = WebSocketPair::new()?;
        self.state.accept_web_socket(&pair.server);
        let kept = pair.server.serialize_attachment(&Attachment {
            sock: key.clone(),
            stream: conn.stream_id().to_string(),
            creds,
        });
        if let Err(e) = kept {
            let _ = pair.server.close(Some(1011), Some("no attachment"));
            return Err(e);
        }
        inst.sockets.borrow_mut().insert(
            key.clone(),
            Socket {
                ws: pair.server,
                conn,
            },
        );
        inst.drain(&key);
        Response::from_websocket(pair.client)
    }

    /// The test routes, with `CLAX_TEST_ROUTES` = `1` and the token (else
    /// `None`: the router answers the path, a 404):
    ///
    /// - `POST /_worker/hibernate` drops the server and builds it again on
    ///   the same storage, as a wake from hibernation does, keeping the
    ///   sockets;
    /// - `GET /_worker/alarm`: `{alarm, now}`, the time the object's alarm
    ///   is set for (ms, `null` with none) and the object's time.
    async fn test_route(&self, inst: &Rc<Instance>, req: &HttpRequest) -> Option<Result<Response>> {
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
        let fresh = match Instance::build(&self.state, &self.env) {
            Ok(i) => i,
            Err(e) => return Some(Response::error(e.to_string(), 500)),
        };
        let old = self.inner.replace(Ok(fresh));
        // The old streams' connections go with the old server, sockets kept.
        if let Ok(old) = old {
            old.sockets.borrow_mut().clear();
        }
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
        let inner = Instance::build(&state, &env).map_err(|e| e.to_string());
        InstanceDo {
            state,
            env,
            inner: RefCell::new(inner),
            revived: Cell::new(false),
            alarm: Cell::new(None),
            wrote: Cell::new(false),
            last_optimize: Cell::new(now_ms()),
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let inst = match self.instance() {
            Ok(i) => i,
            Err(e) => return Response::error(format!("the instance could not start: {e}"), 500),
        };
        let req = HttpRequest::try_from(req)?;
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
        let opened = res
            .extensions_mut()
            .remove::<SocketStream>()
            .and_then(|s| s.0.lock().unwrap_or_else(|e| e.into_inner()).take());
        let out = match opened {
            Some((conn, creds)) => self.accept(&inst, conn, creds),
            None => {
                inst.drain_all();
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
        if let Ok(inst) = self.instance() {
            self.revive(&inst).await;
            if matches!(&message, WebSocketIncomingMessage::String(t) if t == "ping") {
                ws.send_with_str("pong")?;
            }
            inst.drain_all();
            self.schedule(&inst).await;
        }
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
        }
        if self.wrote.get() && now - self.last_optimize.get() >= OPTIMIZE_MS {
            self.last_optimize.set(now);
            self.wrote.set(false);
            let _ = inst.app.store.call(|st| st.optimize()).await;
        }
        inst.drain_all();
        self.schedule(&inst).await;
        Response::ok("")
    }
}

impl InstanceDo {
    /// Drops the stream connection of a closed socket: its stream detaches
    /// and is dropped after its grace, freeing its seats; the alarm is set
    /// for then.
    async fn forget(&self, ws: &WebSocket) {
        let Ok(inst) = self.instance() else { return };
        if let Ok(Some(att)) = ws.deserialize_attachment::<Attachment>() {
            inst.sockets.borrow_mut().remove(&att.sock);
        }
        self.schedule(&inst).await;
    }
}
