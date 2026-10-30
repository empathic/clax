//! Async HTTP client for the daemon's REST API: the local daemon's, with
//! its bearer token, or in remote mode (spec §16, M26) a remote daemon's,
//! every request signed with the grant's key ([`DaemonClient::with_remote`]).

use crate::calls::CallScope;
use crate::git::GitCapture;
use clax_core::RegisterSession;
use clax_core::audit::ToolCallReport;
use clax_core::feedback::MAX_ACK_IDS;
use clax_core::model::Session;
use clax_core::remote::{
    MAX_RESPONSE_BYTES, Remote, SESSION_SECRET_HEADER, Seeker, SessionCredential, SessionKey,
    keys_of_row,
};
#[cfg(not(target_arch = "wasm32"))]
use clax_core::remote::{
    Next, Retry, clock_skew_message, path_and_query, prepare_headers, refusal_message, sign_headers,
};
use clax_core::secret::Secret;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// The agent-channel marker's header (spec §16): an agent channel's every
/// request says `agent`.
pub const CHANNEL_HEADER: &str = "x-clax-channel";

/// Deadline for ordinary requests.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Deadline for publishes and asset uploads.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(120);
/// Deadline for establishing a connection; a live daemon on loopback accepts at once.
#[cfg(not(target_arch = "wasm32"))]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Deadline for a tool-call report, sent in the background.
const REPORT_TIMEOUT: Duration = Duration::from_secs(10);

/// The channel the MCP agent sides name (`x-clax-via`): the stdio shim and
/// the daemon's own `/mcp`. A managed client names it; any other client
/// names what [`DaemonClient::with_via`] gives it, else nothing.
pub const VIA: &str = "mcp";

/// Why a daemon call failed.
#[derive(Debug)]
pub enum ClientError {
    /// No connection: refused, not established within the connect timeout, or
    /// dropped before a response.
    Unreachable(String),
    /// Connected, but the request did not complete within its deadline; a write
    /// may or may not have taken effect.
    Timeout(String),
    /// The daemon answered with a success status but a body that is not the
    /// expected JSON.
    BadResponse(String),
    /// The daemon answered with an error status; `error` is the body's `error`
    /// object (`code`, `message`, and any extra fields such as `current`).
    Api { status: u16, error: Value },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unreachable(m) => write!(f, "daemon unreachable: {m}"),
            ClientError::Timeout(m) => write!(f, "daemon timed out: {m}"),
            ClientError::BadResponse(m) => write!(f, "bad daemon response: {m}"),
            ClientError::Api { status, error } => write!(f, "HTTP {status}: {error}"),
        }
    }
}

impl std::error::Error for ClientError {}

pub type Result<T> = std::result::Result<T, ClientError>;

/// Where a daemon answers: `base` (`http://127.0.0.1:<port>`) for API calls,
/// `browser_base` (`http://localhost:<port>`) for URLs shown to the person, and
/// its bearer token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub base: String,
    pub browser_base: String,
    pub token: String,
}

/// Finds (and, for the auto-start closure, starts) the daemon. It blocks, so it
/// runs on a blocking thread.
pub type Refresh = Arc<dyn Fn() -> anyhow::Result<Endpoint> + Send + Sync>;

/// The answer an [`InProcess`] transport gives: the daemon's whole response.
pub type InProcessFuture = Pin<Box<dyn Future<Output = http::Response<bytes::Bytes>> + Send>>;

/// Carries a request to the daemon's router in the same process, in place
/// of the network ([`DaemonClient::with_in_process`]). The request is what
/// the daemon's HTTP server would hand its router: method, a path-and-query
/// URI, the client's headers with `Host` set from the base URL, and the
/// whole body. The answer is the router's response with its whole body.
/// The transport is the daemon's to implement (it owns the router); a
/// Durable Object, which is its own daemon, reaches its routes this way
/// rather than over `fetch` to itself.
pub trait InProcess: Send + Sync {
    fn call(&self, req: http::Request<bytes::Bytes>) -> InProcessFuture;
}

/// A client of one daemon's REST API.
///
/// A client made with [`DaemonClient::new`] talks to a fixed endpoint. One made
/// with [`DaemonClient::managed`] finds its daemon with a [`Refresh`] and
/// registers a harness session there: lazily on first use, and again whenever a
/// request cannot connect or is refused with 401 (the daemon restarted, on a new
/// port or with a new token, or a remote daemon's `invalid_session_proof`: the
/// session ended under it) or with `unknown_session` (the session was ended
/// under it), after which the request is retried once. A remote daemon's
/// session proves itself with the secret its registration answered (spec
/// 2026-10-08 §6.8), kept in memory and in its credential files. A heartbeat that finds
/// its session ended or gone registers again. Tool requests may start a daemon;
/// heartbeats and ending the session only find one.
///
/// Every request names the client's channel in `x-clax-via` (spec §6.9):
/// `mcp` for a managed client, else what [`DaemonClient::with_via`] set,
/// else none (so a bare token request is the owner's CLI). A client made with
/// [`DaemonClient::under`] acts for one tool call: each of its requests
/// carries the call's `x-clax-call`, and, for a call that captures git
/// state, `x-clax-git` (every request but a `GET` waits for the capture; a
/// `GET` carries it once it is there).
#[derive(Clone)]
pub struct DaemonClient {
    inner: Arc<Inner>,
    call: Option<Arc<CallScope>>,
    via: Option<&'static str>,
    /// The longest one remote clip fetch may take ([`CLIP_FETCH_TIME`]).
    clip_fetch_time: Duration,
    /// The most of a remote daemon's answer read before it is refused
    /// unread ([`MAX_RESPONSE_BYTES`]; a clip's own cap for a clip fetch).
    body_limit: u64,
    /// The deadline of a publish or an asset upload ([`PUBLISH_TIMEOUT`]).
    publish_timeout: Duration,
    auth: Auth,
    /// Set for the CLI's tool commands ([`DaemonClient::waiting_for_confirmations`]):
    /// how a browser confirmation the daemon waits on is shown to the
    /// person, who confirms it while the call waits. Unset, the call
    /// answers [`ClientError::Api`] 202 `confirmation_pending` at once.
    confirm: Option<fn(&str)>,
}

/// The credential a client's requests carry.
#[derive(Clone, Debug)]
enum Auth {
    /// The endpoint's token, as `Bearer` (none for a remote daemon, whose
    /// requests are signed).
    Token,
    /// The `Authorization` of the caller the client acts for
    /// ([`DaemonClient::as_caller`]), or none.
    Caller(Option<http::HeaderValue>),
}

struct Inner {
    http: reqwest::Client,
    state: RwLock<State>,
    managed: Option<Managed>,
    /// Every request carries `X-Clax-Channel: agent` (spec §16): true for
    /// the shim and `/mcp`'s internal client; the CLI's tool commands set it
    /// as the CLI decides ([`DaemonClient::with_channel`]).
    agent: bool,
    /// Remote mode: every request is signed with this grant's key and
    /// retried by [`Retry`]'s rule; no bearer token is sent.
    remote: Option<Arc<Remote>>,
    /// Requests go to the daemon's router in-process, not over the network.
    in_process: Option<Arc<dyn InProcess>>,
}

/// Whether finding the daemon may start one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    AutoStart,
    DiscoverOnly,
}

struct Managed {
    /// Finds or starts the daemon.
    refresh: Refresh,
    /// Finds a running daemon; errors when there is none.
    discover: Refresh,
    registration: RegisterSession,
    /// Captures the registration's git state, when set.
    git: Option<GitCapture>,
    /// Held while refreshing, so concurrent failures refresh once.
    lock: tokio::sync::Mutex<()>,
}

#[derive(Clone, Default)]
struct State {
    endpoint: Option<Endpoint>,
    session_id: Option<String>,
    /// The session's secret (spec 2026-10-08 §6.8): a remote daemon's, or
    /// one a client acting in another's session holds
    /// ([`DaemonClient::in_session`]).
    session_secret: Option<Secret>,
    session: Option<Session>,
    /// True when `session` is registered with the daemon at `endpoint`.
    registered: bool,
}

impl std::fmt::Debug for DaemonClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let st = self.state();
        f.debug_struct("DaemonClient")
            .field("base", &st.endpoint.as_ref().map(|e| &e.base))
            .field("session_id", &st.session_id)
            .field("managed", &self.inner.managed.is_some())
            .finish()
    }
}

/// A snapshot of the endpoint and session one request attempt is sent with.
struct Conn<'a> {
    http: &'a reqwest::Client,
    endpoint: Endpoint,
    session_id: Option<String>,
    /// Sent beside the session it proves. A local daemon's sessions have
    /// none (they act with the daemon token).
    session_secret: Option<Secret>,
    call: Option<Arc<CallScope>>,
    via: Option<&'static str>,
    agent: bool,
    /// A remote daemon's: requests carry the grant, added as they are
    /// signed.
    remote: bool,
    auth: Auth,
}

impl Conn<'_> {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.endpoint.base));
        // The Worker's client has no deadline of its own; a request that
        // sets one later (a publish) replaces this.
        #[cfg(target_arch = "wasm32")]
        {
            req = req.timeout(REQUEST_TIMEOUT);
        }
        match &self.auth {
            _ if self.remote => {}
            Auth::Token => req = req.bearer_auth(&self.endpoint.token),
            Auth::Caller(Some(v)) => req = req.header(http::header::AUTHORIZATION, v.clone()),
            Auth::Caller(None) => {}
        }
        if let Some(v) = self.via {
            req = req.header("x-clax-via", v);
        }
        if let Some(s) = &self.session_id {
            req = req.header("x-clax-session", s);
            if let Some(secret) = &self.session_secret {
                req = req.header(SESSION_SECRET_HEADER, secret.expose());
            }
        }
        if let Some(h) = self.call.as_ref().and_then(|c| c.call_header()) {
            req = req.header("x-clax-call", h);
        }
        if self.agent {
            req = req.header(CHANNEL_HEADER, "agent");
        }
        req
    }

    /// The path of this client's session (`/api/sessions/<id>`).
    fn session_path(&self) -> String {
        format!(
            "/api/sessions/{}",
            self.session_id.as_deref().unwrap_or_default()
        )
    }
}

/// The endpoint and session a failure was seen with; a refresh is skipped when
/// the client has already moved on from them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stale {
    endpoint: Option<Endpoint>,
    session_id: Option<String>,
}

impl Stale {
    fn of(st: &State) -> Stale {
        Stale {
            endpoint: st.endpoint.clone(),
            session_id: st.session_id.clone(),
        }
    }
}

/// A failed attempt, and whether a refresh may cure it: the connection was not
/// established (so the request was never sent), the token was refused, or the
/// session the request named has ended (the daemon refused it before acting).
struct Failure {
    error: ClientError,
    refreshable: bool,
}

/// A daemon's answer: a response from the network, or one an [`InProcess`]
/// transport handed back whole.
enum Reply {
    Net(reqwest::Response),
    Whole {
        status: reqwest::StatusCode,
        headers: http::HeaderMap,
        body: bytes::Bytes,
    },
}

impl Reply {
    fn status(&self) -> reqwest::StatusCode {
        match self {
            Reply::Net(r) => r.status(),
            Reply::Whole { status, .. } => *status,
        }
    }

    fn headers(&self) -> &http::HeaderMap {
        match self {
            Reply::Net(r) => r.headers(),
            Reply::Whole { headers, .. } => headers,
        }
    }

    async fn bytes(self) -> std::result::Result<bytes::Bytes, Failure> {
        match self {
            Reply::Net(r) => r.bytes().await.map_err(transport_error),
            Reply::Whole { body, .. } => Ok(body),
        }
    }

