//! The fleet: the instances this process serves, how a request finds the
//! process that serves its instance, and the background work that keeps
//! them in step with the catalog (see the crate docs for the whole design).
//!
//! # Routing
//!
//! A request's `Host` names its instance ([`Catalog::resolve`]). An
//! instance this process runs answers at once, in process. A paused one
//! answers 503. Otherwise its shard decides: an instance of this process's
//! shard starts here on its first request, and one of another shard is
//! forwarded to the process the peers name ([`Peers`]).
//!
//! A forwarded request carries its hop count ([`HOP_HEADER`]), its
//! instance's name ([`INSTANCE_HEADER`]) and the fleet's peer key
//! ([`KEY_HEADER`]); without the key, the first two are a client's and are
//! ignored. The process a forward reaches serves that instance or answers
//! 503, whatever its own catalog makes of the `Host`. One forwarded twice
//! is answered where it lands, so no request circles. An instance being
//! stopped here is refused 503 until the stop ends. A process that is
//! draining starts nothing.
//!
//! A 503 says only that the instance is unavailable; why goes to the log.
//!
//! # Background
//!
//! [`Fleet::tick`] reloads the catalog: an instance gone, paused, or whose
//! config or shard changed is stopped; its next request starts it as the
//! catalog says.
//!
//! # Stopping
//!
//! A stop runs on a task of its own. [`Fleet::stop`] waits for it at most
//! [`STOP_TIMEOUT`]; a stop that takes longer goes on, and the instance's
//! slot refuses every start until it ends, so a store left running is
//! never opened twice. A stop waits for a start under way to finish
//! first, except in [`Fleet::drain`], which waits for it at most
//! [`STOP_TIMEOUT`] and leaves it to the process's end.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::Home;
use clax_core::embedded::FileTree;
use clax_server::core::{Core, CoreConfig};
use tower::ServiceExt;
use tracing::Instrument;

use crate::catalog::{Catalog, Instance};
use crate::proxy::{HOP_HEADER, INSTANCE_HEADER, KEY_HEADER, Proxy, key_matches};

/// Hops a request may make: one forwarded this many times is answered
/// where it lands.
pub const MAX_HOPS: u8 = 2;

/// How long after an instance's start failed its next start may be tried;
/// doubled for each failure in a row, up to [`START_BACKOFF_MAX`].
/// Requests meanwhile are answered 503 without a start, so a persistent
/// fault (a failing migration, a wrong `CLAX_KEK`, a missing home) costs
/// one attempt per backoff, not one per request.
pub const START_BACKOFF: Duration = Duration::from_secs(2);

/// The longest wait between two starts of an instance that keeps failing.
pub const START_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// How long [`Fleet::stop`] waits for an instance to stop.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// The name of the file in an instance's home that records the public
/// origin it was made for.
pub const ORIGIN_FILE: &str = "fleet-origin";

/// This process's shard: an instance runs on the process whose ordinal is
/// its shard, reached where `peers` says.
pub struct Shard {
    pub ordinal: u32,
    pub peers: Peers,
}

/// Where each shard's process is reached.
pub enum Peers {
    /// `host:port` with `{shard}` replaced by the ordinal: a StatefulSet's
    /// pods behind a headless Service (`clax-{shard}.clax-peers:7480`).
    Template(String),
    /// Each shard's `host:port`, by ordinal.
    List(Vec<String>),
}

impl Peers {
    /// Where shard `shard`'s process is reached, when it is known.
    pub fn addr(&self, shard: u32) -> Option<String> {
        match self {
            Peers::Template(t) => Some(t.replace("{shard}", &shard.to_string())),
            Peers::List(l) => l.get(shard as usize).cloned(),
        }
    }
}

/// What is wrong with `peers` for a fleet of `shards` whose process
/// `pod` is shard `ordinal`: an error when a template names one host for
/// every shard of several, or when what it names for this shard is not a
/// `host:port` (a scheme, a path, no port: a forward would reach no
/// process), and a warning when this shard's host, a name not an address,
/// does not begin with the pod's name (a StatefulSet renamed by a
/// kustomize namePrefix, the template left as it was).
fn check_peers(
    peers: &Peers,
    shards: u32,
    ordinal: u32,
    pod: &str,
) -> Result<Option<String>, String> {
    let Peers::Template(t) = peers else {
        return Ok(None);
    };
    if shards > 1 && !t.contains("{shard}") {
        return Err(format!(
            "the peer template {t} (CLAX_FLEET_PEER) has no {{shard}}, so every shard's forwards would reach one host"
        ));
    }
    let Some(addr) = peers.addr(ordinal) else {
        return Ok(None);
    };
    let authority = addr
        .parse::<axum::http::uri::Authority>()
        .ok()
        .filter(|a| a.port_u16().is_some() && !a.as_str().contains('@'));
    let Some(authority) = authority else {
        return Err(format!(
            "the peer template {t} (CLAX_FLEET_PEER) names {addr} for shard {ordinal}, which is not host:port: give it as clax-{{shard}}.clax-peers:7480, with the port and without a scheme"
        ));
    };
    let host = authority.host();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.parse::<IpAddr>().is_ok() || host.eq_ignore_ascii_case("localhost") {
        return Ok(None);
    }
    let first = host.split('.').next().unwrap_or(host);
    if first.eq_ignore_ascii_case(pod) {
        return Ok(None);
    }
    Ok(Some(format!(
        "the peer template (CLAX_FLEET_PEER) reaches this shard at {addr}, which is not this pod, {pod}: forwards to the other shards will fail; give the template the StatefulSet's pod names"
    )))
}

