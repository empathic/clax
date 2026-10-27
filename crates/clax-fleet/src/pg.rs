//! PostgreSQL: a pool of synchronous clients, each on a thread of its own,
//! the fleet's schema, and the fence every write of an instance passes.
//!
//! The VFS and the content store are synchronous and run on whichever
//! thread calls the store, often one of Tokio's; `postgres`'s synchronous
//! client runs a runtime of its own and cannot be driven from inside
//! another's. So each client lives on a thread of the [`Pool`], and a call
//! hands it a job and blocks on the answer ([`Pool::run`]), which any
//! thread may do. A client whose connection broke is dropped, and the next
//! job on that thread connects again.
//!
//! # Schema
//!
//! Every table is the fleet's, keyed by instance (`clax_fleet_*`, made by
//! [`migrate`] under an advisory lock, so pods starting together make it
//! once):
//!
//! - `members`: each live process, its address and when it joined and was
//!   last seen ([`crate::coord`]);
//! - `leases`: each instance's owner, its address, the lease's epoch and
//!   when it expires ([`crate::coord`]);
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
//! instance first reads its lease row `FOR SHARE` ([`Fence::check`]) and
//! goes on only while the row still names this process and the epoch it
//! acquired. A process taking the lease over updates that row, so it waits
//! for every write the old owner had begun, and every later write of the
//! old owner fails.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use postgres::{Client, Transaction};

/// An error from PostgreSQL or the pool, as text.
pub type PgError = String;

type Job = Box<dyn FnOnce(&mut Option<Client>, &Connector) + Send>;

/// How a thread of the pool connects.
#[derive(Clone)]
struct Connector {
    url: String,
    tls: Option<crate::tls::RustlsConnect>,
}

impl Connector {
    fn connect(&self) -> Result<Client, postgres::Error> {
        match &self.tls {
            Some(tls) => Client::connect(&self.url, tls.clone()),
            None => Client::connect(&self.url, postgres::NoTls),
        }
    }
}

/// Clients on threads of their own; see the [module docs](self).
pub struct Pool {
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

/// TLS for the connections: off, or on with the public roots and the
/// certificates in a PEM file, when one is named.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Tls {
    #[default]
    Off,
    On {
        ca_file: Option<std::path::PathBuf>,
    },
}

fn tls_connector(tls: &Tls) -> Result<Option<crate::tls::RustlsConnect>, PgError> {
    let Tls::On { ca_file } = tls else {
        return Ok(None);
    };
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = ca_file {
        use rustls::pki_types::pem::PemObject;
        let pem = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for cert in rustls::pki_types::CertificateDer::pem_slice_iter(&pem) {
            let cert = cert.map_err(|e| format!("{}: {e}", path.display()))?;
            roots
                .add(cert)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Some(crate::tls::RustlsConnect::new(config)))
}

impl Pool {
    /// A pool of `size` threads (at least one) connecting to `url` (a
    /// libpq connection string or URI), each on its first job. Connects
    /// once here, so a bad URL or an unreachable server fails at once.
    ///
    /// # Errors
    /// The URL does not parse, the CA file cannot be read, or the first
    /// connection fails.
    pub fn connect(url: &str, tls: &Tls, size: usize) -> Result<Arc<Pool>, PgError> {
        let connector = Connector {
            url: url.to_string(),
            tls: tls_connector(tls)?,
        };
        // Connecting outside any runtime: the caller may be inside one.
        let first = {
            let c = connector.clone();
            std::thread::spawn(move || c.connect().map_err(|e| e.to_string()))
                .join()
                .map_err(|_| "the connecting thread panicked".to_string())??
        };
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
                    .name(format!("clax-pg-{i}"))
                    .spawn(move || {
                        loop {
                            let job = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                                Ok(job) => job,
                                Err(_) => return,
                            };
                            job(&mut client, &connector);
                        }
                    })
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(Arc::new(Pool {
            jobs: Mutex::new(Some(tx)),
            threads: Mutex::new(threads),
        }))
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
        let (done, answer) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |client, connector| {
            if client.as_ref().is_none_or(Client::is_closed) {
                match connector.connect() {
                    Ok(c) => *client = Some(c),
                    Err(e) => {
                        let _ = done.send(Err(format!("connecting to PostgreSQL: {e}")));
                        return;
                    }
                }
            }
            let c = client.as_mut().expect("connected above");
            let r = f(c);
            if c.is_closed() {
                *client = None;
            }
            let _ = done.send(r);
        });
        let sent = match &*self.jobs.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(tx) => tx.send(job).is_ok(),
            None => false,
        };
        if !sent {
            return Err("the PostgreSQL pool is closed".into());
        }
        answer
            .recv()
            .map_err(|_| "a PostgreSQL job panicked".to_string())?
    }

    /// [`Pool::run`] from async code: on a blocking thread, so the runtime's
    /// own threads never wait on the database.
    ///
    /// # Errors
    /// [`Pool::run`]'s.
    pub async fn call<R: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&mut Client) -> Result<R, PgError> + Send + 'static,
    ) -> Result<R, PgError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || pool.run(f))
            .await
            .map_err(|e| e.to_string())?
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

