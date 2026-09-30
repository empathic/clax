use crate::client::Client;
use clax_core::Home;
#[derive(clap::Args)]
pub struct Args {
    pub target: String,
}
pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let id = super::open::id_from(&a.target)?;
    let c = Client::connect(home, cli.port_for(home)?)?;
    c.delete(&format!("/api/artifacts/{id}"))?;
    super::print(cli, serde_json::json!({"deleted": id}), |j| {
        format!("deleted {}", j["deleted"].as_str().unwrap())
    });
    Ok(())
}
