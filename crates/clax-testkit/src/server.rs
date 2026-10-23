//! The in-process test daemon ([`TestServer`]) and the requests tests make
//! through it as the owner, a viewer or a paired extension.

use crate::*;
use clax_core::daemon::{browser_host, probe_host};
use clax_core::working::{Clock, ManualClock, SystemClock};
use clax_core::{EventBus, Home};
use clax_http::AppState;
use clax_http::wrap_cache::WrapCache;
use clax_server::router::build_router;
use clax_store::Store;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// The header that makes a test daemon take a request as arriving from
/// another machine on the LAN ([`TestServer::lan`]).
pub const LAN_HEADER: &str = "x-clax-test-lan";
/// The LAN peer a request with [`LAN_HEADER`] arrives from.
pub const LAN_PEER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 7);
/// The header that names another peer address for a request with
/// [`LAN_HEADER`], so a test can speak from many LAN machines.
pub const PEER_HEADER: &str = "x-clax-test-peer";
/// The daemon's LAN interface address a request with [`LAN_HEADER`]
/// arrives on.
pub const LAN_LOCAL: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

/// Rewrites the connection of a request carrying [`LAN_HEADER`] to one from
/// [`LAN_PEER`] to [`LAN_LOCAL`] (ports kept), so tests of the LAN rules run
/// on every machine, whatever interfaces it has.
async fn as_lan(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req.headers().contains_key(LAN_HEADER) {
        let conn = req
            .extensions()
            .get::<axum::extract::ConnectInfo<clax_http::auth::Conn>>()
            .map(|c| c.0);
        let peer = req
            .headers()
            .get(PEER_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<IpAddr>().ok())
            .unwrap_or(IpAddr::V4(LAN_PEER));
        if let Some(c) = conn {
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(clax_http::auth::Conn {
                    peer: SocketAddr::new(peer, c.peer.port()),
                    local: SocketAddr::new(IpAddr::V4(LAN_LOCAL), c.local.port()),
                }));
        }
    }
    next.run(req).await
}

/// A listener on an ephemeral port of `bind`, and for an IPv4 `bind` a
/// socket bound, not listening, to the same port on `[::1]`.
///
/// `localhost` resolves to `::1` first, and another process (a Bazel
/// server, a dev server) may listen on `[::1]` at the port the kernel gave
/// this daemon on `127.0.0.1`: a request to `http://localhost:<port>`
/// (a hosted test daemon's origin, a browser base) would then reach that
/// process. Holding the twin keeps the port free of any such listener, and
/// a connection to it is refused at once, so the client goes on to
/// `127.0.0.1`. A port whose twin is taken is given back and another
/// tried; a machine without IPv6 needs no twin.
async fn listen(bind: IpAddr) -> (tokio::net::TcpListener, Option<tokio::net::TcpSocket>) {
    loop {
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(bind, 0))
            .await
            .unwrap();
        if bind.is_ipv6() {
            return (listener, None);
        }
        let port = listener.local_addr().unwrap().port();
        let Ok(twin) = tokio::net::TcpSocket::new_v6() else {
            return (listener, None);
        };
        match twin.bind(SocketAddr::new(
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            port,
        )) {
            Ok(()) => return (listener, Some(twin)),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(_) => return (listener, None),
        }
    }
}

/// The checkout's web UI (`web/dist`, Bazel's build of it under Bazel), read
/// from disk: the tests read it as run-time data, so a web UI edit reruns
/// them without recompiling them. Empty when the UI is not built.
pub fn web_ui() -> Arc<dyn clax_core::embedded::FileTree> {
    Arc::new(clax_core::embedded::DiskTree::new(
        clax_fake_exe::repo().join("web/dist"),
    ))
}

/// Set to `deferred` to run every [`TestServer`] that keeps content in the
/// home's files on one connection with [`Deferred`] content instead.
pub const CONTENT_MODE_VAR: &str = "CLAX_TEST_SERVER_CONTENT";

/// Whether this run's test daemons keep content in [`Deferred`]
/// ([`CONTENT_MODE_VAR`]).
pub fn deferred_content() -> bool {
    std::env::var(CONTENT_MODE_VAR).as_deref() == Ok("deferred")
}

/// Content kept in memory ([`MemContent`]) whose every operation completes
/// only when awaited by an executor, as a Durable Object's content store
/// waits on JavaScript promises: it says so
/// ([`clax_core::content::ContentStore::needs_executor`]), so the store's
/// synchronous methods refuse it at once; it yields once before doing its
/// work; and when polled by a synchronous driver anyway
/// ([`clax_core::content::driven_synchronously`], such as
/// [`clax_core::content::wait`]) it fails with
/// [`clax_core::CoreError::WouldBlock`]. A server path that calls a
/// synchronous store method for content fails on it; one that awaits the
/// `async` twins works.
///
/// [`MemContent`]: clax_core::content::memory::MemContent
#[derive(Default)]
pub struct Deferred {
    inner: clax_core::content::memory::MemContent,
}

impl Deferred {
    pub fn new() -> Deferred {
        Deferred::default()
    }

