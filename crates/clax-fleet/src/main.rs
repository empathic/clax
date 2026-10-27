//! `clax-fleet`: the server of a Kubernetes deployment of Clax (see the
//! library's docs, `docs/kubernetes.md` and `deploy/k8s`).
//!
//! - `clax-fleet serve`: serves the catalog's instances until SIGTERM, then
//!   drains: stops taking new work at once (`/readyz` answers 503), keeps
//!   serving for `--drain-delay` while the Service stops sending requests
//!   here, then stops every instance (releasing its lease) and ends the
//!   connections left within [`CONNECTIONS_DRAIN`].
//! - `clax-fleet check-catalog <file> [--sqlite]`: validates a catalog and
//!   prints its instances, as `serve` would read it.
//! - `clax-fleet status`: which process runs which instance, across the
//!   fleet: the leases in PostgreSQL, or each shard's `/status`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use clax_core::embedded::{DiskTree, EmbeddedFile, FileTree};
use clax_fleet::catalog::Catalog;
use clax_fleet::fleet::{Fleet, FleetConfig, Peers, Placement, Storage};

/// How long open connections may take to end once every instance stopped.
const CONNECTIONS_DRAIN: Duration = Duration::from_secs(10);

/// Connections the coordinator's own pool keeps: heartbeats, renewals and
/// lease lookups never wait behind an instance's page and content I/O.
const COORD_POOL: usize = 2;

#[derive(Parser)]
#[command(
    name = "clax-fleet",
    version,
    about = "Clax instances across processes, on SQLite or PostgreSQL"
)]
struct Cli {
    /// Log as JSON lines (`json`) or as text (`text`); the filter is
    /// RUST_LOG's (`info` by default).
    #[arg(
        long,
        env = "CLAX_FLEET_LOG_FORMAT",
        value_enum,
        default_value = "text",
        global = true
    )]
    log_format: LogFormat,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    Sqlite,
    Postgres,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum PgTls {
    Off,
    On,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve the catalog's instances.
    Serve(Box<Serve>),
    /// Validate a catalog and list its instances.
    CheckCatalog {
        file: PathBuf,
        /// Check it for the SQLite backend (its shards and each instance's).
        #[arg(long)]
        sqlite: bool,
    },
    /// List which process runs which instance, across the fleet.
    Status(Box<Status>),
}

#[derive(clap::Args)]
struct Pg {
    /// PostgreSQL: the connection string or URI. Prefer the environment to
    /// the command line, which other processes can read.
    #[arg(long, env = "CLAX_POSTGRES_URL", hide_env_values = true)]
    postgres_url: Option<String>,
    /// PostgreSQL: TLS to the server, required when on (never a fallback
    /// to plain text); a URL whose `sslmode` says otherwise is refused.
    #[arg(long, env = "CLAX_POSTGRES_TLS", value_enum, default_value = "off")]
    postgres_tls: PgTls,
    /// PostgreSQL: a PEM file of certificates to trust beside the public
    /// roots (a managed database's own CA).
    #[arg(long, env = "CLAX_POSTGRES_CA")]
    postgres_ca: Option<PathBuf>,
}

impl Pg {
    fn tls(&self) -> clax_fleet::pg::Tls {
        match self.postgres_tls {
            PgTls::Off => clax_fleet::pg::Tls::Off,
            PgTls::On => clax_fleet::pg::Tls::On {
                ca_file: self.postgres_ca.clone(),
            },
        }
    }

    /// A pool of `size` connections named `name`.
    fn pool(&self, name: &str, size: usize) -> anyhow::Result<Arc<clax_fleet::pg::Pool>> {
        let url = self.postgres_url.as_deref().ok_or_else(|| {
            anyhow::anyhow!("the PostgreSQL backend needs --postgres-url (CLAX_POSTGRES_URL)")
        })?;
        let tls = self.tls();
        let config = clax_fleet::pg::config(url, &tls).map_err(anyhow::Error::msg)?;
        clax_fleet::pg::Pool::connect(name, config, &tls, size)
            .map_err(|e| anyhow::anyhow!("PostgreSQL: {e}"))
    }
}