    /// The body, read until it passes `max` bytes (then cut there, one past
    /// `max`, so a caller can tell it was too long).
    async fn bytes_upto(self, max: usize) -> std::result::Result<Vec<u8>, Failure> {
        match self {
            #[cfg(target_arch = "wasm32")]
            Reply::Net(r) => {
                let body = r.bytes().await.map_err(transport_error)?;
                Ok(body[..body.len().min(max + 1)].to_vec())
            }
            #[cfg(not(target_arch = "wasm32"))]
            Reply::Net(mut r) => {
                let mut out = Vec::new();
                while let Some(chunk) = r.chunk().await.map_err(transport_error)? {
                    out.extend_from_slice(&chunk);
                    if out.len() > max {
                        out.truncate(max + 1);
                        break;
                    }
                }
                Ok(out)
            }
            Reply::Whole { body, .. } => Ok(body[..body.len().min(max + 1)].to_vec()),
        }
    }
}

/// The client every request goes out on. It follows no redirect: a
/// signed request and its body never go to a URL a daemon names.
#[cfg(not(target_arch = "wasm32"))]
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client builds with static settings")
}

/// The Worker's client: its `fetch` has no proxy, connect timeout or
/// redirect policy to set (it follows redirects), and the deadline is set
/// on each request ([`Conn::request`]).
#[cfg(target_arch = "wasm32")]
fn http_client() -> reqwest::Client {
    reqwest::Client::new()
}

impl DaemonClient {
    /// A client for the daemon at `base`, an agent channel: every request
    /// carries `X-Clax-Channel: agent`, and `X-Clax-Session` when
    /// `session_id` is set. Proxies are never used.
    pub fn new(base: String, token: String, session_id: Option<String>) -> DaemonClient {
        DaemonClient::with_channel(base, token, session_id, true)
    }

    /// [`DaemonClient::new`], marking its requests as an agent channel's
    /// only when `agent` (the CLI used by hand is the owner).
    pub fn with_channel(
        base: String,
        token: String,
        session_id: Option<String>,
        agent: bool,
    ) -> DaemonClient {
        let endpoint = Endpoint {
            base: base.trim_end_matches('/').to_string(),
            browser_base: String::new(),
            token,
        };
        DaemonClient {
            inner: Arc::new(Inner {
                http: http_client(),
                state: RwLock::new(State {
                    endpoint: Some(endpoint),
                    session_id,
                    session_secret: None,
                    session: None,
                    registered: true,
                }),
                managed: None,
                agent,
                remote: None,
                in_process: None,
            }),
            call: None,
            via: None,
            clip_fetch_time: CLIP_FETCH_TIME,
            body_limit: MAX_RESPONSE_BYTES,
            publish_timeout: PUBLISH_TIMEOUT,
            auth: Auth::Token,
            confirm: None,
        }
    }

    /// This client, talking to the remote daemon `remote` names instead of
    /// its endpoint: every request signed with the grant's key (spec §6.6),
    /// retried once on `stale_request` and with backoff on `limit_reached`
    /// and `busy`. A managed client's finding of the daemon then names
    /// that daemon and never starts one ([`DaemonClient::managed_remote`]).
    /// Call it on a client just made.
    ///
    /// # Panics
    /// When the client has been cloned already.
    pub fn with_remote(mut self, remote: Arc<Remote>) -> DaemonClient {
        let inner = Arc::get_mut(&mut self.inner).expect("with_remote on a client just made");
        let st = inner.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if st.endpoint.is_some() {
            st.endpoint = Some(remote_endpoint(&remote));
        }
        inner.remote = Some(remote);
        self
    }

    /// A managed client (the shim's) of the remote daemon `remote` names:
    /// it registers `registration` there, as a grant's session, and finds
    /// no other daemon.
    pub fn managed_remote(remote: Arc<Remote>, registration: RegisterSession) -> DaemonClient {
        let endpoint = remote_endpoint(&remote);
        let find: Refresh = Arc::new(move || Ok(endpoint.clone()));
        DaemonClient::managed(find.clone(), find, registration).with_remote(remote)
    }

    /// Whether this client talks to a remote daemon.
    pub fn is_remote(&self) -> bool {
        self.inner.remote.is_some()
    }

    /// For a remote daemon, the directory its clips are fetched into
    /// ([`DaemonClient::rewrite_clip_items`]); `None` otherwise.
    pub fn clip_cache(&self) -> Option<PathBuf> {
        self.inner.remote.as_ref().map(|r| r.clip_cache())
    }

    /// This client giving each remote clip fetch at most `t`, instead of
    /// [`CLIP_FETCH_TIME`].
    pub fn with_clip_fetch_time(mut self, t: Duration) -> DaemonClient {
        self.clip_fetch_time = t;
        self
    }

    /// This client giving each publish and asset upload at most `t`,
    /// instead of [`PUBLISH_TIMEOUT`].
    pub fn with_publish_timeout(mut self, t: Duration) -> DaemonClient {
        self.publish_timeout = t;
        self
    }

    /// This client sending its requests through `transport` to the
    /// daemon's router in-process, as it would send them to its base URL
    /// (which still names the `Host` they carry). Every request is the
    /// same, headers and deadlines included; an asset upload's form is
    /// written out whole rather than streamed. Call it on a client just
    /// made.
    ///
    /// # Panics
    /// When the client has been cloned already.
    pub fn with_in_process(mut self, transport: Arc<dyn InProcess>) -> DaemonClient {
        Arc::get_mut(&mut self.inner)
            .expect("with_in_process on a client just made")
            .in_process = Some(transport);
        self
    }

    /// This client naming channel `via` (`mcp`, `cli`, …) on every request.
    pub fn with_via(mut self, via: &'static str) -> DaemonClient {
        self.via = Some(via);
        self
    }

    /// This client waiting out the browser confirmations the daemon holds
    /// a call for, as the CLI's own client does: `tell` shows the person
    /// one line ("Confirm in your browser: <url>"), the client long-polls
    /// `GET /api/confirmations/<id>?wait=` until the owner answers, then
    /// makes the call again with the confirmation's ID. The CLI's tool
    /// commands use it; an agent's tools do not, and get the 202 as
    /// [`ClientError::Api`] `confirmation_pending` to wait on.
    pub fn waiting_for_confirmations(mut self, tell: fn(&str)) -> DaemonClient {
        self.confirm = Some(tell);
        self
    }

    /// A client that finds its daemon with `refresh` (which may start one) or
    /// `discover` (which must not), and registers `registration` there before
    /// its first request; see [`DaemonClient`].
    pub fn managed(
        refresh: Refresh,
        discover: Refresh,
        registration: RegisterSession,
    ) -> DaemonClient {
        DaemonClient {
            inner: Arc::new(Inner {
                http: http_client(),
                state: RwLock::new(State::default()),
                managed: Some(Managed {
                    refresh,
                    discover,
                    registration,
                    git: None,
                    lock: tokio::sync::Mutex::new(()),
                }),
                agent: true,
                remote: None,
                in_process: None,
            }),
            call: None,
            via: Some(VIA),
            clip_fetch_time: CLIP_FETCH_TIME,
            body_limit: MAX_RESPONSE_BYTES,
            publish_timeout: PUBLISH_TIMEOUT,
            auth: Auth::Token,
            confirm: None,
        }
    }

    /// This managed client, capturing the registration's git state with
    /// `git` (spec §9.3). Call it on a client just made.
    ///
    /// # Panics
    /// When the client has been cloned already.
    pub fn with_git(mut self, git: GitCapture) -> DaemonClient {
        let inner = Arc::get_mut(&mut self.inner).expect("with_git on a client just made");
        if let Some(m) = inner.managed.as_mut() {
            m.git = Some(git);
        }
        self
    }

    /// This client, acting for tool call `call`: the endpoint, session and
    /// refreshes are shared; the call's headers go on every request.
    pub fn under(&self, call: Arc<CallScope>) -> DaemonClient {
        DaemonClient {
            inner: self.inner.clone(),
            call: Some(call),
            via: self.via,
            clip_fetch_time: self.clip_fetch_time,
            body_limit: self.body_limit,
            publish_timeout: self.publish_timeout,
            auth: self.auth.clone(),
            confirm: self.confirm,
        }
    }

    /// This client acting for a caller whose credential is
    /// `authorization` (its `Authorization` header, or none): every
    /// request carries it in place of the endpoint's token, so what the
    /// client does is what the caller may do. A daemon's tools reached
    /// in-process ([`DaemonClient::with_in_process`]) act for the MCP
    /// caller this way.
    pub fn as_caller(&self, authorization: Option<http::HeaderValue>) -> DaemonClient {
        DaemonClient {
            auth: Auth::Caller(authorization),
            ..self.clone()
        }
    }

    /// This client acting in `session`, a session registered at its
    /// daemon: every request names it (`X-Clax-Session`), proved by
    /// `secret` when the session has one (`X-Clax-Session-Secret`, spec
    /// 2026-10-08 §6.8: a grant's session), and the session routes are
    /// its. The endpoint, transport and credential are this client's; the
    /// session is the new client's own, so this one is unchanged. For a
    /// client that is neither managed nor remote.
    ///
    /// # Panics
    /// On a managed or remote client, which registers its own session.
    pub fn in_session(&self, session: Session, secret: Option<Secret>) -> DaemonClient {
        assert!(
            self.inner.managed.is_none() && self.inner.remote.is_none(),
            "in_session on a client of its own session"
        );
        let endpoint = self.state().endpoint;
        DaemonClient {
            inner: Arc::new(Inner {
                http: self.inner.http.clone(),
                state: RwLock::new(State {
                    endpoint,
                    session_id: Some(session.id.clone()),
                    session_secret: secret,
                    session: Some(session),
                    registered: true,
                }),
                managed: None,
                agent: self.inner.agent,
                remote: None,
                in_process: self.inner.in_process.clone(),
            }),
            call: self.call.clone(),
            via: self.via,
            clip_fetch_time: self.clip_fetch_time,
            body_limit: self.body_limit,
            publish_timeout: self.publish_timeout,
            auth: self.auth.clone(),
            confirm: self.confirm,
        }
    }

    /// `POST /api/sessions` with `registration`: the session registered,
    /// and its secret when the daemon issued one (a grant's new session,
    /// spec 2026-10-08 §6.8), for a client that registers sessions for
    /// others (a remote MCP endpoint's, one per MCP client), not for
    /// itself.
    pub async fn register_session(
        &self,
        registration: &RegisterSession,
    ) -> Result<(Session, Option<Secret>)> {
        let mut res = self
            .json(|c| {
                c.request(reqwest::Method::POST, "/api/sessions")
                    .json(registration)
            })
            .await?;
        let secret = res["session_secret"]
            .as_str()
            .map(|s| Secret::new(s.to_string()));
        Ok((session_of(res.take())?, secret))
    }

    /// `PATCH /api/sessions/<id>` `{"heartbeat": true}`, naming it
    /// (`X-Clax-Session`) and proving it with `secret` when given
    /// (`X-Clax-Session-Secret`) as a request in that session does: session
    /// `id` marked seen, when this client's credential may act in it. An
    /// ended session is returned as it is, never revived; a grant's ended
    /// session proves nothing (401 `invalid_session_proof`).
    pub async fn heartbeat_session(&self, id: &str, secret: Option<&Secret>) -> Result<Session> {
        let res = self
            .json(|c| {
                let req = c
                    .request(reqwest::Method::PATCH, &format!("/api/sessions/{id}"))
                    .header("x-clax-session", id);
                match secret {
                    Some(s) => req.header(SESSION_SECRET_HEADER, s.expose()),
                    None => req,
                }
                .json(&json!({"heartbeat": true}))
            })
            .await?;
        session_of(res)
    }