    /// Fails inside a synchronous store method; otherwise yields once.
    async fn executor() -> clax_core::Result<()> {
        if clax_core::content::driven_synchronously() {
            return Err(clax_core::CoreError::WouldBlock {
                what: "content I/O that needs an executor",
            });
        }
        let mut yielded = false;
        std::future::poll_fn(|cx| {
            if yielded {
                return std::task::Poll::Ready(());
            }
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        })
        .await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl clax_core::content::ContentStore for Deferred {
    async fn read_file(
        &self,
        id: &clax_core::ArtifactId,
        n: u32,
        path: &str,
    ) -> clax_core::Result<Option<Vec<u8>>> {
        Self::executor().await?;
        self.inner.read_file(id, n, path).await
    }

    async fn write_version(
        &self,
        id: &clax_core::ArtifactId,
        n: u32,
        write: &clax_core::content::VersionWrite<'_>,
    ) -> clax_core::Result<()> {
        Self::executor().await?;
        self.inner.write_version(id, n, write).await
    }

    async fn copy_version(
        &self,
        from: &clax_core::ArtifactId,
        from_n: u32,
        paths: &[&str],
        to: &clax_core::ArtifactId,
        to_n: u32,
    ) -> clax_core::Result<()> {
        Self::executor().await?;
        self.inner.copy_version(from, from_n, paths, to, to_n).await
    }

    async fn remove_version(&self, id: &clax_core::ArtifactId, n: u32) -> clax_core::Result<()> {
        Self::executor().await?;
        self.inner.remove_version(id, n).await
    }

    async fn unrecorded_step(
        &self,
        id: &clax_core::ArtifactId,
        n: u32,
    ) -> clax_core::Result<Option<String>> {
        Self::executor().await?;
        self.inner.unrecorded_step(id, n).await
    }

    async fn delete_artifact(&self, id: &clax_core::ArtifactId) -> clax_core::Result<()> {
        Self::executor().await?;
        self.inner.delete_artifact(id).await
    }

    async fn put_asset(
        &self,
        key: &clax_core::content::AssetKey,
        bytes: &[u8],
    ) -> clax_core::Result<()> {
        Self::executor().await?;
        self.inner.put_asset(key, bytes).await
    }

    async fn read_asset(
        &self,
        key: &clax_core::content::AssetKey,
    ) -> clax_core::Result<Option<Vec<u8>>> {
        Self::executor().await?;
        self.inner.read_asset(key).await
    }

    async fn delete_asset(&self, key: &clax_core::content::AssetKey) -> clax_core::Result<()> {
        Self::executor().await?;
        self.inner.delete_asset(key).await
    }

    async fn put_clip(
        &self,
        id: &clax_core::ArtifactId,
        thread: &str,
        bytes: &[u8],
    ) -> clax_core::Result<()> {
        Self::executor().await?;
        self.inner.put_clip(id, thread, bytes).await
    }

    async fn read_clip(
        &self,
        id: &clax_core::ArtifactId,
        thread: &str,
    ) -> clax_core::Result<Option<Vec<u8>>> {
        Self::executor().await?;
        self.inner.read_clip(id, thread).await
    }

    async fn delete_clip(&self, id: &clax_core::ArtifactId, thread: &str) -> clax_core::Result<()> {
        Self::executor().await?;
        self.inner.delete_clip(id, thread).await
    }

    fn needs_executor(&self) -> bool {
        true
    }
}

/// A running test daemon with its own temporary home and the fixed token `test-token`.
pub struct TestServer {
    pub base: String,
    pub token: String,
    pub home: Home,
    pub client: reqwest::Client,
    /// The server's event bus, for publishing events directly.
    pub events: EventBus,
    /// The server's store, for reading what a request left behind.
    pub store: Arc<Store>,
    /// The server's working registry.
    pub working: Arc<clax_core::working::Working>,
    /// The server's feedback context, for driving its background passes.
    pub feedback: clax_realtime::feedback::FeedbackCtx,
    /// The server's drafts registry.
    pub drafts: Arc<clax_core::drafts::Drafts>,
    /// The server's presence registry.
    pub presence: Arc<clax_core::presence::Presence>,
    /// The address the listener is bound to (may be unspecified, e.g. `0.0.0.0`).
    pub addr: SocketAddr,
    /// The journal state the status route reports; no appender runs unless
    /// a test starts one with it.
    pub journal: Arc<clax_http::audit::JournalStatus>,
    /// The bounds on agents' unverified bodies.
    pub agent_gate: Arc<clax_http::agent::Gate>,
    /// The web UI the daemon serves.
    pub web: Arc<dyn clax_core::embedded::FileTree>,
    /// The live seats, whose limits a test may set.
    pub seats: Arc<clax_http::seats::LiveSeats>,
    /// The live layers' hub.
    pub live_layers: Arc<clax_realtime::live_layers::LiveLayers>,
    /// The server's state, for calling what a host calls beside the router
    /// (a socket's reopening).
    pub state: AppState,
    extension_id: String,
    /// The IPv6 loopback twin of an IPv4 daemon's port ([`listen`]).
    _v6_twin: Option<tokio::net::TcpSocket>,
    _dir: Arc<tempfile::TempDir>,
}

impl TestServer {
    pub async fn spawn() -> TestServer {
        Self::spawn_with(|_| {}).await
    }

    /// A test daemon serving the web UI from `dir` (laid out like
    /// `web/dist`), read from disk on every request.
    pub async fn spawn_with_web(dir: impl Into<std::path::PathBuf>) -> TestServer {
        let web = Arc::new(clax_core::embedded::DiskTree::new(dir));
        Self::spawn_with(move |s| s.web = web).await
    }

    pub async fn spawn_with(f: impl FnOnce(&mut AppState)) -> TestServer {
        Self::spawn_on(IpAddr::V4(Ipv4Addr::LOCALHOST), f).await
    }

    /// Like `spawn_with`, listening on `bind`. `base` targets `bind`, or the
    /// same-family loopback when `bind` is unspecified.
    pub async fn spawn_on(bind: IpAddr, f: impl FnOnce(&mut AppState)) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        Self::start(dir, bind, Arc::new(SystemClock), f, false, None).await
    }

    /// [`TestServer::spawn_on`] with the shutdown route
    /// (`POST /api/admin/shutdown`) the daemon serves.
    pub async fn spawn_on_with_shutdown(bind: IpAddr) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        Self::start(dir, bind, Arc::new(SystemClock), |_| {}, true, None).await
    }

    /// A test daemon listening on every interface (it may invite people)
    /// whose bounds on agents' unverified bodies wait and measure rates on
    /// the returned timer.
    pub async fn spawn_agent_timer() -> (TestServer, Arc<ManualTimer>) {
        Self::spawn_agent_timer_with_budget(clax_http::agent::BUDGET).await
    }

    /// [`TestServer::spawn_agent_timer`] with an overall budget of
    /// `budget` bytes of unverified bodies.
    pub async fn spawn_agent_timer_with_budget(budget: usize) -> (TestServer, Arc<ManualTimer>) {
        let timer = Arc::new(ManualTimer::default());
        let t = timer.clone();
        let ts = Self::spawn_on(IpAddr::V4(Ipv4Addr::UNSPECIFIED), move |s| {
            s.agent_gate = Arc::new(clax_http::agent::Gate::with_budget(t, budget));
        })
        .await;
        (ts, timer)
    }

