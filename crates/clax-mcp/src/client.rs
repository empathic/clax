//! Async HTTP client for the daemon's REST API: the local daemon's, with
//! its bearer token, or in remote mode (spec §16, M26) a remote daemon's,
//! every request signed with the grant's key ([`DaemonClient::with_remote`]).

use crate::calls::CallScope;
use crate::git::GitCapture;
use clax_core::RegisterSession;
use clax_core::audit::ToolCallReport;
use clax_core::model::Session;
#[cfg(not(target_arch = "wasm32"))]
use clax_core::remote::{
    MAX_RESPONSE_BYTES, Next, Retry, clock_skew_message, path_and_query, prepare_headers,
    refusal_message, sign_headers,
};
use clax_core::remote::{
    Remote, SESSION_SECRET_HEADER, Seeker, SessionCredential, SessionKey, keys_of_row,
};
use clax_core::secret::Secret;
use serde_json::{Value, json};
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
    /// A remote daemon's session's secret (spec 2026-10-08 §6.8).
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
    /// Sent beside the session it proves, to a remote daemon only.
    session_secret: Option<Secret>,
    call: Option<Arc<CallScope>>,
    via: Option<&'static str>,
    agent: bool,
    /// A remote daemon's: requests carry the grant, added as they are
    /// signed.
    remote: bool,
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
        if !self.remote {
            req = req.bearer_auth(&self.endpoint.token);
        }
        if let Some(v) = self.via {
            req = req.header("x-clax-via", v);
        }
        if let Some(s) = &self.session_id {
            req = req.header("x-clax-session", s);
            if let Some(secret) = self.session_secret.as_ref().filter(|_| self.remote) {
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
            }),
            call: None,
            via: None,
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

    /// This client naming channel `via` (`mcp`, `cli`, …) on every request.
    pub fn with_via(mut self, via: &'static str) -> DaemonClient {
        self.via = Some(via);
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
            }),
            call: None,
            via: Some(VIA),
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
        }
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
        })
    }

    /// Sends `req` with the current call's `x-clax-git`, unless it has one:
    /// every request but a `GET` waits for the capture, and a `GET` carries
    /// it once it is there, so reads made while it runs are not held up.
    async fn send_once(
        &self,
        req: reqwest::RequestBuilder,
    ) -> std::result::Result<reqwest::Response, Failure> {
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

    /// Sends `req`: as it is, or for a remote daemon signed
    /// ([`DaemonClient::attempt_signed`]).
    async fn attempt(
        &self,
        req: reqwest::RequestBuilder,
    ) -> std::result::Result<reqwest::Response, Failure> {
        match &self.inner.remote {
            Some(r) => attempt_signed(r, req).await,
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
    async fn send<F>(&self, build: F) -> Result<reqwest::Response>
    where
        F: Fn(&Conn<'_>) -> reqwest::RequestBuilder,
    {
        self.send_with(Mode::AutoStart, build).await
    }

    async fn send_with<F>(&self, mode: Mode, build: F) -> Result<reqwest::Response>
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

    /// `GET /api/artifacts/<id>`: `{artifact, versions}`.
    pub async fn get(&self, id: &str) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, &format!("/api/artifacts/{id}")))
            .await
    }

    /// `POST /api/artifacts`: creates an artifact; `{artifact, version, url}`.
    pub async fn create(&self, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::POST, "/api/artifacts")
                .timeout(PUBLISH_TIMEOUT)
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
            .timeout(PUBLISH_TIMEOUT)
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
        res.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| transport_error(e).error)
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
        // A remote daemon's request is signed over its body, which must
        // be whole bytes: the form is written out rather than streamed.
        let whole = self
            .is_remote()
            .then(|| multipart_bytes(filename, content_type, &bytes));
        self.json(|c| {
            let req = c
                .request(
                    reqwest::Method::POST,
                    &format!("/api/artifacts/{id}/assets"),
                )
                .timeout(PUBLISH_TIMEOUT);
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
                &format!("/api/artifacts/{id}/threads/{tid}"),
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
                &format!("/api/artifacts/{id}/threads/{tid}/comments"),
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
                &format!("/api/artifacts/{id}/threads/{tid}/resolve"),
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
    /// mode, terminal_after_s, surface_open}`.
    pub async fn ask_create(
        &self,
        questions: &[clax_core::questions::Question],
        artifact: Option<&str>,
    ) -> Result<Value> {
        let body = json!({"source": "ask", "questions": questions, "artifact_id": artifact});
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

    /// `GET /api/sessions/<sid>/notices?wait=<wait_s>`: `{notices, lines,
    /// waited_s}`; the deadline is `wait_s` plus 10 s. Like `heartbeat`, it
    /// finds a running daemon but never starts one.
    pub async fn notices(&self, wait_s: u64) -> Result<Value> {
        let q = [("wait", wait_s.to_string())];
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
    /// only the rows for those comments; `{acknowledged}`.
    pub async fn ack_comments(&self, comment_ids: &[String]) -> Result<Value> {
        let body = json!({"comment_ids": comment_ids});
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("{}/feedback/ack", c.session_path()),
            )
            .json(&body)
        })
        .await
    }

    /// `POST /api/sessions/<sid>/feedback/ack` `{thread_ids}`: acknowledges
    /// every row on those threads; `{acknowledged}`.
    pub async fn ack(&self, thread_ids: &[String]) -> Result<Value> {
        let body = json!({"thread_ids": thread_ids});
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("{}/feedback/ack", c.session_path()),
            )
            .json(&body)
        })
        .await
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
        Ok(session)
    }

    /// `PATCH /api/sessions/<id>` `{"ended": true}` for the registered session.
    /// Never starts a daemon. `None` when no session was registered. For a
    /// remote daemon, the session's credential files go with it (spec
    /// 2026-10-08 §6.8).
    pub async fn end_session(&self) -> Result<Option<Session>> {
        let Some(held) = self.state().session else {
            return Ok(None);
        };
        let res = self
            .send_with(Mode::DiscoverOnly, |c| {
                c.request(reqwest::Method::PATCH, &c.session_path())
                    .json(&json!({"ended": true}))
            })
            .await;
        self.forget_credentials(&held);
        session_of(body_json(res?).await?).map(Some)
    }

    /// Removes the credential files of session `s` (a remote daemon's),
    /// under the names its row and its registration give.
    fn forget_credentials(&self, s: &Session) {
        let Some(remote) = &self.inner.remote else {
            return;
        };
        let mut keys = keys_of_row(&s.harness, s.harness_session_id.as_deref(), s.parent_pid);
        if let Some(m) = &self.inner.managed {
            keys.extend(registration_keys(&m.registration));
        }
        remote.sessions.remove(&keys, &s.id);
    }
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
) -> std::result::Result<reqwest::Response, Failure> {
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
        let bytes = capped_body(res).await?;
        let whole = {
            let mut b = http::Response::builder().status(status);
            if let Some(h) = b.headers_mut() {
                *h = headers.clone();
            }
            reqwest::Response::from(b.body(bytes).map_err(|e| bad(&e.to_string()))?)
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
) -> std::result::Result<reqwest::Response, Failure> {
    Err(Failure {
        error: ClientError::Unreachable(format!(
            "the remote Clax at {} is reached from a native client only",
            remote.signer.base()
        )),
        refreshable: false,
    })
}

/// The body of `res`, at most [`MAX_RESPONSE_BYTES`].
#[cfg(not(target_arch = "wasm32"))]
async fn capped_body(mut res: reqwest::Response) -> std::result::Result<bytes::Bytes, Failure> {
    let too_big = || Failure {
        error: ClientError::BadResponse(format!(
            "the daemon's answer is larger than {MAX_RESPONSE_BYTES} bytes"
        )),
        refreshable: false,
    };
    if res.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES) {
        return Err(too_big());
    }
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(transport_error)? {
        if (body.len() + chunk.len()) as u64 > MAX_RESPONSE_BYTES {
            return Err(too_big());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(body))
}

/// Sends `req`; a non-success status becomes `ClientError::Api` with the body's
/// `error` object.
async fn attempt(req: reqwest::RequestBuilder) -> std::result::Result<reqwest::Response, Failure> {
    check(req.send().await.map_err(transport_error)?).await
}

/// `res` when its status is a success; else `ClientError::Api` with the
/// body's `error` object.
async fn check(res: reqwest::Response) -> std::result::Result<reqwest::Response, Failure> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    let body: Value = res.json().await.unwrap_or(Value::Null);
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

async fn body_json(res: reqwest::Response) -> Result<Value> {
    if res.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(json!({}));
    }
    let bytes = res.bytes().await.map_err(|e| transport_error(e).error)?;
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

/// Percent-encodes every byte of `path` outside the RFC 3986 unreserved set,
/// keeping `/` as the segment separator.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
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

    #[test]
    fn encode_path_escapes_reserved_bytes_but_not_slashes() {
        assert_eq!(super::encode_path("a/b c#?.js"), "a/b%20c%23%3F.js");
        assert_eq!(super::encode_path("é"), "%C3%A9");
    }
}
