use crate::client::Client;
use clax_core::Home;
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
    /// Answer no request without a credential (the bearer token, or the
    /// cookies a WebView of the app that started the daemon holds), but
    /// `/healthz`, the web UI's static files and page content by artifact
    /// ID; `GET /api/token` included. For a daemon on a loopback that other
    /// programs share, such as an Android device's.
    #[arg(long, requires = "foreground")]
    pub require_credential: bool,
    /// Report this version instead of the build's, so a test can run a
    /// genuine older daemon. Debug builds only.
    #[cfg(debug_assertions)]
    #[arg(long, hide = true, requires = "foreground")]
    pub report_version: Option<String>,
    /// Serve the web UI from this directory, laid out like `web/dist`, in
    /// place of `web/dist`, so the browser tests can run their own build of
    /// it. Debug builds only.
    #[cfg(debug_assertions)]
    #[arg(long, hide = true, requires = "foreground")]
    pub web_dist: Option<std::path::PathBuf>,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    if a.foreground {
        #[cfg(debug_assertions)]
        let web_dist = a.web_dist.clone();
        #[cfg(not(debug_assertions))]
        let web_dist = None;
        return crate::serve(
            home,
            crate::ServeArgs {
                bind: a.bind.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                port: cli.port_for(home)?,
                version: reported_version(a),
                web_dist,
                require_credential: a.require_credential,
            },
        );
    }
    let bind = a.bind.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let port = cli.port_for(home)?;
    let c = Client::connect_matching_version_with_bind(home, port, bind)?;
    if let Some(requested) = a.bind {
        c.require_bind(requested)?;
    }
    let mut out = super::daemon_json(&c);
    // A daemon kept at an older version because upgrading it to this
    // build failed recently: say so, and what to do.
    if let Some(h) = crate::client::held_upgrade(home, &c) {
        eprintln!("warning: {}", h.line(home, &c.info.version));
        out["upgrade_held"] = h.to_json(home);
    }
    super::print(cli, out, |j| {
        format!(
            "clax daemon running at {} (pid {}, v{})",
            j["url"].as_str().unwrap(),
            j["pid"],
            j["version"].as_str().unwrap_or("?")
        )
    });
    Ok(())
}

/// The version a foreground daemon reports: the build's, or in a debug build
/// `--report-version`.
fn reported_version(a: &Args) -> &'static str {
    #[cfg(debug_assertions)]
    if let Some(v) = &a.report_version {
        return Box::leak(v.clone().into_boxed_str());
    }
    let _ = a;
    env!("CARGO_PKG_VERSION")
}
