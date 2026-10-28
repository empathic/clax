//! PostgreSQL: a pool of synchronous clients, each on a thread of its own,
//! the fleet's schema, and the fence every write of an instance passes.
//!
//! The VFS and the content store are synchronous and run on whichever
//! thread calls the store, often one of Tokio's; `postgres`'s synchronous
//! client runs a runtime of its own and cannot be driven from inside
//! another's. So each client lives on a thread of the [`Pool`], and a call
//! hands it a job and blocks on the answer ([`Pool::run`]), which any
//! thread may do. A client whose connection broke is dropped, and the next
//! job on that thread connects again. [`metrics`] reports each pool's
//! queue, the time jobs waited in it, and its connections.
//!
//! # Connections
//!
//! [`config`] sets every connection's bounds on top of what the URL says:
//! TCP keepalives and a user timeout that notice a server lost without a
//! reset ([`LOST_SERVER`]), and startup options that set the server's
//! `statement_timeout`, `idle_in_transaction_session_timeout` and its own
//! keepalives for the session. Options the URL gives are kept and come
//! last, so one of them overrides the fleet's. PgBouncer passes no startup
//! options on; behind it the role must carry the settings
//! (docs/kubernetes.md, Connections), and [`Pool::connect`] logs when a
//! session has no timeouts.
//!
//! # Schema
//!
//! Every table is the fleet's, keyed by instance (`clax_fleet_*`, made by
//! [`migrate`] under an advisory lock, so pods starting together make it
//! once):
//!
//! - `members`: each live process, its address, its run, the store schema
//!   its build migrates to, and when it joined and was last seen
//!   ([`crate::coord`]);
//! - `leases`: each instance's owner, its address, the lease's epoch, when
//!   it expires, and the highest store schema a holder opened the instance
//!   at ([`crate::coord`]);
//! - `fences`: each instance's holder and epoch as every write checks them
//!   ([`Fence::check`]), kept apart from `leases` so renewals never wait
//!   for writes;
//! - `files` and `chunks`: the instances' SQLite files as chunks
//!   ([`crate::pages`]);
//! - `blobs`, `versions`, `version_files` and `assets`: artifact content
//!   ([`crate::content`]).
//!
//! # The fence
//!
//! Exactly one process owns an instance at a time: the holder of its lease.
//! The VFS and the content store assume no one else writes the instance's
//! rows. A process that lost its lease (paused past its expiry, cut off
//! from the database) may still try, so every write transaction of an
//! instance first reads its fence row `FOR SHARE` ([`Fence::check`]) and
//! goes on only while the row still names this process and the epoch it
//! acquired. A process taking the lease over updates that row in the same
//! statement as the lease's ([`crate::coord::Coordinator::acquire`]), so it
//! waits for every write the old owner had begun, and every later write of
//! the old owner fails. Renewing a lease updates only its lease row, which
//! no write locks: a slow write never holds up the renewal of the pod's
//! other leases.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use postgres::config::SslMode;
use postgres::{Client, Transaction};

pub use postgres::types::Type;

/// An error from PostgreSQL or the pool, as text.
pub type PgError = String;

type Job = Box<dyn FnOnce(&mut Option<Client>, &Connector) + Send>;

/// How long a connection may take to open.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a connection may sit idle before TCP keepalives probe it.
pub const KEEPALIVE_IDLE: Duration = Duration::from_secs(15);
/// How often an unanswered keepalive is sent again.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
/// How many keepalives may go unanswered before the connection is dropped.
pub const KEEPALIVE_RETRIES: u32 = 3;
/// About how long a server lost without a reset takes to fail the calls
/// on its connections: keepalives notice it on a connection waiting for
/// an answer ([`KEEPALIVE_IDLE`], then [`KEEPALIVE_RETRIES`] probes
/// [`KEEPALIVE_INTERVAL`] apart), and `TCP_USER_TIMEOUT` on one whose sent
/// data goes unacknowledged.
pub const LOST_SERVER: Duration = Duration::from_secs(30);

/// The longest one statement may run (`statement_timeout`): a call that
/// would hang on a stuck server fails instead.
pub const STATEMENT_TIMEOUT: Duration = Duration::from_secs(120);