    /// A test daemon whose store, working and presence registries and
    /// question clock all read `clock`.
    pub async fn spawn_with_clock(clock: Arc<ManualClock>) -> TestServer {
        Self::spawn_on_with_clock(IpAddr::V4(Ipv4Addr::LOCALHOST), clock).await
    }

    /// [`TestServer::spawn_with_clock`] with `f` applied to the state
    /// before it serves, as [`TestServer::spawn_with`] does.
    pub async fn spawn_with_clock_and(
        clock: Arc<ManualClock>,
        f: impl FnOnce(&mut AppState),
    ) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        Self::start(dir, IpAddr::V4(Ipv4Addr::LOCALHOST), clock, f, false, None).await
    }

    /// [`TestServer::spawn_with_clock`] listening on `bind` (`0.0.0.0` for
    /// one that may invite people: its invites name [`LAN_LOCAL`]).
    pub async fn spawn_on_with_clock(bind: IpAddr, clock: Arc<ManualClock>) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        Self::start(dir, bind, clock, |_| {}, false, None).await
    }

    /// A hosted test daemon ([`clax_http::hosted`]) whose public origin is
    /// `http://localhost:<its port>` and whose setup takes `secret` (`None`:
    /// setup closed), every time read from `clock`.
    pub async fn spawn_hosted(clock: Arc<ManualClock>, secret: Option<&str>) -> TestServer {
        Self::spawn_hosted_with(clock, secret, |_| {}).await
    }

    /// [`TestServer::spawn_hosted`] whose bounds on agents' unverified
    /// bodies wait and measure rates on the returned timer.
    pub async fn spawn_hosted_agent_timer(
        clock: Arc<ManualClock>,
        secret: Option<&str>,
    ) -> (TestServer, Arc<ManualTimer>) {
        let timer = Arc::new(ManualTimer::default());
        let t = timer.clone();
        let ts = Self::spawn_hosted_with(clock, secret, move |s| {
            s.agent_gate = Arc::new(clax_http::agent::Gate::new(t));
        })
        .await;
        (ts, timer)
    }

    /// [`TestServer::spawn_hosted`], `f` changing the state before it
    /// serves (a git remotes host, say).
    pub async fn spawn_hosted_with(
        clock: Arc<ManualClock>,
        secret: Option<&str>,
        f: impl FnOnce(&mut AppState),
    ) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        let secret = secret.map(str::to_string);
        let limits_clock: Arc<dyn clax_core::working::Clock> = clock.clone();
        let hosted = move |s: &mut AppState| {
            let origin = format!("http://localhost:{}", s.port);
            s.hosted = clax_http::hosted::Hosted::new(&origin, secret.as_deref())
                .map(|h| Arc::new(h.with_clock(limits_clock)));
            f(s);
        };
        Self::start(
            dir,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            clock,
            hosted,
            false,
            None,
        )
        .await
    }

    /// [`TestServer::spawn_hosted`] with artifact content on a content
    /// origin of its own, `http://[::1]:<its port>`
    /// ([`clax_http::hosted::Hosted::with_content_origin`]).
    pub async fn spawn_hosted_with_content(
        clock: Arc<ManualClock>,
        secret: Option<&str>,
    ) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        let secret = secret.map(str::to_string);
        let limits_clock: Arc<dyn clax_core::working::Clock> = clock.clone();
        let hosted = move |s: &mut AppState| {
            let origin = format!("http://localhost:{}", s.port);
            let content = format!("http://[::1]:{}", s.port);
            s.hosted = clax_http::hosted::Hosted::new(&origin, secret.as_deref())
                .and_then(|h| h.with_clock(limits_clock).with_content_origin(&content))
                .map(Arc::new);
        };
        Self::start(
            dir,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            clock,
            hosted,
            false,
            None,
        )
        .await
    }

    /// A test daemon on a fresh copy of fixture `name` (a file of
    /// [`fixtures_dir`], loaded by [`load_fixture`]), its clock
    /// ([`TestServer::spawn_with_clock`]) stopped at `at` (RFC 3339). The
    /// store migrates the fixture's database on open, as a daemon does.
    ///
    /// # Panics
    /// When the fixture cannot be read or loaded, or `at` does not parse.
    pub async fn from_fixture_with_clock(name: &str, at: &str) -> TestServer {
        let path = fixtures_dir().join(name);
        let sql = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        Self::from_sql_with_clock(&sql, at).await
    }

    /// A test daemon on a database made by fixture text `sql` (as
    /// [`load_fixture`] loads a fixture file), its clock stopped at `at`
    /// (RFC 3339). The store migrates the database on open, as a daemon
    /// does.
    ///
    /// # Panics
    /// When `sql` cannot be loaded, or `at` does not parse.
    pub async fn from_sql_with_clock(sql: &str, at: &str) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        load_fixture(&Home::at(dir.path().join("ax")), sql);
        let clock = Arc::new(ManualClock::at(at));
        Self::start(
            dir,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            clock,
            |_| {},
            false,
            None,
        )
        .await
    }

    /// A test daemon whose store runs on one connection
    /// ([`Store::open_on`], as a Durable Object's does), keeping version
    /// files, assets and clips in `content`.
    pub async fn spawn_on_one_connection(
        content: Arc<dyn clax_core::content::ContentStore>,
    ) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        let bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
        Self::start(
            dir,
            bind,
            Arc::new(SystemClock),
            |_| {},
            false,
            Some((content, true)),
        )
        .await
    }

    /// [`TestServer::spawn_on_one_connection`] on a state `f` configures
    /// first.
    pub async fn spawn_on_one_connection_with(
        content: Arc<dyn clax_core::content::ContentStore>,
        f: impl FnOnce(&mut AppState),
    ) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        let bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
        Self::start(
            dir,
            bind,
            Arc::new(SystemClock),
            f,
            false,
            Some((content, true)),
        )
        .await
    }

    /// A test daemon whose store runs on its pool of database threads, as
    /// the daemon's does, keeping version files, assets and clips in
    /// `content`.
    pub async fn spawn_on_pool_with_content(
        content: Arc<dyn clax_core::content::ContentStore>,
    ) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        let bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
        Self::start(
            dir,
            bind,
            Arc::new(SystemClock),
            |_| {},
            false,
            Some((content, false)),
        )
        .await
    }

    /// A second daemon on this one's home, started as after a restart:
    /// the database and files are this one's, and nothing held in memory
    /// (streams, claims, caches) carries over. Both serve until dropped.
    pub async fn reopened(&self) -> TestServer {
        Self::start(
            self._dir.clone(),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            Arc::new(SystemClock),
            |_| {},
            false,
            None,
        )
        .await
    }

    /// Serves the home `ax` under `dir` on `bind`, every time read from
    /// `clock`: on the store's pool and the home's files, or with `content`,
    /// on one connection when its flag says so
    /// ([`TestServer::spawn_on_one_connection`]), else on the pool
    /// ([`TestServer::spawn_on_pool_with_content`]).
    async fn start(
        dir: impl Into<Arc<tempfile::TempDir>>,
        bind: IpAddr,
        clock: Arc<dyn Clock>,
        f: impl FnOnce(&mut AppState),
        shutdown: bool,
        content: Option<(Arc<dyn clax_core::content::ContentStore>, bool)>,
    ) -> TestServer {
        let dir: Arc<tempfile::TempDir> = dir.into();
        let home = Home::at(dir.path().join("ax"));
        let content = content.or_else(|| {
            deferred_content().then(|| {
                (
                    Arc::new(Deferred::new()) as Arc<dyn clax_core::content::ContentStore>,
                    true,
                )
            })
        });
        let store = Arc::new(match content {
            None => Store::open_with_clock(&home, clock.clone()).unwrap(),
            Some((content, false)) => Store::open_with_clock(&home, clock.clone())
                .unwrap()
                .with_content(content),
            Some((content, true)) => {
                home.ensure_dirs().unwrap();
                let conn = rusqlite::Connection::open(home.db_path()).unwrap();
                Store::open_on(&home, conn, clock.clone(), content).unwrap()
            }
        });
        let token = "test-token".to_string();
        let (listener, v6_twin) = listen(bind).await;
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let live_ids = Arc::new(clax_realtime::live::LiveIds::load(&store).unwrap());
        let ext_creds =
            Arc::new(clax_http::extension::Credentials::load_with(&store, clock.clone()).unwrap());
        let started_at = store.now();
        let grant_auth = Arc::new(clax_http::grant_auth::GrantAuth::load(&store).unwrap());
        let vis_gen = clax_http::state::VisGen::install(&store);
        let mut state = AppState {
            store,
            home: home.clone(),
            token: token.clone(),
            require_credential: false,
            events: EventBus::new(),
            started_at,
            version: "test",
            shutdown: tokio::sync::watch::channel(false).1,
            wrap_cache: Arc::new(WrapCache::new(clax_http::wrap_cache::DEFAULT_MAX_BYTES)),
            request_timeout: Duration::from_secs(30),
            publish_timeout: Duration::from_secs(120),
            sse_keep_alive: Duration::from_secs(15),
            self_base: format!("http://{}:{port}", probe_host(&bind.to_string())),
            browser_base: format!("http://{}:{port}", browser_host(&bind.to_string())),
            feedback_waiters: Arc::new(Default::default()),
            followers: Arc::new(Default::default()),
            feedback_states: Arc::new(Default::default()),
            // Push off; tests about push set it with `spawn_with`.
            codex: Arc::new(Default::default()),
            working: Arc::new(clax_core::working::Working::new(clock.clone())),
            drafts: Arc::new(clax_core::drafts::Drafts::new(clock.clone())),
            presence: Arc::new(clax_core::presence::Presence::new(clock.clone())),
            rooms: Arc::new(clax_realtime::room::Rooms::default()),
            live_layers: Arc::new(clax_realtime::live_layers::LiveLayers::new(clock.clone())),
            // Sampling off; tests that sample set their own with `spawn_with`.
            sample: Arc::new(clax_sample::Sampler::disabled()),
            // The system resolver and reqwest; tests of `net` set their own.
            net: Arc::new(clax_http::net::Net::default()),
            stream: clax_realtime::stream::Hub::new(live_ids.clone()),
            seats: clax_http::seats::LiveSeats::new(Default::default(), clock.clone()),
            sockets: Arc::default(),
            live_ids,
            ext_creds,
            extension_id: clax_core::extension::extension_id_in_effect(home.root()),
            questions: Arc::new(Default::default()),
            question_grace: Duration::from_secs(5),
            question_sleeper: Arc::new(clax_realtime::questions::TokioSleeper),
            question_clock: clock,
            terminal_after_s: clax_core::config::TERMINAL_AFTER_S,
            calibration: Arc::default(),
            verify_times: Arc::default(),
            audit_wake: clax_http::audit::AuditWake::new(),
            // No appender; tests of the journal start their own.
            journal: clax_http::audit::JournalStatus::off(None),
            exports: clax_http::exports::Exports::new(),
            call_ids: Arc::new(clax_realtime::call_ids::CallIdWait::new(
                clax_realtime::call_ids::CALL_ID_GRACE,
                clax_realtime::call_ids::CALL_ID_CAP,
            )),
            bind,
            port,
            // Invites from a daemon on every interface name the address
            // `lan()` requests arrive on.
            lan_host: bind.is_unspecified().then_some(IpAddr::V4(LAN_LOCAL)),
            grant_auth,
            agent_gate: Arc::new(clax_http::agent::Gate::new(Arc::new(
                clax_http::agent::TokioTimer::default(),
            ))),
            vis_gen,
            test_daemon: true,
            // The checkout's web UI; tests of the shell serve their own
            // with `spawn_with_web`.
            web: web_ui(),
            hosted: None,
            // No app links or store pages; tests of them set their own.
            clax_app: Arc::default(),
            embed: None,
            git_remotes: Arc::default(),
            linked: Arc::new(clax_realtime::linked::Linked::new(Arc::new(
                clax_realtime::questions::TokioSleeper,
            ))),
            jobs_owners: Arc::default(),
        };
        f(&mut state);
        state.audit_wake.install(&state.store);
        state.stream.listen(&state.events);
        clax_http::seats::install(&state);
        let seats = state.seats.clone();
        let live_layers = state.live_layers.clone();
        {
            let store = state.store.clone();
            state.stream.check_reads(move |reader, source| {
                clax_http::db_caller::reads_now(&store, reader, source)
            });
            let store = state.store.clone();
            state.stream.hide(move |aid| store.is_hidden(aid));
        }
        clax_http::views::inbox::listen(&state);
        clax_http::views::approval::listen(&state);
        clax_server::routes::people::listen(&state);
        clax_server::routes::jobs::listen(&state);
        clax_http::pushes::listen(&state);
        clax_http::git_remotes::listen(&state);
        clax_server::linked::listen(&state);
        let events = state.events.clone();
        let store = state.store.clone();
        let working = state.working.clone();
        let feedback = state.feedback_ctx();
        let presence = state.presence.clone();
        let drafts = state.drafts.clone();
        let extension_id = state.extension_id.clone();
        let journal = state.journal.clone();
        let agent_gate = state.agent_gate.clone();
        let web = state.web.clone();
        let kept_state = state.clone();
        let app = if shutdown {
            clax_server::router::build_router_with_shutdown(
                state,
                tokio::sync::watch::channel(false).0,
            )
        } else {
            build_router(state)
        }
        .layer(axum::middleware::from_fn(as_lan));
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<clax_http::auth::Conn>(),
            )
            .await
            .unwrap();
        });
        TestServer {
            base: format!("http://{}:{port}", probe_host(&bind.to_string())),
            token,
            home,
            client: reqwest::Client::new(),
            events,
            store,
            working,
            feedback,
            presence,
            drafts,
            addr,
            journal,
            agent_gate,
            web,
            seats,
            live_layers,
            state: kept_state,
            extension_id,
            _v6_twin: v6_twin,
            _dir: dir,
        }
    }

    /// A client whose requests reach this daemon as from another machine on
    /// the LAN (peer [`LAN_PEER`], arriving on [`LAN_LOCAL`] with that
    /// address as their `Host`), and the base URL to send them to. A request
    /// may still set its own `Host`.
    pub fn lan(&self) -> (reqwest::Client, String) {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(LAN_HEADER, reqwest::header::HeaderValue::from_static("1"));
        h.insert(
            reqwest::header::HOST,
            format!("{LAN_LOCAL}:{}", self.addr.port()).parse().unwrap(),
        );
        let client = reqwest::Client::builder()
            .default_headers(h)
            .build()
            .unwrap();
        (client, self.base.clone())
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(format!("{}{}", self.base, path))
            .send()
            .await
            .unwrap()
    }

    /// A GET carrying the bearer token.
    pub async fn get_authed(&self, path: &str) -> reqwest::Response {
        self.authed(self.client.get(format!("{}{}", self.base, path)))
            .send()
            .await
            .unwrap()
    }

    pub fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.token)
    }

    pub async fn post_json(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.authed(
            self.client
                .post(format!("{}{}", self.base, path))
                .json(&body),
        )
        .send()
        .await
        .unwrap()
    }

    /// Publishes a new artifact; `files` are (path, utf8 content).
    pub async fn publish(&self, title: &str, files: &[(&str, &str)]) -> serde_json::Value {
        let files: serde_json::Map<String, serde_json::Value> = files
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    serde_json::json!({"content": v, "encoding": "utf8"}),
                )
            })
            .collect();
        let res = self
            .post_json(
                "/api/artifacts",
                serde_json::json!({"title": title, "files": files}),
            )
            .await;
        assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
        res.json().await.unwrap()
    }
}