#[derive(clap::Args)]
struct Serve {
    /// Where the instances keep their data.
    #[arg(long, env = "CLAX_FLEET_BACKEND", value_enum)]
    backend: Backend,
    /// The catalog: the instances served, reloaded as it changes.
    #[arg(
        long,
        env = "CLAX_FLEET_CATALOG",
        default_value = "/etc/clax/fleet.toml"
    )]
    catalog: PathBuf,
    /// The instances' homes go under `<data-dir>/instances`.
    #[arg(long, env = "CLAX_FLEET_DATA_DIR", default_value = "/var/lib/clax")]
    data_dir: PathBuf,
    /// Where the instances are served, to clients and to the other
    /// processes.
    #[arg(long, env = "CLAX_FLEET_LISTEN", default_value = "0.0.0.0:7480")]
    listen: SocketAddr,
    /// Where `/healthz`, `/readyz`, `/status` and `/metrics` are served.
    #[arg(long, env = "CLAX_FLEET_ADMIN_LISTEN", default_value = "0.0.0.0:7481")]
    admin_listen: SocketAddr,
    /// This process's name in the fleet (the pod's).
    #[arg(long, env = "POD_NAME")]
    pod: Option<String>,
    /// Where the other processes reach this one, `host:port` (the pod's IP
    /// and the listen port when absent).
    #[arg(long, env = "CLAX_FLEET_ADVERTISE")]
    advertise: Option<String>,
    /// This pod's IP, for the advertised address.
    #[arg(long, env = "POD_IP")]
    pod_ip: Option<String>,
    /// SQLite: this process's shard (the ordinal ending the pod's name when
    /// absent). The number of shards is the catalog's.
    #[arg(long, env = "CLAX_FLEET_ORDINAL")]
    ordinal: Option<u32>,
    /// SQLite: where shard `{shard}`'s process is reached.
    #[arg(
        long,
        env = "CLAX_FLEET_PEER",
        default_value = "clax-{shard}.clax-peers:7480"
    )]
    peer: String,
    /// The key forwards between the fleet's processes carry, so a client
    /// cannot pass for one. On PostgreSQL it is kept in the database when
    /// unset; on SQLite every shard needs the same one.
    #[arg(long, env = "CLAX_FLEET_PEER_KEY", hide_env_values = true)]
    peer_key: Option<String>,
    #[command(flatten)]
    pg: Pg,
    /// PostgreSQL: connections in the data pool. Every pod holds this many
    /// and two more; keep pods x (pool + 2) under the server's
    /// max_connections.
    #[arg(long, env = "CLAX_POSTGRES_POOL", default_value_t = 8)]
    postgres_pool: usize,
    /// PostgreSQL: SQLite's page cache per instance database, in MiB.
    #[arg(long, env = "CLAX_FLEET_DB_CACHE_MIB", default_value_t = 32)]
    db_cache_mib: u64,
    /// PostgreSQL: the most one transaction of an instance database may
    /// write, in MiB (a migration that rebuilds a table writes it whole).
    #[arg(long, env = "CLAX_FLEET_DB_MAX_TRANSACTION_MIB", default_value_t = 256)]
    db_max_transaction_mib: u64,
    /// How long, after SIGTERM, to keep serving before stopping instances.
    #[arg(long, env = "CLAX_FLEET_DRAIN_DELAY", default_value = "5")]
    drain_delay_secs: u64,
    /// The memory all of this process's SQLite databases aim to stay under,
    /// in MiB (SQLite's soft heap limit: past it, page caches give memory
    /// back); 0 for none. Size it under the pod's memory limit.
    #[arg(long, env = "CLAX_FLEET_SQLITE_HEAP_MIB", default_value_t = 1024)]
    sqlite_heap_mib: u64,
}