/// How long the server lets a session sit idle inside a transaction
/// (`idle_in_transaction_session_timeout`) before it ends the session. No
/// transaction of the fleet waits on anything between its statements, so
/// only a client lost mid-transaction reaches it, and its locks (an
/// instance's fence row) are freed before the lease it held expires
/// ([`crate::coord::LEASE_TTL`]) and another process may take it.
pub const IDLE_IN_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(20);

/// How a thread of the pool connects.
#[derive(Clone)]
struct Connector {
    config: postgres::Config,
    tls: Option<crate::tls::RustlsConnect>,
    stats: Arc<Stats>,
}

impl Connector {
    fn connect(&self) -> Result<Client, postgres::Error> {
        let c = match &self.tls {
            Some(tls) => self.config.connect(tls.clone()),
            None => self.config.connect(postgres::NoTls),
        };
        let counter = if c.is_ok() {
            &self.stats.connects
        } else {
            &self.stats.connect_failures
        };
        counter.fetch_add(1, Ordering::Relaxed);
        c
    }
}

/// What a pool counts, for [`metrics`].
#[derive(Default)]
struct Stats {
    /// Jobs queued that no thread has taken yet.
    waiting: AtomicU64,
    /// Jobs a thread is running.
    running: AtomicU64,
    /// Jobs taken, and the microseconds they waited in all.
    taken: AtomicU64,
    waited_micros: AtomicU64,
    connects: AtomicU64,
    connect_failures: AtomicU64,
}

/// Marks a job taken from the queue as running until it is dropped.
struct Running<'a>(&'a Stats);

