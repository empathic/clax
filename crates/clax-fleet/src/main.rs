//! `clax-fleet`: the server of a Kubernetes deployment of Clax (see the
//! library's docs, `docs/kubernetes.md` and `deploy/k8s`).
//!
//! - `clax-fleet serve`: serves the catalog's instances until SIGTERM, then
//!   drains: stops taking new work at once (`/readyz` answers 503), keeps
//!   serving for `--drain-delay-secs` while the Service stops sending requests
//!   here, then stops every instance and ends the connections left within
//!   [`CONNECTIONS_DRAIN`].
//! - `clax-fleet check-catalog <file>`: validates a catalog and prints its
//!   instances, as `serve` would read it.
//! - `clax-fleet status`: which process runs which instance, across the
//!   fleet, from each shard's `/status`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use clax_core::embedded::{DiskTree, EmbeddedFile, FileTree};
use clax_fleet::catalog::Catalog;
use clax_fleet::fleet::{Fleet, FleetConfig, Peers, Shard};

/// How long open connections may take to end once every instance stopped.
const CONNECTIONS_DRAIN: Duration = Duration::from_secs(10);

/// How often the catalog is read again.
const TICK_EVERY: Duration = Duration::from_secs(5);

#[derive(Parser)]
#[command(
    name = "clax-fleet",
    version,
    about = "Clax instances across processes, each shard's on its own volume"
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

#[derive(Subcommand)]
enum Cmd {
    /// Serve the catalog's instances.
    Serve(Box<Serve>),
    /// Validate a catalog and list its instances.
    CheckCatalog { file: PathBuf },
    /// List which process runs which instance, across the fleet.
    Status(Box<Status>),
}

#[derive(clap::Args)]
struct Serve {
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
    /// Where `/healthz`, `/readyz`, `/metrics` and, to a caller with the
    /// peer key, `/status` are served.
    #[arg(long, env = "CLAX_FLEET_ADMIN_LISTEN", default_value = "0.0.0.0:7481")]
    admin_listen: SocketAddr,
    /// Where the admin routes are served to this pod alone, `/status`
    /// without the peer key (`kubectl port-forward`): a loopback address.
    #[arg(
        long,
        env = "CLAX_FLEET_LOCAL_ADMIN_LISTEN",
        default_value = "127.0.0.1:7482"
    )]
    local_admin_listen: SocketAddr,
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
    /// This process's shard (the ordinal ending the pod's name when
    /// absent). The number of shards is the catalog's.
    #[arg(long, env = "CLAX_FLEET_ORDINAL")]
    ordinal: Option<u32>,
    /// Where shard `{shard}`'s process is reached.
    #[arg(
        long,
        env = "CLAX_FLEET_PEER",
        default_value = "clax-{shard}.clax-peers:7480"
    )]
    peer: String,
    /// The key forwards between the fleet's processes carry, so a client
    /// cannot pass for one; every shard needs the same one.
    #[arg(long, env = "CLAX_FLEET_PEER_KEY", hide_env_values = true)]
    peer_key: Option<String>,
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
    /// The catalog, for the number of shards.
    #[arg(
        long,
        env = "CLAX_FLEET_CATALOG",
        default_value = "/etc/clax/fleet.toml"
    )]
    catalog: PathBuf,
    /// Where shard `{shard}`'s admin port is reached.
    #[arg(
        long,
        env = "CLAX_FLEET_ADMIN_PEER",
        default_value = "clax-{shard}.clax-peers:7481"
    )]
    admin_peer: String,
    /// The fleet's peer key, which each shard's `/status` asks for.
    #[arg(long, env = "CLAX_FLEET_PEER_KEY", hide_env_values = true)]
    peer_key: Option<String>,
}

