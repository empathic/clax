use crate::client::Client;
use clax_core::{ArtifactId, Home};

#[derive(clap::Args)]
pub struct Args {
    /// The artifact (its ID or URL); the gallery when absent.
    pub target: Option<String>,
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

/// Opens the artifact (the gallery without one) in a browser (text mode)
/// or prints its URL (`--json`, which never opens). Text mode opens it
/// through a one-use login link
/// ([`Client::login_url`]), so a browser that is not yet one of the
/// owner's becomes one, and prints the artifact's own URL, never the link.
/// The link reaches the opener in a page of the home's login directory,
/// never on its command line ([`clax_mcp::tools::open_login_in_browser`]);
/// it fails when the page cannot be written or the opener fails, by the
/// rule of [`clax_mcp::tools::open_in_browser`].
pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let id = a.target.as_deref().map(id_from).transpose()?;
    let c = Client::connect(home, cli.port_for(home)?)?;
    let path = match &id {
        Some(id) => {
            c.get(&format!("/api/artifacts/{id}"))?;
            format!("/a/{id}")
        }
        None => "/".to_string(),
    };
    let url = c.browser_url(&path);
    if !cli.json && !clax_mcp::tools::open_login_in_browser(&home.login_dir(), &c.login_url(&path)?) {
        anyhow::bail!("could not open a browser; run `clax login {path}` for a link to open");
    }
    super::print(cli, serde_json::json!({"url": url}), |j| {
        j["url"].as_str().unwrap().to_string()
    });
    Ok(())
}
