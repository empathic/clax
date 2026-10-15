//! `clax people`, `clax agents` and `clax approvals` (spec §11, §16): the
//! owner's people with their machines and agents; the caller's person's
//! agents, and renaming one; and the approvals and approval requests of the
//! caller's person: the owner's on this machine's daemon, a connected
//! person's on a remote one (spec §6.7). Invites, rights, elevation,
//! narrowing and approvals are changed in a browser (M20): their commands
//! print the People link, change nothing and exit 2.

use crate::client::{Client, ForPeople, NotForAgents};
use crate::term;
use clap::Subcommand;
use clax_core::Home;
use serde_json::{Value, json};

/// The arguments a browser-only command takes and ignores.
#[derive(clap::Args, Debug)]
pub struct Ignored {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum PeopleCmd {
    /// Invite a person or a machine: prints the People link (invites take
    /// your browser).
    Invite(Ignored),
    /// Set a person's rights: prints the People link (rights take your
    /// browser).
    Rights(Ignored),
    /// Revoke a person: prints the People link (revoking takes your
    /// browser).
    Revoke(Ignored),
}

#[derive(Subcommand, Debug)]
pub enum AgentsCmd {
    /// Rename a running agent, by its name or handle (the owner, for any
    /// agent; a connected person renames theirs in their browser, and this
    /// prints the People link). The agent can no longer rename itself.
    Rename {
        /// The agent's current name, or its handle.
        agent: String,
        /// Its new name: letters, digits, `.`, `_` and `-`.
        name: String,
    },
    /// Narrow an agent's rights: prints the People link (narrowing takes
    /// your browser).
    Narrow(Ignored),
    /// Elevate an agent's rights: prints the People link (elevating takes
    /// your browser).
    Elevate(Ignored),
}

#[derive(Subcommand, Debug)]
pub enum ApprovalsCmd {
    /// Approve an author for your agents: prints the People link
    /// (approving takes your browser).
    #[command(alias = "allow")]
    Approve(Ignored),
    /// Decide an approval request: prints the People link (deciding takes
    /// your browser).
    #[command(alias = "decline")]
    Decide(Ignored),
    /// Revoke an approval: prints the People link (revoking takes your
    /// browser).
    Revoke(Ignored),
}

/// The People link of the daemon `c` talks to.
fn people_link(c: &Client) -> String {
    c.browser_url("/people")
}

/// The refusal of a change M20 reserves for a browser: `what` (a sentence
/// subject such as "Invites are made"), then the People link.
fn in_browser(cli: &crate::Cli, home: &Home, what: &str) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    Err(ForPeople {
        line: format!(
            "{what} in your browser, on the People page: {}",
            people_link(&c)
        ),
    }
    .into())
}

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

