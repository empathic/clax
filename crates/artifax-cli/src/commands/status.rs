use crate::client::Client;
use artifax_core::Home;

#[derive(clap::Args)]
pub struct Args {
    /// Start a daemon if none is running.
    #[arg(long)]
    pub start: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let c = if a.start {
        Some(Client::connect(home, cli.port)?)
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
    Ok(())
}