    /// The tool call this client acts for.
    pub fn call(&self) -> Option<&Arc<CallScope>> {
        self.call.as_ref()
    }

    /// Notes the artifact the current tool call's arguments named.
    pub fn note_artifact(&self, id: &str) {
        if let Some(c) = &self.call {
            c.note_artifact(id);
        }
    }

    /// The agent's working directory: the session's, once registered and
    /// known, else a managed client's registration's; `None` for neither.
    pub fn agent_cwd(&self) -> Option<String> {
        self.state()
            .session
            .map(|s| s.cwd)
            .filter(|c| !c.is_empty())
            .or_else(|| {
                let m = self.inner.managed.as_ref()?;
                Some(m.registration.cwd.clone()).filter(|c| !c.is_empty())
            })
    }

    /// `POST /api/sessions/<sid>/tool-calls` (`/api/tool-calls` without a
    /// session): reports a finished tool call. Never starts a daemon.
    pub async fn report_tool_call(&self, report: &ToolCallReport) -> Result<Value> {
        let res = self
            .send_with(Mode::DiscoverOnly, |c| {
                let path = match &c.session_id {
                    Some(_) => format!("{}/tool-calls", c.session_path()),
                    None => "/api/tool-calls".to_string(),
                };
                c.request(reqwest::Method::POST, &path)
                    .timeout(REPORT_TIMEOUT)
                    .json(report)
            })
            .await?;
        body_json(res).await
    }

    fn state(&self) -> State {
        self.inner
            .state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn update(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.inner.state.write().unwrap_or_else(|e| e.into_inner()));
    }

    /// Whether every request carries `X-Clax-Channel: agent`.
    pub fn agent_channel(&self) -> bool {
        self.inner.agent
    }

    /// The base URL requests are sent to, once known.
    pub fn base(&self) -> Option<String> {
        self.state().endpoint.map(|e| e.base)
    }

    /// The browser base URL of a managed client's current daemon, once known.
    pub fn browser_base(&self) -> Option<String> {
        self.inner.managed.as_ref()?;
        self.state().endpoint.map(|e| e.browser_base)
    }

    /// The session a managed client last registered or heartbeat, once
    /// registered.
    pub fn session(&self) -> Option<Session> {
        self.state().session
    }

    /// For a managed client: finds (or starts) the daemon and registers the
    /// session unless both are done. Other clients are always ready.
    pub async fn ensure_session(&self) -> Result<()> {
        self.ensure(Mode::AutoStart).await
    }

    async fn ensure(&self, mode: Mode) -> Result<()> {
        if self.inner.managed.is_none() {
            return Ok(());
        }
        let st = self.state();
        if st.endpoint.is_some() && st.registered {
            return Ok(());
        }
        self.refresh(Stale::of(&st), mode).await
    }

    /// Re-discovers the daemon and registers the session there, unless another
    /// caller already moved on from `stale` (the endpoint and session the
    /// failure was seen with).
    async fn refresh(&self, stale: Stale, mode: Mode) -> Result<()> {
        let Some(m) = &self.inner.managed else {
            return Ok(());
        };
        let _guard = m.lock.lock().await;
        let st = self.state();
        if st.endpoint.is_some() && st.registered && Stale::of(&st) != stale {
            return Ok(());
        }
        let refresh = match mode {
            Mode::AutoStart => m.refresh.clone(),
            Mode::DiscoverOnly => m.discover.clone(),
        };
        // The registration's git state, captured while the daemon is found.
        let git = m.git.as_ref().map(|g| {
            let cwd = Some(m.registration.cwd.as_str()).filter(|c| !c.is_empty());
            g.start(cwd)
        });
        let endpoint = clax_rt::spawn_blocking(move || refresh())
            .await
            .map_err(|e| ClientError::Unreachable(format!("daemon discovery failed: {e}")))?
            .map_err(|e| ClientError::Unreachable(format!("{e:#}")))?;
        self.update(|st| {
            st.endpoint = Some(endpoint.clone());
            st.registered = false;
        });
        let git = match git {
            Some(crate::calls::GitStart::Ready(f)) => Some(f),
            Some(crate::calls::GitStart::Running(t)) => Some(
                t.await
                    .unwrap_or(clax_core::gitctx::GitField::Capture("unavailable")),
            ),
            None => None,
        };
        let git = git.as_ref().and_then(clax_core::gitctx::encode_header);
        // A remote daemon's live session is refreshed only with its secret
        // (spec 2026-10-08 §6.8): the one held, else the one in its file,
        // else none, which makes a new session.
        let candidates = self.registration_credentials(&st, m);
        let last = candidates.len() - 1;
        for (i, cred) in candidates.into_iter().enumerate() {
            let conn = Conn {
                http: &self.inner.http,
                endpoint: endpoint.clone(),
                session_id: cred.as_ref().map(|c| c.session.clone()),
                session_secret: cred.as_ref().map(|c| c.secret.clone()),
                call: self.call.clone(),
                via: self.via,
                agent: self.inner.agent,
                remote: self.inner.remote.is_some(),
                auth: self.auth.clone(),
            };
            let mut req = conn
                .request(reqwest::Method::POST, "/api/sessions")
                .json(&m.registration);
            if let Some(h) = &git {
                req = req.header("x-clax-git", h);
            }
            let res = match self.send_once(req).await {
                Ok(res) => res,
                // A credential the daemon refuses, or whose session it has
                // no row for (another or a reset daemon): the next one.
                Err(f) if i < last && credential_refused(&f.error) => continue,
                Err(f) => return Err(f.error),
            };
            let mut answer = body_json(res).await?;
            let issued = answer["session_secret"]
                .as_str()
                .map(|s| Secret::new(s.to_string()));
            let session = session_of(answer.take())?;
            let secret = issued.or(cred.filter(|c| c.session == session.id).map(|c| c.secret));
            if let (Some(r), Some(secret)) = (&self.inner.remote, &secret) {
                let cred = SessionCredential {
                    session: session.id.clone(),
                    secret: secret.clone(),
                };
                let written = r.sessions.write(
                    &session.harness,
                    session.harness_session_id.as_deref(),
                    session.parent_pid,
                    m.registration.harness_session_id.as_deref(),
                    &cred,
                );
                if let Err(e) = written {
                    tracing::warn!("the session's credential file could not be written: {e}");
                }
            }
            self.update(|st| {
                st.session_id = Some(session.id.clone());
                st.session_secret = secret;
                st.session = Some(session);
                st.registered = true;
            });
            return Ok(());
        }
        unreachable!("the last candidate returns")
    }

    /// The credentials a registration presents, in order (spec 2026-10-08
    /// §6.8): for a remote daemon, the session and secret held, then the
    /// one in the session's credential file when it differs, then none;
    /// for the local daemon, none.
    fn registration_credentials(&self, st: &State, m: &Managed) -> Vec<Option<SessionCredential>> {
        let Some(remote) = &self.inner.remote else {
            return vec![None];
        };
        let mut out = Vec::new();
        let held = match (&st.session_id, &st.session_secret) {
            (Some(session), Some(secret)) => Some(SessionCredential {
                session: session.clone(),
                secret: secret.clone(),
            }),
            _ => None,
        };
        if let Some(h) = &held {
            out.push(Some(h.clone()));
        }
        let r = &m.registration;
        let pids: Vec<u32> = r.parent_pid.into_iter().collect();
        let seeker = Seeker {
            harness: &r.harness,
            harness_session_id: r.harness_session_id.as_deref(),
            pids: &pids,
        };
        match remote.sessions.find(&seeker) {
            Ok(Some(f)) if !held.as_ref().is_some_and(|h| h.same_as(&f)) => out.push(Some(f)),
            Ok(_) => {}
            Err(e) => tracing::warn!("{e}"),
        }
        out.push(None);
        out
    }