/// Instance secret `var`. With a secrets directory `dir`, the file
/// `<dir>/<var>` less surrounding whitespace, and nothing when there is no
/// such file (or it is empty): the directory is the Secret as it is now, so
/// a key removed from it is gone, whatever the environment the process
/// started with holds. A file that cannot be read is logged and counts as
/// none. Without a directory, `env`'s.
fn read_secret(
    dir: Option<&std::path::Path>,
    var: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let Some(dir) = dir else {
        return env(var);
    };
    if !var.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let path = dir.join(var);
    match std::fs::read_to_string(&path) {
        Ok(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "an instance's secret file cannot be read; the instance starts without it");
            None
        }
    }
}

/// Refuses a secrets directory that is not one: every instance secret
/// would read as absent, with nothing else said.
fn check_secrets_dir(dir: Option<&std::path::Path>) -> anyhow::Result<()> {
    if let Some(dir) = dir {
        anyhow::ensure!(
            dir.is_dir(),
            "the secrets directory {} (CLAX_FLEET_SECRETS_DIR) is not a directory: mount the clax-secrets Secret there, or leave it unset to read instances' secrets from the environment",
            dir.display()
        );
    }
    Ok(())
}

/// What a fleet runs with.
pub struct FleetConfig {
    /// This process's name: the pod's.
    pub pod: String,
    /// Where the other processes reach this one (`host:port`).
    pub advertise: String,
    /// The instances' homes are `<data_dir>/instances/<name>`, on this
    /// process's volume, as `clax serve` keeps one.
    pub data_dir: PathBuf,
    /// The catalog file, reloaded each tick.
    pub catalog: PathBuf,
    /// The version `/status` reports and each instance's `/healthz`.
    pub version: &'static str,
    /// The web UI every instance serves.
    pub web: Arc<dyn FileTree>,
    pub shard: Shard,
    /// The key-encryption key every instance's sealed secrets are under,
    /// written to a home that has none; a home that holds another key is
    /// never overwritten (the instance does not start). `None` leaves each
    /// home to make its own, which its volume keeps.
    pub kek: Option<[u8; 32]>,
    /// The key forwards between this fleet's processes carry.
    pub peer_key: String,
    /// Where instances' secrets are read, each from the file named as its
    /// variable (a mounted Secret, which the kubelet keeps current, so an
    /// instance started later reads a key added since, and not one removed
    /// since); the environment only when absent.
    pub secrets_dir: Option<PathBuf>,
    /// The port the instances are told they serve on.
    pub port: u16,
    /// The clock a failed start's backoff is timed by.
    pub clock: Arc<dyn clax_core::working::Clock>,
}

/// An instance running here.
struct Running {
    core: Option<Core>,
    /// The catalog config it started with.
    config: String,
}

/// One instance's place in this process: its router while it runs (read
/// without waiting on a start or a stop), and the lock starts and stops
/// take.
#[derive(Default)]
struct Slot {
    router: RwLock<Option<axum::Router>>,
    running: tokio::sync::Mutex<Option<Running>>,
    /// A stop is under way: no request may start it again meanwhile.
    stopping: AtomicBool,
    /// Its last start failed, and no start since succeeded.
    failed: Mutex<Option<Failed>>,
}

/// A start that failed: the next is not tried before its backoff ends.
struct Failed {
    /// The catalog entry it failed with: another entry is tried at once.
    instance: Instance,
    /// When it failed, in milliseconds on [`FleetConfig::clock`].
    at_ms: i64,
    /// Starts that failed in a row.
    failures: u32,
    error: String,
}

impl Failed {
    /// How long after it the next start waits: [`START_BACKOFF`], doubled
    /// for each failure in a row after the first, at most
    /// [`START_BACKOFF_MAX`].
    fn backoff(&self) -> Duration {
        START_BACKOFF
            .saturating_mul(1 << self.failures.saturating_sub(1).min(16))
            .min(START_BACKOFF_MAX)
    }

    /// The milliseconds left of its backoff at `now_ms`.
    fn left_ms(&self, now_ms: i64) -> i64 {
        let wait = i64::try_from(self.backoff().as_millis()).unwrap_or(i64::MAX);
        (self.at_ms.saturating_add(wait) - now_ms).max(0)
    }
}

/// Where a request goes.
pub enum Route {
    Local(axum::Router),
    Forward(String),
    /// Refused 503; the reason goes to the log, never to the client.
    Unavailable(String),
}

/// What the fleet counts, served on `/metrics`.
#[derive(Default)]
struct Metrics {
    local: AtomicU64,
    forwarded: AtomicU64,
    unavailable: AtomicU64,
    forward_errors: AtomicU64,
    starts: AtomicU64,
    start_failures: AtomicU64,
    stops: AtomicU64,
    catalog_errors: AtomicU64,
}

/// The catalog's reload state, for `/status`.
#[derive(Default)]
struct CatalogState {
    loaded_at: String,
    error: Option<String>,
}

/// The fleet (see the [module docs](self)).
pub struct Fleet {
    cfg: FleetConfig,
    proxy: Proxy,
    catalog: RwLock<Arc<Catalog>>,
    catalog_state: Mutex<CatalogState>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    draining: AtomicBool,
    metrics: Metrics,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn unavailable() -> Response {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::RETRY_AFTER, "2")
        .body(Body::from("this Clax instance is unavailable; retry\n"))
        .expect("a static response")
}

fn now_stamp() -> String {
    clax_core::time::stamp_now()
}

