//! `clax logout`: ends every owner browser session of this machine's
//! daemon.

use crate::client::Client;
use clax_core::Home;

/// Ends every owner browser session (`DELETE /api/browser-login`): no
/// browser's cookie stands for the owner after, and each browser of the
/// owner's needs a new login code (`clax open`, `clax login`). A daemon
/// that starts again (a new token) ends them as well.
pub fn run(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    let v = c.delete_json("/api/browser-login")?;
    let ended = v["ended"].as_u64().unwrap_or(0);
    super::print(cli, serde_json::json!({"ended": ended}), |_| match ended {
        0 => "no browser was signed in as the owner".to_string(),
        1 => "ended 1 browser session: run `clax open` to sign a browser in again".to_string(),
        n => format!("ended {n} browser sessions: run `clax open` to sign a browser in again"),
    });
    Ok(())
}