    fn conn(&self) -> Result<Conn<'_>> {
        let st = self.state();
        let endpoint = st
            .endpoint
            .ok_or_else(|| ClientError::Unreachable("no daemon found".into()))?;
        Ok(Conn {
            http: &self.inner.http,
            endpoint,
            session_id: st.session_id,
            session_secret: st.session_secret,
            call: self.call.clone(),
            via: self.via,
            agent: self.inner.agent,
            remote: self.inner.remote.is_some(),
            auth: self.auth.clone(),
        })
    }

    /// Sends `req` with the current call's `x-clax-git`, unless it has one:
    /// every request but a `GET` waits for the capture, and a `GET` carries
    /// it once it is there, so reads made while it runs are not held up.
    async fn send_once(&self, req: reqwest::RequestBuilder) -> std::result::Result<Reply, Failure> {
        let Some(call) = self.call.as_ref().filter(|c| c.has_git()) else {
            return self.attempt(req).await;
        };
        let (client, built) = req.build_split();
        let mut built = built.map_err(transport_error)?;
        if !built.headers().contains_key("x-clax-git") {
            let value = if built.method() == reqwest::Method::GET {
                call.git_header_now()
            } else {
                call.git_header().await
            };
            if let Some(v) = value.and_then(|v| reqwest::header::HeaderValue::from_str(&v).ok()) {
                built.headers_mut().insert("x-clax-git", v);
            }
        }
        self.attempt(reqwest::RequestBuilder::from_parts(client, built))
            .await
    }

    /// Sends `req`: as it is, for a remote daemon signed
    /// ([`attempt_signed`]), or in-process ([`attempt_in_process`]).
    async fn attempt(&self, req: reqwest::RequestBuilder) -> std::result::Result<Reply, Failure> {
        if let Some(t) = &self.inner.in_process {
            return attempt_in_process(t.as_ref(), req).await;
        }
        match &self.inner.remote {
            Some(r) => attempt_signed(r, req, self.body_limit).await,
            None => attempt(req).await,
        }
    }

    /// Registers the session again (a new row when the old one ended) unless
    /// another caller already did since `stale`.
    async fn reregister(&self, stale: Stale, mode: Mode) -> Result<Session> {
        self.refresh(stale, mode).await?;
        self.session()
            .ok_or_else(|| ClientError::BadResponse("no session after registering".into()))
    }

    /// Sends the request `build` makes. A managed client first ensures its
    /// session, and on a refreshable failure refreshes and retries once; when
    /// the refresh fails, its error is returned (it says why the daemon
    /// cannot be reached, such as an upgrade that was rolled back), naming
    /// the original failure too.
    async fn send<F>(&self, build: F) -> Result<Reply>
    where
        F: Fn(&Conn<'_>) -> reqwest::RequestBuilder,
    {
        self.send_with(Mode::AutoStart, build).await
    }

    async fn send_with<F>(&self, mode: Mode, build: F) -> Result<Reply>
    where
        F: Fn(&Conn<'_>) -> reqwest::RequestBuilder,
    {
        let first = self.send_refreshing(mode, &build).await;
        let (Some(tell), Err(ClientError::Api { status: 202, error })) = (self.confirm, &first)
        else {
            return first;
        };
        // Letters, digits, `_` and `-` alone: it goes in a path.
        let Some(id) = error["confirmation_id"]
            .as_str()
            .filter(|v| {
                !v.is_empty()
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
            .map(str::to_string)
        else {
            return first;
        };
        tell(&clax_core::confirm::line(
            error["confirm_url"].as_str().unwrap_or_default(),
        ));
        self.wait_for_answer(&id).await?;
        self.send_refreshing(mode, &|c: &Conn<'_>| {
            build(c).header(clax_core::confirm::CONFIRMATION_HEADER, id.as_str())
        })
        .await
    }

    /// Waits until confirmation `id` is no longer open, long-polling for at
    /// most [`clax_core::confirm::MAX_WAIT_SECS`] a time; the daemon
    /// expires it after ten minutes, which ends the wait.
    async fn wait_for_answer(&self, id: &str) -> Result<()> {
        let wait = clax_core::confirm::MAX_WAIT_SECS;
        let path = format!("/api/confirmations/{id}?wait={wait}");
        loop {
            let v = body_json(
                self.send_refreshing(Mode::DiscoverOnly, &|c: &Conn<'_>| {
                    c.request(reqwest::Method::GET, &path)
                        .timeout(Duration::from_secs(wait + 15))
                })
                .await?,
            )
            .await?;
            if v["confirmation"]["status"] != "open" {
                return Ok(());
            }
        }
    }

    async fn send_refreshing<F>(&self, mode: Mode, build: &F) -> Result<Reply>
    where
        F: Fn(&Conn<'_>) -> reqwest::RequestBuilder,
    {
        self.ensure(mode).await?;
        let conn = self.conn()?;
        match self.send_once(build(&conn)).await {
            Err(f) if f.refreshable && self.inner.managed.is_some() => {
                let stale = Stale {
                    endpoint: Some(conn.endpoint.clone()),
                    session_id: conn.session_id.clone(),
                };
                if let Err(r) = self.refresh(stale, mode).await {
                    return Err(refresh_failed(f.error, r));
                }
                self.send_once(build(&self.conn()?))
                    .await
                    .map_err(|f| f.error)
            }
            r => r.map_err(|f| f.error),
        }
    }

    async fn json<F>(&self, build: F) -> Result<Value>
    where
        F: Fn(&Conn<'_>) -> reqwest::RequestBuilder,
    {
        body_json(self.send(build).await?).await
    }

    /// `GET /healthz`: `{version, pid, started_at}`.
    pub async fn healthz(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, "/healthz"))
            .await
    }

    /// `GET /api/artifacts`: `{artifacts: [..]}`, pinned first, then most recently updated.
    pub async fn list(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, "/api/artifacts"))
            .await
    }

    /// `POST /api/browser-login {next}`: a one-use login code for the
    /// browser the `open` tool opens, `{path, url, expires_in}`, `path`
    /// being `/login?code=…&next=…` (the local daemon's alone).
    pub async fn browser_login(&self, next: &str) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::POST, "/api/browser-login")
                .json(&serde_json::json!({"next": next}))
        })
        .await
    }

    /// `GET /api/artifacts/<id>`: `{artifact, versions}`.
    pub async fn get(&self, id: &str) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, &format!("/api/artifacts/{id}")))
            .await
    }

    /// `POST /api/artifacts`: creates an artifact; `{artifact, version, url}`.
    pub async fn create(&self, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::POST, "/api/artifacts")
                .timeout(self.publish_timeout)
                .json(body)
        })
        .await
    }

    /// `POST /api/artifacts/<id>/versions`: publishes a new version; `{artifact, version, url}`.
    pub async fn publish_version(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("/api/artifacts/{id}/versions"),
            )
            .timeout(self.publish_timeout)
            .json(body)
        })
        .await
    }

    /// `PATCH /api/artifacts/<id>` with metadata fields; `{artifact}`.
    pub async fn patch(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::PATCH, &format!("/api/artifacts/{id}"))
                .json(body)
        })
        .await
    }

    /// `GET /api/artifacts?tag=…`: the artifacts carrying every tag in
    /// `tags` (each an ID or a name), as [`Client::list`] gives them.
    pub async fn list_tagged(&self, tags: &[String]) -> Result<Value> {
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(tags.iter().map(|t| ("tag", t.as_str())))
            .finish();
        let path = format!("/api/artifacts?{query}");
        self.json(|c| c.request(reqwest::Method::GET, &path)).await
    }

    /// `GET /api/tags`: `{tags: [{id, name, colour, description, count, …}],
    /// colours, may_manage}`.
    pub async fn tags(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, "/api/tags"))
            .await
    }

    /// `GET /api/queues`: `{queues}`, the queues the caller's person is a
    /// member of.
    pub async fn queues(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, "/api/queues"))
            .await
    }

    /// `GET /api/queues/<qid>/reports?state=&cursor=`: `{reports, next}`.
    pub async fn queue_reports(
        &self,
        qid: &str,
        state: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Value> {
        let mut q: Vec<(&str, &str)> = Vec::new();
        if let Some(s) = state {
            q.push(("state", s));
        }
        if let Some(c) = cursor {
            q.push(("cursor", c));
        }
        self.json(|c| {
            c.request(
                reqwest::Method::GET,
                &format!("/api/queues/{}/reports", encode_segment(qid)),
            )
            .query(&q)
        })
        .await
    }

    /// `POST /api/queues/<qid>/reports/<tid>/<action>` (`claim`, `renew`,
    /// `release`, `resolve`, `comments`) with JSON `body`: the route's
    /// answer.
    pub async fn queue_report_action(
        &self,
        qid: &str,
        tid: &str,
        action: &str,
        body: &Value,
    ) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!(
                    "/api/queues/{}/reports/{}/{action}",
                    encode_segment(qid),
                    encode_segment(tid)
                ),
            )
            .json(body)
        })
        .await
    }

    /// `PATCH /api/artifacts/<id>/tags {add, remove}`: changes the tags the
    /// artifact carries; `{artifact_id, tags, changed}`.
    pub async fn edit_tags(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::PATCH, &format!("/api/artifacts/{id}/tags"))
                .json(body)
        })
        .await
    }

    /// `POST /api/artifacts/<id>/fork`: forks a version (`{version?}`) into a
    /// new artifact; `{artifact, version, url}`.
    pub async fn fork(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::POST, &format!("/api/artifacts/{id}/fork"))
                .timeout(self.publish_timeout)
                .json(body)
        })
        .await
    }

    /// `POST /api/artifacts/<id>/push`: publishes it to a public Clax
    /// (`{remote?, mode?, question_id?}`); `{status: "pushed", url, …}`,
    /// or `{status: "approval_pending", question_id, inbox_url}` while the
    /// owner has not answered.
    pub async fn push(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::POST, &format!("/api/artifacts/{id}/push"))
                .timeout(self.publish_timeout)
                .json(body)
        })
        .await
    }

    /// `DELETE /api/artifacts/<id>`.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.send(|c| c.request(reqwest::Method::DELETE, &format!("/api/artifacts/{id}")))
            .await
            .map(|_| ())
    }

    /// The stored bytes of `path` in version `n`, unwrapped
    /// (`GET /api/artifacts/<id>/versions/<n>/files/<path>`).
    pub async fn file_bytes(&self, id: &str, n: u32, path: &str) -> Result<Vec<u8>> {
        let path = format!(
            "/api/artifacts/{id}/versions/{n}/files/{}",
            encode_path(path)
        );
        let res = self
            .send(|c| c.request(reqwest::Method::GET, &path))
            .await?;
        res.bytes().await.map(|b| b.to_vec()).map_err(|f| f.error)
    }

    /// `POST /api/artifacts/<id>/assets` (multipart field `file`): `{asset, url}`.
    pub async fn upload_asset(
        &self,
        id: &str,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<Value> {
        let bytes = bytes::Bytes::from(bytes);
        #[cfg(not(target_arch = "wasm32"))]
        let len = bytes.len() as u64;
        // Validated once up front so that building the part per attempt cannot fail.
        reqwest::multipart::Part::bytes(Vec::new())
            .mime_str(content_type)
            .map_err(|e| ClientError::Api {
                status: 400,
                error: json!({"code": "invalid_content_type", "message": e.to_string()}),
            })?;
        // A remote daemon's request is signed over its body, and an
        // in-process one carries it, so both must be whole bytes: the form
        // is written out rather than streamed.
        let whole = (self.is_remote() || self.inner.in_process.is_some())
            .then(|| multipart_bytes(filename, content_type, &bytes));
        self.json(|c| {
            let req = c
                .request(
                    reqwest::Method::POST,
                    &format!("/api/artifacts/{id}/assets"),
                )
                .timeout(self.publish_timeout);
            match &whole {
                Some((ct, body)) => req
                    .header(reqwest::header::CONTENT_TYPE, ct)
                    .body(body.clone()),
                None => {
                    #[cfg(not(target_arch = "wasm32"))]
                    let part = reqwest::multipart::Part::stream_with_length(bytes.clone(), len);
                    // `fetch` takes the form whole: there is no length to stream with.
                    #[cfg(target_arch = "wasm32")]
                    let part = reqwest::multipart::Part::bytes(bytes.to_vec());
                    let part = part
                        .file_name(filename.to_string())
                        .mime_str(content_type)
                        .expect("content type validated above");
                    req.multipart(reqwest::multipart::Form::new().part("file", part))
                }
            }
        })
        .await
    }

    fn docs_url(id: &str, path: &str) -> String {
        format!("/api/artifacts/{id}/docs/{}", encode_path(path))
    }

    fn with_level(req: reqwest::RequestBuilder, as_level: Option<&str>) -> reqwest::RequestBuilder {
        match as_level {
            Some(l) => req.query(&[("as_level", l)]),
            None => req,
        }
    }

    /// `GET /api/artifacts/<id>/docs/<path>`: `{doc}` (404 when absent or unreadable).
    pub async fn doc_get(&self, id: &str, path: &str, as_level: Option<&str>) -> Result<Value> {
        let url = Self::docs_url(id, path);
        self.json(|c| Self::with_level(c.request(reqwest::Method::GET, &url), as_level))
            .await
    }

    /// `PUT /api/artifacts/<id>/docs/<path>` with `{data, if_version?}`: `{doc, created}`.
    pub async fn doc_put(
        &self,
        id: &str,
        path: &str,
        body: &Value,
        as_level: Option<&str>,
    ) -> Result<Value> {
        let url = Self::docs_url(id, path);
        self.json(|c| Self::with_level(c.request(reqwest::Method::PUT, &url).json(body), as_level))
            .await
    }

    /// `PATCH /api/artifacts/<id>/docs/<path>` with `{data, if_version?}`: `{doc}`.
    pub async fn doc_patch(
        &self,
        id: &str,
        path: &str,
        body: &Value,
        as_level: Option<&str>,
    ) -> Result<Value> {
        let url = Self::docs_url(id, path);
        self.json(|c| {
            Self::with_level(c.request(reqwest::Method::PATCH, &url).json(body), as_level)
        })
        .await
    }

    /// `DELETE /api/artifacts/<id>/docs/<path>?if_version=`: `{deleted}`.
    pub async fn doc_delete(
        &self,
        id: &str,
        path: &str,
        if_version: Option<u64>,
        as_level: Option<&str>,
    ) -> Result<Value> {
        let url = Self::docs_url(id, path);
        self.json(|c| {
            let mut r = c.request(reqwest::Method::DELETE, &url);
            if let Some(v) = if_version {
                r = r.query(&[("if_version", v)]);
            }
            Self::with_level(r, as_level)
        })
        .await
    }

    /// `GET /api/artifacts/<id>/docs?collection=...` with `query` pairs: `{docs, next_cursor}`.
    pub async fn doc_list(&self, id: &str, query: &[(String, String)]) -> Result<Value> {
        let url = format!("/api/artifacts/{id}/docs");
        self.json(|c| c.request(reqwest::Method::GET, &url).query(query))
            .await
    }

    /// `POST /api/artifacts/<id>/docs:batch`: `{results}`.
    pub async fn doc_batch(&self, id: &str, body: &Value, as_level: Option<&str>) -> Result<Value> {
        let url = format!("/api/artifacts/{id}/docs:batch");
        self.json(|c| Self::with_level(c.request(reqwest::Method::POST, &url).json(body), as_level))
            .await
    }

    /// `POST /api/artifacts/<id>/docs:str_replace`: `{doc}`.
    pub async fn doc_str_replace(
        &self,
        id: &str,
        body: &Value,
        as_level: Option<&str>,
    ) -> Result<Value> {
        let url = format!("/api/artifacts/{id}/docs:str_replace");
        self.json(|c| Self::with_level(c.request(reqwest::Method::POST, &url).json(body), as_level))
            .await
    }

    /// `GET /api/artifacts/<id>/threads`: `{threads, next_cursor}`.
    pub async fn threads(
        &self,
        id: &str,
        include_resolved: bool,
        cursor: Option<&str>,
    ) -> Result<Value> {
        let mut q: Vec<(&str, String)> = vec![("include_resolved", include_resolved.to_string())];
        if let Some(c) = cursor {
            q.push(("cursor", c.to_string()));
        }
        self.json(|c| {
            c.request(
                reqwest::Method::GET,
                &format!("/api/artifacts/{id}/threads"),
            )
            .query(&q)
        })
        .await
    }

    /// `GET /api/artifacts/<id>/threads/<tid>`: `{thread}` (with `clip_path`,
    /// since the token is sent).
    pub async fn thread(&self, id: &str, tid: &str) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::GET,
                &format!("/api/artifacts/{id}/threads/{}", encode_segment(tid)),
            )
        })
        .await
    }

    /// An agent reply: `{comment, thread}` (with `addressed: "pending"` when
    /// `addressed` marked a live page's thread), or `{guidance}` on a thread
    /// not sent to the agent. `addressed` is sent only when true, so a reply
    /// without it reads the same to a daemon that predates it.
    pub async fn reply(&self, id: &str, tid: &str, text: &str, addressed: bool) -> Result<Value> {
        let mut body = json!({"body": text, "author_kind": "agent"});
        if addressed {
            body["addressed"] = json!(true);
        }
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!(
                    "/api/artifacts/{id}/threads/{}/comments",
                    encode_segment(tid)
                ),
            )
            .json(&body)
        })
        .await
    }

    /// Resolves as the agent: `{thread}`, or `{guidance}` on a thread not sent
    /// to the agent.
    pub async fn resolve(&self, id: &str, tid: &str) -> Result<Value> {
        let body = json!({"as": "agent"});
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!(
                    "/api/artifacts/{id}/threads/{}/resolve",
                    encode_segment(tid)
                ),
            )
            .json(&body)
        })
        .await
    }

    /// `PUT /api/sessions/<sid>/watches/<id>`: `{watch}`.
    pub async fn watch(&self, id: &str, replies: bool) -> Result<Value> {
        let body = json!({"replies_armed": replies});
        self.json(|c| {
            c.request(
                reqwest::Method::PUT,
                &format!("{}/watches/{id}", c.session_path()),
            )
            .json(&body)
        })
        .await
    }

    /// `DELETE /api/sessions/<sid>/watches/<id>`.
    pub async fn unwatch(&self, id: &str) -> Result<()> {
        self.send(|c| {
            c.request(
                reqwest::Method::DELETE,
                &format!("{}/watches/{id}", c.session_path()),
            )
        })
        .await
        .map(|_| ())
    }

    /// `PUT /api/sessions/<sid>/remote-watches {remote, artifact_id, url,
    /// replies_armed}`: watches artifact `raid` of connected remote `rid`
    /// (at `url` there); `{watch, mirror, artifact_id, url, remote}`.
    pub async fn remote_watch(
        &self,
        rid: &str,
        raid: &str,
        url: &str,
        replies: bool,
    ) -> Result<Value> {
        let body =
            json!({"remote": rid, "artifact_id": raid, "url": url, "replies_armed": replies});
        self.json(|c| {
            c.request(
                reqwest::Method::PUT,
                &format!("{}/remote-watches", c.session_path()),
            )
            .json(&body)
        })
        .await
    }

    /// `DELETE /api/sessions/<sid>/remote-watches {remote, artifact_id,
    /// url}`: stops watching artifact `raid` of remote `rid`.
    pub async fn remote_unwatch(&self, rid: &str, raid: &str, url: &str) -> Result<()> {
        let body = json!({"remote": rid, "artifact_id": raid, "url": url});
        self.send(|c| {
            c.request(
                reqwest::Method::DELETE,
                &format!("{}/remote-watches", c.session_path()),
            )
            .json(&body)
        })
        .await
        .map(|_| ())
    }

    /// `PUT /api/sessions/<sid>/live-watches`: a scope watch on the page
    /// `url`; `{live_watch, page, covered}`.
    pub async fn live_watch(&self, url: &str, replies: bool) -> Result<Value> {
        let body = json!({"url": url, "replies_armed": replies});
        self.json(|c| {
            c.request(
                reqwest::Method::PUT,
                &format!("{}/live-watches", c.session_path()),
            )
            .json(&body)
        })
        .await
    }

    /// `DELETE /api/sessions/<sid>/live-watches?url=`: `{page_url, removed}`.
    pub async fn live_unwatch(&self, url: &str) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::DELETE,
                &format!("{}/live-watches", c.session_path()),
            )
            .query(&[("url", url)])
        })
        .await
    }

    /// `GET /api/live/pages?url=`: `{page, route}`, `page` null when the URL
    /// has no live page.
    pub async fn live_page(&self, url: &str) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::GET, "/api/live/pages")
                .query(&[("url", url)])
        })
        .await
    }

    /// `GET /api/resolve?url=`: what the URL names, `{kind, …}`
    /// ([`crate::target::resolved`] reads it).
    pub async fn resolve_url(&self, url: &str) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::GET, "/api/resolve")
                .query(&[("url", url)])
        })
        .await
    }

    /// The daemon's address of artifact `raid` on connected remote `rid`
    /// (`/api/remotes/<rid>/artifacts/<raid>`), under which its remote
    /// artifact routes are.
    fn remote_artifact_path(rid: &str, raid: &str) -> String {
        format!(
            "/api/remotes/{}/artifacts/{}",
            encode_segment(rid),
            encode_segment(raid)
        )
    }

    /// `GET /api/remotes/<rid>/artifacts/<raid>`: the remote's `{artifact,
    /// versions}`.
    pub async fn remote_get(&self, rid: &str, raid: &str) -> Result<Value> {
        let path = Self::remote_artifact_path(rid, raid);
        self.json(|c| c.request(reqwest::Method::GET, &path)).await
    }

    /// The bytes of `path` in version `n` of remote artifact `raid`
    /// (`GET /api/remotes/<rid>/artifacts/<raid>/versions/<n>/files/<path>`).
    pub async fn remote_file_bytes(
        &self,
        rid: &str,
        raid: &str,
        n: u32,
        path: &str,
    ) -> Result<Vec<u8>> {
        let path = format!(
            "{}/versions/{n}/files/{}",
            Self::remote_artifact_path(rid, raid),
            encode_path(path)
        );
        let res = self
            .send(|c| c.request(reqwest::Method::GET, &path))
            .await?;
        res.bytes().await.map(|b| b.to_vec()).map_err(|f| f.error)
    }

    /// `GET /api/remotes/<rid>/artifacts/<raid>/threads`: the remote's
    /// `{threads, next_cursor}`.
    pub async fn remote_threads(
        &self,
        rid: &str,
        raid: &str,
        include_resolved: bool,
        cursor: Option<&str>,
    ) -> Result<Value> {
        let mut q: Vec<(&str, String)> = vec![("include_resolved", include_resolved.to_string())];
        if let Some(c) = cursor {
            q.push(("cursor", c.to_string()));
        }
        let path = format!("{}/threads", Self::remote_artifact_path(rid, raid));
        self.json(|c| c.request(reqwest::Method::GET, &path).query(&q))
            .await
    }

    /// `GET /api/remotes/<rid>/artifacts/<raid>/threads/<tid>`: the
    /// remote's `{thread}`.
    pub async fn remote_thread(&self, rid: &str, raid: &str, tid: &str) -> Result<Value> {
        let path = format!(
            "{}/threads/{}",
            Self::remote_artifact_path(rid, raid),
            encode_segment(tid)
        );
        self.json(|c| c.request(reqwest::Method::GET, &path)).await
    }

    /// An agent reply on remote thread `tid`, under idempotency key `key`:
    /// the remote's `{comment, thread}`, or `{guidance}` on a thread not
    /// sent to an agent there.
    pub async fn remote_reply(
        &self,
        rid: &str,
        raid: &str,
        tid: &str,
        text: &str,
        key: &str,
    ) -> Result<Value> {
        let body = json!({"body": text, "author_kind": "agent"});
        let path = format!(
            "{}/threads/{}/comments",
            Self::remote_artifact_path(rid, raid),
            encode_segment(tid)
        );
        self.json(|c| {
            c.request(reqwest::Method::POST, &path)
                .header("idempotency-key", key)
                .json(&body)
        })
        .await
    }

    /// Resolves remote thread `tid` as the agent, under idempotency key
    /// `key`: the remote's `{thread}`, or `{guidance}` on a thread not sent
    /// to an agent there.
    pub async fn remote_resolve(
        &self,
        rid: &str,
        raid: &str,
        tid: &str,
        key: &str,
    ) -> Result<Value> {
        let body = json!({"as": "agent"});
        let path = format!(
            "{}/threads/{}/resolve",
            Self::remote_artifact_path(rid, raid),
            encode_segment(tid)
        );
        self.json(|c| {
            c.request(reqwest::Method::POST, &path)
                .header("idempotency-key", key)
                .json(&body)
        })
        .await
    }

    /// Hands remote thread `tid` to the agent `to` names there, or
    /// releases it: the remote's `{thread}`.
    pub async fn remote_handoff(
        &self,
        rid: &str,
        raid: &str,
        tid: &str,
        to: Option<&str>,
    ) -> Result<Value> {
        let body = match to {
            Some(t) => json!({"to": t}),
            None => json!({}),
        };
        let path = format!(
            "{}/threads/{}/handoff",
            Self::remote_artifact_path(rid, raid),
            encode_segment(tid)
        );
        self.json(|c| c.request(reqwest::Method::POST, &path).json(&body))
            .await
    }

    /// `POST /api/remotes/requests {url, reason?}` as this session: asks
    /// the person to connect the Clax instance `url` is on: `{status,
    /// request?, remote?, until?}`.
    pub async fn remote_request(&self, url: &str, reason: Option<&str>) -> Result<Value> {
        let mut body = json!({"url": url});
        if let Some(r) = reason {
            body["reason"] = json!(r);
        }
        self.json(|c| {
            c.request(reqwest::Method::POST, "/api/remotes/requests")
                .json(&body)
        })
        .await
    }

    /// `GET /api/remotes/requests/<id>?wait=<wait_s>`: `{request_id,
    /// status, origin, name, remote?, waited_s, call_again}`; the deadline
    /// is `wait_s` plus 10 s. `id` must be a ULID.
    pub async fn remote_request_wait(&self, id: &str, wait_s: u64) -> Result<Value> {
        let q = [("wait", wait_s.to_string())];
        let deadline = Duration::from_secs(wait_s + 10);
        self.json(|c| {
            c.request(reqwest::Method::GET, &format!("/api/remotes/requests/{id}"))
                .query(&q)
                .timeout(deadline)
        })
        .await
    }

    /// `PUT /api/sessions/<sid>/working/<id>`: `{working, message_truncated}`.
    pub async fn set_working(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::PUT,
                &format!("{}/working/{id}", c.session_path()),
            )
            .json(body)
        })
        .await
    }

    /// `DELETE /api/sessions/<sid>/working/<id>[?thread_ids=…]`: `{cleared, working}`.
    pub async fn clear_working(&self, id: &str, threads: Option<&[String]>) -> Result<Value> {
        let q: Vec<(&str, String)> = threads
            .map(|t| vec![("thread_ids", t.join(","))])
            .unwrap_or_default();
        self.json(|c| {
            c.request(
                reqwest::Method::DELETE,
                &format!("{}/working/{id}", c.session_path()),
            )
            .query(&q)
        })
        .await
    }

    /// `PUT /api/sessions/<sid>/drafts/<id>`: `{draft, note_truncated}`.
    pub async fn put_draft(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::PUT,
                &format!("{}/drafts/{id}", c.session_path()),
            )
            .json(body)
        })
        .await
    }

    /// `POST /api/sessions/<sid>/git-token/<id>` `{scope}`: `{remote, token,
    /// expires_at, clone_command, jj_clone_command, existing_checkout, …}`.
    pub async fn git_token(&self, id: &str, scope: &str) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("{}/git-token/{id}", c.session_path()),
            )
            .json(&serde_json::json!({ "scope": scope }))
        })
        .await
    }

    /// `DELETE /api/sessions/<sid>/drafts/<id>`: `{ended}`.
    pub async fn delete_draft(&self, id: &str) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::DELETE,
                &format!("{}/drafts/{id}", c.session_path()),
            )
        })
        .await
    }

    /// `POST /api/sessions/<sid>/working/renew`: `{renewed}`.
    pub async fn renew_working(&self) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("{}/working/renew", c.session_path()),
            )
            .json(&json!({}))
        })
        .await
    }

    /// `POST /api/sessions/<sid>/working/end`: `{cleared}`.
    pub async fn end_working(&self) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("{}/working/end", c.session_path()),
            )
            .json(&json!({}))
        })
        .await
    }

    /// `GET /api/sessions/<sid>`: `{session, push}`.
    pub async fn session_info(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, &c.session_path()))
            .await
    }

    /// `PATCH /api/sessions/<sid>` `{name}`: the agent names itself (spec
    /// §11); `{session}`.
    pub async fn set_name(&self, name: &str) -> Result<Value> {
        let body = json!({"name": name});
        self.json(|c| {
            c.request(reqwest::Method::PATCH, &c.session_path())
                .json(&body)
        })
        .await
    }

    /// `POST /api/sessions/<sid>/approvals` `{artifact_id?, thread_id?,
    /// author?}` (spec §9.2): `{approval_id, status, scope, request,
    /// requests}`.
    pub async fn approval_ask(&self, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("{}/approvals", c.session_path()),
            )
            .json(body)
        })
        .await
    }

    /// `GET /api/sessions/<sid>/approvals/<rid>?wait=<wait_s>`:
    /// `{approval_id, status, scope, request, waited_s}`; the deadline is
    /// `wait_s` plus 10 s. `rid` must be a ULID.
    pub async fn approval_wait(&self, rid: &str, wait_s: u64) -> Result<Value> {
        let q = [("wait", wait_s.to_string())];
        let deadline = Duration::from_secs(wait_s + 10);
        self.json(|c| {
            c.request(
                reqwest::Method::GET,
                &format!("{}/approvals/{rid}", c.session_path()),
            )
            .query(&q)
            .timeout(deadline)
        })
        .await
    }

    /// `POST /api/sessions/<sid>/threads/<tid>/handoff` `{to?}` (spec §12):
    /// `{thread}`. `tid` must be a ULID.
    pub async fn handoff(&self, tid: &str, to: Option<&str>) -> Result<Value> {
        let body = match to {
            Some(t) => json!({"to": t}),
            None => json!({}),
        };
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("{}/threads/{tid}/handoff", c.session_path()),
            )
            .json(&body)
        })
        .await
    }

    /// Thread `tid`'s clip on artifact `aid`
    /// (`GET /api/artifacts/<aid>/threads/<tid>/clip`), as bytes, checked
    /// to be a PNG of at most [`clax_core::clips::MAX_BYTES`]
    /// ([`clax_core::clips::check`]); a body that is not is
    /// `BadResponse`.
    pub async fn clip_bytes(&self, aid: &str, tid: &str) -> Result<Vec<u8>> {
        let path = format!("/api/artifacts/{aid}/threads/{tid}/clip");
        // A remote daemon's answer is read no further than a clip may go.
        let limited = DaemonClient {
            body_limit: self.body_limit.min(clax_core::clips::MAX_BYTES as u64 + 1),
            ..self.clone()
        };
        let res = limited
            .send(|c| c.request(reqwest::Method::GET, &path))
            .await?;
        let ct = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = res
            .bytes_upto(clax_core::clips::MAX_BYTES)
            .await
            .map_err(|f| f.error)?;
        clax_core::clips::check(ct.as_deref(), &bytes).map_err(ClientError::BadResponse)?;
        Ok(bytes)
    }

    /// For a remote daemon (spec §16): `text` with its `Clip:` lines
    /// rewritten to the clips fetched (signed) into
    /// `<home>/remote-cache/clips/` ([`clax_core::clips::rewrite_text`]).
    /// The local daemon's text as it is.
    pub async fn rewrite_clip_text(&self, text: &str) -> String {
        let Some(fetched) = self
            .fetch_clips(&clax_core::clips::references_in(text))
            .await
        else {
            return text.to_string();
        };
        clax_core::clips::rewrite_text(text, &mut |a, t| lookup(&fetched, a, t))
    }

    /// For a remote daemon: each of `items` (feedback items or thread
    /// views) with its `clip_path` rewritten as
    /// [`DaemonClient::rewrite_clip_text`] rewrites a line
    /// ([`clax_core::clips::rewrite_json`]).
    pub async fn rewrite_clip_items(&self, items: &mut [Value]) {
        let refs: Vec<(String, String)> = items
            .iter()
            .filter_map(|v| v["clip_path"].as_str())
            .filter_map(clax_core::clips::parse_reference)
            .collect();
        let Some(fetched) = self.fetch_clips(&refs).await else {
            return;
        };
        for v in items {
            clax_core::clips::rewrite_json(v, &mut |a, t| lookup(&fetched, a, t));
        }
    }

    /// [`DaemonClient::fetch_clips`] on wasm32 (a hosted Worker), which is
    /// never a remote daemon's client: nothing to fetch.
    #[cfg(target_arch = "wasm32")]
    async fn fetch_clips(
        &self,
        _refs: &[(String, String)],
    ) -> Option<HashMap<(String, String), std::result::Result<PathBuf, String>>> {
        None
    }

    /// Each clip of `refs` fetched into the remote cache, or why not;
    /// `None` for the local daemon.
    #[cfg(not(target_arch = "wasm32"))]
    async fn fetch_clips(
        &self,
        refs: &[(String, String)],
    ) -> Option<HashMap<(String, String), std::result::Result<PathBuf, String>>> {
        let remote = self.inner.remote.as_ref()?;
        let dir = remote.clip_cache();
        // Every distinct clip given at most CLIP_FETCH_TIME from now, its
        // wait for a turn included, with CLIP_FETCHES_AT_ONCE in flight at
        // most: a slow clip never holds the feedback it came with, and the
        // clips held in memory at once stay bounded.
        let turns = Arc::new(tokio::sync::Semaphore::new(CLIP_FETCHES_AT_ONCE));
        let mut tasks = tokio::task::JoinSet::new();
        let mut out = HashMap::new();
        for r in refs {
            if out.contains_key(r) {
                continue;
            }
            out.insert(r.clone(), Err(clax_core::clips::TIMED_OUT.to_string()));
            let (client, dir, r) = (self.clone(), dir.clone(), r.clone());
            let turns = turns.clone();
            tasks.spawn(async move {
                let t = client.clip_fetch_time;
                let fetch = async {
                    let _turn = turns.acquire().await;
                    client.clip_bytes(&r.0, &r.1).await
                };
                let got = match tokio::time::timeout(t, fetch).await {
                    Err(_) => Err(clax_core::clips::TIMED_OUT.to_string()),
                    Ok(Ok(bytes)) => clax_core::clips::store(&dir, &r.0, &r.1, &bytes)
                        .map_err(|e| format!("could not be cached: {e}")),
                    Ok(Err(ClientError::Api { error, .. })) => {
                        Err(error["code"].as_str().unwrap_or("refused").to_string())
                    }
                    Ok(Err(ClientError::BadResponse(why))) => Err(why),
                    Ok(Err(ClientError::Timeout(_))) => {
                        Err(clax_core::clips::TIMED_OUT.to_string())
                    }
                    Ok(Err(e)) => Err(e.to_string()),
                };
                (r, got)
            });
        }
        while let Some(done) = tasks.join_next().await {
            if let Ok((r, got)) = done {
                out.insert(r, got);
            }
        }
        Some(out)
    }

    /// `GET /api/sessions/<sid>/watches`: `{watches}`.
    pub async fn watches(&self) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::GET,
                &format!("{}/watches", c.session_path()),
            )
        })
        .await
    }

    /// `GET /api/sessions/<sid>/feedback?tier=<tier>&wait=<wait_s>[&artifact=<id>]`:
    /// `{feedback, text, waited_s}`; the deadline is `wait_s` plus 10 s.
    pub async fn feedback(&self, tier: &str, wait_s: u64, artifact: Option<&str>) -> Result<Value> {
        let mut q: Vec<(&str, String)> =
            vec![("tier", tier.to_string()), ("wait", wait_s.to_string())];
        if let Some(a) = artifact {
            q.push(("artifact", a.to_string()));
        }
        let deadline = Duration::from_secs(wait_s + 10);
        self.json(|c| {
            c.request(
                reqwest::Method::GET,
                &format!("{}/feedback", c.session_path()),
            )
            .query(&q)
            .timeout(deadline)
        })
        .await
    }

    /// `POST /api/sessions/<sid>/questions` with `source: "ask"`: `{question,
    /// mode, terminal_after_s, surface_open, where_to_ask}`. `thread` is a
    /// thread of `artifact`; `in_terminal` records an in-terminal question,
    /// which Clax shows but does not ask.
    pub async fn ask_create(
        &self,
        questions: &[clax_core::questions::Question],
        artifact: Option<&str>,
        thread: Option<&str>,
        in_terminal: bool,
    ) -> Result<Value> {
        let body = json!({"source": "ask", "questions": questions, "artifact_id": artifact,
            "thread_id": thread, "in_terminal": in_terminal});
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("{}/questions", c.session_path()),
            )
            .json(&body)
        })
        .await
    }

    /// `GET /api/sessions/<sid>/questions/<qid>?wait=<wait_s>`: `{question,
    /// waited_s}`; the deadline is `wait_s` plus 10 s. `qid` must be a ULID.
    pub async fn ask_wait(&self, qid: &str, wait_s: u64) -> Result<Value> {
        let q = [("wait", wait_s.to_string())];
        let deadline = Duration::from_secs(wait_s + 10);
        self.json(|c| {
            c.request(
                reqwest::Method::GET,
                &format!("{}/questions/{qid}", c.session_path()),
            )
            .query(&q)
            .timeout(deadline)
        })
        .await
    }

    /// `POST /api/sessions/<sid>/questions/<qid>/withdraw`: `{question}`; 409
    /// `question_closed` when it is not open. `qid` must be a ULID.
    pub async fn ask_withdraw(&self, qid: &str) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("{}/questions/{qid}/withdraw", c.session_path()),
            )
            .json(&json!({}))
        })
        .await
    }

    /// `GET /api/sessions/<sid>/where-to-ask`: `{where_to_ask, signals}`.
    pub async fn where_to_ask(&self) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::GET,
                &format!("{}/where-to-ask", c.session_path()),
            )
        })
        .await
    }

    /// `GET /api/sessions/<sid>/notices?wait=<wait_s>`: `{notices, lines,
    /// waited_s}`; the deadline is `wait_s` plus 10 s. Like `heartbeat`, it
    /// finds a running daemon but never starts one.
    pub async fn notices(&self, wait_s: u64) -> Result<Value> {
        self.notices_via(wait_s, None).await
    }

    /// [`DaemonClient::notices`] polled by `via` (`channel`: the shim for
    /// its Claude Code channel, which the daemon counts apart from a
    /// follower and which takes no notice while a follower is connected).
    pub async fn notices_via(&self, wait_s: u64, via: Option<&str>) -> Result<Value> {
        let mut q = vec![("wait", wait_s.to_string())];
        if let Some(v) = via {
            q.push(("via", v.to_string()));
        }
        let deadline = Duration::from_secs(wait_s + 10);
        let res = self
            .send_with(Mode::DiscoverOnly, |c| {
                c.request(
                    reqwest::Method::GET,
                    &format!("{}/notices", c.session_path()),
                )
                .query(&q)
                .timeout(deadline)
            })
            .await?;
        body_json(res).await
    }

    /// `POST /api/sessions/<sid>/feedback/ack` `{comment_ids}`: acknowledges
    /// only the rows for those comments, in requests of at most
    /// [`MAX_ACK_IDS`] IDs; `{acknowledged}`, summed over the requests.
    pub async fn ack_comments(&self, comment_ids: &[String]) -> Result<Value> {
        self.ack_by("comment_ids", comment_ids).await
    }

    /// `POST /api/sessions/<sid>/feedback/ack` `{thread_ids}`: acknowledges
    /// every row on those threads, in requests of at most [`MAX_ACK_IDS`]
    /// IDs; `{acknowledged}`, summed over the requests.
    pub async fn ack(&self, thread_ids: &[String]) -> Result<Value> {
        self.ack_by("thread_ids", thread_ids).await
    }

    async fn ack_by(&self, key: &str, ids: &[String]) -> Result<Value> {
        let mut acknowledged = 0;
        for chunk in ids.chunks(MAX_ACK_IDS) {
            let body = json!({ key: chunk });
            let r = self
                .json(|c| {
                    c.request(
                        reqwest::Method::POST,
                        &format!("{}/feedback/ack", c.session_path()),
                    )
                    .json(&body)
                })
                .await?;
            acknowledged += r["acknowledged"].as_u64().unwrap_or(0);
        }
        Ok(json!({"acknowledged": acknowledged}))
    }

    /// `PATCH /api/sessions/<id>` `{"heartbeat": true}` for a managed client's
    /// session, registering it first when needed, and keeps the returned row as
    /// [`DaemonClient::session`] (so a working directory the harness hook filled
    /// in is used from then on). When the session has ended or no longer exists
    /// (404), registers again and returns the new row. Never starts a daemon.
    pub async fn heartbeat(&self) -> Result<Session> {
        self.ensure(Mode::DiscoverOnly).await?;
        let stale = Stale::of(&self.state());
        let res = self
            .send_with(Mode::DiscoverOnly, |c| {
                c.request(reqwest::Method::PATCH, &c.session_path())
                    .json(&json!({"heartbeat": true}))
            })
            .await;
        let session = match res {
            Ok(res) => session_of(body_json(res).await?)?,
            Err(ClientError::Api { status: 404, .. }) if self.inner.managed.is_some() => {
                return self.reregister(stale, Mode::DiscoverOnly).await;
            }
            Err(e) => return Err(e),
        };
        if session.ended_at.is_some() && self.inner.managed.is_some() {
            return self.reregister(stale, Mode::DiscoverOnly).await;
        }
        self.update(|st| {
            if st.session_id.as_deref() == Some(session.id.as_str()) {
                st.session = Some(session.clone());
            }
        });
        // A live session's credential files are in use (a remote daemon's;
        // see `SessionFiles::prune`).
        if let Some(remote) = &self.inner.remote {
            remote
                .sessions
                .touch(&self.credential_keys(&session), &session.id);
        }
        Ok(session)
    }

    /// `PATCH /api/sessions/<id>` `{"ended": true}` for the registered session.
    /// Never starts a daemon. `None` when no session was registered, or when
    /// the daemon says the session is no longer this client's to name (it
    /// ended already: `unknown_session`, or a remote daemon's
    /// `invalid_session_proof`): there is nothing left to end, so no
    /// session is registered just to end it. For a remote daemon, the
    /// session's credential files go with it (spec 2026-10-08 §6.8).
    pub async fn end_session(&self) -> Result<Option<Session>> {
        let Some(held) = self.state().session else {
            return Ok(None);
        };
        let build = |c: &Conn<'_>| {
            c.request(reqwest::Method::PATCH, &c.session_path())
                .json(&json!({"ended": true}))
        };
        self.ensure(Mode::DiscoverOnly).await?;
        let conn = self.conn()?;
        let res = match self.send_once(build(&conn)).await {
            Err(f) if session_gone(&f.error) => {
                self.forget_credentials(&held);
                return Ok(None);
            }
            Err(f) if f.refreshable && self.inner.managed.is_some() => {
                // The daemon moved (a restart on another port): end the
                // session there.
                let stale = Stale {
                    endpoint: Some(conn.endpoint.clone()),
                    session_id: conn.session_id.clone(),
                };
                match self.refresh(stale, Mode::DiscoverOnly).await {
                    Ok(()) => self
                        .send_once(build(&self.conn()?))
                        .await
                        .map_err(|f| f.error),
                    Err(r) => Err(refresh_failed(f.error, r)),
                }
            }
            r => r.map_err(|f| f.error),
        };
        self.forget_credentials(&held);
        session_of(body_json(res?).await?).map(Some)
    }

    /// Removes the credential files of session `s` (a remote daemon's),
    /// under the names its row and its registration give.
    fn forget_credentials(&self, s: &Session) {
        let Some(remote) = &self.inner.remote else {
            return;
        };
        remote.sessions.remove(&self.credential_keys(s), &s.id);
    }

    /// The names session `s`'s credential files go by: its row's and its
    /// registration's.
    fn credential_keys<'a>(&'a self, s: &'a Session) -> Vec<SessionKey<'a>> {
        let mut keys = keys_of_row(&s.harness, s.harness_session_id.as_deref(), s.parent_pid);
        if let Some(m) = &self.inner.managed {
            keys.extend(registration_keys(&m.registration));
        }
        keys
    }
}

