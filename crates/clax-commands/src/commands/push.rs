//! `clax push <artifact> [--to <remote>]`: publishes a local artifact to a
//! public Clax (`clax remote add`), every version it has not had there, in
//! order; later pushes send the new ones. When the public copy has
//! versions this Clax did not push, the push is refused: `--fork` pushes
//! onto a fork of it, `--new` as a new public artifact.

use crate::client::Client;
use clax_core::Home;
use serde_json::json;

#[derive(clap::Args)]
pub struct Args {
    /// The artifact (ID or URL).
    pub target: String,
    /// The public Clax (name or origin); the only one when there is one.
    #[arg(long)]
    pub to: Option<String>,
    /// Push every version as a new public artifact.
    #[arg(long, conflicts_with = "fork")]
    pub new: bool,
    /// Push onto a fork of the public copy, from the version pushed last.
    #[arg(long)]
    pub fork: bool,
    /// Let agents push this artifact's new versions without asking you.
    #[arg(long, conflicts_with = "no_follow")]
    pub follow: bool,
    /// Make agents ask you again before each push of this artifact.
    #[arg(long)]
    pub no_follow: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let id = super::open::id_from(&a.target)?;
    let c = Client::connect(home, cli.port_for(home)?)?;
    let mode = if a.new {
        "new"
    } else if a.fork {
        "fork"
    } else {
        "update"
    };
    let auto = if a.follow {
        Some(true)
    } else if a.no_follow {
        Some(false)
    } else {
        None
    };
    let v = c.post(
        &format!("/api/artifacts/{id}/push"),
        &json!({"remote": a.to, "mode": mode, "auto": auto}),
    )?;
    super::print(cli, v, |j| {
        let n = j["pushed"].as_array().map(Vec::len).unwrap_or(0);
        let url = j["url"].as_str().unwrap_or_default();
        let mut out = match n {
            0 => format!("up to date: {url}"),
            1 => format!("pushed 1 version: {url}"),
            n => format!("pushed {n} versions: {url}"),
        };
        if let Some(k) = j["assets_not_pushed"].as_u64().filter(|k| *k > 0) {
            out.push_str(&format!(
                "\n{k} asset{} that no pushed version references stayed on this machine",
                if k == 1 { "" } else { "s" },
            ));
        }
        out
    });
    Ok(())
}
