use crate::client::Client;
use artifax_core::Home;
use artifax_server::daemon::{ServeConfig, serve};
use std::net::IpAddr;

#[derive(clap::Args)]
pub struct Args {
    /// Address to bind (use 0.0.0.0 for LAN access).
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: IpAddr,
    /// Run the daemon in this process instead of the background.
    #[arg(long)]
    pub foreground: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    if a.foreground {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
            )
            .init();
        let rt = tokio::runtime::Runtime::new()?;
        let cfg = ServeConfig {
            home: home.clone(),
            bind: a.bind,
            port: cli.port,
            version: env!("CARGO_PKG_VERSION"),
        };
        return rt.block_on(serve(cfg, None));
    }
    let c = Client::connect_with_bind(home, cli.port, a.bind)?;
    super::print(cli, super::daemon_json(&c), |j| {
        format!(
            "artifax daemon running at {} (pid {})",
            j["url"].as_str().unwrap(),
            j["pid"]
        )
    });
    Ok(())
}