/// Bytes that pass the daemon's PNG check (signature only); not a decodable image.
pub const FAKE_PNG: &[u8] = b"\x89PNG\r\n\x1a\nclax-test-clip";

/// An element anchor on `body > main > h2` quoting "Quarterly goals".
pub fn element_anchor() -> serde_json::Value {
    serde_json::json!({"kind": "element", "selector": "body > main > h2", "quote": "Quarterly goals",
        "prefix": "", "suffix": "", "html_hash": "sha256:00", "rect": null, "custom_name": null})
}

impl TestServer {
    /// Registers a live session; returns the session object.
    pub async fn register_session(&self, harness: &str, hsid: &str) -> serde_json::Value {
        let res = self
            .post_json(
                "/api/sessions",
                serde_json::json!({"harness": harness, "harness_session_id": hsid, "cwd": "/w"}),
            )
            .await;
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["session"].clone()
    }

    /// Creates an artifact attributed to `session_id` (so the session owns and watches it).
    pub async fn publish_as(&self, session_id: &str, title: &str, html: &str) -> serde_json::Value {
        let res = self
            .authed(self.client.post(format!("{}/api/artifacts", self.base)))
            .header("x-clax-session", session_id)
            .json(&serde_json::json!({"title": title, "files": {"index.html": {"content": html, "encoding": "utf8"}}}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        res.json().await.unwrap()
    }

    /// `POST /api/artifacts/<aid>/threads` as a browser does, with [`element_anchor`].
    pub async fn create_thread(
        &self,
        aid: &str,
        version: u32,
        body: &str,
        clip: Option<&[u8]>,
    ) -> reqwest::Response {
        let mut form = reqwest::multipart::Form::new()
            .text("anchor", element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", version.to_string());
        if let Some(bytes) = clip {
            form = form.part(
                "clip",
                reqwest::multipart::Part::bytes(bytes.to_vec())
                    .file_name("clip.png")
                    .mime_str("image/png")
                    .unwrap(),
            );
        }
        self.client
            .post(format!("{}/api/artifacts/{aid}/threads", self.base))
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    /// Creates a thread without a clip; returns the thread view.
    pub async fn thread(&self, aid: &str, version: u32, body: &str) -> serde_json::Value {
        let res = self.create_thread(aid, version, body, None).await;
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }

    /// Presses "Send to agent"; returns the thread view.
    pub async fn send_thread(&self, aid: &str, tid: &str) -> serde_json::Value {
        let res = self
            .client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/send",
                self.base
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }

    /// Opens `/api/events<query>` and returns a reader past nothing yet.
    pub async fn events(&self, query: &str) -> EventReader {
        self.events_as(query, None).await
    }

    /// Like [`TestServer::events`], as the viewer whose cookie value is `cookie`.
    pub async fn events_as(&self, query: &str, cookie: Option<&str>) -> EventReader {
        self.events_with(query, |r| match cookie {
            Some(c) => r.header("cookie", format!("clax_viewer={c}")),
            None => r,
        })
        .await
    }

    /// Opens `/api/events<query>` with the request shaped by `build` (headers
    /// such as a cookie or the token; `query` may carry `&token=`).
    pub async fn events_with(
        &self,
        query: &str,
        build: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> EventReader {
        use futures::StreamExt;
        let res = build(self.client.get(format!("{}/api/events{query}", self.base)))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let stream = res
            .bytes_stream()
            .map(|r| r.map(|b| b.to_vec()).map_err(|e| e.to_string()));
        EventReader {
            stream: Box::pin(stream),
            buf: String::new(),
            keep_ready: false,
        }
    }
}

/// A browser viewer created through `GET /api/viewers/me`: its cookie value
/// (send it as `Cookie: clax_viewer=<cookie>`) and its public ID.
pub struct TestViewer {
    pub cookie: String,
    pub public_id: String,
    base: String,
    client: reqwest::Client,
}

impl TestViewer {
    /// Opens `/api/stream` with this viewer's cookie and subscribes it to
    /// `topics`; the status of the subscription request.
    pub async fn subscribe_status(&self, topics: &[&str]) -> u16 {
        let cookie = format!("clax_viewer={}", self.cookie);
        let res = self
            .client
            .get(format!("{}/api/stream", self.base))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let mut events = EventReader::from_response(res);
        let id = events.next_named("ready").await["stream"]
            .as_str()
            .unwrap()
            .to_string();
        self.client
            .post(format!("{}/api/stream/{id}", self.base))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "subscribe": topics }))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }
}

impl TestViewer {
    /// `PUT /api/viewers/me/looked`: this viewer looked at `tids` of `aid`;
    /// asserts a success.
    pub async fn look(&self, aid: &str, tids: &[&str]) {
        let res = self
            .client
            .put(format!("{}/api/viewers/me/looked", self.base))
            .header("cookie", format!("clax_viewer={}", self.cookie))
            .json(&serde_json::json!({"artifact_id": aid, "thread_ids": tids}))
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
    }
}

impl TestServer {
    /// Creates a viewer, named `name` when given.
    pub async fn viewer(&self, name: Option<&str>) -> TestViewer {
        let res = self.get("/api/viewers/me").await;
        let set = res.headers()["set-cookie"].to_str().unwrap().to_string();
        let cookie = set
            .split(';')
            .next()
            .and_then(|kv| kv.strip_prefix("clax_viewer="))
            .expect("clax_viewer cookie")
            .to_string();
        let mut v: serde_json::Value = res.json().await.unwrap();
        if let Some(n) = name {
            let res = self
                .client
                .put(format!("{}/api/viewers/me", self.base))
                .header("cookie", format!("clax_viewer={cookie}"))
                .json(&serde_json::json!({"display_name": n}))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200);
            v = res.json().await.unwrap();
        }
        TestViewer {
            cookie,
            public_id: v["viewer"]["public_id"].as_str().unwrap().to_string(),
            base: self.base.clone(),
            client: self.client.clone(),
        }
    }

    /// A connected person's browser named `name`, made straight in the
    /// store (a viewer marked a person, with no grant or machine): for tests
    /// of what a person's browser may do, by the dozen.
    pub async fn person(&self, name: &str) -> TestViewer {
        let v = self.viewer(Some(name)).await;
        let conn = rusqlite::Connection::open(self.home.db_path()).unwrap();
        conn.busy_timeout(Duration::from_secs(10)).unwrap();
        let n = conn
            .execute(
                "UPDATE viewers SET person = 1 WHERE public_id = ?1",
                [&v.public_id],
            )
            .unwrap();
        assert_eq!(n, 1);
        v
    }

    /// The owner identity's public ID, made as a browser of the owner's
    /// makes it when there is none yet.
    pub async fn owner_public_id(&self) -> String {
        let v: serde_json::Value = self
            .client
            .get(format!("{}/api/viewers/me", self.base))
            .header("cookie", self.owner_cookie())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["viewer"]["public_id"].as_str().unwrap().to_string()
    }

    /// The owner viewer, as a browser of the owner's sees it (made as a
    /// browser's when there is none yet).
    pub async fn owner_viewer(&self) -> clax_core::model::Viewer {
        let v: serde_json::Value = self
            .client
            .get(format!("{}/api/viewers/me", self.base))
            .header("cookie", self.owner_cookie())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        serde_json::from_value(v["viewer"].clone()).unwrap()
    }

    /// The extension ID in effect for this server's home (spec
    /// 2026-10-05-chrome-overlay-design L15).
    pub fn extension_id(&self) -> String {
        self.extension_id.clone()
    }

    /// The owner cookie a browser of the owner's holds (`name=value`), as the
    /// shell's token request sets it.
    pub fn owner_cookie(&self) -> String {
        let host = self.base.trim_start_matches("http://");
        format!(
            "{}={}",
            clax_http::identity::owner_cookie_name(host),
            clax_http::identity::owner_cookie_value(&self.token)
        )
    }

    /// Creates a thread as the viewer whose cookie value is `cookie`; returns the thread view.
    pub async fn thread_as(&self, aid: &str, cookie: &str, body: &str) -> serde_json::Value {
        let form = reqwest::multipart::Form::new()
            .text("anchor", element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", "1");
        let res = self
            .client
            .post(format!("{}/api/artifacts/{aid}/threads", self.base))
            .header("cookie", format!("clax_viewer={cookie}"))
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }

    /// Replies on `tid` as the viewer whose cookie value is `cookie`; returns the thread view.
    pub async fn reply_as(
        &self,
        aid: &str,
        tid: &str,
        cookie: &str,
        body: &str,
    ) -> serde_json::Value {
        let res = self
            .client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/comments",
                self.base
            ))
            .header("cookie", format!("clax_viewer={cookie}"))
            .json(&serde_json::json!({"body": body}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }
}

/// A paired extension's requests ([`TestServer::extension`]): from its
/// origin, with its credential, as the extension's service worker sends
/// them.
pub struct TestExtension {
    base: String,
    client: reqwest::Client,
    origin: String,
    credential: String,
}

impl TestExtension {
    fn request(&self, m: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(m, format!("{}{path}", self.base))
            .header("origin", &self.origin)
            .header("sec-fetch-site", "cross-site")
            .header(
                "authorization",
                format!("Clax-Extension {}", self.credential),
            )
    }

    /// `GET <path>` as the extension.
    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::GET, path)
            .send()
            .await
            .unwrap()
    }

