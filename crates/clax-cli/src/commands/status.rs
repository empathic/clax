use crate::client::Client;
use clax_core::Home;

#[derive(clap::Args)]
pub struct Args {
    /// Start a daemon if none is running.
    #[arg(long)]
    pub start: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let c = if a.start {
        Some(Client::connect(home, cli.port_for(home)?)?)
    } else {
        Client::discover(home)
    };
    match c {
        Some(c) => super::print(cli, super::daemon_json(&c), |j| {
            format!(
                "running at {} (pid {}, v{})",
                j["url"].as_str().unwrap(),
                j["pid"],
                j["version"].as_str().unwrap()
            )
        }),
        None => super::print(
            cli,
            serde_json::json!({"running": false, "home": home.root()}),
            |_| format!("not running (home {})", home.root().display()),
        ),
    }
    // A config.toml that would stop the next start is named here, where a
    // person looking into "not running" will see it.
    if let Err(e) = cli.port_for(home) {
        eprintln!("error: {e}");
    }
    Ok(())
}
