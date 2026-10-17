use crate::client::Client;
use clax_core::Home;
#[derive(clap::Args)]
pub struct Args {
    pub target: String,
}
pub fn run(cli: &crate::Cli, home: &Home, a: &Args, pinned: bool) -> anyhow::Result<()> {
    let id = super::open::id_from(&a.target)?;
    let c = Client::connect(home, cli.port_for(home)?)?;
    c.patch(
        &format!("/api/artifacts/{id}"),
        &serde_json::json!({"pinned": pinned}),
    )?;
    super::print(cli, serde_json::json!({"id": id, "pinned": pinned}), |j| {
        format!(
            "{} {}",
            if pinned { "pinned" } else { "unpinned" },
            j["id"].as_str().unwrap()
        )
    });
    Ok(())
}