impl Fleet {
    /// A fleet over the catalog at `cfg.catalog`.
    ///
    /// # Errors
    /// The catalog cannot be read or is invalid.
    pub fn new(cfg: FleetConfig) -> anyhow::Result<Arc<Fleet>> {
        let catalog = Catalog::load(&cfg.catalog).map_err(|e| {
            anyhow::anyhow!("{e}\n(the catalog in effect on the other processes stays; fix the ConfigMap, or check it first with clax-fleet check-catalog)")
        })?;
        match check_peers(
            &cfg.shard.peers,
            catalog.shards,
            cfg.shard.ordinal,
            &cfg.pod,
        ) {
            Err(e) => anyhow::bail!(e),
            Ok(Some(warning)) => tracing::warn!("{warning}"),
            Ok(None) => {}
        }
        check_secrets_dir(cfg.secrets_dir.as_deref())?;
        std::fs::create_dir_all(cfg.data_dir.join("instances"))?;
        let proxy = Proxy::new(&cfg.peer_key);
        let fleet = Arc::new(Fleet {
            cfg,
            proxy,
            catalog: RwLock::new(Arc::new(catalog)),
            catalog_state: Mutex::new(CatalogState {
                loaded_at: now_stamp(),
                error: None,
            }),
            slots: Mutex::default(),
            draining: AtomicBool::new(false),
            metrics: Metrics::default(),
        });
        fleet.check_ordinal();
        Ok(fleet)
    }

    /// Logs when this shard's ordinal is past the catalog's shards: it then
    /// serves nothing of its own, only forwards.
    fn check_ordinal(&self) {
        let ordinal = self.cfg.shard.ordinal;
        let n = self.catalog().shards;
        if ordinal >= n {
            tracing::warn!(
                ordinal,
                shards = n,
                "this shard is past the catalog's shards: it keeps no instance; raise shards in the catalog"
            );
        }
    }

    /// The catalog in effect.
    pub fn catalog(&self) -> Arc<Catalog> {
        self.catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The names of the instances running here, sorted.
    pub fn running(&self) -> Vec<String> {
        let mut names: Vec<String> = lock(&self.slots)
            .iter()
            .filter(|(_, s)| s.router.read().unwrap_or_else(|e| e.into_inner()).is_some())
            .map(|(n, _)| n.clone())
            .collect();
        names.sort();
        names
    }

    /// Whether this process should get requests: it is not draining.
    pub fn ready(&self) -> bool {
        !self.draining.load(Ordering::SeqCst)
    }

    /// Whether the peer key is `given`'s.
    pub fn is_peer(&self, headers: &HeaderMap) -> bool {
        headers
            .get(KEY_HEADER)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|k| key_matches(&self.cfg.peer_key, k))
    }

    fn slot(&self, name: &str) -> Arc<Slot> {
        lock(&self.slots)
            .entry(name.to_string())
            .or_default()
            .clone()
    }

    fn local_router(&self, name: &str) -> Option<axum::Router> {
        let slot = lock(&self.slots).get(name).cloned()?;
        let r = slot.router.read().unwrap_or_else(|e| e.into_inner());
        r.clone()
    }

    /// Answers `req`, a request for `inst` that has made `hops` hops: here,
    /// forwarded to the process that serves it, or 503.
    pub async fn handle(self: &Arc<Self>, inst: &Instance, hops: u8, req: Request) -> Response {
        let span = tracing::info_span!("request", instance = %inst.name);
        async move {
            match self.route(inst, hops).await {
                Route::Local(router) => {
                    self.metrics.local.fetch_add(1, Ordering::Relaxed);
                    match router.oneshot(req).await {
                        Ok(r) => r,
                        Err(never) => match never {},
                    }
                }
                Route::Forward(addr) => {
                    self.metrics.forwarded.fetch_add(1, Ordering::Relaxed);
                    let resp = self.proxy.forward(&addr, hops + 1, &inst.name, req).await;
                    if resp.status() == StatusCode::BAD_GATEWAY {
                        self.metrics.forward_errors.fetch_add(1, Ordering::Relaxed);
                    }
                    resp
                }
                Route::Unavailable(why) => {
                    self.metrics.unavailable.fetch_add(1, Ordering::Relaxed);
                    tracing::info!(%why, "refused 503");
                    unavailable()
                }
            }
        }
        .instrument(span)
        .await
    }

    /// Where a request for `inst` that has made `hops` hops goes (see the
    /// [module docs](self)).
    pub async fn route(self: &Arc<Self>, inst: &Instance, hops: u8) -> Route {
        if let Some(r) = self.local_router(&inst.name) {
            return Route::Local(r);
        }
        if inst.paused {
            return Route::Unavailable(format!("instance {} is paused", inst.name));
        }
        let can_forward = hops < MAX_HOPS;
        let draining = self.draining.load(Ordering::SeqCst);
        let Shard { ordinal, peers } = &self.cfg.shard;
        let shard = inst.shard;
        if shard == *ordinal {
            if draining {
                return Route::Unavailable("this process is stopping".into());
            }
            return self.start(inst).await;
        }
        if can_forward && let Some(addr) = peers.addr(shard) {
            return Route::Forward(addr);
        }
        Route::Unavailable(format!(
            "instance {} is on shard {shard}, and this is shard {ordinal}",
            inst.name
        ))
    }