/// `e` as a [`PgError`].
pub fn err(e: postgres::Error) -> PgError {
    match e.as_db_error() {
        Some(db) => format!("{}: {}", db.code().code(), db.message()),
        None => e.to_string(),
    }
}

/// The schema's version; [`migrate`] brings a database to it.
const SCHEMA_VERSION: i32 = 1;

/// The fleet's tables (see the [module docs](self)).
const SCHEMA_1: &str = "
CREATE TABLE IF NOT EXISTS clax_fleet_members (
    pod text PRIMARY KEY,
    addr text NOT NULL,
    joined timestamptz NOT NULL DEFAULT now(),
    seen timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS clax_fleet_leases (
    instance text PRIMARY KEY,
    holder text NOT NULL,
    addr text NOT NULL,
    epoch bigint NOT NULL,
    expires timestamptz NOT NULL
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

/// Brings the database to the fleet's schema, once, under an advisory lock
/// (a fleet's pods may start together). Refuses a database a newer fleet
/// migrated.
///
/// # Errors
/// PostgreSQL's, or a schema newer than this build's.
pub fn migrate(pool: &Pool) -> Result<(), PgError> {
    pool.run(|c| {
        let mut tx = c.transaction().map_err(err)?;
        // The key is arbitrary and the fleet's alone: "clax" in ASCII.
        tx.execute("SELECT pg_advisory_xact_lock(1668047224)", &[])
            .map_err(err)?;
        tx.batch_execute(
            "CREATE TABLE IF NOT EXISTS clax_fleet_schema (version integer NOT NULL)",
        )
        .map_err(err)?;
        let version: Option<i32> = tx
            .query_opt("SELECT max(version) FROM clax_fleet_schema", &[])
            .map_err(err)?
            .and_then(|r| r.get(0));
        let version = version.unwrap_or(0);
        if version > SCHEMA_VERSION {
            return Err(format!(
                "the database has the fleet's schema {version}, newer than this build's {SCHEMA_VERSION}: run a newer clax-fleet"
            ));
        }
        if version < 1 {
            tx.batch_execute(SCHEMA_1).map_err(err)?;
        }
        if version < SCHEMA_VERSION {
            tx.execute("DELETE FROM clax_fleet_schema", &[]).map_err(err)?;
            tx.execute(
                "INSERT INTO clax_fleet_schema (version) VALUES ($1)",
                &[&SCHEMA_VERSION],
            )
            .map_err(err)?;
        }
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

    /// Fails unless the instance's lease row still names this process and
    /// its epoch, and keeps the row from changing until `tx` ends.
    ///
    /// # Errors
    /// `lease_lost` when it does not, or PostgreSQL's.
    pub fn check(&self, tx: &mut Transaction<'_>) -> Result<(), PgError> {
        let epoch = self.epoch();
        let held = tx
            .query_opt(
                "SELECT 1 FROM clax_fleet_leases
                 WHERE instance = $1 AND holder = $2 AND epoch = $3
                 FOR SHARE",
                &[&self.instance, &self.holder, &epoch],
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