/// Whether `e` says the session a request named is no longer the client's
/// to name: it ended (`unknown_session`), or a remote daemon refused its
/// proof (`invalid_session_proof`, its secret cleared at the end).
fn session_gone(e: &ClientError) -> bool {
    matches!(e, ClientError::Api { error, .. }
        if matches!(error["code"].as_str(), Some("unknown_session" | "invalid_session_proof")))
}

/// The longest one clip fetch may take while feedback is handed over
/// (spec §16): a clip not fetched by then reads `Clip: unavailable (timed
/// out)`.
pub const CLIP_FETCH_TIME: Duration = Duration::from_secs(5);

/// How many clips of one handover are fetched from a remote daemon at
/// once; each holds at most [`clax_core::clips::MAX_BYTES`] in memory.
pub const CLIP_FETCHES_AT_ONCE: usize = 4;

/// The fetched clip of `(aid, tid)` in `fetched`, or why there is none.
fn lookup(
    fetched: &HashMap<(String, String), std::result::Result<PathBuf, String>>,
    aid: &str,
    tid: &str,
) -> std::result::Result<PathBuf, String> {
    fetched
        .get(&(aid.to_string(), tid.to_string()))
        .cloned()
        .unwrap_or_else(|| Err("not fetched".into()))
}

/// The names a registration's session is found by (spec 2026-10-08 §6.8):
/// its harness session ID and its parent PID, when it has them.
fn registration_keys(r: &RegisterSession) -> Vec<SessionKey<'_>> {
    keys_of_row(&r.harness, r.harness_session_id.as_deref(), r.parent_pid)
}

