use crate::client::Client;
use clax_core::Home;
use clax_server::daemon::{ServeConfig, serve};
use std::net::{IpAddr, Ipv4Addr};

#[derive(clap::Args)]
pub struct Args {
    /// Address to bind (use 0.0.0.0 for LAN access).
    /// Defaults to 127.0.0.1.
    #[arg(long)]
    pub bind: Option<IpAddr>,
    /// Run the daemon in this process instead of the background.
    #[arg(long)]
    pub foreground: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    if a.foreground {
        close_inherited_fds();
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
            )
            .init();
        let rt = tokio::runtime::Runtime::new()?;
        let cfg = ServeConfig {
            home: home.clone(),
            bind: a.bind.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            stale_check_interval: std::time::Duration::from_secs(30),
            reap_interval: std::time::Duration::from_secs(60),
            port: cli.port_for(home),
            version: env!("CARGO_PKG_VERSION"),
            codex: clax_server::push::CodexPush::from_env(
                std::env::var_os("CLAX_CODEX_BIN"),
                std::env::var_os("PATH").as_deref(),
            ),
        };
        return rt.block_on(serve(cfg, None));
    }
    let c = Client::connect_with_bind(
        home,
        cli.port_for(home),
        a.bind.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
    )?;
    if let Some(requested) = a.bind {
        c.require_bind(requested)?;
    }
    super::print(cli, super::daemon_json(&c), |j| {
        format!(
            "clax daemon running at {} (pid {})",
            j["url"].as_str().unwrap(),
            j["pid"]
        )
    });
    Ok(())
}

/// Closes every descriptor above stdio that this process inherited.
///
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
    let fds: Vec<libc::c_int> = match std::fs::read_dir(FD_DIR) {
        Ok(entries) => entries
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .filter(|&fd| fd > 2)
            .collect(),
        Err(_) => return,
    };
    for fd in fds {
        // SAFETY: the process is single-threaded here and owns no descriptor
        // above stdio yet; closing the listing's own (already closed) handle
        // just fails with EBADF.
        unsafe { libc::close(fd) };
    }
}