    /// Starts `inst` here, unless it already runs.
    async fn start(self: &Arc<Self>, inst: &Instance) -> Route {
        let slot = self.slot(&inst.name);
        let mut running = slot.running.lock().await;
        if let Some(r) = running.as_ref().and_then(|r| r.core.as_ref()) {
            return Route::Local(r.router());
        }
        // Checked under the slot's lock: a drain stops every slot after
        // taking this lock, so a start either sees it or is stopped by it.
        if self.draining.load(Ordering::SeqCst) {
            return Route::Unavailable("this process is stopping".into());
        }
        if slot.stopping.load(Ordering::SeqCst) {
            return Route::Unavailable(format!("instance {} is still stopping here", inst.name));
        }
        // The catalog in effect now, not the one the request was matched
        // in: a reload while this start waited has the last word.
        let Some(inst) = self.catalog().get(&inst.name).cloned() else {
            return Route::Unavailable(format!("instance {} left the catalog", inst.name));
        };
        if inst.paused {
            return Route::Unavailable(format!("instance {} is paused", inst.name));
        }
        let inst = &inst;
        if let Some(why) = self.backing_off(&slot, inst) {
            return Route::Unavailable(why);
        }
        let began = Instant::now();
        match self.open(inst).await {
            Ok(r) => {
                let router = r.core.as_ref().expect("just started").router();
                *slot.router.write().unwrap_or_else(|e| e.into_inner()) = Some(router.clone());
                *running = Some(r);
                *lock(&slot.failed) = None;
                self.metrics.starts.fetch_add(1, Ordering::Relaxed);
                tracing::info!(
                    instance = %inst.name,
                    millis = began.elapsed().as_millis() as u64,
                    "instance started"
                );
                Route::Local(router)
            }
            Err(e) => {
                self.metrics.start_failures.fetch_add(1, Ordering::Relaxed);
                let error = format!("{e:#}");
                let (failures, backoff) = {
                    let mut failed = lock(&slot.failed);
                    let failures = failed
                        .as_ref()
                        .filter(|f| f.instance == *inst)
                        .map_or(1, |f| f.failures.saturating_add(1));
                    let f = Failed {
                        instance: inst.clone(),
                        at_ms: self.now_ms(),
                        failures,
                        error: error.clone(),
                    };
                    let backoff = f.backoff();
                    *failed = Some(f);
                    (failures, backoff)
                };
                tracing::error!(
                    instance = %inst.name,
                    error = %error,
                    failures,
                    retry_in_secs = backoff.as_secs(),
                    "instance did not start"
                );
                Route::Unavailable(format!("instance {} did not start", inst.name))
            }
        }
    }

    fn now_ms(&self) -> i64 {
        self.cfg.clock.now().timestamp_millis()
    }

    /// Why `inst` is not started now, when its last start failed with the
    /// same catalog entry and the failure's backoff has not ended. A failure
    /// under another entry is forgotten: the catalog's change is tried at
    /// once.
    fn backing_off(&self, slot: &Slot, inst: &Instance) -> Option<String> {
        let mut failed = lock(&slot.failed);
        let f = failed.as_ref()?;
        if f.instance != *inst {
            *failed = None;
            return None;
        }
        let left = f.left_ms(self.now_ms());
        (left > 0).then(|| {
            format!(
                "instance {} failed to start ({} in a row; next try in {} ms): {}",
                inst.name, f.failures, left, f.error
            )
        })
    }

    /// The instances whose last start failed, with why and when the next
    /// start may be tried, for `/status`.
    fn failing(&self) -> serde_json::Value {
        let now = self.now_ms();
        let slots: Vec<(String, Arc<Slot>)> = lock(&self.slots)
            .iter()
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect();
        let mut out = serde_json::Map::new();
        for (name, slot) in slots {
            if let Some(f) = &*lock(&slot.failed) {
                out.insert(
                    name,
                    serde_json::json!({
                        "error": f.error,
                        "failures": f.failures,
                        "retry_in_ms": f.left_ms(now),
                    }),
                );
            }
        }
        serde_json::Value::Object(out)
    }

    /// Prepares `inst`'s home (its config, its origin, the key-encryption
    /// key) and starts its core.
    async fn open(&self, inst: &Instance) -> anyhow::Result<Running> {
        let home = Home::at(self.cfg.data_dir.join("instances").join(&inst.name));
        // The home is the instance: one missing here was never made here, or
        // was moved away. Only a new instance makes one.
        if !home.db_path().exists() && !inst.create {
            anyhow::bail!(
                "instance {} has no home on this shard ({}); copy its home here (docs/kubernetes.md, Moving an instance), or set create = true for a new instance",
                inst.name,
                home.root().display()
            );
        }
        if !home.db_path().exists() {
            tracing::warn!(
                instance = %inst.name,
                home = %home.root().display(),
                "making a new, empty home for the instance (create = true); remove create = true from its catalog entry once it has started"
            );
        }
        home.ensure_dirs()?;
        claim_origin_file(&home, &inst.name, &inst.origin)?;
        write_if_changed(
            &home.root().join("config.toml"),
            inst.config.as_bytes(),
            0o644,
        )?;
        if let Some(kek) = &self.cfg.kek {
            place_kek(&home, &inst.name, kek)?;
        }
        let config = clax_core::config::HomeConfig::load(home.root())?;
        let hosted_cfg = config
            .hosted()?
            .ok_or_else(|| anyhow::anyhow!("instance {} has no [hosted]", inst.name))?;
        let env = |var: &str| std::env::var(var).ok();
        let secret = |var: &str| read_secret(self.cfg.secrets_dir.as_deref(), var, &env);
        let hosted =
            clax_http::hosted::Hosted::from_config_with(&hosted_cfg, &secret).ok_or_else(|| {
                anyhow::anyhow!(
                    "instance {}'s public_origin {} is not usable",
                    inst.name,
                    hosted_cfg.public_origin
                )
            })?;
        if !hosted.setup_open() {
            tracing::warn!(
                instance = %inst.name,
                env = %hosted_cfg.setup_secret_env,
                secrets_dir = ?self.cfg.secrets_dir,
                "no setup secret (in the secrets directory, or in this process's environment when there is none), so /setup stays closed; add it to the clax-secrets Secret, and pause and unpause the instance once the pod has it (or restart the pods, when the Secret is not mounted)"
            );
        }
        let core_cfg = CoreConfig {
            home: home.clone(),
            version: self.cfg.version,
            reap_interval: Duration::from_secs(60),
            // An instance on a server has no agent of the owner's to push to.
            codex: clax_realtime::push::CodexPush::from_env(Some(Default::default()), None),
            sample: Arc::new(clax_sample::Sampler::from_home(home.root(), |k| {
                std::env::var(k).ok()
            })),
            web: self.cfg.web.clone(),
            hosted: Some(Arc::new(hosted)),
            require_credential: false,
            bind: IpAddr::from([0, 0, 0, 0]),
            port: self.cfg.port,
            embed: None,
            // Each person new to the instance is seeded with the web build's
            // quickstart, as on the Worker.
            quickstart: true,
            catalog_url: None,
        };
        Ok(Running {
            core: Some(Core::start(core_cfg, &mut |_| {}).await?),
            config: inst.config.clone(),
        })
    }