/// Whether `e` refuses the credential a registration presented (spec
/// 2026-10-08 §6.8): a remote daemon's 401 `invalid_session_proof` (the
/// secret does not prove the session) or 400 `unknown_session` (the daemon
/// has no such session).
fn credential_refused(e: &ClientError) -> bool {
    matches!(e, ClientError::Api { status: 401, error } if error["code"] == "invalid_session_proof")
        || matches!(e, ClientError::Api { status: 400, error } if error["code"] == "unknown_session")
}

/// The error for a request that failed with `original` and whose refresh
/// then failed with `refresh`: the refresh's reason first, since it is why
/// the retry could not happen.
fn refresh_failed(original: ClientError, refresh: ClientError) -> ClientError {
    match refresh {
        ClientError::Unreachable(why) => {
            ClientError::Unreachable(format!("{why} (the request had failed with: {original})"))
        }
        other => other,
    }
}

/// The endpoint of the remote daemon `remote` names: its URL for API calls
/// and for the person's browser; the grant goes on each request as it is
/// signed.
fn remote_endpoint(remote: &Remote) -> Endpoint {
    Endpoint {
        base: remote.config.url.clone(),
        browser_base: remote.config.url.clone(),
        token: String::new(),
    }
}

/// A `multipart/form-data` body with one part, `file`, holding `bytes` as
/// `filename` of `content_type`: the content type (with its boundary) and
/// the body. The file name is written as browsers write it, with `"`, CR
/// and LF percent-encoded.
fn multipart_bytes(filename: &str, content_type: &str, bytes: &[u8]) -> (String, bytes::Bytes) {
    let boundary = format!("clax-{}", clax_core::sig::new_nonce());
    let name = filename
        .replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A");
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {content_type}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (
        format!("multipart/form-data; boundary={boundary}"),
        bytes::Bytes::from(body),
    )
}