#[derive(clap::Args)]
struct Status {
    /// Where the instances keep their data.
    #[arg(long, env = "CLAX_FLEET_BACKEND", value_enum)]
    backend: Backend,
    /// SQLite: the catalog, for the number of shards.
    #[arg(
        long,
        env = "CLAX_FLEET_CATALOG",
        default_value = "/etc/clax/fleet.toml"
    )]
    catalog: PathBuf,
    /// SQLite: where shard `{shard}`'s admin port is reached.
    #[arg(
        long,
        env = "CLAX_FLEET_ADMIN_PEER",
        default_value = "clax-{shard}.clax-peers:7481"
    )]
    admin_peer: String,
    /// SQLite: the fleet's peer key, which each shard's `/status` asks for.
    #[arg(long, env = "CLAX_FLEET_PEER_KEY", hide_env_values = true)]
    peer_key: Option<String>,
    #[command(flatten)]
    pg: Pg,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    match cli.log_format {
        LogFormat::Text => tracing_subscriber::fmt().with_env_filter(filter).init(),
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_current_span(true)
            .with_env_filter(filter)
            .init(),
    }
    let r = match cli.cmd {
        Cmd::CheckCatalog { file, sqlite } => check_catalog(&file, sqlite),
        Cmd::Serve(s) => serve(*s),
        Cmd::Status(s) => status(*s),
    };
    match r {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("clax-fleet: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn check_catalog(file: &std::path::Path, sqlite: bool) -> anyhow::Result<()> {
    let c = Catalog::load(file, sqlite).map_err(anyhow::Error::msg)?;
    if let Some(n) = c.shards {
        println!("shards: {n}");
    }
    for i in &c.instances {
        let shard = i.shard.map(|s| format!(" shard {s}")).unwrap_or_default();
        let default = if i.default { " (default)" } else { "" };
        let paused = if i.paused { " (paused)" } else { "" };
        println!("{}{shard}{default}{paused}: {}", i.name, i.origin);
    }
    Ok(())
}

/// The ordinal a StatefulSet pod's name ends with (`clax-3` → 3).
fn ordinal_of(pod: &str) -> Option<u32> {
    pod.rsplit_once('-')?.1.parse().ok()
}

/// `CLAX_KEK`: 64 hex digits, or nothing.
fn kek_from_env() -> anyhow::Result<Option<[u8; 32]>> {
    let Ok(hex) = std::env::var("CLAX_KEK") else {
        return Ok(None);
    };
    let hex = hex.trim();
    let bytes: Option<Vec<u8>> = (hex.len() == 64 && hex.is_ascii())
        .then(|| {
            (0..32)
                .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
                .collect()
        })
        .flatten();
    let bytes = bytes.ok_or_else(|| anyhow::anyhow!("CLAX_KEK is not 64 hex digits"))?;
    Ok(Some(bytes.try_into().expect("32 bytes")))
}

fn random_key() -> String {
    clax_core::secret::random_bytes::<32>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn serve(a: Serve) -> anyhow::Result<()> {
    let pod = match a.pod {
        Some(p) => p,
        None => nix_hostname()?,
    };
    let advertise = match (a.advertise, a.pod_ip) {
        (Some(addr), _) => addr,
        (None, Some(ip)) if ip.contains(':') => format!("[{ip}]:{}", a.listen.port()),
        (None, Some(ip)) => format!("{ip}:{}", a.listen.port()),
        (None, None) => format!("{pod}:{}", a.listen.port()),
    };
    let kek = kek_from_env()?;
    if a.sqlite_heap_mib > 0 {
        // Process-wide: one bound for every instance's caches together.
        rusqlite::Connection::open_in_memory()?.query_row(
            &format!("PRAGMA soft_heap_limit = {}", a.sqlite_heap_mib << 20),
            [],
            |_| Ok(()),
        )?;
    }
    let rt = tokio::runtime::Runtime::new()?;
    let (placement, storage, peer_key) = match a.backend {
        Backend::Sqlite => {
            let ordinal = a.ordinal.or_else(|| ordinal_of(&pod)).ok_or_else(|| {
                anyhow::anyhow!("no --ordinal, and the pod name {pod} does not end in one")
            })?;
            let key = match a.peer_key {
                Some(k) => k,
                None => {
                    tracing::warn!(
                        "no CLAX_FLEET_PEER_KEY: forwards between shards are not told from a client's request (their instance is found by Host alone); give every shard the same key"
                    );
                    random_key()
                }
            };
            (
                Placement::Shards {
                    ordinal,
                    peers: Peers::Template(a.peer),
                },
                Storage::Disk,
                key,
            )
        }
        Backend::Postgres => {
            anyhow::ensure!(
                kek.is_some(),
                "the PostgreSQL backend needs CLAX_KEK (64 hex digits): instances move between processes, and every one must open the secrets the others sealed"
            );
            let pool = a.pg.pool("data", a.postgres_pool)?;
            clax_fleet::pg::migrate(&pool).map_err(|e| anyhow::anyhow!("PostgreSQL: {e}"))?;
            let key = clax_fleet::pg::peer_key(&pool, a.peer_key.as_deref())
                .map_err(|e| anyhow::anyhow!("PostgreSQL: {e}"))?;
            let coord_pool = a.pg.pool("coord", COORD_POOL)?;
            let coord = Arc::new(clax_fleet::coord::PgCoordinator::new(coord_pool));
            (Placement::Leases(coord), Storage::Postgres(pool), key)
        }
    };
    let fleet = Fleet::new(FleetConfig {
        pod: pod.clone(),
        advertise: advertise.clone(),
        data_dir: a.data_dir,
        catalog: a.catalog,
        version: env!("CARGO_PKG_VERSION"),
        web: web_ui(),
        placement,
        storage,
        kek,
        peer_key,
        db_limits: clax_pagevfs::Limits {
            cache_bytes: a.db_cache_mib << 20,
            max_transaction_bytes: a.db_max_transaction_mib << 20,
        },
        port: a.listen.port(),
    })?;
    let drain_delay = Duration::from_secs(a.drain_delay_secs);
    let r = rt.block_on(run(fleet, a.listen, a.admin_listen, drain_delay));
    tracing::info!(%pod, "stopped");
    rt.shutdown_background();
    r
}

async fn run(
    fleet: Arc<Fleet>,
    listen: SocketAddr,
    admin_listen: SocketAddr,
    drain_delay: Duration,
) -> anyhow::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    // Leases an earlier run under this pod's name held are free at once.
    fleet.release_own_leases().await;
    // The first tick joins the membership before the first request.
    fleet.tick().await;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let admin = tokio::net::TcpListener::bind(admin_listen).await?;
    tracing::info!(%listen, %admin_listen, instances = fleet.catalog().instances.len(), "clax-fleet serving");
    let ticker = {
        let fleet = fleet.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(clax_fleet::coord::HEARTBEAT_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            every.tick().await;
            loop {
                every.tick().await;
                fleet.tick().await;
            }
        })
    };
    let admin_server = {
        let app = fleet
            .admin_router()
            .into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(async move { axum::serve(admin, app).await })
    };
    let (stopping_tx, stopping_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(
        clax_http::auth::TunedListener(listener),
        fleet
            .router()
            .into_make_service_with_connect_info::<clax_http::auth::Conn>(),
    )
    .with_graceful_shutdown(async move {
        let _ = stopping_rx.await;
    });
    let mut server = tokio::spawn(async move { server.await });
    tokio::select! {
        _ = sigterm.recv() => tracing::info!("SIGTERM: draining"),
        _ = sigint.recv() => tracing::info!("SIGINT: draining"),
        r = &mut server => {
            ticker.abort();
            admin_server.abort();
            return r?.map_err(Into::into);
        }
    }
    fleet.begin_draining();
    tokio::time::sleep(drain_delay).await;
    // The ticks go on renewing the leases of what still runs while the
    // instances stop; once all have, nothing is left for them.
    fleet.drain().await;
    ticker.abort();
    let _ = stopping_tx.send(());
    match tokio::time::timeout(CONNECTIONS_DRAIN, server).await {
        Ok(r) => r??,
        Err(_) => tracing::warn!("connections did not end in time"),
    }
    admin_server.abort();
    Ok(())
}

fn status(a: Status) -> anyhow::Result<()> {
    match a.backend {
        Backend::Postgres => {
            let pool = a.pg.pool("status", 1)?;
            let rows = pool
                .run(|c| {
                    let rows = c
                        .query(
                            "SELECT l.instance, l.holder, l.addr, l.epoch,
                                    extract(epoch FROM l.expires - now())::float8,
                                    m.pod IS NOT NULL
                             FROM clax_fleet_leases l
                             LEFT JOIN clax_fleet_members m
                               ON m.pod = l.holder AND m.seen >= now() - interval '20 seconds'
                             ORDER BY l.instance",
                            &[],
                        )
                        .map_err(clax_fleet::pg::err)?;
                    Ok(rows
                        .into_iter()
                        .map(|r| {
                            let (i, h, ad, e, left, live): (
                                String,
                                String,
                                String,
                                i64,
                                f64,
                                bool,
                            ) = (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4), r.get(5));
                            (i, h, ad, e, left, live)
                        })
                        .collect::<Vec<_>>())
                })
                .map_err(anyhow::Error::msg)?;
            println!(
                "{:<24} {:<28} {:<22} {:>6}  lease",
                "instance", "pod", "address", "epoch"
            );
            for (i, h, ad, e, left, live) in rows {
                let lease = if left > 0.0 {
                    format!(
                        "held, {left:.0}s left{}",
                        if live { "" } else { " (pod not seen)" }
                    )
                } else {
                    "free".to_string()
                };
                println!("{i:<24} {h:<28} {ad:<22} {e:>6}  {lease}");
            }
            Ok(())
        }
        Backend::Sqlite => {
            let catalog = Catalog::load(&a.catalog, true).map_err(anyhow::Error::msg)?;
            let key = a.peer_key.ok_or_else(|| {
                anyhow::anyhow!(
                    "each shard's /status asks for the peer key: set CLAX_FLEET_PEER_KEY"
                )
            })?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let client: hyper_util::client::legacy::Client<_, axum::body::Body> =
                    hyper_util::client::legacy::Client::builder(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .build_http();
                for shard in 0..catalog.shards.unwrap_or(0) {
                    let addr = a.admin_peer.replace("{shard}", &shard.to_string());
                    let req = axum::http::Request::get(format!("http://{addr}/status"))
                        .header(clax_fleet::proxy::KEY_HEADER, &key)
                        .body(axum::body::Body::empty())?;
                    match client.request(req).await {
                        Ok(resp) => {
                            use http_body_util::BodyExt;
                            let body = resp.into_body().collect().await?.to_bytes();
                            let v: serde_json::Value =
                                serde_json::from_slice(&body).unwrap_or_default();
                            println!(
                                "shard {shard} ({}): ready {}, running {}",
                                v["pod"].as_str().unwrap_or("?"),
                                v["ready"],
                                v["running"]
                            );
                        }
                        Err(e) => println!("shard {shard} ({addr}): did not answer: {e}"),
                    }
                }
                anyhow::Ok(())
            })
        }
    }
}