pub fn agents(cli: &crate::Cli, home: &Home, cmd: Option<&AgentsCmd>) -> anyhow::Result<()> {
    match cmd {
        None => {}
        Some(AgentsCmd::Rename { agent, name }) => return rename(cli, home, agent, name),
        Some(AgentsCmd::Narrow(_)) => return in_browser(cli, home, "Agents are narrowed"),
        Some(AgentsCmd::Elevate(_)) => return in_browser(cli, home, "Agents are elevated"),
    }
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

pub fn approvals(cli: &crate::Cli, home: &Home, cmd: Option<&ApprovalsCmd>) -> anyhow::Result<()> {
    if cmd.is_some() {
        return in_browser(cli, home, "Approvals are decided, made and revoked");
    }
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

/// `clax agents rename <agent> <name>` (spec §11): `PUT /api/agents/<handle>
/// {name}` with the owner's token, for the agent of the caller's listing
/// (`GET /api/agents`) named `agent` (ignoring case) or with that handle. A
/// connected person renames in their browser, and an agent never renames
/// another: both get the People link.
fn rename(cli: &crate::Cli, home: &Home, agent: &str, name: &str) -> anyhow::Result<()> {
    if clax_core::remote::Mode::of(home).is_remote() {
        return in_browser(cli, home, "Your agents are renamed");
    }
    let c = Client::connect(home, cli.port_for(home)?)?;
    if crate::channel::from_process() {
        return Err(NotForAgents {
            url: people_link(&c),
            inbox: false,
        }
        .into());
    }
    let list = agents_of(&c)?;
    let found = list
        .iter()
        .find(|a| {
            a["handle"].as_str() == Some(agent)
                || a["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(agent))
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no running agent of yours is named {}",
                term::clean_line(agent)
            )
        })?;
    let handle = found["handle"].as_str().unwrap_or_default();
    let old = found["name"].as_str().unwrap_or(handle).to_string();
    let v = c.send(
        reqwest::Method::PUT,
        &format!("/api/agents/{handle}"),
        Some(&json!({ "name": name })),
    )?;
    super::print(cli, v, |j| {
        format!(
            "renamed {} to {}",
            term::clean_line(&old),
            term::clean_line(j["agent"]["name"].as_str().unwrap_or(name))
        )
    });
    Ok(())
}

/// A rights value's name: its preset's when it is exactly one, else
/// `custom rights`.
fn rights_name(v: &Value) -> &'static str {
    use clax_core::rights::{PRESET_COLLABORATOR, PRESET_FULL, PRESET_READER, Rights};
    let Ok(r) = Rights::parse(v) else {
        return "custom rights";
    };
    [PRESET_READER, PRESET_COLLABORATOR, PRESET_FULL]
        .into_iter()
        .find(|p| Rights::preset(p).as_ref() == Some(&r))
        .unwrap_or("custom rights")
}

/// One line for machine (grant) `g`, `None` for one that is no longer
/// connected or waiting.
fn machine_line(g: &Value) -> Option<String> {
    let state = match g["status"].as_str() {
        Some("approved") => "connected",
        Some("pending") => "waiting for your approval",
        _ => return None,
    };
    Some(format!(
        "machine {}, {state}",
        term::clean_line(g["client"]["name"].as_str().unwrap_or("?"))
    ))
}

/// `clax people` (spec §16, the owner): every person (`GET /api/people`),
/// with their rights, machines and running agents, then the owner's own
/// machines (`GET /api/people/me`) and the People link; `invite`, `rights`
/// and `revoke` print the link (M20).
pub fn people(cli: &crate::Cli, home: &Home, cmd: Option<&PeopleCmd>) -> anyhow::Result<()> {
    match cmd {
        None => {}
        Some(PeopleCmd::Invite(_)) => {
            return in_browser(cli, home, "Invites are made and revoked");
        }
        Some(PeopleCmd::Rights(_)) => return in_browser(cli, home, "People's rights are set"),
        Some(PeopleCmd::Revoke(_)) => return in_browser(cli, home, "People are revoked"),
    }
    let c = Client::connect(home, cli.port_for(home)?)?;
    let v = c.get("/api/people")?;
    let me = c.get("/api/people/me")?;
    let out = json!({
        "people": v["people"],
        "machines": me["grants"],
        "unconnected": v["unconnected"],
        "url": people_link(&c),
    });
    super::print(cli, out, |j| {
        let list = |k: &str| j[k].as_array().cloned().unwrap_or_default();
        let mut lines = Vec::new();
        let people = list("people");
        if people.is_empty() {
            lines.push("no people".to_string());
        } else {
            lines.push("people:".to_string());
        }
        for p in &people {
            let name = term::clean_line(p["display_name"].as_str().unwrap_or("?"));
            if let Some(at) = p["revoked_at"].as_str() {
                lines.push(format!("  {name}: revoked {}", term::clean_line(at)));
                continue;
            }
            lines.push(format!("  {name}: {}", rights_name(&p["rights"])));
            let grants = p["grants"].as_array().cloned().unwrap_or_default();
            lines.extend(
                grants
                    .iter()
                    .filter_map(machine_line)
                    .map(|l| format!("    {l}")),
            );
            for a in p["agents"].as_array().cloned().unwrap_or_default() {
                lines.push(format!(
                    "    agent {} ({})",
                    term::clean_line(a["name"].as_str().or(a["handle"].as_str()).unwrap_or("?")),
                    term::clean_line(a["harness"].as_str().unwrap_or("agent"))
                ));
            }
        }
        let mine: Vec<String> = list("machines").iter().filter_map(machine_line).collect();
        if !mine.is_empty() {
            lines.push("your machines:".to_string());
            lines.extend(mine.into_iter().map(|l| format!("  {l}")));
        }
        if j["unconnected"]["on"] == true {
            lines.push("viewers who are not connected reach your agents".to_string());
        }
        lines.push(format!(
            "invite people and change their rights in your browser: {}",
            j["url"].as_str().unwrap_or_default()
        ));
        lines.join("\n")
    });
    Ok(())
}