    /// `POST <path>` with JSON `body` as the extension.
    pub async fn post(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.request(reqwest::Method::POST, path)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
}

impl TestServer {
    /// `POST /api/sessions/<sid>/questions` with `body`; the response body,
    /// after asserting a 200 or 201.
    pub async fn ask(&self, sid: &str, body: serde_json::Value) -> serde_json::Value {
        let res = self
            .post_json(&format!("/api/sessions/{sid}/questions"), body)
            .await;
        let status = res.status();
        let v: serde_json::Value = res.json().await.unwrap();
        assert!(status == 200 || status == 201, "{status}: {v}");
        v
    }

    /// Answers question `qid` as the owner through the shell; asserts 200.
    pub async fn answer_question(&self, qid: &str, body: serde_json::Value) {
        let res = self.answer_question_raw(qid, body).await;
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    }

    /// Answers question `qid` as the owner through the shell: `POST
    /// /api/questions/<qid>/answer` with the owner cookie.
    pub async fn answer_question_raw(
        &self,
        qid: &str,
        body: serde_json::Value,
    ) -> reqwest::Response {
        self.client
            .post(format!("{}/api/questions/{qid}/answer", self.base))
            .header("cookie", self.owner_cookie())
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    /// Opens `/api/stream` as a browser of the owner's (the owner cookie)
    /// and subscribes it to `topics`; a reader past nothing yet. While the
    /// reader is held, or for the hub's grace after, an owner surface is
    /// open.
    pub async fn stream_as_owner(&self, topics: &[&str]) -> EventReader {
        let res = self
            .client
            .get(format!("{}/api/stream", self.base))
            .header("cookie", self.owner_cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let mut events = EventReader::from_response(res);
        let id = events.next_named("ready").await["stream"]
            .as_str()
            .unwrap()
            .to_string();
        let res = self
            .client
            .post(format!("{}/api/stream/{id}", self.base))
            .header("cookie", self.owner_cookie())
            .json(&serde_json::json!({ "subscribe": topics }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
        events
    }

    /// How many `wait_for_feedback` polls of session `sid` are in progress.
    pub async fn feedback_waiters(&self, sid: &str) -> u64 {
        let v: serde_json::Value = self
            .get_authed(&format!("/api/_test/feedback/{sid}/waiters"))
            .await
            .json()
            .await
            .unwrap();
        v["count"].as_u64().expect("a count")
    }

    /// Returns once `n` `wait_for_feedback` polls of session `sid` are in
    /// progress; panics past the [`HANG`](crate::HANG) guard.
    pub async fn wait_feedback_waiters(&self, sid: &str, n: u64) {
        self.count_until(
            &format!("/api/_test/feedback/{sid}/waiters?until={n}"),
            n,
            &format!("{n} feedback polls of {sid}"),
        )
        .await;
    }

    /// Returns once `n` feedback polls of session `sid` with tier `tier`
    /// are parked waiting to be woken; panics past the
    /// [`HANG`](crate::HANG) guard.
    pub async fn wait_parked_feedback_polls(&self, sid: &str, tier: &str, n: u64) {
        self.count_until(
            &format!("/api/_test/feedback/{sid}/waiters?parked={tier}&until={n}"),
            n,
            &format!("{n} parked {tier} polls of {sid}"),
        )
        .await;
    }

    /// Asks a debug waiter route (`path`, which names `until={n}`) until
    /// its `count` is `n`. The route answers once the count is `n` or after
    /// a few seconds, whichever is first; each answer short of `n` is asked
    /// again, so a slow machine only makes the wait longer. Panics, naming
    /// `what` and the last count, past the [`HANG`](crate::HANG) guard.
    async fn count_until(&self, path: &str, n: u64, what: &str) {
        let start = std::time::Instant::now();
        loop {
            let res = self.get_authed(path).await;
            assert_eq!(res.status(), 200, "{what}: {path}");
            let v: serde_json::Value = res.json().await.unwrap();
            if v["count"].as_u64() == Some(n) {
                return;
            }
            assert!(
                start.elapsed() < crate::HANG,
                "{what}: still {} after {:?} (hang guard)",
                v["count"],
                crate::HANG
            );
        }
    }

    /// Stops each feedback poll of session `sid` after its take (`hold`),
    /// or lets them go on.
    pub async fn hold_feedback(&self, sid: &str, hold: bool) {
        self.hold_feedback_at(sid, hold, "after_take").await;
    }

    /// Stops each feedback poll of session `sid` at `at` (`first_take`,
    /// `before_take`, `after_take`), or lets them go on.
    pub async fn hold_feedback_at(&self, sid: &str, hold: bool, at: &str) {
        let res = self
            .post_json(
                &format!("/api/_test/feedback/{sid}/hold"),
                serde_json::json!({"hold": hold, "at": at}),
            )
            .await;
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    }

    /// Returns once `n` feedback polls of session `sid` are stopped after
    /// their take; panics past the [`HANG`](crate::HANG) guard.
    pub async fn wait_held_feedback_polls(&self, sid: &str, n: u64) {
        self.count_until(
            &format!("/api/_test/feedback/{sid}/waiters?held=true&until={n}"),
            n,
            &format!("{n} held polls of {sid}"),
        )
        .await;
    }

    /// Ends every `wait_for_feedback` poll of session `sid` now, as if its
    /// wait had run out.
    pub async fn expire_feedback(&self, sid: &str) {
        let res = self
            .post_json(
                &format!("/api/_test/feedback/{sid}/expire"),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    }

    /// A paired extension: a fresh credential for this server's extension
    /// ID, whose requests come from the extension's origin.
    pub async fn extension(&self) -> TestExtension {
        let v: serde_json::Value = self
            .authed(
                self.client
                    .post(format!("{}/api/extension/credentials", self.base)),
            )
            .json(&serde_json::json!({"extension_id": self.extension_id}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        TestExtension {
            base: self.base.clone(),
            client: self.client.clone(),
            origin: clax_core::extension::extension_origin(&self.extension_id),
            credential: v["credential"].as_str().expect("a credential").to_string(),
        }
    }

    /// Question `qid`'s stored status.
    pub async fn question_status(&self, qid: &str) -> String {
        self.store
            .question(qid)
            .unwrap()
            .expect("the question exists")
            .status
            .as_str()
            .to_string()
    }

    /// Ends session `sid` as its harness does; asserts 200.
    pub async fn end_session(&self, sid: &str) {
        let res = self
            .authed(
                self.client
                    .patch(format!("{}/api/sessions/{sid}", self.base))
                    .json(&serde_json::json!({"ended": true})),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }

    /// How many polls hold question `qid` now.
    pub async fn question_waiters(&self, qid: &str) -> u64 {
        let v: serde_json::Value = self
            .get_authed(&format!("/api/_test/questions/{qid}/waiters"))
            .await
            .json()
            .await
            .unwrap();
        v["count"].as_u64().expect("a count")
    }

    /// Returns once `n` polls hold question `qid`; panics past the
    /// [`HANG`](crate::HANG) guard.
    pub async fn wait_question_waiters(&self, qid: &str, n: u64) {
        self.count_until(
            &format!("/api/_test/questions/{qid}/waiters?until={n}"),
            n,
            &format!("{n} polls of {qid}"),
        )
        .await;
    }

    /// Returns once `n` polls of question `qid` have read it and wait to
    /// be woken; panics past the [`HANG`](crate::HANG) guard.
    pub async fn wait_parked_question_polls(&self, qid: &str, n: u64) {
        self.count_until(
            &format!("/api/_test/questions/{qid}/waiters?parked=true&until={n}"),
            n,
            &format!("{n} parked polls of {qid}"),
        )
        .await;
    }
}

impl TestServer {
    /// A thread the owner leaves (in a browser of the owner's) on version 1
    /// of `aid` and sends to the agent; returns the thread view.
    pub async fn thread_as_owner(&self, aid: &str, body: &str) -> serde_json::Value {
        let form = reqwest::multipart::Form::new()
            .text("anchor", element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", "1");
        let res = self
            .client
            .post(format!("{}/api/artifacts/{aid}/threads", self.base))
            .header("cookie", self.owner_cookie())
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        let t: serde_json::Value = res.json().await.unwrap();
        let tid = t["thread"]["id"].as_str().unwrap();
        let res = self
            .client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/send",
                self.base
            ))
            .header("cookie", self.owner_cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }

    /// An agent reply of session `sid` on thread `tid` of `aid`, through the
    /// comments route; returns the comment.
    pub async fn reply_as_agent(
        &self,
        sid: &str,
        aid: &str,
        tid: &str,
        body: &str,
    ) -> serde_json::Value {
        let res = self
            .authed(self.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/comments",
                self.base
            )))
            .header("x-clax-session", sid)
            .json(&serde_json::json!({"body": body, "author_kind": "agent"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
        res.json::<serde_json::Value>().await.unwrap()["comment"].clone()
    }

    /// `PUT /api/viewers/me/looked` as a browser of the owner's; asserts a
    /// success.
    pub async fn look_as_owner(&self, aid: &str, tids: &[&str]) {
        let res = self
            .client
            .put(format!("{}/api/viewers/me/looked", self.base))
            .header("cookie", self.owner_cookie())
            .json(&serde_json::json!({"artifact_id": aid, "thread_ids": tids}))
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
    }

    /// `PUT /api/sessions/<sid>/working/<aid>` with `body`; asserts 200.
    pub async fn put_working(&self, sid: &str, aid: &str, body: serde_json::Value) {
        let res = self
            .authed(
                self.client
                    .put(format!("{}/api/sessions/{sid}/working/{aid}", self.base)),
            )
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    }

    /// `DELETE /api/sessions/<sid>/working/<aid>`: the agent says it is
    /// done; asserts 200.
    pub async fn delete_working(&self, sid: &str, aid: &str) {
        let res = self
            .authed(
                self.client
                    .delete(format!("{}/api/sessions/{sid}/working/{aid}", self.base)),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    }

    /// Moves the working clock `secs` forward and sweeps (the debug skew
    /// route); asserts 200.
    pub async fn skew_working(&self, secs: i64) {
        let res = self
            .post_json("/api/_test/working/skew", serde_json::json!({"secs": secs}))
            .await;
        assert_eq!(res.status(), 200);
    }

    /// Deletes thread `tid` of `aid` as the owner (the token); asserts a
    /// success.
    pub async fn delete_thread_as_owner(&self, aid: &str, tid: &str) {
        let res = self
            .authed(
                self.client
                    .delete(format!("{}/api/artifacts/{aid}/threads/{tid}", self.base)),
            )
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
    }
}

impl TestServer {
    /// Returns once the agent gate's state satisfies `f`; panics past the
    /// [`HANG`](crate::HANG) guard.
    pub async fn wait_gate(
        &self,
        what: &str,
        f: impl Fn(clax_http::agent::GateState) -> bool,
    ) -> clax_http::agent::GateState {
        self.agent_gate
            .wait_until(f, crate::HANG)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "{what}: not within {:?} (hang guard): {:?}",
                    crate::HANG,
                    self.agent_gate.state()
                )
            })
    }

    /// `GET /api/_test/agent/gate`: what the agent gate holds.
    pub async fn gate(&self) -> serde_json::Value {
        let res = self.get_authed("/api/_test/agent/gate").await;
        assert_eq!(res.status(), 200);
        res.json().await.unwrap()
    }

    /// Registers a live session with parent PID `pid`; the session.
    pub async fn register_session_pid(&self, harness: &str, hsid: &str, pid: u32) -> String {
        let res = self
            .post_json(
                "/api/sessions",
                serde_json::json!({"harness": harness, "harness_session_id": hsid,
                                   "cwd": "/w", "parent_pid": pid}),
            )
            .await;
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["session"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The ID of an artifact the owner published, made for the asking.
    pub async fn any_artifact(&self) -> String {
        self.publish("any", &[("index.html", "<p>any</p>")]).await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}