impl<'a> Running<'a> {
    fn start(stats: &'a Stats, queued: Instant) -> Running<'a> {
        stats.waiting.fetch_sub(1, Ordering::Relaxed);
        stats.running.fetch_add(1, Ordering::Relaxed);
        stats.taken.fetch_add(1, Ordering::Relaxed);
        let waited = u64::try_from(queued.elapsed().as_micros()).unwrap_or(u64::MAX);
        stats.waited_micros.fetch_add(waited, Ordering::Relaxed);
        Running(stats)
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.running.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Clients on threads of their own; see the [module docs](self).
pub struct Pool {
    name: String,
    size: usize,
    stats: Arc<Stats>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

/// TLS for the connections: off, or required (no connection without it),
/// trusting the certificates in a PEM file when one is named, else the
/// public roots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Tls {
    #[default]
    Off,
    On {
        ca_file: Option<std::path::PathBuf>,
    },
}

/// The roots a TLS connection trusts: the certificates in `ca_file` alone
/// when one is named (as libpq's `sslrootcert`), so no other CA can vouch
/// for the database, else the public roots.
///
/// # Errors
/// The file cannot be read, a certificate in it does not parse, or it
/// holds none.
fn roots(ca_file: Option<&std::path::Path>) -> Result<rustls::RootCertStore, PgError> {
    let mut roots = rustls::RootCertStore::empty();
    let Some(path) = ca_file else {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        return Ok(roots);
    };
    use rustls::pki_types::pem::PemObject;
    let pem = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    for cert in rustls::pki_types::CertificateDer::pem_slice_iter(&pem) {
        let cert = cert.map_err(|e| format!("{}: {e}", path.display()))?;
        roots
            .add(cert)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if roots.is_empty() {
        return Err(format!(
            "{}: no PEM certificate (-----BEGIN CERTIFICATE-----) in it; CLAX_POSTGRES_CA names the file of the CA certificates the database's is issued under",
            path.display()
        ));
    }
    Ok(roots)
}

fn tls_connector(tls: &Tls) -> Result<Option<crate::tls::RustlsConnect>, PgError> {
    let Tls::On { ca_file } = tls else {
        return Ok(None);
    };
    let roots = roots(ca_file.as_deref())?;
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Some(crate::tls::RustlsConnect::new(config)))
}

/// The connection settings `url` (a libpq connection string or URI) gives,
/// with TLS as `tls` says and the fleet's bounds (see the [module
/// docs](self#connections)): a connect timeout, keepalives and a TCP user
/// timeout where the URL sets none, and the server settings as startup
/// options ahead of the URL's own. TLS on is required: the connection never
/// falls back to plain text, whatever the server answers. A URL whose
/// `sslmode` contradicts `tls` is refused rather than either silently
/// winning.
///
/// # Errors
/// The URL does not parse, or its `sslmode` contradicts `tls`.
pub fn config(url: &str, tls: &Tls) -> Result<postgres::Config, PgError> {
    let mut c: postgres::Config = url
        .parse()
        .map_err(|e| format!("the PostgreSQL URL: {e}"))?;
    let asked = c.get_ssl_mode();
    match tls {
        Tls::On { .. } if asked == SslMode::Disable => {
            return Err(
                "the PostgreSQL URL says sslmode=disable, but CLAX_POSTGRES_TLS is on".into(),
            );
        }
        Tls::On { .. } => {
            c.ssl_mode(SslMode::Require);
        }
        Tls::Off if asked == SslMode::Require => {
            return Err(
                "the PostgreSQL URL asks for TLS (sslmode), but CLAX_POSTGRES_TLS is off: set it to on"
                    .into(),
            );
        }
        Tls::Off => {
            c.ssl_mode(SslMode::Disable);
        }
    }
    if c.get_connect_timeout().is_none() {
        c.connect_timeout(CONNECT_TIMEOUT);
    }
    if c.get_keepalives_interval().is_none() {
        c.keepalives_interval(KEEPALIVE_INTERVAL);
    }
    if c.get_keepalives_retries().is_none() {
        c.keepalives_retries(KEEPALIVE_RETRIES);
    }
    if c.get_tcp_user_timeout().is_none() {
        c.tcp_user_timeout(LOST_SERVER);
    }
    c.keepalives(true).keepalives_idle(KEEPALIVE_IDLE);
    let ours = format!(
        "-c statement_timeout={} -c idle_in_transaction_session_timeout={} -c tcp_keepalives_idle={} -c tcp_keepalives_interval={} -c tcp_keepalives_count={}",
        STATEMENT_TIMEOUT.as_millis(),
        IDLE_IN_TRANSACTION_TIMEOUT.as_millis(),
        KEEPALIVE_IDLE.as_secs(),
        KEEPALIVE_INTERVAL.as_secs(),
        KEEPALIVE_RETRIES,
    );
    let options = match c.get_options() {
        Some(given) if !given.trim().is_empty() => format!("{ours} {given}"),
        _ => ours,
    };
    c.options(&options);
    Ok(c)
}

/// Names `config`'s sessions `name` (`application_name`, as
/// `pg_stat_activity` and the server's log show them), unless the URL
/// named them.
pub fn name_sessions(config: &mut postgres::Config, name: &str) {
    if config.get_application_name().is_none() {
        config.application_name(name);
    }
}

/// The timeouts [`config`] sets as startup options that `client`'s
/// session has off (`0`): what a pooler passing on no startup options
/// leaves to the role's settings.
fn unbounded(client: &mut Client) -> Result<Vec<&'static str>, postgres::Error> {
    const BOUNDS: [&str; 2] = ["statement_timeout", "idle_in_transaction_session_timeout"];
    let mut off = Vec::new();
    for name in BOUNDS {
        let row = client.query_typed_one("SELECT current_setting($1)", &[(&name, Type::TEXT)])?;
        if row.get::<_, String>(0) == "0" {
            off.push(name);
        }
    }
    Ok(off)
}

impl Pool {
    /// A pool of `size` threads (at least one) connecting with `config`
    /// ([`config`]), each on its first job. Connects once here, so an
    /// unreachable server fails at once, and logs when that session has no
    /// statement or idle-in-transaction timeout. `name` names the threads
    /// and the pool in [`metrics`].
    ///
    /// # Errors
    /// The CA file cannot be read, or the first connection fails.
    pub fn connect(
        name: &str,
        config: postgres::Config,
        tls: &Tls,
        size: usize,
    ) -> Result<Arc<Pool>, PgError> {
        let stats = Arc::new(Stats::default());
        let connector = Connector {
            config,
            tls: tls_connector(tls)?,
            stats: stats.clone(),
        };
        // Connecting outside any runtime: the caller may be inside one.
        let (first, off) = {
            let c = connector.clone();
            std::thread::spawn(move || {
                let mut client = c.connect().map_err(err)?;
                let off = unbounded(&mut client).map_err(err)?;
                Ok::<_, PgError>((client, off))
            })
            .join()
            .map_err(|_| "the connecting thread panicked".to_string())??
        };
        if !off.is_empty() {
            tracing::warn!(
                pool = name,
                off = ?off,
                "PostgreSQL sessions run without these timeouts (a pooler such as PgBouncer passes no startup options on): set them on the role (docs/kubernetes.md, Connections)"
            );
        }
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let mut threads = Vec::new();
        let mut first = Some(first);
        for i in 0..size.max(1) {
            let rx = rx.clone();
            let connector = connector.clone();
            let mut client = first.take();
            threads.push(
                std::thread::Builder::new()
                    .name(format!("clax-pg-{name}-{i}"))
                    .spawn(move || {
                        loop {
                            let job = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                                Ok(job) => job,
                                Err(_) => return,
                            };
                            // A job that panics drops its answer (its caller
                            // sees the panic as an error) and the client, never
                            // the thread.
                            let ran =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    job(&mut client, &connector)
                                }));
                            if ran.is_err() {
                                client = None;
                            }
                        }
                    })
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(Arc::new(Pool {
            name: name.to_string(),
            size: size.max(1),
            stats,
            jobs: Mutex::new(Some(tx)),
            threads: Mutex::new(threads),
        }))
    }