    /// Stops instance `name` when it runs here, saying `why` in the log. Waits for it at most
    /// [`STOP_TIMEOUT`]; a stop that takes longer goes on (see the
    /// [module docs](self#stopping)).
    pub async fn stop(self: &Arc<Self>, name: &str, why: &str) {
        self.stop_after(name, why, None).await;
    }

    /// [`Fleet::stop`], waiting for a start under way at most `start_wait`
    /// when given: past it, the start goes on and the instance is left
    /// running.
    async fn stop_after(self: &Arc<Self>, name: &str, why: &str, start_wait: Option<Duration>) {
        let Some(slot) = lock(&self.slots).get(name).cloned() else {
            return;
        };
        slot.stopping.store(true, Ordering::SeqCst);
        let mut running = match start_wait {
            None => slot.running.lock().await,
            Some(wait) => match tokio::time::timeout(wait, slot.running.lock()).await {
                Ok(running) => running,
                Err(_) => {
                    tracing::warn!(
                        instance = %name,
                        "the instance is still starting; this process stops without it"
                    );
                    return;
                }
            },
        };
        *slot.router.write().unwrap_or_else(|e| e.into_inner()) = None;
        let Some(r) = running.take() else {
            slot.stopping.store(false, Ordering::SeqCst);
            return;
        };
        drop(running);
        self.metrics.stops.fetch_add(1, Ordering::Relaxed);
        let (name_owned, why_owned) = (name.to_string(), why.to_string());
        let done = tokio::spawn(async move {
            let mut r = r;
            if let Some(core) = r.core.take() {
                core.stop().await;
            }
            drop(r);
            slot.stopping.store(false, Ordering::SeqCst);
            tracing::info!(instance = %name_owned, why = %why_owned, "instance stopped");
        });
        if tokio::time::timeout(STOP_TIMEOUT, done).await.is_err() {
            tracing::warn!(
                instance = %name,
                "the instance did not stop in time; it goes on stopping, and starts here wait for it"
            );
        }
    }

    /// The background work of one tick (see the [module docs](self)).
    pub async fn tick(self: &Arc<Self>) {
        let stops = self.reload().await.into_iter().map(|(name, why)| {
            let fleet = self.clone();
            async move { fleet.stop(&name, why).await }
        });
        futures::future::join_all(stops).await;
    }

    /// Reads the catalog again, and answers each running instance it no
    /// longer has, or that is paused, or whose config or shard changed,
    /// with why it must stop. It compares every tick, whether or not the
    /// catalog changed: an instance started from the catalog a reload
    /// replaced stops too. A catalog that does not load, or that moves an
    /// instance to another shard while `create = true`, is logged once per
    /// distinct error and kept out of effect.
    async fn reload(&self) -> Vec<(String, &'static str)> {
        let loaded = Catalog::load(&self.cfg.catalog).and_then(|next| {
            match moved_with_create(&self.catalog(), &next) {
                Some(e) => Err(format!("{}: {e}", self.cfg.catalog.display())),
                None => Ok(next),
            }
        });
        match loaded {
            Ok(next) => {
                let changed = *self.catalog() != next;
                if changed {
                    tracing::info!(instances = next.instances.len(), "catalog reloaded");
                    *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(next);
                    self.check_ordinal();
                }
                let mut s = lock(&self.catalog_state);
                if changed || s.error.is_some() {
                    s.loaded_at = now_stamp();
                    s.error = None;
                }
            }
            Err(e) => {
                self.metrics.catalog_errors.fetch_add(1, Ordering::Relaxed);
                let mut s = lock(&self.catalog_state);
                if s.error.as_deref() != Some(&e) {
                    tracing::warn!(error = %e, "the catalog did not reload; keeping the one in effect");
                    s.error = Some(e);
                }
            }
        }
        let catalog = self.catalog();
        let mut stops = Vec::new();
        for name in self.running() {
            let slot = self.slot(&name);
            let Ok(running) = slot.running.try_lock() else {
                continue;
            };
            let config = running.as_ref().map(|r| r.config.clone());
            drop(running);
            let why = match (catalog.get(&name), config) {
                (None, _) => Some("it left the catalog"),
                (Some(i), _) if i.paused => Some("it is paused"),
                (Some(i), Some(c)) if i.config != c => Some("its config changed"),
                (Some(i), _) if i.shard != self.cfg.shard.ordinal => Some("its shard changed"),
                _ => None,
            };
            if let Some(why) = why {
                stops.push((name, why));
            }
        }
        stops
    }