/// Sends `req` to the remote daemon `remote` names, signed with its grant
/// (spec §6.6) by `clax_core::remote`'s shared helpers: each attempt names
/// the daemon's host, carries its agent headers trimmed of surrounding
/// spaces and tabs, and is signed anew. [`Retry`] decides on
/// `stale_request`, `limit_reached` and `busy`; every other answer is read
/// whole, at most [`MAX_RESPONSE_BYTES`], and [`check`]ed as a local one is.
#[cfg(not(target_arch = "wasm32"))]
async fn attempt_signed(
    remote: &Remote,
    req: reqwest::RequestBuilder,
    limit: u64,
) -> std::result::Result<Reply, Failure> {
    let bad = |m: &str| Failure {
        error: ClientError::BadResponse(m.to_string()),
        refreshable: false,
    };
    let (client, built) = req.build_split();
    let mut built = built.map_err(transport_error)?;
    let body = match built.body() {
        None => Vec::new(),
        Some(b) => b
            .as_bytes()
            .ok_or_else(|| bad("a streamed body cannot be signed"))?
            .to_vec(),
    };
    let pq = path_and_query(built.url());
    let method = built.method().as_str().to_string();
    let signer = &remote.signer;
    let cred = remote.grant();
    prepare_headers(signer, built.headers_mut(), cred).map_err(|e| bad(&e))?;
    let mut retry = Retry::default();
    loop {
        let mut attempt = built
            .try_clone()
            .ok_or_else(|| bad("a streamed body cannot be signed"))?;
        sign_headers(signer, &method, &pq, &body, cred, attempt.headers_mut())
            .map_err(|e| bad(&e))?;
        let res = client.execute(attempt).await.map_err(|e| {
            let mut f = transport_error(e);
            if let ClientError::Unreachable(m) = &f.error {
                f.error = ClientError::Unreachable(format!(
                    "the remote Clax at {} (connected with `clax connect`): {m}",
                    signer.base()
                ));
            }
            f
        })?;
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = capped_body(res, limit).await?;
        let whole = {
            let mut b = http::Response::builder().status(status);
            if let Some(h) = b.headers_mut() {
                *h = headers.clone();
            }
            Reply::Net(reqwest::Response::from(
                b.body(bytes).map_err(|e| bad(&e.to_string()))?,
            ))
        };
        if !matches!(status.as_u16(), 401 | 429 | 503) {
            return check(whole).await;
        }
        let date = headers
            .get(reqwest::header::DATE)
            .and_then(|d| d.to_str().ok())
            .map(str::to_string);
        let Err(failure) = check(whole).await else {
            unreachable!("a refusal is a failure");
        };
        let (code, message) = match &failure.error {
            ClientError::Api { error, .. } => (
                error["code"].as_str().unwrap_or_default().to_string(),
                error["message"].as_str().unwrap_or_default().to_string(),
            ),
            _ => (String::new(), String::new()),
        };
        // A session that ended under the client (its secret cleared) is
        // registered again, as `unknown_session` is (spec 2026-10-08 §6.8).
        let given_up = |code: &str, message: String| Failure {
            error: ClientError::Api {
                status: status.as_u16(),
                error: json!({"code": code, "message": message}),
            },
            refreshable: code == "invalid_session_proof",
        };
        match retry.next(
            status.as_u16(),
            &code,
            date.as_deref(),
            signer,
            clax_core::remote::jitter(),
        ) {
            Next::Resign => {}
            Next::After(d) => clax_rt::time::sleep(d).await,
            Next::ClockSkew(o) => return Err(given_up("clock_skew", clock_skew_message(o))),
            Next::GiveUp => {
                return Err(given_up(&code, refusal_message(&code, &message, signer)));
            }
        }
    }
}