    /// Queues `f` with `done` to hear its answer.
    fn send<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Client) -> Result<R, PgError> + Send + 'static,
        done: impl FnOnce(Result<R, PgError>) + Send + 'static,
    ) -> Result<(), PgError> {
        let queued = Instant::now();
        let stats = self.stats.clone();
        let job: Job = Box::new(move |client, connector| {
            let _running = Running::start(&stats, queued);
            if client.as_ref().is_none_or(Client::is_closed) {
                match connector.connect() {
                    Ok(c) => *client = Some(c),
                    Err(e) => {
                        done(Err(format!("connecting to PostgreSQL: {}", err(e))));
                        return;
                    }
                }
            }
            let c = client.as_mut().expect("connected above");
            let r = f(c);
            if c.is_closed() {
                *client = None;
            }
            done(r);
        });
        self.stats.waiting.fetch_add(1, Ordering::Relaxed);
        let sent = match &*self.jobs.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(tx) => tx.send(job).is_ok(),
            None => false,
        };
        if sent {
            Ok(())
        } else {
            self.stats.waiting.fetch_sub(1, Ordering::Relaxed);
            Err("the PostgreSQL pool is closed".into())
        }
    }

    /// Runs `f` on a client of the pool and answers what it answers,
    /// blocking the calling thread until then. A connection error drops
    /// the client, so the next job connects again; `f` is not retried.
    ///
    /// # Errors
    /// `f`'s error, a failed connection, or a pool already closed.
    pub fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Client) -> Result<R, PgError> + Send + 'static,
    ) -> Result<R, PgError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(f, move |r| {
            let _ = tx.send(r);
        })?;
        rx.recv()
            .map_err(|_| "a PostgreSQL job panicked".to_string())?
    }

    /// [`Pool::run`] from async code: the caller's task awaits the answer,
    /// and no thread of the runtime waits on the database.
    ///
    /// # Errors
    /// [`Pool::run`]'s.
    pub async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Client) -> Result<R, PgError> + Send + 'static,
    ) -> Result<R, PgError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(f, move |r| {
            let _ = tx.send(r);
        })?;
        rx.await
            .map_err(|_| "a PostgreSQL job panicked".to_string())?
    }

    /// Stops taking jobs and joins the threads once their jobs end.
    pub fn close(&self) {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).take();
        let threads = std::mem::take(&mut *self.threads.lock().unwrap_or_else(|e| e.into_inner()));
        for t in threads {
            let _ = t.join();
        }
    }
}

/// The pools' queues and connections in Prometheus's text format, each
/// pool's under its name (`pool="data"`): jobs waiting for a thread and
/// running, the time taken jobs waited (a summary's sum and count), the
/// threads, and connections opened and failed.
pub fn metrics(pools: &[&Pool]) -> String {
    let rows: Vec<(&str, usize, &Stats)> = pools
        .iter()
        .map(|p| (p.name.as_str(), p.size, &*p.stats))
        .collect();
    render(&rows)
}

