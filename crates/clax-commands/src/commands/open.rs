use crate::client::Client;
use clax_core::{ArtifactId, Home};

#[derive(clap::Args)]
pub struct Args {
    pub target: String,
}

/// Accepts a bare artifact ID or any URL whose path contains `/a/<id>`.
pub fn id_from(s: &str) -> anyhow::Result<String> {
    let candidate = match s.find("/a/") {
        Some(i) => s[i + 3..].split(['/', '?', '#']).next().unwrap_or(""),
        None => s,
    };
    Ok(ArtifactId::parse(candidate)
        .map_err(|e| anyhow::anyhow!("invalid_id: {e}"))?
        .as_str()
        .to_string())
}

/// Opens the artifact in a browser (text mode) or prints its URL (`--json`,
/// which never opens). Text mode fails when the opener fails, by the rule of
/// [`clax_mcp::tools::open_in_browser`].
pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let id = id_from(&a.target)?;
    let c = Client::connect(home, cli.port_for(home)?)?;
    c.get(&format!("/api/artifacts/{id}"))?;
    let url = c.browser_url(&format!("/a/{id}"));
    if !cli.json && !clax_mcp::tools::open_in_browser(&url) {
        anyhow::bail!("could not open a browser; open {url} yourself");
    }
    super::print(cli, serde_json::json!({"url": url}), |j| {
        j["url"].as_str().unwrap().to_string()
    });
    Ok(())
}