fn nix_hostname() -> anyhow::Result<String> {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| anyhow::anyhow!("no --pod (POD_NAME), and HOSTNAME is not set"))
}

/// [`clax_embedded::web_ui`] as a tree.
struct WebUi;

impl FileTree for WebUi {
    fn get(&self, path: &str) -> Option<EmbeddedFile> {
        clax_embedded::web_ui(path).map(|f| EmbeddedFile {
            data: f.data,
            sha256: f.sha256,
        })
    }

    fn paths(&self) -> Vec<String> {
        clax_embedded::web_ui_paths()
    }
}

/// The web UI the instances serve: the checkout's `web/dist` from disk where
/// the build reads it there (a debug build that does not embed it), else the
/// embed.
fn web_ui() -> Arc<dyn FileTree> {
    match clax_embedded::web_ui_dir() {
        Some(dir) => Arc::new(DiskTree::new(dir)),
        None => Arc::new(WebUi),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_statefulset_pod_name_ends_in_its_ordinal() {
        assert_eq!(ordinal_of("clax-0"), Some(0));
        assert_eq!(ordinal_of("clax-fleet-12"), Some(12));
        assert_eq!(ordinal_of("clax"), None);
        assert_eq!(ordinal_of("clax-abc12"), None);
    }

    /// `deploy/k8s`, from the repository root.
    fn manifests() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/k8s")
    }

    fn files(dir: &std::path::Path, ext: &str, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                files(&p, ext, out);
            } else if p.extension().is_some_and(|x| x == ext) {
                out.push(p);
            }
        }
    }

    /// Every variable the manifests give the container is one `serve`
    /// reads (a flag's, or `CLAX_KEK`), or an instance's setup secret,
    /// which the catalog names.
    #[test]
    fn the_manifests_set_only_variables_the_binary_reads() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let mut known: Vec<String> = cmd
            .get_subcommands()
            .flat_map(|c| c.get_arguments())
            .chain(cmd.get_arguments())
            .filter_map(|a| a.get_env())
            .map(|e| e.to_string_lossy().into_owned())
            .collect();
        known.push("CLAX_KEK".into());
        let mut yamls = Vec::new();
        files(&manifests(), "yaml", &mut yamls);
        assert!(yamls.len() >= 8, "{yamls:?}");
        let mut seen = 0;
        for path in yamls {
            let text = std::fs::read_to_string(&path).unwrap();
            if !text.contains("name: clax-fleet") {
                continue;
            }
            for line in text.lines() {
                let Some(name) = line.trim().strip_prefix("- name: ") else {
                    continue;
                };
                if name.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                    assert!(
                        known.iter().any(|k| k == name),
                        "{} sets {name}, which clax-fleet serve does not read",
                        path.display()
                    );
                    seen += 1;
                }
            }
        }
        assert!(
            seen >= 10,
            "only {seen} variables found: the scan missed them"
        );
    }

    /// The example catalog is valid for both backends, and its instances'
    /// setup secrets are in the variables the postgres/ overlay's comments
    /// put in the Secret.
    #[test]
    fn the_example_catalog_is_valid_for_both_backends() {
        let path = manifests().join("base/fleet.toml");
        let pg = Catalog::load(&path, false).unwrap();
        let sqlite = Catalog::load(&path, true).unwrap();
        assert_eq!(pg, sqlite);
        let secret =
            std::fs::read_to_string(manifests().join("postgres/kustomization.yaml")).unwrap();
        for i in &pg.instances {
            let config = clax_core::config::HomeConfig::from_text(
                path.clone(),
                &i.config,
                std::iter::empty(),
            )
            .unwrap();
            let env = config.hosted().unwrap().unwrap().setup_secret_env;
            assert!(
                secret.contains(&env),
                "{env} is not in the Secret's example"
            );
        }
    }
}