fn render(pools: &[(&str, usize, &Stats)]) -> String {
    let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let mut out = String::new();
    let mut family =
        |name: &str, kind: &str, help: &str, value: &dyn Fn(usize, &Stats) -> String| {
            out.push_str(&format!(
                "# HELP clax_fleet_pg_{name} {help}\n# TYPE clax_fleet_pg_{name} {kind}\n"
            ));
            for (pool, size, stats) in pools {
                out.push_str(&format!(
                    "clax_fleet_pg_{name}{{pool=\"{pool}\"}} {}\n",
                    value(*size, stats)
                ));
            }
        };
    family(
        "jobs_waiting",
        "gauge",
        "PostgreSQL jobs queued for a thread of the pool.",
        &|_, s| load(&s.waiting).to_string(),
    );
    family(
        "jobs_running",
        "gauge",
        "PostgreSQL jobs a thread of the pool is running.",
        &|_, s| load(&s.running).to_string(),
    );
    family(
        "threads",
        "gauge",
        "Threads (each with a connection) of the pool.",
        &|size, _| size.to_string(),
    );
    family(
        "connects_total",
        "counter",
        "PostgreSQL connections the pool opened.",
        &|_, s| load(&s.connects).to_string(),
    );
    family(
        "connect_failures_total",
        "counter",
        "PostgreSQL connections the pool failed to open.",
        &|_, s| load(&s.connect_failures).to_string(),
    );
    out.push_str(
        "# HELP clax_fleet_pg_job_wait_seconds How long PostgreSQL jobs waited for a thread of the pool.\n# TYPE clax_fleet_pg_job_wait_seconds summary\n",
    );
    for (pool, _, s) in pools {
        out.push_str(&format!(
            "clax_fleet_pg_job_wait_seconds_sum{{pool=\"{pool}\"}} {}\nclax_fleet_pg_job_wait_seconds_count{{pool=\"{pool}\"}} {}\n",
            load(&s.waited_micros) as f64 / 1e6,
            load(&s.taken)
        ));
    }
    out
}

/// `e` as a [`PgError`].
pub fn err(e: postgres::Error) -> PgError {
    match e.as_db_error() {
        Some(db) => format!("{}: {}", db.code().code(), db.message()),
        None => e.to_string(),
    }
}

/// The schema's version; [`migrate`] brings a database to it.
const SCHEMA_VERSION: i32 = 2;

/// One step of the schema: the version it brings a database to, what it
/// runs, and the oldest build (by its [`SCHEMA_VERSION`]) that still runs
/// on what it leaves. A step that only adds what older builds never read
/// keeps the compatible version; one that changes what they read or write
/// raises it to its own.
#[derive(Debug)]
struct Step {
    version: i32,
    sql: &'static str,
    compatible: i32,
}

const STEPS: [Step; 2] = [
    Step {
        version: 1,
        sql: SCHEMA_1,
        compatible: 1,
    },
    Step {
        version: 2,
        sql: SCHEMA_2,
        compatible: 1,
    },
];