fn main() -> std::process::ExitCode {
    clax_core::set_build_commit(env!("CLAX_BUILD_COMMIT"));
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
    let logged = fatal_is_logged(&cli.cmd, cli.log_format);
    let r = match cli.cmd {
        Cmd::CheckCatalog { file } => check_catalog(&file),
        Cmd::Serve(s) => serve(*s),
        Cmd::Status(s) => status(*s),
    };
    match r {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) if logged => {
            tracing::error!(error = %format!("{e:#}"), "clax-fleet failed");
            std::process::ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("clax-fleet: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Whether the error `cmd` ends with goes to the log rather than to stderr
/// as text: for `serve` logging JSON lines, whose log pipeline must read
/// the one line that says why a pod crash-loops. The other commands'
/// errors are a person's to read.
fn fatal_is_logged(cmd: &Cmd, format: LogFormat) -> bool {
    matches!(cmd, Cmd::Serve(_)) && format == LogFormat::Json
}

fn check_catalog(file: &std::path::Path) -> anyhow::Result<()> {
    let c = Catalog::load(file).map_err(anyhow::Error::msg)?;
    println!("shards: {}", c.shards);
    for i in &c.instances {
        let default = if i.default { " (default)" } else { "" };
        let paused = if i.paused { " (paused)" } else { "" };
        println!(
            "{} shard {}{default}{paused}: {}",
            i.name, i.shard, i.origin
        );
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

/// The peer key, which every shard must be given alike: one a shard made
/// up for itself would be no other shard's, so none would believe another's
/// forwards.
fn peer_key(given: Option<String>) -> anyhow::Result<String> {
    given.filter(|k| !k.trim().is_empty()).ok_or_else(|| {
        anyhow::anyhow!(
            "clax-fleet needs CLAX_FLEET_PEER_KEY, the same on every shard (the clax-secrets Secret; docs/kubernetes.md, Deploying)"
        )
    })
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
    anyhow::ensure!(
        a.local_admin_listen.ip().is_loopback(),
        "--local-admin-listen (CLAX_FLEET_LOCAL_ADMIN_LISTEN) {} is not a loopback address: what it serves asks for no key",
        a.local_admin_listen
    );
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
    let ordinal = a.ordinal.or_else(|| ordinal_of(&pod)).ok_or_else(|| {
        anyhow::anyhow!("no --ordinal, and the pod name {pod} does not end in one")
    })?;
    let peer_key = peer_key(a.peer_key)?;
    let fleet = Fleet::new(FleetConfig {
        pod: pod.clone(),
        advertise: advertise.clone(),
        data_dir: a.data_dir,
        catalog: a.catalog,
        version: env!("CARGO_PKG_VERSION"),
        web: web_ui(),
        shard: Shard {
            ordinal,
            peers: Peers::Template(a.peer),
        },
        kek,
        peer_key,
        port: a.listen.port(),
        clock: Arc::new(clax_core::working::SystemClock),
    })?;
    let drain_delay = Duration::from_secs(a.drain_delay_secs);
    let listens = Listens {
        instances: a.listen,
        admin: a.admin_listen,
        local_admin: a.local_admin_listen,
    };
    let r = rt.block_on(run(fleet, listens, drain_delay));
    tracing::info!(%pod, "stopped");
    rt.shutdown_background();
    r
}

/// Where `serve` listens.
struct Listens {
    instances: SocketAddr,
    admin: SocketAddr,
    local_admin: SocketAddr,
}

async fn run(fleet: Arc<Fleet>, listens: Listens, drain_delay: Duration) -> anyhow::Result<()> {
    let Listens {
        instances: listen,
        admin: admin_listen,
        local_admin: local_admin_listen,
    } = listens;
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    fleet.tick().await;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let admin = tokio::net::TcpListener::bind(admin_listen).await?;
    let local_admin = tokio::net::TcpListener::bind(local_admin_listen).await?;
    tracing::info!(%listen, %admin_listen, %local_admin_listen, instances = fleet.catalog().instances.len(), "clax-fleet serving");
    let ticker = {
        let fleet = fleet.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(TICK_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            every.tick().await;
            loop {
                every.tick().await;
                fleet.tick().await;
            }
        })
    };
    let admin_server = {
        let (app, local) = (fleet.admin_router(), fleet.local_admin_router());
        tokio::spawn(async move {
            use std::future::IntoFuture;
            tokio::try_join!(
                axum::serve(admin, app).into_future(),
                axum::serve(local_admin, local).into_future()
            )
        })
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
    let catalog = Catalog::load(&a.catalog).map_err(anyhow::Error::msg)?;
    let key = a.peer_key.ok_or_else(|| {
        anyhow::anyhow!("each shard's /status asks for the peer key: set CLAX_FLEET_PEER_KEY")
    })?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let client = status_client();
        let shards = (0..catalog.shards).map(|shard| {
            let addr = a.admin_peer.replace("{shard}", &shard.to_string());
            let (client, key) = (&client, &key);
            async move {
                let line = shard_status(client, &addr, key, STATUS_TIMEOUT).await;
                format!("shard {shard} {line}")
            }
        });
        for line in futures::future::join_all(shards).await {
            println!("{line}");
        }
        anyhow::Ok(())
    })
}

/// How long `clax-fleet status` waits for each shard's `/status`.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

type StatusClient = hyper_util::client::legacy::Client<
    hyper_util::client::legacy::connect::HttpConnector,
    axum::body::Body,
>;

fn status_client() -> StatusClient {
    let mut connector = hyper_util::client::legacy::connect::HttpConnector::new();
    connector.set_connect_timeout(Some(clax_fleet::proxy::CONNECT_TIMEOUT));
    hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector)
}

/// One shard's line of `clax-fleet status`, from its `/status` at `addr`
/// asked with the peer key `key`: its pod, readiness and running
/// instances, or why there are none (refused, another status, no answer
/// within `timeout`).
async fn shard_status(client: &StatusClient, addr: &str, key: &str, timeout: Duration) -> String {
    use http_body_util::BodyExt;
    let ask = async {
        let req = axum::http::Request::get(format!("http://{addr}/status"))
            .header(clax_fleet::proxy::KEY_HEADER, key)
            .body(axum::body::Body::empty())
            .map_err(|e| e.to_string())?;
        let resp = client.request(req).await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .to_bytes();
        Ok::<_, String>((status, body))
    };
    match tokio::time::timeout(timeout, ask).await {
        Err(_) => format!("({addr}): did not answer within {timeout:?}"),
        Ok(Err(e)) => format!("({addr}): did not answer: {e}"),
        Ok(Ok((status, _))) if status == axum::http::StatusCode::FORBIDDEN => {
            format!("({addr}): refused: 403, check CLAX_FLEET_PEER_KEY")
        }
        Ok(Ok((status, _))) if status != axum::http::StatusCode::OK => {
            format!("({addr}): answered {status}")
        }
        Ok(Ok((_, body))) => match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(v) => format!(
                "({}): ready {}, running {}",
                v["pod"].as_str().unwrap_or("?"),
                v["ready"],
                v["running"]
            ),
            Err(e) => format!("({addr}): its /status is not JSON: {e}"),
        },
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

    #[test]
    fn only_serve_logging_json_logs_its_fatal_error() {
        let serve = Cli::parse_from(["clax-fleet", "serve"]).cmd;
        let check = Cli::parse_from(["clax-fleet", "check-catalog", "f.toml"]).cmd;
        assert!(fatal_is_logged(&serve, LogFormat::Json));
        assert!(!fatal_is_logged(&serve, LogFormat::Text));
        assert!(!fatal_is_logged(&check, LogFormat::Json));
    }

    #[test]
    fn serving_needs_the_peer_key() {
        assert_eq!(peer_key(Some("k".into())).unwrap(), "k");
        for missing in [None, Some(String::new()), Some(" ".into())] {
            let err = peer_key(missing).unwrap_err().to_string();
            assert!(err.contains("needs CLAX_FLEET_PEER_KEY"), "{err}");
        }
    }

    /// A shard's line says when it refused the key, answered otherwise, or
    /// did not answer in time, never a live-looking `ready null`.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_shard_that_refuses_or_does_not_answer_says_so() {
        use axum::routing::get;
        let app = axum::Router::new().route(
            "/status",
            get(|| async { axum::http::StatusCode::FORBIDDEN }),
        );
        let refusing = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let refusing_addr = refusing.local_addr().unwrap().to_string();
        tokio::spawn(async move { axum::serve(refusing, app).await });
        let app = axum::Router::new().route(
            "/status",
            get(|| async {
                axum::Json(serde_json::json!({"pod": "clax-1", "ready": true, "running": ["acme"]}))
            }),
        );
        let answering = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let answering_addr = answering.local_addr().unwrap().to_string();
        tokio::spawn(async move { axum::serve(answering, app).await });
        // Takes the connection and never answers.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent_addr = silent.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((c, _)) = silent.accept().await {
                held.push(c);
            }
        });

        let client = status_client();
        let line = |addr: String| {
            let client = &client;
            async move { shard_status(client, &addr, "k", Duration::from_millis(100)).await }
        };
        let refused = line(refusing_addr).await;
        assert!(
            refused.ends_with("refused: 403, check CLAX_FLEET_PEER_KEY"),
            "{refused}"
        );
        assert_eq!(
            line(answering_addr).await,
            r#"(clax-1): ready true, running ["acme"]"#
        );
        let silent = line(silent_addr).await;
        assert!(silent.contains("did not answer within"), "{silent}");
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
            seen >= 5,
            "only {seen} variables found: the scan missed them"
        );
    }

    /// The example catalog is valid, and its instances' setup secrets are
    /// in the variables the sqlite/ overlay's comments put in the Secret.
    #[test]
    fn the_example_catalog_is_valid() {
        let path = manifests().join("base/fleet.toml");
        let catalog = Catalog::load(&path).unwrap();
        let secret =
            std::fs::read_to_string(manifests().join("sqlite/kustomization.yaml")).unwrap();
        for i in &catalog.instances {
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

    /// Every Secret a container reads (`envFrom: secretRef`) is
    /// `clax-secrets`, which the overlays' comments say to create, or one a
    /// kustomization's `secretGenerator` makes.
    #[test]
    fn every_secret_the_manifests_read_is_created_or_documented() {
        let mut yamls = Vec::new();
        files(&manifests(), "yaml", &mut yamls);
        let uncommented = |path: &std::path::Path| -> Vec<String> {
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .map(str::to_owned)
                .collect()
        };
        let mut known = vec!["clax-secrets".to_owned()];
        for path in &yamls {
            let lines = uncommented(path);
            let Some(start) = lines.iter().position(|l| l == "secretGenerator:") else {
                continue;
            };
            known.extend(
                lines[start + 1..]
                    .iter()
                    .take_while(|l| l.starts_with(' '))
                    .filter_map(|l| l.strip_prefix("  - name: "))
                    .map(|n| n.trim().to_owned()),
            );
        }
        let mut seen = 0;
        for path in &yamls {
            let lines = uncommented(path);
            for (i, line) in lines.iter().enumerate() {
                if !line.trim_end().ends_with("secretRef:") {
                    continue;
                }
                let name = lines[i + 1..]
                    .iter()
                    .find_map(|l| l.trim().strip_prefix("name: "))
                    .unwrap_or_else(|| panic!("{}: a secretRef without a name", path.display()));
                assert!(
                    known.iter().any(|k| k == name),
                    "{} reads Secret {name}, which nothing creates (known: {known:?})",
                    path.display()
                );
                seen += 1;
            }
        }
        assert!(
            seen >= 1,
            "only {seen} secretRefs found: the scan missed them"
        );
    }
}
