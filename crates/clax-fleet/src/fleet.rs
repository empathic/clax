//! The fleet: the instances this process serves, how a request finds the
//! process that serves its instance, and the background work that keeps
//! ownership right (see the crate docs for the whole design).
//!
//! # Routing
//!
//! A request's `Host` names its instance ([`Catalog::resolve`]). An
//! instance this process runs answers at once, in process. Otherwise:
//!
//! - **Shards** (SQLite): the instance's shard is a process ordinal. This
//!   process starts the instances of its own shard on their first request
//!   and forwards the others to the process the peer template names.
//! - **Leases** (PostgreSQL): a live lease names the owner, and the request
//!   is forwarded there. With no live lease, the instance goes to the member
//!   rendezvous hashing prefers ([`coord::preferred`]): this process when it
//!   is preferred, which takes the lease and starts the instance, else the
//!   preferred member, which does.
//!
//! A forwarded request carries its hop count ([`HOP_HEADER`]) and its
//! instance's name ([`INSTANCE_HEADER`]): the process it reaches serves that
//! instance or answers 503, whatever its own catalog makes of the `Host`.
//! One forwarded twice is answered where it lands, so no request circles,
//! and an instance is only ever started on the member hashing prefers. An
//! instance being stopped here (a handover, a drain) still holds its lease
//! until the stop releases it; a request for it meanwhile is refused 503,
//! never started here again. A process that is draining starts nothing.
//!
//! # Background
//!
//! [`Fleet::tick`], every [`coord::HEARTBEAT_EVERY`]: reloads the catalog
//! (an instance gone or whose config changed is stopped; its next request
//! starts it as the catalog says), and on leases heartbeats, renews each
//! held lease (an instance whose lease is lost, or not renewed for
//! [`RENEW_FAILURES`] ticks in a row, is stopped), and hands over every
//! instance rendezvous hashing now prefers another member for, once that
//! member has been alive for [`coord::SETTLE`]: the instance stops here and
//! its lease is released, and its next request starts it there.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::Home;
use clax_core::embedded::FileTree;
use clax_server::core::{Core, CoreConfig};
use tower::ServiceExt;

use crate::catalog::{Catalog, Instance};
use crate::coord::{self, Coordinator};
use crate::pg::{Fence, Pool};
use crate::proxy::{self, HOP_HEADER, INSTANCE_HEADER};

/// Hops a request may make: one forwarded this many times is answered
/// where it lands.
pub const MAX_HOPS: u8 = 2;

/// Ticks in a row a held lease may fail to renew before its instance stops.
pub const RENEW_FAILURES: u32 = 4;

/// How long a forward's target is remembered, sparing the database a
/// lookup per request.
pub const ROUTE_CACHE: Duration = Duration::from_secs(2);