    /// Starts nothing more here, and answers not ready, while still serving
    /// the instances running here.
    pub fn begin_draining(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    /// Drains: starts nothing more, and stops every instance here. A start
    /// under way is waited for at most [`STOP_TIMEOUT`], so a slow one (a
    /// long migration) cannot hold the process past its grace period.
    pub async fn drain(self: &Arc<Self>) {
        self.begin_draining();
        // Every slot, not just the running: a start under way holds its
        // slot's lock, and its stop waits for it.
        let names: Vec<String> = lock(&self.slots).keys().cloned().collect();
        let stops = names
            .into_iter()
            .map(|name| {
                let fleet = self.clone();
                async move {
                    fleet
                        .stop_after(&name, "this process is stopping", Some(STOP_TIMEOUT))
                        .await;
                }
            })
            .collect::<Vec<_>>();
        futures::future::join_all(stops).await;
    }

    /// The fleet's answer to every request: the instance's, wherever it
    /// runs.
    pub fn router(self: &Arc<Self>) -> axum::Router {
        axum::Router::new()
            .fallback(dispatch)
            .with_state(self.clone())
    }

    /// This process as `/status` reports it.
    pub fn status(&self) -> serde_json::Value {
        let catalog = self.catalog();
        let state = lock(&self.catalog_state);
        serde_json::json!({
            "pod": self.cfg.pod,
            "advertise": self.cfg.advertise,
            "version": self.cfg.version,
            "commit": clax_core::build_commit(),
            "ordinal": self.cfg.shard.ordinal,
            "shards": catalog.shards,
            "ready": self.ready(),
            "draining": self.draining.load(Ordering::SeqCst),
            "catalog": {
                "path": self.cfg.catalog.display().to_string(),
                "instances": catalog.instances.iter().map(|i| &i.name).collect::<Vec<_>>(),
                "loaded_at": state.loaded_at,
                "error": state.error,
            },
            "running": self.running(),
            "failing": self.failing(),
        })
    }

    /// The fleet's counters and gauges in Prometheus's text format.
    pub fn metrics(&self) -> String {
        let m = &self.metrics;
        let mut out = String::new();
        let mut counter = |name: &str, help: &str, v: &AtomicU64| {
            out.push_str(&format!(
                "# HELP clax_fleet_{name} {help}\n# TYPE clax_fleet_{name} counter\nclax_fleet_{name} {}\n",
                v.load(Ordering::Relaxed)
            ));
        };
        counter(
            "requests_local_total",
            "Requests answered by an instance running here.",
            &m.local,
        );
        counter(
            "requests_forwarded_total",
            "Requests forwarded to the process that runs their instance.",
            &m.forwarded,
        );
        counter(
            "requests_unavailable_total",
            "Requests refused 503.",
            &m.unavailable,
        );
        counter(
            "forward_errors_total",
            "Forwards whose owner did not answer (502).",
            &m.forward_errors,
        );
        counter(
            "instance_starts_total",
            "Instances started here.",
            &m.starts,
        );
        counter(
            "instance_start_failures_total",
            "Instance starts that failed.",
            &m.start_failures,
        );
        counter("instance_stops_total", "Instances stopped here.", &m.stops);
        counter(
            "catalog_reload_failures_total",
            "Catalog reloads that failed.",
            &m.catalog_errors,
        );
        let catalog = self.catalog();
        let failing = lock(&self.catalog_state).error.is_some();
        out.push_str(&format!(
            "# HELP clax_fleet_instances_running Instances running here.\n# TYPE clax_fleet_instances_running gauge\nclax_fleet_instances_running {}\n\
             # HELP clax_fleet_catalog_instances Instances in the catalog in effect.\n# TYPE clax_fleet_catalog_instances gauge\nclax_fleet_catalog_instances {}\n\
             # HELP clax_fleet_catalog_stale The mounted catalog does not load (1), so an older one is in effect.\n# TYPE clax_fleet_catalog_stale gauge\nclax_fleet_catalog_stale {}\n\
             # HELP clax_fleet_ready This process takes requests (1).\n# TYPE clax_fleet_ready gauge\nclax_fleet_ready {}\n",
            self.running().len(),
            catalog.instances.len(),
            u8::from(failing),
            u8::from(self.ready()),
        ));
        out
    }

    /// The admin port's routes: `/healthz` (alive) and `/readyz` (taking
    /// requests: 503 while draining) for the kubelet, `/metrics` (Prometheus: counters and
    /// gauges, no instance's name or data) for whoever the NetworkPolicy
    /// lets reach the port, and `/status` (this process, its catalog and
    /// its instances, as JSON) only to a caller with the peer key (another
    /// process, `clax-fleet status`). No caller is trusted by its address:
    /// under a sidecar mesh every connection comes from loopback.
    pub fn admin_router(self: &Arc<Self>) -> axum::Router {
        self.admin_routes(true)
    }

    /// The admin routes for a listener on this pod's loopback alone
    /// (`kubectl port-forward`, `kubectl exec`): [`Fleet::admin_router`]'s,
    /// with `/status` asking for no key.
    pub fn local_admin_router(self: &Arc<Self>) -> axum::Router {
        self.admin_routes(false)
    }

    fn admin_routes(self: &Arc<Self>, status_needs_key: bool) -> axum::Router {
        use axum::routing::get;
        axum::Router::new()
            .route("/healthz", get(|| async { "ok\n" }))
            .route(
                "/readyz",
                get(|State(f): State<Arc<Fleet>>| async move {
                    if f.ready() {
                        (StatusCode::OK, "ready\n")
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, "not ready\n")
                    }
                }),
            )
            .route(
                "/status",
                get(
                    move |State(f): State<Arc<Fleet>>, headers: HeaderMap| async move {
                        if status_needs_key && !f.is_peer(&headers) {
                            return StatusCode::FORBIDDEN.into_response();
                        }
                        axum::Json(f.status()).into_response()
                    },
                ),
            )
            .route(
                "/metrics",
                get(|State(f): State<Arc<Fleet>>| async move {
                    (
                        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
                        f.metrics(),
                    )
                        .into_response()
                }),
            )
            .with_state(self.clone())
    }
}

