//! `clax serve --foreground`: the daemon in this process
//! ([`clax_commands::Env::serve`]).

use clax_commands::ServeArgs;
use clax_core::Home;
use clax_core::embedded::{DiskTree, FileTree};
use clax_server::serve::{ServeConfig, serve};
use std::sync::Arc;
use std::time::Duration;

/// Runs the daemon for `home` until it stops.
pub fn run(home: &Home, a: ServeArgs) -> anyhow::Result<()> {
    close_inherited_fds();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();
    let web: Arc<dyn FileTree> = match a.web_dist {
        Some(dir) => Arc::new(DiskTree::new(dir)),
        None => crate::bundle::web_ui(),
    };
    let rt = tokio::runtime::Runtime::new()?;
    let cfg = ServeConfig {
        home: home.clone(),
        bind: a.bind,
        stale_check_interval: Duration::from_secs(30),
        reap_interval: Duration::from_secs(60),
        port: a.port,
        version: a.version,
        codex: clax_realtime::push::CodexPush::from_env(
            std::env::var_os("CLAX_CODEX_BIN"),
            std::env::var_os("PATH").as_deref(),
        ),
        sample: Arc::new(clax_sample::Sampler::from_home(home.root(), |k| {
            std::env::var(k).ok()
        })),
        web,
        hosted: hosted(home)?,
        trusted_hosts: trusted_hosts(home)?,
        tailscale_identity: tailscale_identity(home)?,
        require_credential: a.require_credential,
        embed: a
            .embed
            .map(|(shell, content)| clax_http::state::EmbedOrigins { shell, content }),
        quickstart: clax_core::config::HomeConfig::load(home.root())
            .map(|c| c.gallery_quickstart())
            .unwrap_or(true),
        stop: clax_server::serve::StopOn::Signals,
        owner_tailscale_login: owner_tailscale_login(home)?,
        trusted_host_frames: trusted_host_frames(home)?,
    };
    let r = rt.block_on(serve(cfg, None));
    // Dropping the runtime would wait without limit for its blocking tasks:
    // a store call `serve` stopped waiting for would hold the exit, past
    // every signal. Leave them to the process exit instead.
    rt.shutdown_background();
    r
}

/// Hosted mode as the home's `[hosted]` sets it, or `None` without one. A
/// bad `[hosted]` stops the daemon: started as a local daemon, it would
/// serve the internet as a LAN.
fn hosted(home: &Home) -> anyhow::Result<Option<Arc<clax_http::hosted::Hosted>>> {
    let Some(c) = clax_core::config::HomeConfig::load(home.root())?.hosted()? else {
        return Ok(None);
    };
    let h = clax_http::hosted::Hosted::from_config(&c).ok_or_else(|| {
        anyhow::anyhow!("[hosted] public_origin {} is not usable", c.public_origin)
    })?;
    if !h.setup_open() {
        tracing::warn!(
            env = %c.setup_secret_env,
            "hosted mode: no setup secret in the environment, so /setup stays closed"
        );
    }
    tracing::info!(origin = %h.origin, "hosted mode");
    Ok(Some(Arc::new(h)))
}

/// The home's `[serve] trusted_hosts`. A bad list stops the daemon: it
/// trusts none of a list it cannot read.
fn trusted_hosts(home: &Home) -> anyhow::Result<Vec<String>> {
    let hosts = clax_core::config::HomeConfig::load(home.root())?.serve_trusted_hosts()?;
    if !hosts.is_empty() {
        tracing::info!(hosts = ?hosts, "the API also answers to [serve] trusted_hosts");
    }
    Ok(hosts)
}

/// The home's `[serve] tailscale_identity`. A bad value, or `true` with no
/// trusted hosts, stops the daemon rather than read the headers from anyone.
fn tailscale_identity(home: &Home) -> anyhow::Result<bool> {
    let on = clax_core::config::HomeConfig::load(home.root())?.serve_tailscale_identity()?;
    if on {
        tracing::info!("viewers through [serve] trusted_hosts are named by their tailnet login");
    }
    Ok(on)
}

/// The home's `[serve] owner_tailscale_login`. A bad value, or one without
/// `tailscale_identity = true`, stops the daemon.
fn owner_tailscale_login(home: &Home) -> anyhow::Result<Option<String>> {
    let login = clax_core::config::HomeConfig::load(home.root())?.serve_owner_tailscale_login()?;
    if let Some(l) = &login {
        tracing::info!(login = %l, "the tailnet login's browser through [serve] trusted_hosts is the owner's");
    }
    Ok(login)
}

/// The home's `[serve] trusted_host_frames`. A bad value, or `true` with
/// no trusted hosts, stops the daemon.
fn trusted_host_frames(home: &Home) -> anyhow::Result<bool> {
    let on = clax_core::config::HomeConfig::load(home.root())?.serve_trusted_host_frames()?;
    if on {
        tracing::info!("artifacts are framed from a path of [serve] trusted_hosts, sandboxed");
    }
    Ok(on)
}

/// The daemon outlives whoever started it, so any descriptor it inherits stays
/// open for its whole life. A spawner that is itself multi-threaded can leak
/// descriptors it never meant to pass: on platforms without `pipe2` (macOS)
/// std creates a pipe and marks it close-on-exec in a second call, and a fork
/// on another thread between the two copies the pipe into the child, which
/// hands it on to the daemon. A daemon holding the write end of someone else's
/// pipe keeps that pipe's reader from ever seeing EOF. Stdio (0–2) is kept:
/// the auto-start points it at `/dev/null` and the log.
///
/// Must run before the process opens anything of its own or starts threads:
/// it closes every descriptor numbered 3 and up.
fn close_inherited_fds() {
    #[cfg(target_os = "linux")]
    const FD_DIR: &str = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    const FD_DIR: &str = "/dev/fd";
    // Collect first, then close: the directory handle is itself a descriptor
    // in the listing, and is closed (by the drop) before the loop runs.
    let fds: Vec<std::os::fd::RawFd> = match std::fs::read_dir(FD_DIR) {
        Ok(entries) => entries
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .filter(|&fd| fd > 2)
            .collect(),
        Err(_) => return,
    };
    // The process is single-threaded here and owns no descriptor above
    // stdio yet; closing the listing's own (already closed) handle just
    // fails with EBADF.
    for fd in fds {
        let _ = nix::unistd::close(fd);
    }
}