/// A Worker's tool set reaches no remote daemon: connecting to one with a
/// grant (`clax connect`) is a native client's. Refused without a request.
#[cfg(target_arch = "wasm32")]
async fn attempt_signed(
    remote: &Remote,
    _req: reqwest::RequestBuilder,
    _limit: u64,
) -> std::result::Result<Reply, Failure> {
    Err(Failure {
        error: ClientError::Unreachable(format!(
            "the remote Clax at {} is reached from a native client only",
            remote.signer.base()
        )),
        refreshable: false,
    })
}

/// The body of `res`, at most `limit` bytes: a body declared or found
/// longer is refused, read no further.
#[cfg(not(target_arch = "wasm32"))]
async fn capped_body(
    mut res: reqwest::Response,
    limit: u64,
) -> std::result::Result<bytes::Bytes, Failure> {
    let too_big = || Failure {
        error: ClientError::BadResponse(format!(
            "the daemon's answer is larger than {limit} bytes"
        )),
        refreshable: false,
    };
    if res.content_length().is_some_and(|n| n > limit) {
        return Err(too_big());
    }
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(transport_error)? {
        if (body.len() + chunk.len()) as u64 > limit {
            return Err(too_big());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(body))
}

/// Sends `req`; a non-success status becomes `ClientError::Api` with the body's
/// `error` object.
async fn attempt(req: reqwest::RequestBuilder) -> std::result::Result<Reply, Failure> {
    check(Reply::Net(req.send().await.map_err(transport_error)?)).await
}

/// Hands `req` to the daemon's router through `transport`, as the daemon's
/// HTTP server would: the URL's path and query as the URI, its authority
/// as `Host`, the body whole, under the request's deadline (else
/// [`REQUEST_TIMEOUT`]). The answer is [`check`]ed as a network one is.
async fn attempt_in_process(
    transport: &dyn InProcess,
    req: reqwest::RequestBuilder,
) -> std::result::Result<Reply, Failure> {
    let bad = |m: String| Failure {
        error: ClientError::BadResponse(m),
        refreshable: false,
    };
    let built = req.build().map_err(transport_error)?;
    let body = match built.body() {
        None => bytes::Bytes::new(),
        Some(b) => bytes::Bytes::copy_from_slice(
            b.as_bytes()
                .ok_or_else(|| bad("a streamed body cannot go in-process".into()))?,
        ),
    };
    let url = built.url();
    let path_and_query = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    };
    let mut request = http::Request::builder()
        .method(built.method().clone())
        .uri(path_and_query);
    if let Some(h) = request.headers_mut() {
        *h = built.headers().clone();
        let host = match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            (None, _) => String::new(),
        };
        if let Ok(v) = http::HeaderValue::from_str(&host) {
            h.insert(http::header::HOST, v);
        }
    }
    let request = request.body(body).map_err(|e| bad(e.to_string()))?;
    let deadline = built.timeout().copied().unwrap_or(REQUEST_TIMEOUT);
    let res = clax_rt::time::timeout(deadline, transport.call(request))
        .await
        .map_err(|_| Failure {
            error: ClientError::Timeout(format!("the daemon did not answer within {deadline:?}")),
            refreshable: false,
        })?;
    let (parts, body) = res.into_parts();
    check(Reply::Whole {
        status: parts.status,
        headers: parts.headers,
        body,
    })
    .await
}

/// `res` when its status is a success; else `ClientError::Api` with the
/// body's `error` object.
async fn check(res: Reply) -> std::result::Result<Reply, Failure> {
    let status = res.status();
    let held = status == reqwest::StatusCode::ACCEPTED
        && res
            .headers()
            .contains_key(clax_core::confirm::CONFIRMATION_HEADER);
    if held {
        // A decision held for the owner's browser (`clax_core::confirm`):
        // the 202's fields, with its one line as the message, for the
        // agent to wait on.
        let mut error = match res.bytes().await {
            Ok(b) => serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
        if let Some(o) = error.as_object_mut() {
            o.remove("status");
            let line = clax_core::confirm::line(
                o.get("confirm_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            o.insert("code".into(), json!(clax_core::confirm::PENDING));
            o.insert("message".into(), json!(line));
        }
        return Err(Failure {
            refreshable: false,
            error: ClientError::Api {
                status: status.as_u16(),
                error,
            },
        });
    }
    if status.is_success() {
        return Ok(res);
    }
    let body: Value = match res.bytes().await {
        Ok(b) => serde_json::from_slice(&b).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    let error = match body.get("error") {
        Some(e) if e.is_object() => e.clone(),
        _ => json!({
            "code": "http_error",
            "message": format!("daemon answered HTTP {}", status.as_u16()),
        }),
    };
    let unknown_session =
        status == reqwest::StatusCode::BAD_REQUEST && error["code"] == "unknown_session";
    Err(Failure {
        refreshable: status == reqwest::StatusCode::UNAUTHORIZED || unknown_session,
        error: ClientError::Api {
            status: status.as_u16(),
            error,
        },
    })
}

async fn body_json(res: Reply) -> Result<Value> {
    if res.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(json!({}));
    }
    let bytes = res.bytes().await.map_err(|f| f.error)?;
    serde_json::from_slice(&bytes).map_err(|e| ClientError::BadResponse(e.to_string()))
}

/// The `session` of a sessions route's response body.
fn session_of(mut res: Value) -> Result<Session> {
    serde_json::from_value(res["session"].take())
        .map_err(|e| ClientError::BadResponse(format!("session: {e}")))
}

/// Classifies a reqwest failure: a failed connection is `Unreachable` (even when
/// it was the connect timeout) and refreshable; any other timeout is `Timeout`;
/// anything else (such as a connection dropped mid-request) is `Unreachable`
/// but not retried, since the request may have taken effect.
fn transport_error(e: reqwest::Error) -> Failure {
    // `fetch` does not say whether the connection failed.
    #[cfg(not(target_arch = "wasm32"))]
    let connect = e.is_connect();
    #[cfg(target_arch = "wasm32")]
    let connect = false;
    if connect {
        Failure {
            error: ClientError::Unreachable(e.to_string()),
            refreshable: true,
        }
    } else if e.is_timeout() {
        Failure {
            error: ClientError::Timeout(e.to_string()),
            refreshable: false,
        }
    } else {
        Failure {
            error: ClientError::Unreachable(e.to_string()),
            refreshable: false,
        }
    }
}

/// Percent-encodes every byte of `segment` outside the RFC 3986 unreserved
/// set, and `/` and `.`: an ID from a tool's arguments names one path
/// segment, never another route (`..`, `../..`).
fn encode_segment(segment: &str) -> String {
    encode_path(segment).replace('/', "%2F").replace('.', "%2E")
}

/// Percent-encodes every byte of `path` outside the RFC 3986 unreserved set,
/// keeping `/` as the segment separator; a `.` or `..` segment is encoded
/// whole, so the path stays under the route it is appended to (the daemon
/// refuses such a segment) rather than climbing out of it as the URL is
/// resolved.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for (i, seg) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        if seg == "." || seg == ".." {
            out.push_str(&"%2E".repeat(seg.len()));
            continue;
        }
        for b in seg.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// The shim's and `/mcp`'s clients are agent channels; the CLI's tool
    /// commands mark theirs as the CLI decides.
    #[test]
    fn requests_carry_the_agent_channel_marker_unless_by_hand() {
        let marker = |c: &super::DaemonClient| {
            let conn = c.conn().unwrap();
            let r = conn
                .request(reqwest::Method::GET, "/api/artifacts")
                .build()
                .unwrap();
            r.headers()
                .get(super::CHANNEL_HEADER)
                .map(|v| v.to_str().unwrap().to_string())
        };
        let agent = super::DaemonClient::new("http://127.0.0.1:1".into(), "t".into(), None);
        assert_eq!(marker(&agent).as_deref(), Some("agent"));
        let by_hand =
            super::DaemonClient::with_channel("http://127.0.0.1:1".into(), "t".into(), None, false);
        assert_eq!(marker(&by_hand), None);
    }

    /// A client acting for a caller sends the caller's `Authorization`, or
    /// none, never its own token, and keeps doing so under a tool call.
    #[test]
    fn a_client_acting_for_a_caller_sends_only_the_callers_credential() {
        let auth = |c: &super::DaemonClient| {
            let conn = c.conn().unwrap();
            let r = conn
                .request(reqwest::Method::GET, "/api/artifacts")
                .build()
                .unwrap();
            let all: Vec<String> = r
                .headers()
                .get_all(http::header::AUTHORIZATION)
                .iter()
                .map(|v| v.to_str().unwrap().to_string())
                .collect();
            all
        };
        let own = super::DaemonClient::new("http://127.0.0.1:1".into(), "own".into(), None);
        assert_eq!(auth(&own), ["Bearer own"]);
        let caller = own.as_caller(Some(http::HeaderValue::from_static("Bearer cxa_theirs")));
        assert_eq!(auth(&caller), ["Bearer cxa_theirs"]);
        let call = std::sync::Arc::new(crate::calls::CallScope::begin("list", None, None));
        assert_eq!(auth(&caller.under(call)), ["Bearer cxa_theirs"]);
        assert!(auth(&own.as_caller(None)).is_empty());
        assert_eq!(auth(&own), ["Bearer own"]);
    }

    #[test]
    fn encode_path_escapes_reserved_bytes_but_not_slashes() {
        assert_eq!(super::encode_path("a/b c#?.js"), "a/b%20c%23%3F.js");
        assert_eq!(super::encode_path("é"), "%C3%A9");
        assert_eq!(
            super::encode_path("../../../a/./b..c/.."),
            "%2E%2E/%2E%2E/%2E%2E/a/%2E/b..c/%2E%2E"
        );
    }

    #[test]
    fn encode_segment_keeps_an_id_to_one_segment() {
        assert_eq!(super::encode_segment("01JABC"), "01JABC");
        assert_eq!(super::encode_segment(".."), "%2E%2E");
        assert_eq!(
            super::encode_segment("../../mcp/json?x="),
            "%2E%2E%2F%2E%2E%2Fmcp%2Fjson%3Fx%3D"
        );
    }
}