/// How long stopping an instance waits for its store.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Where each instance runs.
pub enum Placement {
    /// The SQLite backend: an instance runs on the process whose ordinal is
    /// its shard, reached where `peers` says.
    Shards { ordinal: u32, peers: Peers },
    /// The PostgreSQL backend: on whichever process holds its lease.
    Leases(Arc<dyn Coordinator>),
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

/// Where an instance keeps its database, content and app data.
pub enum Storage {
    /// Its home on this process's disk, as `clax serve` keeps one.
    Disk,
    /// PostgreSQL, through the page VFS and the content store; the home on
    /// disk holds only what can be lost (its config, the audit journal's
    /// export, caches).
    Postgres(Arc<Pool>),
}

/// What a fleet runs with.
pub struct FleetConfig {
    /// This process's name: the pod's.
    pub pod: String,
    /// Where the other processes reach this one (`host:port`).
    pub advertise: String,
    /// The instances' homes are `<data_dir>/instances/<name>`.
    pub data_dir: PathBuf,
    /// The catalog file, reloaded each tick.
    pub catalog: PathBuf,
    /// On shards, how many there are (each instance names one below it).
    pub shards: Option<u32>,
    /// The version `/healthz` reports.
    pub version: &'static str,
    /// The web UI every instance serves.
    pub web: Arc<dyn FileTree>,
    pub placement: Placement,
    pub storage: Storage,
    /// The key-encryption key every instance's sealed secrets are under,
    /// written to its home; `None` leaves each home to make its own (on
    /// disk that persists; on PostgreSQL it must be given).
    pub kek: Option<[u8; 32]>,
    /// The port the instances are told they serve on.
    pub port: u16,
}

/// An instance running here.
struct Running {
    // Stopped first; the database's VFS registration is dropped after it.
    core: Option<Core>,
    /// On leases, the hold its writes are fenced by, and its renewals that
    /// failed in a row.
    hold: Option<(Arc<Fence>, u32)>,
    /// The catalog config it started with.
    config: String,
    _db: Option<clax_pagevfs::Registered>,
}

/// One instance's place in this process: its router while it runs (read
/// without waiting on a start or a stop), and the lock starts and stops
/// take.
#[derive(Default)]
struct Slot {
    router: RwLock<Option<axum::Router>>,
    running: tokio::sync::Mutex<Option<Running>>,
    /// A stop is under way: its lease is still this process's until the
    /// stop releases it, and no request may start it again meanwhile.
    stopping: AtomicBool,
}

/// Where a request goes.
pub enum Route {
    Local(axum::Router),
    Forward(String),
    Unavailable(String),
}

/// The fleet (see the [module docs](self)).
pub struct Fleet {
    cfg: FleetConfig,
    catalog: RwLock<Arc<Catalog>>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    routes: Mutex<HashMap<String, (Instant, String)>>,
    draining: AtomicBool,
    /// On leases: the last heartbeat reached the database.
    healthy: AtomicBool,
    ticks: AtomicU32,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn unavailable(why: &str) -> Response {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::RETRY_AFTER, "2")
        .body(Body::from(format!("{why}\n")))
        .expect("a static response")
}

impl Fleet {
    /// A fleet over the catalog at `cfg.catalog`.
    ///
    /// # Errors
    /// The catalog cannot be read or is invalid.
    pub fn new(cfg: FleetConfig) -> anyhow::Result<Arc<Fleet>> {
        let catalog = Catalog::load(&cfg.catalog, cfg.shards).map_err(anyhow::Error::msg)?;
        std::fs::create_dir_all(cfg.data_dir.join("instances"))?;
        Ok(Arc::new(Fleet {
            cfg,
            catalog: RwLock::new(Arc::new(catalog)),
            slots: Mutex::default(),
            routes: Mutex::default(),
            draining: AtomicBool::new(false),
            healthy: AtomicBool::new(true),
            ticks: AtomicU32::new(0),
        }))
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

    /// Whether this process should get requests: it is not draining, and on
    /// leases its last heartbeat reached the database.
    pub fn ready(&self) -> bool {
        !self.draining.load(Ordering::SeqCst) && self.healthy.load(Ordering::SeqCst)
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

    fn is_stopping(&self, name: &str) -> bool {
        lock(&self.slots)
            .get(name)
            .is_some_and(|s| s.stopping.load(Ordering::SeqCst))
    }

    fn cached_route(&self, name: &str) -> Option<String> {
        let routes = lock(&self.routes);
        let (at, addr) = routes.get(name)?;
        (at.elapsed() < ROUTE_CACHE).then(|| addr.clone())
    }

    fn remember_route(&self, name: &str, addr: &str) {
        lock(&self.routes).insert(name.to_string(), (Instant::now(), addr.to_string()));
    }

    fn forget_route(&self, name: &str) {
        lock(&self.routes).remove(name);
    }

    /// Answers `req`, a request whose `Host` named `inst` and which has
    /// made `hops` hops: here, forwarded to the process that serves it, or
    /// 503.
    pub async fn handle(self: &Arc<Self>, inst: &Instance, hops: u8, req: Request) -> Response {
        match self.route(inst, hops).await {
            Route::Local(router) => match router.oneshot(req).await {
                Ok(r) => r,
                Err(never) => match never {},
            },
            Route::Forward(addr) => {
                let resp = proxy::forward(&addr, hops + 1, &inst.name, req).await;
                if resp.status() == StatusCode::BAD_GATEWAY {
                    self.forget_route(&inst.name);
                }
                resp
            }
            Route::Unavailable(why) => unavailable(&why),
        }
    }

    /// Where a request for `inst` that has made `hops` hops goes (see the
    /// [module docs](self)).
    pub async fn route(self: &Arc<Self>, inst: &Instance, hops: u8) -> Route {
        if let Some(r) = self.local_router(&inst.name) {
            return Route::Local(r);
        }
        let can_forward = hops < MAX_HOPS;
        let draining = self.draining.load(Ordering::SeqCst);
        match &self.cfg.placement {
            Placement::Shards { ordinal, peers } => {
                let shard = inst.shard.unwrap_or(0);
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
            Placement::Leases(coord) => {
                // Only a request no process has forwarded yet: a process a
                // forward reached because it is preferred, or held the lease
                // when the forwarder looked, looks again, so a stale cache
                // never sends a request back where it came from.
                if hops == 0
                    && let Some(addr) = self.cached_route(&inst.name)
                {
                    return Route::Forward(addr);
                }
                match coord.lease(&inst.name).await {
                    Err(e) => return Route::Unavailable(format!("the fleet's database: {e}")),
                    Ok(Some(l)) if l.holder != self.cfg.pod => {
                        if can_forward {
                            self.remember_route(&inst.name, &l.addr);
                            return Route::Forward(l.addr);
                        }
                        return Route::Unavailable(format!(
                            "instance {} is held by {}",
                            inst.name, l.holder
                        ));
                    }
                    // This process's own live lease of an instance it is
                    // stopping (a handover, a drain): it goes where it is
                    // going once the stop releases it, never back here.
                    Ok(Some(_)) if self.is_stopping(&inst.name) => {
                        return Route::Unavailable(format!("instance {} is moving", inst.name));
                    }
                    Ok(_) => {}
                }
                let members = match coord.members().await {
                    Err(e) => return Route::Unavailable(format!("the fleet's database: {e}")),
                    Ok(m) if draining => m.into_iter().filter(|m| m.pod != self.cfg.pod).collect(),
                    Ok(m) => m,
                };
                // Started only where it is preferred: a request out of hops
                // (or one a client marked so) never places it elsewhere.
                if let Some(p) = coord::preferred(&inst.name, &members)
                    && p.pod != self.cfg.pod
                {
                    if can_forward {
                        return Route::Forward(p.addr.clone());
                    }
                    return Route::Unavailable(format!(
                        "instance {} goes to {}, not here",
                        inst.name, p.pod
                    ));
                }
                if draining {
                    return Route::Unavailable("this process is stopping".into());
                }
                match self.start(inst).await {
                    Route::Unavailable(why) if can_forward => match coord.lease(&inst.name).await {
                        Ok(Some(l)) if l.holder != self.cfg.pod => Route::Forward(l.addr),
                        _ => Route::Unavailable(why),
                    },
                    r => r,
                }
            }
        }
    }

    /// Starts `inst` here, unless it already runs: on leases, once its lease
    /// is taken.
    async fn start(self: &Arc<Self>, inst: &Instance) -> Route {
        let slot = self.slot(&inst.name);
        let mut running = slot.running.lock().await;
        if let Some(r) = running.as_ref().and_then(|r| r.core.as_ref()) {
            return Route::Local(r.router());
        }
        // The catalog in effect now, not the one the request was matched
        // in: a reload while this start waited has the last word.
        let Some(inst) = self.catalog().get(&inst.name).cloned() else {
            return Route::Unavailable(format!("instance {} left the catalog", inst.name));
        };
        let inst = &inst;
        let fence = match &self.cfg.placement {
            Placement::Shards { .. } => None,
            Placement::Leases(coord) => {
                match coord
                    .acquire(&inst.name, &self.cfg.pod, &self.cfg.advertise)
                    .await
                {
                    Ok(Some(epoch)) => Some(Fence::new(&inst.name, &self.cfg.pod, epoch)),
                    Ok(None) => {
                        return Route::Unavailable(format!(
                            "instance {} is held by another process",
                            inst.name
                        ));
                    }
                    Err(e) => return Route::Unavailable(format!("the fleet's database: {e}")),
                }
            }
        };
        match self.open(inst, fence.clone()).await {
            Ok(r) => {
                let router = r.core.as_ref().expect("just started").router();
                *slot.router.write().unwrap_or_else(|e| e.into_inner()) = Some(router.clone());
                *running = Some(r);
                self.forget_route(&inst.name);
                tracing::info!(instance = %inst.name, epoch = fence.as_ref().map(|f| f.epoch()), "instance started");
                Route::Local(router)
            }
            Err(e) => {
                tracing::error!(instance = %inst.name, error = %e, "instance did not start");
                if let (Some(f), Placement::Leases(coord)) = (&fence, &self.cfg.placement) {
                    let epoch = f.epoch();
                    f.lose();
                    let _ = coord.release(&inst.name, &self.cfg.pod, epoch).await;
                }
                Route::Unavailable(format!("instance {} did not start", inst.name))
            }
        }
    }

    /// Prepares `inst`'s home (its config and the key-encryption key) and
    /// starts its core, on its storage.
    async fn open(&self, inst: &Instance, fence: Option<Arc<Fence>>) -> anyhow::Result<Running> {
        let home = Home::at(self.cfg.data_dir.join("instances").join(&inst.name));
        home.ensure_dirs()?;
        write_if_changed(
            &home.root().join("config.toml"),
            inst.config.as_bytes(),
            0o644,
        )?;
        if let Some(kek) = &self.cfg.kek {
            let dir = home
                .kek_path()
                .parent()
                .expect("the KEK has a directory")
                .to_path_buf();
            std::fs::create_dir_all(&dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            }
            write_if_changed(&home.kek_path(), kek, 0o600)?;
        }
        let config = clax_core::config::HomeConfig::load(home.root())?;
        let hosted_cfg = config
            .hosted()?
            .ok_or_else(|| anyhow::anyhow!("instance {} has no [hosted]", inst.name))?;
        let hosted = clax_http::hosted::Hosted::from_config(&hosted_cfg).ok_or_else(|| {
            anyhow::anyhow!(
                "instance {}'s public_origin {} is not usable",
                inst.name,
                hosted_cfg.public_origin
            )
        })?;
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
        };
        match &self.cfg.storage {
            Storage::Disk => Ok(Running {
                core: Some(Core::start(core_cfg, &mut |_| {}).await?),
                hold: fence.map(|f| (f, 0)),
                config: inst.config.clone(),
                _db: None,
            }),
            Storage::Postgres(pool) => {
                let fence = fence.ok_or_else(|| {
                    anyhow::anyhow!("an instance on PostgreSQL runs only under a lease")
                })?;
                let pages = Arc::new(crate::pages::PgPages::new(pool.clone(), fence.clone()));
                let content = Arc::new(crate::content::PgContent::new(pool.clone(), fence.clone()));
                let (store_home, store_pages) = (home.clone(), pages.clone());
                // Opening reads the pages and migrates: blocking work.
                let (store, registered) = tokio::task::spawn_blocking(move || {
                    let db: Arc<dyn clax_pagevfs::PageStore> = store_pages;
                    let (conn, registered) = clax_pagevfs::open("db", db)?.into_parts();
                    let store = clax_store::Store::open_on(
                        &store_home,
                        conn,
                        Arc::new(clax_core::working::SystemClock),
                        content,
                    )?;
                    anyhow::Ok((Arc::new(store), registered))
                })
                .await??;
                let app_data = crate::app_data::PgAppData::new(pages, store.clock());
                let app_data: Arc<dyn clax_http::app_data_host::AppDataHost> = Arc::new(
                    clax_http::app_data_host::MovingAppData::new(app_data, store.clone()),
                );
                let core = Core::start_on(core_cfg, store, Some(app_data)).await?;
                Ok(Running {
                    core: Some(core),
                    hold: Some((fence, 0)),
                    config: inst.config.clone(),
                    _db: Some(registered),
                })
            }
        }
    }

    /// Stops instance `name` when it runs here, and on leases releases its
    /// lease, saying `why` in the log.
    pub async fn stop(&self, name: &str, why: &str) {
        let Some(slot) = lock(&self.slots).get(name).cloned() else {
            return;
        };
        slot.stopping.store(true, Ordering::SeqCst);
        let mut running = slot.running.lock().await;
        *slot.router.write().unwrap_or_else(|e| e.into_inner()) = None;
        let Some(mut r) = running.take() else {
            slot.stopping.store(false, Ordering::SeqCst);
            return;
        };
        if let Some(core) = r.core.take()
            && !core.stop_bounded(STOP_TIMEOUT).await
        {
            tracing::warn!(instance = %name, "the instance's store did not stop in time");
        }
        if let Some((fence, _)) = &r.hold {
            let epoch = fence.epoch();
            // Nothing the instance left running writes once the fence is
            // lost, before another process can take the lease.
            fence.lose();
            if let Placement::Leases(coord) = &self.cfg.placement
                && let Err(e) = coord.release(name, &self.cfg.pod, epoch).await
            {
                tracing::warn!(instance = %name, error = %e, "releasing the lease failed; it expires");
            }
        }
        drop(r);
        drop(running);
        slot.stopping.store(false, Ordering::SeqCst);
        tracing::info!(instance = %name, why, "instance stopped");
    }

    /// The background work of one tick (see the [module docs](self)):
    /// the heartbeat and every renewal first, so no stop can hold a lease's
    /// renewal past its expiry, then the stops the catalog, lost leases and
    /// the handover call for, all at once.
    pub async fn tick(self: &Arc<Self>) {
        let n = self.ticks.fetch_add(1, Ordering::SeqCst);
        let mut stops: Vec<(String, &'static str)> = Vec::new();
        if let Placement::Leases(coord) = &self.cfg.placement {
            let draining = self.draining.load(Ordering::SeqCst);
            if !draining {
                match coord.heartbeat(&self.cfg.pod, &self.cfg.advertise).await {
                    Ok(()) => self.healthy.store(true, Ordering::SeqCst),
                    Err(e) => {
                        self.healthy.store(false, Ordering::SeqCst);
                        tracing::warn!(error = %e, "heartbeat failed");
                    }
                }
            }
            for name in self.running() {
                if let Some(why) = self.renew(coord.as_ref(), &name).await {
                    stops.push((name, why));
                }
            }
            // Every third tick, the handover.
            if !draining
                && n % 3 == 2
                && let Ok(members) = coord.members().await
            {
                for name in self.running() {
                    if let Some(p) = coord::preferred(&name, &members)
                        && p.pod != self.cfg.pod
                        && p.age >= coord::SETTLE
                    {
                        tracing::info!(instance = %name, to = %p.pod, "handing the instance over");
                        stops.push((name, "another member is preferred"));
                    }
                }
            }
        }
        stops.extend(self.reload().await);
        stops.sort();
        stops.dedup_by(|a, b| a.0 == b.0);
        let stops = stops.into_iter().map(|(name, why)| {
            let fleet = self.clone();
            async move { fleet.stop(&name, why).await }
        });
        futures::future::join_all(stops).await;
    }

    /// Renews the lease of `name`, running here: why it must stop, when the
    /// lease is lost or failed to renew [`RENEW_FAILURES`] times in a row.
    async fn renew(&self, coord: &dyn Coordinator, name: &str) -> Option<&'static str> {
        let slot = self.slot(name);
        if slot.stopping.load(Ordering::SeqCst) {
            return None;
        }
        // The epoch is read under the slot's lock, and the renewal made
        // without it: a stop under way is never waited for.
        let epoch = {
            let running = slot.running.lock().await;
            running.as_ref()?.hold.as_ref()?.0.epoch()
        };
        let renewed = coord.renew(name, &self.cfg.pod, epoch).await;
        let mut running = slot.running.lock().await;
        let (fence, failures) = running.as_mut()?.hold.as_mut()?;
        if fence.epoch() != epoch {
            return None;
        }
        match renewed {
            Ok(true) => {
                *failures = 0;
                None
            }
            Ok(false) => Some("its lease was lost"),
            Err(e) => {
                *failures += 1;
                tracing::warn!(instance = %name, error = %e, failures = *failures, "renewing the lease failed");
                (*failures >= RENEW_FAILURES).then_some("its lease could not be renewed")
            }
        }
    }

    /// Reads the catalog again, and answers each running instance it no
    /// longer has, or whose config or shard changed, with why it must stop.
    /// It compares every tick, whether or not the catalog changed: an
    /// instance started from the catalog a reload replaced stops too.
    async fn reload(&self) -> Vec<(String, &'static str)> {
        match Catalog::load(&self.cfg.catalog, self.cfg.shards) {
            Ok(next) if *self.catalog() != next => {
                tracing::info!(instances = next.instances.len(), "catalog reloaded");
                *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(next);
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "the catalog did not reload; keeping the one in effect");
            }
        }
        let catalog = self.catalog();
        let mut stops = Vec::new();
        for name in self.running() {
            let slot = self.slot(&name);
            let config = slot.running.lock().await.as_ref().map(|r| r.config.clone());
            let why = match (catalog.get(&name), config) {
                (None, _) => Some("it left the catalog"),
                (Some(i), Some(c)) if i.config != c => Some("its config changed"),
                (Some(i), _) if matches!(self.cfg.placement, Placement::Shards { ordinal, .. } if i.shard != Some(ordinal)) => {
                    Some("its shard changed")
                }
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

    /// Drains: starts nothing more, leaves the membership, and stops every
    /// instance here (releasing their leases).
    pub async fn drain(self: &Arc<Self>) {
        self.begin_draining();
        if let Placement::Leases(coord) = &self.cfg.placement
            && let Err(e) = coord.leave(&self.cfg.pod).await
        {
            tracing::warn!(error = %e, "leaving the membership failed; it lapses");
        }
        // Every slot, not just the running: a start that passed the draining
        // check before it began ends first (the stop waits for its lock).
        let names: Vec<String> = lock(&self.slots).keys().cloned().collect();
        let stops = names
            .into_iter()
            .map(|name| {
                let fleet = self.clone();
                async move { fleet.stop(&name, "this process is stopping").await }
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

    /// The admin port's routes: `/healthz` (alive), `/readyz` (taking
    /// requests: 503 while draining or, on leases, cut off from the
    /// database) and `/status` (this process and its instances, as JSON).
    pub fn admin_router(self: &Arc<Self>) -> axum::Router {
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
                get(|State(f): State<Arc<Fleet>>| async move {
                    axum::Json(serde_json::json!({
                        "pod": f.cfg.pod,
                        "advertise": f.cfg.advertise,
                        "ready": f.ready(),
                        "instances": f.catalog().instances.iter().map(|i| &i.name).collect::<Vec<_>>(),
                        "running": f.running(),
                    }))
                }),
            )
            .with_state(self.clone())
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
    let hops = hops(&req);
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
                return unavailable(&format!(
                    "instance {name:?} is not in this process's catalog yet"
                ));
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
    req.headers_mut().remove(HOP_HEADER);
    req.headers_mut().remove(INSTANCE_HEADER);
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
