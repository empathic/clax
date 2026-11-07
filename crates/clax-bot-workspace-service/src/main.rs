//! The workspace service: see the library's documentation and
//! [`clax_bot_workspace_service::settings`] for what it reads.

use anyhow::{Context, Result, bail};
use clax_bot_workspace::config::for_home;
use clax_bot_workspace_service::{Service, Settings, router};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    let settings = Settings::read(|name| std::env::var(name).ok())?;
    let Some(workspace) = for_home(&settings.home, None)? else {
        bail!("{} holds no workspace.toml", settings.home.display());
    };
    let service = Service::open(workspace, &settings.home)?;
    if !settings.addr.ip().is_loopback() {
        tracing::warn!(addr = %settings.addr, "listening beyond loopback");
    }
    let listener = tokio::net::TcpListener::bind(settings.addr)
        .await
        .with_context(|| format!("listening on {}", settings.addr))?;
    tracing::info!(addr = %settings.addr, "the workspace service is listening");
    axum::serve(listener, router(service, settings.token, settings.instance))
        .await
        .context("serving")
}
