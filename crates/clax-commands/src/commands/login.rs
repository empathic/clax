//! `clax login [path]`: a one-use login link for a browser of this machine.

use crate::client::Client;
use clax_core::Home;

#[derive(clap::Args)]
pub struct Args {
    /// The daemon path the link opens once the browser is signed in
    /// (`/a/<id>`, `/inbox`); the gallery when absent.
    pub path: Option<String>,
}

/// Prints a fresh login link ([`Client::login_url`]): opened in a browser
/// on this machine within two minutes, once, it makes that browser one of
/// the owner's and opens `path`. Opens nothing itself.
pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let path = a.path.as_deref().unwrap_or("/");
    if !path.starts_with('/') {
        anyhow::bail!("invalid_path: a path of this Clax starts with /, as /a/<id>");
    }
    let c = Client::connect(home, cli.port_for(home)?)?;
    let url = c.login_url(path)?;
    super::print(cli, serde_json::json!({"url": url}), |j| {
        j["url"].as_str().unwrap().to_string()
    });
    Ok(())
}
