//! `clax-fleet`: the server of a Kubernetes deployment of Clax (see the
//! library's docs, `docs/kubernetes.md` and `deploy/k8s`).
//!
//! - `clax-fleet serve`: serves the catalog's instances until SIGTERM, then
//!   drains: stops taking new work at once (`/readyz` answers 503), keeps
//!   serving for `--drain-delay` while the Service stops sending requests
//!   here, then stops every instance (releasing its lease) and ends the
//!   connections left within [`CONNECTIONS_DRAIN`].
//! - `clax-fleet check-catalog <file>`: validates a catalog and prints its
//!   instances, as `serve` would read it.

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

#[derive(Parser)]
#[command(
    name = "clax-fleet",
    version,
    about = "Clax instances across processes, on SQLite or PostgreSQL"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
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
        /// Check it for the SQLite backend with this many shards.
        #[arg(long)]
        shards: Option<u32>,
    },
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
    /// Where `/healthz`, `/readyz` and `/status` are served.
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
    /// SQLite: how many shards (StatefulSet replicas) there are.
    #[arg(long, env = "CLAX_FLEET_SHARDS")]
    shards: Option<u32>,
    /// SQLite: this process's shard (the ordinal ending the pod's name when
    /// absent).
    #[arg(long, env = "CLAX_FLEET_ORDINAL")]
    ordinal: Option<u32>,
    /// SQLite: where shard `{shard}`'s process is reached.
    #[arg(
        long,
        env = "CLAX_FLEET_PEER",
        default_value = "clax-{shard}.clax-peers:7480"
    )]
    peer: String,
    /// PostgreSQL: the connection string or URI. Prefer the environment to
    /// the command line, which other processes can read.
    #[arg(long, env = "CLAX_POSTGRES_URL", hide_env_values = true)]
    postgres_url: Option<String>,
    /// PostgreSQL: TLS to the server.
    #[arg(long, env = "CLAX_POSTGRES_TLS", value_enum, default_value = "off")]
    postgres_tls: PgTls,
    /// PostgreSQL: a PEM file of certificates to trust beside the public
    /// roots (a managed database's own CA).
    #[arg(long, env = "CLAX_POSTGRES_CA")]
    postgres_ca: Option<PathBuf>,
    /// PostgreSQL: connections in the pool.
    #[arg(long, env = "CLAX_POSTGRES_POOL", default_value_t = 16)]
    postgres_pool: usize,
    /// How long, after SIGTERM, to keep serving before stopping instances.
    #[arg(long, env = "CLAX_FLEET_DRAIN_DELAY", default_value = "5")]
    drain_delay_secs: u64,
}

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::CheckCatalog { file, shards } => check_catalog(&file, shards),
        Cmd::Serve(s) => serve(*s),
    };
    match r {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("clax-fleet: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn check_catalog(file: &std::path::Path, shards: Option<u32>) -> anyhow::Result<()> {
    let c = Catalog::load(file, shards).map_err(anyhow::Error::msg)?;
    for i in &c.instances {
        let shard = i.shard.map(|s| format!(" shard {s}")).unwrap_or_default();
        let default = if i.default { " (default)" } else { "" };
        println!("{}{shard}{default}: {}", i.name, i.origin);
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
    let rt = tokio::runtime::Runtime::new()?;
    let (placement, storage, shards) = match a.backend {
        Backend::Sqlite => {
            let shards = a.shards.ok_or_else(|| {
                anyhow::anyhow!("the SQLite backend needs --shards (CLAX_FLEET_SHARDS)")
            })?;
            let ordinal = a.ordinal.or_else(|| ordinal_of(&pod)).ok_or_else(|| {
                anyhow::anyhow!("no --ordinal, and the pod name {pod} does not end in one")
            })?;
            anyhow::ensure!(
                ordinal < shards,
                "ordinal {ordinal} is past the {shards} shards"
            );
            (
                Placement::Shards {
                    ordinal,
                    peers: Peers::Template(a.peer),
                },
                Storage::Disk,
                Some(shards),
            )
        }
        Backend::Postgres => {
            let url = a.postgres_url.ok_or_else(|| {
                anyhow::anyhow!("the PostgreSQL backend needs --postgres-url (CLAX_POSTGRES_URL)")
            })?;
            anyhow::ensure!(
                kek.is_some(),
                "the PostgreSQL backend needs CLAX_KEK (64 hex digits): instances move between processes, and every one must open the secrets the others sealed"
            );
            let tls = match a.postgres_tls {
                PgTls::Off => clax_fleet::pg::Tls::Off,
                PgTls::On => clax_fleet::pg::Tls::On {
                    ca_file: a.postgres_ca,
                },
            };
            let pool = clax_fleet::pg::Pool::connect(&url, &tls, a.postgres_pool)
                .map_err(|e| anyhow::anyhow!("PostgreSQL: {e}"))?;
            clax_fleet::pg::migrate(&pool).map_err(|e| anyhow::anyhow!("PostgreSQL: {e}"))?;
            let coord = Arc::new(clax_fleet::coord::PgCoordinator::new(pool.clone()));
            (Placement::Leases(coord), Storage::Postgres(pool), None)
        }
    };
    let fleet = Fleet::new(FleetConfig {
        pod: pod.clone(),
        advertise: advertise.clone(),
        data_dir: a.data_dir,
        catalog: a.catalog,
        shards,
        version: env!("CARGO_PKG_VERSION"),
        web: web_ui(),
        placement,
        storage,
        kek,
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
    // The first tick joins the membership before the first request.
    fleet.tick().await;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let admin = tokio::net::TcpListener::bind(admin_listen).await?;
    tracing::info!(%listen, %admin_listen, instances = fleet.catalog().instances.len(), "clax-fleet serving");
    let ticker = {
        let fleet = fleet.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(clax_fleet::coord::HEARTBEAT_EVERY);
            every.tick().await;
            loop {
                every.tick().await;
                fleet.tick().await;
            }
        })
    };
    let admin_server = {
        let app = fleet.admin_router();
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
    ticker.abort();
    fleet.drain().await;
    let _ = stopping_tx.send(());
    match tokio::time::timeout(CONNECTIONS_DRAIN, server).await {
        Ok(r) => r??,
        Err(_) => tracing::warn!("connections did not end in time"),
    }
    admin_server.abort();
    Ok(())
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
}