/// Why `next` may not replace `prev`, when it gives an instance of `prev`
/// another shard while `create = true`: the new shard, finding no home
/// there, would make an empty one and answer the tenant's hosts with it.
/// `create` is for an instance's first start; a moved instance's home is
/// copied to its new shard first (docs/kubernetes.md, Moving an instance).
fn moved_with_create(prev: &Catalog, next: &Catalog) -> Option<String> {
    next.instances.iter().find_map(|i| {
        let was = prev.get(&i.name)?;
        (i.create && was.shard != i.shard).then(|| {
            format!(
                "instance {}'s shard changes from {} to {} while it has create = true, which would make an empty home on shard {}; remove create = true and move its home (docs/kubernetes.md, Moving an instance)",
                i.name, was.shard, i.shard, i.shard
            )
        })
    })
}

/// Places the fleet's key-encryption key in `home` when it has none, and
/// refuses one that holds another: every secret it sealed depends on it.
fn place_kek(home: &Home, name: &str, kek: &[u8; 32]) -> anyhow::Result<()> {
    let path = home.kek_path();
    match std::fs::read(&path) {
        Ok(held) if held == kek => return Ok(()),
        Ok(held) => {
            let fp = |k: &[u8]| -> String {
                use sha2::Digest;
                sha2::Sha256::digest(k)[..4]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            };
            anyhow::bail!(
                "instance {name}'s home holds a key-encryption key (fingerprint {}) other than CLAX_KEK (fingerprint {}); replacing it would make every secret sealed under it unreadable. Set CLAX_KEK to the home's key ({}), or remove the key and the secrets sealed under it (docs/kubernetes.md, CLAX_KEK)",
                fp(&held),
                fp(kek),
                path.display()
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let dir = path
        .parent()
        .expect("the KEK has a directory")
        .to_path_buf();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    write_if_changed(&path, kek, 0o600)?;
    Ok(())
}

/// Records `origin` in `home` as the public origin the instance's data was
/// made for, and refuses a home made for another: a name the catalog gave
/// again never opens another tenant's data.
fn claim_origin_file(home: &Home, name: &str, origin: &str) -> anyhow::Result<()> {
    let path = home.root().join(ORIGIN_FILE);
    match std::fs::read_to_string(&path) {
        Ok(held) if held.trim() == origin => Ok(()),
        Ok(held) => anyhow::bail!(
            "instance {name}'s home was made for {}, not {origin}; a name is never given to another tenant (docs/kubernetes.md, The catalog)",
            held.trim()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            write_if_changed(&path, format!("{origin}\n").as_bytes(), 0o644)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// The hops `req` has made, as [`HOP_HEADER`] counts them.
fn hops(req: &Request) -> u8 {
    req.headers()
        .get(HOP_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

async fn dispatch(State(fleet): State<Arc<Fleet>>, mut req: Request) -> Response {
    let catalog = fleet.catalog();
    // A client's hop and instance headers are not believed: only a forward
    // carrying the peer key is another process's.
    let peer = fleet.is_peer(req.headers());
    let hops = if peer { hops(&req) } else { 0 };
    let inst = if hops > 0 {
        // A forward names its instance; this process's catalog must have it.
        let name = req
            .headers()
            .get(INSTANCE_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        match catalog.get(name) {
            Some(i) => i,
            None => {
                tracing::info!(instance = %name, "a forward for an instance not in this process's catalog yet");
                return unavailable();
            }
        }
    } else {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .or_else(|| req.uri().authority().map(|a| a.as_str()));
        match catalog.resolve(host) {
            Some(i) => i,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    [(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("text/plain; charset=utf-8"),
                    )],
                    "no Clax instance answers at this host\n",
                )
                    .into_response();
            }
        }
    };
    for h in [HOP_HEADER, INSTANCE_HEADER, KEY_HEADER] {
        req.headers_mut().remove(h);
    }
    fleet.handle(inst, hops, req).await
}

/// Writes `bytes` to `path` with `mode` unless it holds them already, by a
/// temporary file renamed over it.
fn write_if_changed(path: &std::path::Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|b| b == bytes) {
        return Ok(());
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        use std::io::Write;
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut open, mode);
        #[cfg(not(unix))]
        let _ = mode;
        let mut f = open.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_keeps_its_key_and_its_origin() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("h"));
        home.ensure_dirs().unwrap();
        place_kek(&home, "acme", &[1; 32]).unwrap();
        place_kek(&home, "acme", &[1; 32]).unwrap();
        let err = place_kek(&home, "acme", &[2; 32]).unwrap_err().to_string();
        assert!(err.contains("other than CLAX_KEK"), "{err}");
        assert_eq!(std::fs::read(home.kek_path()).unwrap(), [1; 32]);

        claim_origin_file(&home, "acme", "https://acme.test").unwrap();
        claim_origin_file(&home, "acme", "https://acme.test").unwrap();
        let err = claim_origin_file(&home, "acme", "https://other.test")
            .unwrap_err()
            .to_string();
        assert!(err.contains("made for https://acme.test"), "{err}");
    }

    /// With a secrets directory, an instance's secret is the file of its
    /// variable's name there, a mounted Secret the kubelet keeps current,
    /// less the newline a file ends in, and nothing without one: a key
    /// removed from the Secret is not read from the environment the pod
    /// started with. Without a directory, the environment's.
    #[test]
    fn a_secrets_directory_is_the_only_source_of_instance_secrets() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("secrets");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("CLAX_SETUP_SECRET_ACME"), "from-file\n").unwrap();
        std::fs::write(dir.join("EMPTY"), "\n").unwrap();
        std::fs::create_dir(dir.join("A_DIRECTORY")).unwrap();
        std::fs::write(root.path().join("OUTSIDE"), "x").unwrap();
        let env = |var: &str| (var != "UNSET").then(|| format!("env-{var}"));
        let read = |var: &str| read_secret(Some(&dir), var, &env);
        assert_eq!(read("CLAX_SETUP_SECRET_ACME").as_deref(), Some("from-file"));
        // Removed from the Secret (or never in it): none, though the
        // environment has it.
        assert_eq!(read("CLAX_SETUP_SECRET_REMOVED"), None);
        assert_eq!(read("EMPTY"), None);
        // A file that cannot be read is none too.
        assert_eq!(read("A_DIRECTORY"), None);
        assert_eq!(read("UNSET"), None);
        // No variable name reaches outside the directory.
        assert_eq!(read("../OUTSIDE"), None);
        assert_eq!(
            read_secret(None, "CLAX_SETUP_SECRET_ACME", &env).as_deref(),
            Some("env-CLAX_SETUP_SECRET_ACME")
        );
        assert!(check_secrets_dir(Some(&dir)).is_ok());
        assert!(check_secrets_dir(None).is_ok());
        let err = check_secrets_dir(Some(&root.path().join("missing")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("CLAX_FLEET_SECRETS_DIR"), "{err}");
    }

    /// A peer template is refused without `{shard}` when there is more than
    /// one shard, or when it is not `host:port` (no port, a scheme, a
    /// path), and a template whose host for this shard does not begin with
    /// this pod's name (a StatefulSet renamed by a namePrefix) is named in
    /// a warning.
    #[test]
    fn a_peer_template_is_checked_against_the_shards_and_the_pod() {
        let t = |s: &str| Peers::Template(s.into());
        let ok = t("clax-{shard}.clax-peers:7480");
        assert_eq!(check_peers(&ok, 3, 1, "clax-1"), Ok(None));
        let err = check_peers(&t("clax.clax-peers:7480"), 2, 0, "clax-0").unwrap_err();
        assert!(
            err.contains("CLAX_FLEET_PEER") && err.contains("{shard}"),
            "{err}"
        );
        assert_eq!(check_peers(&t("localhost:7480"), 1, 0, "fleet"), Ok(None));
        let warn = check_peers(&ok, 3, 1, "prod-clax-1").unwrap().unwrap();
        assert!(
            warn.contains("clax-1.clax-peers:7480") && warn.contains("prod-clax-1"),
            "{warn}"
        );
        assert_eq!(check_peers(&t("127.0.0.1:748{shard}"), 3, 1, "x"), Ok(None));
        assert_eq!(check_peers(&t("[::1]:748{shard}"), 3, 1, "x"), Ok(None));
        let list = Peers::List(vec!["a:1".into(), "b:1".into()]);
        assert_eq!(check_peers(&list, 2, 0, "x"), Ok(None));
        for bad in [
            "clax-{shard}.clax-peers",
            "http://clax-{shard}.clax-peers:7480",
            "clax-{shard}.clax-peers:7480/",
            "clax-{shard}.clax-peers:port",
            "user@clax-{shard}.clax-peers:7480",
        ] {
            let err = check_peers(&t(bad), 3, 1, "clax-1").unwrap_err();
            assert!(
                err.contains("CLAX_FLEET_PEER") && err.contains("host:port"),
                "{bad}: {err}"
            );
        }
    }

    /// A drain waits for a start under way at most [`STOP_TIMEOUT`], so a
    /// slow start (a long migration) cannot hold the process past its
    /// grace period; a stop the catalog asks for still waits for it.
    #[tokio::test(start_paused = true)]
    async fn a_drain_waits_for_a_start_under_way_only_so_long() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("fleet.toml");
        std::fs::write(
            &catalog,
            "shards = 1\n\n[[instance]]\nname = \"acme\"\nshard = 0\nconfig = '''\n[hosted]\npublic_origin = \"https://acme.test\"\nsetup_secret_env = \"CLAX_SETUP_SECRET_ACME\"\n'''\n",
        )
        .unwrap();
        let fleet = Fleet::new(FleetConfig {
            pod: "clax-0".into(),
            advertise: "127.0.0.1:7480".into(),
            data_dir: dir.path().join("data"),
            catalog,
            version: "test",
            web: Arc::new(clax_core::embedded::MemTree(Default::default())),
            shard: Shard {
                ordinal: 0,
                peers: Peers::Template("127.0.0.1:{shard}".into()),
            },
            kek: None,
            peer_key: "k".into(),
            secrets_dir: None,
            port: 7480,
            clock: Arc::new(clax_core::working::ManualClock::at("2026-01-01T00:00:00Z")),
        })
        .unwrap();
        // A start under way holds its slot's lock.
        let slot = fleet.slot("acme");
        let start = slot.running.lock().await;

        let a_day = Duration::from_secs(86_400);
        let stop = tokio::time::timeout(a_day, fleet.stop("acme", "paused")).await;
        assert!(stop.is_err(), "a catalog's stop waits for the start");
        let drained = tokio::time::timeout(a_day, fleet.drain()).await;
        assert!(drained.is_ok(), "the drain waited for the start for a day");
        drop(start);
    }
}
