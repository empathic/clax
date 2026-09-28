use crate::client::Client;
use artifax_core::{ArtifactId, Home};

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

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let id = id_from(&a.target)?;
    let c = Client::connect(home, cli.port)?;
    c.get(&format!("/api/artifacts/{id}"))?;
    let url = c.browser_url(&format!("/a/{id}"));
    if !cli.json {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(&url)
            .spawn()
            .map_err(|e| anyhow::anyhow!("cannot run {opener}: {e}"))?;
    }
    super::print(cli, serde_json::json!({"url": url}), |j| {
        j["url"].as_str().unwrap().to_string()
    });
    Ok(())
}
