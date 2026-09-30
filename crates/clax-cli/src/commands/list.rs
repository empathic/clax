use crate::client::Client;
use clax_core::Home;

#[derive(clap::Args)]
pub struct Args {
    /// Show the current version's files for this artifact instead.
    #[arg(long)]
    pub files: Option<String>,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home))?;
    if let Some(target) = &a.files {
        let id = super::open::id_from(target)?;
        let res = c.get(&format!("/api/artifacts/{id}/files"))?;
        super::print(cli, res, |j| {
            j["files"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{:>8}  {:<24} {}",
                        v["size"],
                        v["content_type"].as_str().unwrap_or(""),
                        k
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
        return Ok(());
    }
    let res = c.get("/api/artifacts")?;
    super::print(cli, res, |j| {
        let mut lines = vec![format!(
            "{:<12}  {:>3}  {:<6}  {:<24}  {}",
            "ID", "V", "PINNED", "UPDATED", "TITLE"
        )];
        for a in j["artifacts"].as_array().unwrap() {
            lines.push(format!(
                "{:<12}  {:>3}  {:<6}  {:<24}  {}",
                a["id"].as_str().unwrap(),
                a["current_version"],
                if a["pinned"].as_bool().unwrap_or(false) {
                    "yes"
                } else {
                    ""
                },
                a["updated_at"].as_str().unwrap_or(""),
                a["title"].as_str().unwrap_or("")
            ));
        }
        lines.join("\n")
    });
    Ok(())
}
