//! `clax agents` and `clax approvals` (spec §16): the caller's person's agents,
//! and the approvals and approval requests of the caller's person: the
//! owner's on this machine's daemon, a connected person's on a remote one
//! (spec §6.7). Approving and revoking are the person's, in their browser
//! (M20): these only list.

use crate::client::Client;
use crate::term;
use clax_core::Home;
use serde_json::{Value, json};

/// The caller's person's live agents (`GET /api/agents`, spec §6.7, §13):
/// every agent for the owner by hand, a connected person's own remotely.
pub fn agents_of(c: &Client) -> anyhow::Result<Vec<Value>> {
    Ok(own_live(&c.get("/api/agents")?))
}

/// The live agents of a `GET /api/agents` answer that are the caller's
/// person's own (those it names by handle), as the listing shows them.
pub fn own_live(v: &Value) -> Vec<Value> {
    v["agents"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|a| a["live"] == true && a["handle"].is_string())
        .map(|a| {
            json!({
                "name": a["name"],
                "handle": a["handle"],
                "person": a["person"],
                "harness": a["harness"],
                "project": a["project"],
                "state": a["state"],
                "since": a["since"],
                "blocked": a["blocked"],
            })
        })
        .collect()
}

/// One line for an agent of [`agents_of`]: its name, harness, project and
/// state.
pub fn agent_line(a: &Value) -> String {
    let name = a["name"]
        .as_str()
        .or_else(|| a["handle"].as_str())
        .map(term::clean_line)
        .unwrap_or_default();
    let state = match (a["state"].as_str(), a["blocked"]["since"].as_str()) {
        (Some("waiting"), Some(since)) => {
            format!("waiting on you since {}", term::clean_line(since))
        }
        (Some(s), _) => term::clean_line(s),
        (None, _) => "?".to_string(),
    };
    format!(
        "  {name} ({}) in {}, {state}",
        term::clean_line(a["harness"].as_str().unwrap_or("agent")),
        term::clean_line(a["project"].as_str().unwrap_or("?"))
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