/// The fleet's tables (see the [module docs](self)).
const SCHEMA_1: &str = "
CREATE TABLE IF NOT EXISTS clax_fleet_members (
    pod text PRIMARY KEY,
    addr text NOT NULL,
    run text NOT NULL,
    store_schema integer NOT NULL,
    joined timestamptz NOT NULL DEFAULT now(),
    seen timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS clax_fleet_leases (
    instance text PRIMARY KEY,
    holder text NOT NULL,
    addr text NOT NULL,
    epoch bigint NOT NULL,
    expires timestamptz NOT NULL,
    store_schema integer NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS clax_fleet_fences (
    instance text PRIMARY KEY,
    holder text NOT NULL,
    epoch bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS clax_fleet_files (
    instance text NOT NULL,
    file text NOT NULL,
    size bigint NOT NULL,
    PRIMARY KEY (instance, file)
);
CREATE TABLE IF NOT EXISTS clax_fleet_chunks (
    instance text NOT NULL,
    file text NOT NULL,
    idx bigint NOT NULL,
    bytes bytea NOT NULL,
    PRIMARY KEY (instance, file, idx)
);
CREATE TABLE IF NOT EXISTS clax_fleet_blobs (
    instance text NOT NULL,
    artifact text NOT NULL,
    hash bytea NOT NULL,
    bytes bytea NOT NULL,
    PRIMARY KEY (instance, artifact, hash)
);
CREATE TABLE IF NOT EXISTS clax_fleet_versions (
    instance text NOT NULL,
    artifact text NOT NULL,
    n integer NOT NULL,
    step text,
    PRIMARY KEY (instance, artifact, n)
);
CREATE TABLE IF NOT EXISTS clax_fleet_version_files (
    instance text NOT NULL,
    artifact text NOT NULL,
    n integer NOT NULL,
    path text NOT NULL,
    hash bytea NOT NULL,
    PRIMARY KEY (instance, artifact, n, path),
    FOREIGN KEY (instance, artifact, n)
        REFERENCES clax_fleet_versions (instance, artifact, n) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS clax_fleet_version_files_hash
    ON clax_fleet_version_files (instance, artifact, hash);
CREATE TABLE IF NOT EXISTS clax_fleet_assets (
    instance text NOT NULL,
    artifact text NOT NULL,
    id text NOT NULL,
    ext text NOT NULL,
    bytes bytea NOT NULL,
    PRIMARY KEY (instance, artifact, id, ext)
);
";

/// What version 2 adds: each chunk kept inline (compressed when that helps)
/// rather than in TOAST, and its table vacuumed sooner, since every commit
/// rewrites its pages' rows; each instance's public origin, as it first
/// started, so a name given to another tenant never opens its data
/// ([`claim_origin`]); and the fleet's settings, its peer key among them
/// ([`peer_key`]).
const SCHEMA_2: &str = "
ALTER TABLE clax_fleet_chunks ALTER COLUMN bytes SET STORAGE MAIN;
ALTER TABLE clax_fleet_chunks SET (autovacuum_vacuum_scale_factor = 0.02);
CREATE TABLE IF NOT EXISTS clax_fleet_instances (
    instance text PRIMARY KEY,
    origin text NOT NULL
);
CREATE TABLE IF NOT EXISTS clax_fleet_settings (
    name text PRIMARY KEY,
    value text NOT NULL
);
";

/// What [`migrate`] does to a database at `version`, which builds of
/// schema `compatible` or later run on: the steps to run and the
/// compatible version they leave, or nothing to do (the schema is this
/// build's, or a newer one this build still runs on).
///
/// # Errors
/// The database's schema needs a newer build than this one.
fn plan(version: i32, compatible: i32) -> Result<(Vec<&'static Step>, i32), PgError> {
    if compatible > SCHEMA_VERSION {
        return Err(format!(
            "the database has the fleet's schema {version}, which builds of schema {compatible} or later run on, and this build's is {SCHEMA_VERSION}: run a newer clax-fleet"
        ));
    }
    let steps: Vec<&Step> = STEPS.iter().filter(|s| s.version > version).collect();
    let compatible = steps
        .iter()
        .map(|s| s.compatible)
        .fold(compatible, i32::max);
    Ok((steps, compatible))
}

/// Brings the database to the fleet's schema, once, under an advisory lock
/// (a fleet's pods may start together). A database a newer fleet migrated
/// is left as it is when this build still runs on it (its compatible
/// version, [`Step`]), and refused otherwise.
///
/// # Errors
/// PostgreSQL's, or a schema that needs a newer build.
pub fn migrate(pool: &Pool) -> Result<(), PgError> {
    pool.run(|c| {
        let mut tx = c.transaction().map_err(err)?;
        // The key is arbitrary and the fleet's alone: "clax" in ASCII.
        tx.execute("SELECT pg_advisory_xact_lock(1668047224)", &[])
            .map_err(err)?;
        tx.batch_execute(
            "CREATE TABLE IF NOT EXISTS clax_fleet_schema (
                version integer NOT NULL,
                compatible integer NOT NULL
            )",
        )
        .map_err(err)?;
        let (version, compatible): (i32, i32) = tx
            .query_typed_opt(
                "SELECT version, compatible FROM clax_fleet_schema ORDER BY version DESC LIMIT 1",
                &[],
            )
            .map_err(err)?
            .map_or((0, 0), |r| (r.get(0), r.get(1)));
        let (steps, compatible) = plan(version, compatible)?;
        if steps.is_empty() {
            if version > SCHEMA_VERSION {
                tracing::info!(
                    version,
                    compatible,
                    this = SCHEMA_VERSION,
                    "the database has a newer fleet schema, which this build runs on"
                );
            }
            return tx.commit().map_err(err);
        }
        for step in steps {
            tx.batch_execute(step.sql).map_err(err)?;
        }
        tx.execute("DELETE FROM clax_fleet_schema", &[])
            .map_err(err)?;
        tx.execute_typed(
            "INSERT INTO clax_fleet_schema (version, compatible) VALUES ($1, $2)",
            &[(&SCHEMA_VERSION, Type::INT4), (&compatible, Type::INT4)],
        )
        .map_err(err)?;
        tx.commit().map_err(err)
    })
}

/// This process's hold on one instance: its name, this process's name as
/// the lease names it, and the epoch of the lease it holds (0 while it
/// holds none).
pub struct Fence {
    pub instance: String,
    pub holder: String,
    epoch: AtomicI64,
}

impl Fence {
    pub fn new(instance: &str, holder: &str, epoch: i64) -> Arc<Fence> {
        Arc::new(Fence {
            instance: instance.to_string(),
            holder: holder.to_string(),
            epoch: AtomicI64::new(epoch),
        })
    }

    /// The epoch held.
    pub fn epoch(&self) -> i64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Holds no lease any more: every later write fails.
    pub fn lose(&self) {
        self.epoch.store(0, Ordering::SeqCst);
    }

    /// Fails unless the instance's fence row still names this process and
    /// its epoch, and keeps the row from changing (another process taking
    /// the lease) until `tx` ends.
    ///
    /// # Errors
    /// `lease_lost` when it does not, or PostgreSQL's.
    pub fn check(&self, tx: &mut Transaction<'_>) -> Result<(), PgError> {
        let epoch = self.epoch();
        let held = tx
            .query_typed_opt(
                "SELECT 1 FROM clax_fleet_fences
                 WHERE instance = $1 AND holder = $2 AND epoch = $3
                 FOR SHARE",
                &[
                    (&self.instance, Type::TEXT),
                    (&self.holder, Type::TEXT),
                    (&epoch, Type::INT8),
                ],
            )
            .map_err(err)?;
        if held.is_none() || epoch == 0 {
            return Err(format!(
                "lease_lost: {} no longer holds instance {}'s lease (epoch {epoch})",
                self.holder, self.instance
            ));
        }
        Ok(())
    }
}

/// Records `origin` as instance `instance`'s, under its lease, the first
/// time it starts, and fails when an earlier start recorded another: its
/// rows are another tenant's, kept under a name the catalog gave again.
///
/// # Errors
/// `instance_reused` naming both origins, `lease_lost`, or PostgreSQL's.
pub fn claim_origin(pool: &Pool, fence: &Arc<Fence>, origin: &str) -> Result<(), PgError> {
    let (fence, origin) = (fence.clone(), origin.to_string());
    pool.run(move |c| {
        let mut tx = c.transaction().map_err(err)?;
        fence.check(&mut tx)?;
        tx.execute_typed(
            "INSERT INTO clax_fleet_instances (instance, origin) VALUES ($1, $2)
             ON CONFLICT (instance) DO NOTHING",
            &[(&fence.instance, Type::TEXT), (&origin, Type::TEXT)],
        )
        .map_err(err)?;
        let held: String = tx
            .query_typed_one(
                "SELECT origin FROM clax_fleet_instances WHERE instance = $1",
                &[(&fence.instance, Type::TEXT)],
            )
            .map_err(err)?
            .get(0);
        tx.commit().map_err(err)?;
        if held != origin {
            return Err(format!(
                "instance_reused: instance {}'s data was made for {held}, not {origin}; a name is never given to another tenant (docs/kubernetes.md, The catalog)",
                fence.instance
            ));
        }
        Ok(())
    })
}

/// The fleet's peer key: the one `CLAX_FLEET_PEER_KEY` names when set
/// (stored for the processes that have none), else the one the database
/// holds, made at random by the first process to ask.
///
/// # Errors
/// PostgreSQL's.
pub fn peer_key(pool: &Pool, given: Option<&str>) -> Result<String, PgError> {
    let fresh = given.map(str::to_string).unwrap_or_else(|| {
        let bytes: [u8; 32] = clax_core::secret::random_bytes::<32>();
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    });
    let set = given.is_some();
    pool.run(move |c| {
        let sql = if set {
            "INSERT INTO clax_fleet_settings (name, value) VALUES ('peer_key', $1)
             ON CONFLICT (name) DO UPDATE SET value = EXCLUDED.value RETURNING value"
        } else {
            "INSERT INTO clax_fleet_settings (name, value) VALUES ('peer_key', $1)
             ON CONFLICT (name) DO UPDATE SET value = clax_fleet_settings.value RETURNING value"
        };
        Ok(c.query_typed_one(sql, &[(&fresh, Type::TEXT)])
            .map_err(err)?
            .get(0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-signed CA certificate, PEM-encoded.
    const CA: &str = "-----BEGIN CERTIFICATE-----
MIIBKjCB0gIJANHIfuMJN8mQMAoGCCqGSM49BAMCMB0xGzAZBgNVBAMMEmNsYXgt
ZmxlZXQgdGVzdCBDQTAgFw0yNjEwMjgwMTIyNDFaGA8yMTI2MTAwNDAxMjI0MVow
HTEbMBkGA1UEAwwSY2xheC1mbGVldCB0ZXN0IENBMFkwEwYHKoZIzj0CAQYIKoZI
zj0DAQcDQgAEmvpq6iS5uloIU+3YuijPCuZgvkyH76sErhKORQjcfewlFsm8MWWO
76i161Xdb2X/hhAxWTfXdvasHGZATxQbGTAKBggqhkjOPQQDAgNHADBEAiAC2iZ6
XphHoh27dDt5sm/FbKLoBD7PL0e6PSiJ/9ADAwIgBsWflFgZJNuNza/ymHRyg3cD
vvNV8seyl3vDywTP6vo=
-----END CERTIFICATE-----
";

    /// A CA file names the only roots trusted, and one without a
    /// certificate in it is refused, naming it; with none named, the
    /// public roots are trusted.
    #[test]
    fn a_ca_file_replaces_the_public_roots_and_must_hold_a_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, CA).unwrap();
        assert_eq!(roots(Some(&ca)).unwrap().len(), 1);
        for (name, text) in [
            ("empty.pem", ""),
            (
                "key.pem",
                "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, text).unwrap();
            let e = roots(Some(&path)).unwrap_err();
            assert!(
                e.contains(&path.display().to_string()) && e.contains("no PEM certificate"),
                "{e}"
            );
        }
        assert!(roots(Some(&dir.path().join("missing.pem"))).is_err());
        assert!(roots(None).unwrap().len() > 100);
    }

    /// A newer schema this build still runs on is left alone, one that
    /// needs a newer build is refused, and an older one is migrated, its
    /// compatible version the highest its steps leave.
    #[test]
    fn migrate_runs_on_a_newer_schema_only_while_it_is_compatible() {
        let (steps, compatible) = plan(0, 0).unwrap();
        assert_eq!(steps.len(), STEPS.len());
        assert_eq!(compatible, 1);
        let (steps, _) = plan(SCHEMA_VERSION, 1).unwrap();
        assert!(steps.is_empty());
        let (steps, compatible) = plan(SCHEMA_VERSION + 3, SCHEMA_VERSION).unwrap();
        assert!(steps.is_empty());
        assert_eq!(compatible, SCHEMA_VERSION);
        let e = plan(SCHEMA_VERSION + 1, SCHEMA_VERSION + 1).unwrap_err();
        assert!(e.contains("run a newer clax-fleet"), "{e}");
        assert!(STEPS.windows(2).all(|w| w[0].version < w[1].version));
        assert_eq!(STEPS.last().unwrap().version, SCHEMA_VERSION);
    }

    /// The pool's metrics name each pool and count its queue, its waits
    /// and its connections.
    #[test]
    fn pool_metrics_count_waiting_running_and_connections() {
        let stats = Stats::default();
        stats.waiting.fetch_add(2, Ordering::Relaxed);
        {
            let _job = Running::start(&stats, Instant::now());
            assert_eq!(stats.running.load(Ordering::Relaxed), 1);
        }
        stats.connects.fetch_add(3, Ordering::Relaxed);
        let idle = Stats::default();
        let text = render(&[("data", 8, &stats), ("coord", 2, &idle)]);
        for line in [
            "clax_fleet_pg_jobs_waiting{pool=\"data\"} 1",
            "clax_fleet_pg_jobs_running{pool=\"data\"} 0",
            "clax_fleet_pg_threads{pool=\"coord\"} 2",
            "clax_fleet_pg_connects_total{pool=\"data\"} 3",
            "clax_fleet_pg_job_wait_seconds_count{pool=\"data\"} 1",
            "clax_fleet_pg_job_wait_seconds_count{pool=\"coord\"} 0",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} not in\n{text}");
        }
        assert_eq!(
            text.matches("# TYPE clax_fleet_pg_jobs_waiting ").count(),
            1
        );
    }
}
