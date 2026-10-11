//! `clax agents` and `clax approvals` (spec §16): the caller's own agents,
//! and the approvals and approval requests of the caller's person: the
//! owner's on this machine's daemon, a connected person's on a remote one
//! (spec §6.7). Approving and revoking are the person's, in their browser
//! (M20): these only list.

use crate::client::Client;
use crate::term;
use clax_core::Home;
use serde_json::{Value, json};

/// The agents of this credential: the daemon token's sessions here, or,
/// remotely, this machine's grant's (`GET /api/sessions?live=true`, which
/// answers each credential with its own sessions, spec §6.7).
pub fn agents_of(c: &Client) -> anyhow::Result<Vec<Value>> {
    let v = c.get("/api/sessions?live=true")?;
    Ok(v["sessions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(agent_json)
        .collect())
}

/// A session as the listing shows it.
fn agent_json(s: &Value) -> Value {
    json!({
        "id": s["id"],
        "name": s["name"],
        "handle": s["agent_handle"],
        "harness": s["harness"],
        "cwd": s["cwd"],
        "started_at": s["started_at"],
        "last_seen_at": s["last_seen_at"],
    })
}

/// One line for an agent of [`agents_of`].
pub fn agent_line(a: &Value) -> String {
    let name = a["name"]
        .as_str()
        .or_else(|| a["handle"].as_str())
        .map(term::clean_line)
        .unwrap_or_default();
    format!(
        "  {name} ({}) in {}",
        term::clean_line(a["harness"].as_str().unwrap_or("agent")),
        term::clean_line(a["cwd"].as_str().unwrap_or("?"))
    )
}

pub fn agents(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    let list = agents_of(&c)?;
    super::print(cli, json!({"agents": list}), |j| {
        let list = j["agents"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if list.is_empty() {
            return "no agents are running".to_string();
        }
        let mut out = vec!["agents:".to_string()];
        out.extend(list.iter().map(agent_line));
        out.join("\n")
    });
    Ok(())
}

pub fn approvals(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    let a = c.get("/api/approvals")?;
    let r = c.get("/api/approvals/requests?status=all")?;
    let out = json!({
        "approvals": a["approvals"],
        "requests": r["requests"],
        "open": r["open"],
        "people": c.browser_url("/people"),
    });
    super::print(cli, out, |j| {
        let name = |v: &Value| term::clean_line(v["author"]["name"].as_str().unwrap_or("?"));
        let mut lines = Vec::new();
        let approvals = j["approvals"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if approvals.is_empty() {
            lines.push("no approvals".to_string());
        } else {
            lines.push("approvals:".to_string());
            for x in approvals {
                let at = match x["scope"].as_str() {
                    Some("artifact") => format!(" on {}", x["artifact_id"].as_str().unwrap_or("?")),
                    Some("thread") => format!(
                        " in thread {} of {}",
                        x["thread_id"].as_str().unwrap_or("?"),
                        x["artifact_id"].as_str().unwrap_or("?")
                    ),
                    _ => " everywhere".to_string(),
                };
                lines.push(format!("  {}{at}", name(x)));
            }
        }
        let requests = j["requests"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if !requests.is_empty() {
            lines.push(format!("requests ({} open):", j["open"]));
            for x in requests {
                lines.push(format!(
                    "  {} {} on {}",
                    x["status"].as_str().unwrap_or("?"),
                    name(x),
                    x["artifact_id"].as_str().unwrap_or("?")
                ));
            }
        }
        lines.push(format!(
            "approve or revoke in your browser: {}",
            j["people"].as_str().unwrap_or_default()
        ));
        lines.join("\n")
    });
    Ok(())
}
