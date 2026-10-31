//! `clax remote add|list|remove|comments|unlink|request`: the public Clax instances this Clax
//! publishes to (the daemon's `/api/remotes`). Adding one signs this Clax
//! in there by device code: the command opens the public instance's
//! approval page, the person signed in there approves this machine, and
//! the daemon receives an agent token of theirs. Not `clax connect`, which
//! joins another machine's daemon as a person's machine. Adding is the
//! owner's: an agent asks for one with `clax remote request`, which the
//! person answers in Clax.

use crate::client::Client;
use clax_core::Home;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Sign this Clax in to a public Clax (`https://clax.example`) to publish there.
    ///
    /// Prints a code and opens the public instance's approval page; approve
    /// it there, signed in as the person to publish as. Waits until it is
    /// approved, denied or expired.
    Add(AddArgs),
    /// List the public Clax instances this Clax is signed in to.
    List,
    /// Forget a public Clax (by name or origin), revoking its agent token
    /// there. When the public Clax cannot revoke it, revoke it on its
    /// People page.
    Remove(RemoveArgs),
    /// Set how a pushed artifact's comment threads follow its public copy.
    ///
    /// `two-way` (the default): threads, replies and resolves go both
    /// ways. `download-only`: threads from the public copy still come here
    /// and replies and resolves on them still go up, but threads started
    /// here stay here.
    Comments(CommentsArgs),
    /// End a pushed artifact's link to its public copy. The public copy
    /// stays; its threads stay here as read-only history.
    Unlink(UnlinkArgs),
    /// Ask the person to connect this Clax to another Clax instance (for
    /// agents): a URL on it, or its origin.
    ///
    /// The person sees one card in Clax and answers Connect (approving on
    /// the other instance), Not now or Never for this instance. Waits for
    /// the answer and prints it: connected, dismissed, muted, not_now (with
    /// until) or expired. Asks as the agent's Clax session: --session, else
    /// the live session its harness's session variable names
    /// (CODEX_THREAD_ID, GROK_SESSION_ID, CLAUDE_CODE_SESSION_ID), else the
    /// live session whose harness process is an ancestor of this one.
    Request(RequestArgs),
}

#[derive(clap::Args)]
pub struct RequestArgs {
    /// A URL on the Clax instance, or its origin.
    pub url: String,
    /// Why, in a sentence the person reads.
    #[arg(long)]
    pub reason: Option<String>,
    /// The asking agent's Clax session (`status` reports it as session.id);
    /// found from the harness when absent.
    #[arg(long)]
    pub session: Option<String>,
    /// Seconds to wait for the answer (default 600, 50 under Codex, as the
    /// connect_instance tool waits).
    #[arg(long)]
    pub wait: Option<u64>,
}

/// The harness session variables, each with its harness: a CLI run in a
/// harness's shell asks as the live session registered with that ID.
const SESSION_VARIABLES: &[(&str, &str)] = &[
    ("CODEX_THREAD_ID", "codex"),
    ("GROK_SESSION_ID", "grok"),
    ("CLAUDE_CODE_SESSION_ID", "claude"),
];

/// The live session among `sessions` (`GET /api/sessions?live=true`'s)
/// this CLI runs for: the one a harness session variable `env` gives
/// names, else the nearest of `ancestors` (this process's, nearest first)
/// that is a session's harness process (`parent_pid`).
fn session_among(
    sessions: &[Value],
    env: impl Fn(&str) -> Option<String>,
    ancestors: &[u32],
) -> Option<String> {
    let id = |s: &Value| s["id"].as_str().map(str::to_string);
    for (var, harness) in SESSION_VARIABLES {
        let Some(hsid) = env(var).filter(|v| !v.is_empty()) else {
            continue;
        };
        if let Some(s) = sessions
            .iter()
            .find(|s| s["harness"] == *harness && s["harness_session_id"].as_str() == Some(&hsid))
        {
            return id(s);
        }
    }
    ancestors.iter().find_map(|pid| {
        sessions
            .iter()
            .find(|s| s["parent_pid"].as_u64() == Some(u64::from(*pid)))
            .and_then(id)
    })
}

/// The session `clax remote request` asks as ([`session_among`]).
fn own_session(c: &Client) -> anyhow::Result<Option<String>> {
    let v = c.get("/api/sessions?live=true")?;
    let sessions = v["sessions"].as_array().cloned().unwrap_or_default();
    Ok(session_among(
        &sessions,
        |k| std::env::var(k).ok(),
        &super::hook::ancestors(std::process::id()),
    ))
}

/// The default wait of `clax remote request`: the connect_instance tool's
/// ([`clax_mcp::tools::default_ask_wait`]) under the harness the
/// environment `env` names, Codex or another.
fn default_wait(env: impl Fn(&str) -> Option<String>) -> u64 {
    let codex = ["CODEX_THREAD_ID", "CODEX_CI", "CODEX_SANDBOX"]
        .iter()
        .any(|k| env(k).is_some_and(|v| !v.is_empty()));
    clax_mcp::tools::default_ask_wait(if codex { "codex" } else { "" })
}

#[derive(clap::Args)]
pub struct CommentsArgs {
    /// The artifact (ID or URL).
    pub target: String,
    /// `two-way` or `download-only`.
    #[arg(value_parser = ["two-way", "download-only"])]
    pub mode: String,
    /// The public Clax (name or origin); the only one it is linked to when
    /// absent.
    #[arg(long)]
    pub remote: Option<String>,
}

#[derive(clap::Args)]
pub struct UnlinkArgs {
    /// The artifact (ID or URL).
    pub target: String,
    /// The public Clax (name or origin); the only one it is linked to when
    /// absent.
    #[arg(long)]
    pub remote: Option<String>,
}

#[derive(clap::Args)]
pub struct AddArgs {
    /// The public Clax's address.
    pub url: String,
    /// A short name for it here (its host by default).
    #[arg(long)]
    pub name: Option<String>,
    /// What its approval page calls this machine.
    #[arg(long)]
    pub machine: Option<String>,
    /// Print the approval page's address instead of opening it.
    #[arg(long)]
    pub no_open: bool,
}

#[derive(clap::Args)]
pub struct RemoveArgs {
    /// The remote's name, origin or ID.
    pub remote: String,
}

/// The remote artifact `id` is linked to that `named` names, or its only
/// link's.
fn linked_remote(c: &Client, id: &str, named: Option<&str>) -> anyhow::Result<String> {
    if let Some(r) = named {
        return Ok(r.to_string());
    }
    let v = c.get(&format!("/api/artifacts/{id}/remotes"))?;
    let links = v["links"].as_array().cloned().unwrap_or_default();
    match links.as_slice() {
        [one] => Ok(one["remote_id"].as_str().unwrap_or_default().to_string()),
        [] => anyhow::bail!("no_link: this artifact was never pushed: `clax push` it first"),
        _ => anyhow::bail!(
            "remote_required: it is linked to several public Clax: name one with --remote"
        ),
    }
}

pub fn run(cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    match cmd {
        Cmd::Comments(a) => {
            let id = super::open::id_from(&a.target)?;
            let r = linked_remote(&c, &id, a.remote.as_deref())?;
            let v = c.patch(
                &format!("/api/artifacts/{id}/remotes/{}", encode(&r)),
                &json!({"download_only": a.mode == "download-only"}),
            )?;
            super::print(cli, v, |j| {
                if j["download_only"] == true {
                    "download only: threads started here stay here; replies and resolves on linked threads still go up".into()
                } else {
                    "two-way: threads, replies and resolves go both ways".into()
                }
            });
            Ok(())
        }
        Cmd::Unlink(a) => {
            let id = super::open::id_from(&a.target)?;
            let r = linked_remote(&c, &id, a.remote.as_deref())?;
            let v = c.send(
                reqwest::Method::DELETE,
                &format!("/api/artifacts/{id}/remotes/{}", encode(&r)),
                None,
            )?;
            super::print(cli, v, |j| {
                let n = j["unsent"].as_array().map_or(0, Vec::len);
                if n == 0 {
                    "unlinked: its threads stay here as read-only history".into()
                } else {
                    format!(
                        "unlinked: its threads stay here as read-only history; {n} comment(s) were not sent: drop them in its Publish menu"
                    )
                }
            });
            Ok(())
        }
        Cmd::Add(a) => add(cli, &c, a),
        Cmd::Request(a) => request(cli, &c, a),
        Cmd::List => {
            let v = c.get("/api/remotes")?;
            super::print(cli, v, |j| {
                let rs = j["remotes"].as_array().cloned().unwrap_or_default();
                if rs.is_empty() {
                    return "no public Clax yet: add one with `clax remote add <url>`".into();
                }
                rs.iter().map(line).collect::<Vec<_>>().join("\n")
            });
            Ok(())
        }
        Cmd::Remove(a) => {
            let v = c.send(
                reqwest::Method::DELETE,
                &format!("/api/remotes/{}", encode(&a.remote)),
                None,
            )?;
            super::print(cli, v, |j| {
                let name = j["remote"]["name"].as_str().unwrap_or_default();
                let origin = j["remote"]["origin"].as_str().unwrap_or_default();
                if j["revoked"] == true {
                    format!("removed {name}, and revoked its agent token there")
                } else {
                    format!(
                        "removed {name}; {origin} could not revoke its agent token: revoke it on {origin}/people"
                    )
                }
            });
            Ok(())
        }
    }
}

/// A remote's line: its name, origin, and whose it is or that it waits.
fn line(r: &Value) -> String {
    let name = r["name"].as_str().unwrap_or_default();
    let origin = r["origin"].as_str().unwrap_or_default();
    match r["status"].as_str() {
        Some("connected") => format!(
            "{name}\t{origin}\tas {}",
            r["person_name"].as_str().unwrap_or("someone")
        ),
        Some("signed_out") => format!(
            "{name}\t{origin}\tsigned out: its token was revoked there (connect it again with `clax remote add {origin}`)"
        ),
        _ => format!(
            "{name}\t{origin}\twaiting for approval (code {})",
            r["user_code"].as_str().unwrap_or("?")
        ),
    }
}

pub(crate) fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `clax remote request`: asks, then waits until the request is answered
/// or `a.wait` runs out, as the connect_instance tool does
/// ([`clax_mcp::DaemonClient::remote_request_wait_up_to`]), but polling
/// again when every wait on the request is taken.
fn request(cli: &crate::Cli, c: &Client, a: &RequestArgs) -> anyhow::Result<()> {
    let session = match &a.session {
        Some(s) => Some(s.clone()),
        None => own_session(c)?,
    };
    let wait = a
        .wait
        .unwrap_or_else(|| default_wait(|k| std::env::var(k).ok()));
    let v = super::tools::with_daemon(c, session.clone(), |d| async move {
        d.remote_request(&a.url, a.reason.as_deref()).await
    })?;
    if v["status"] != "open" {
        super::print(cli, v, answer_line);
        return Ok(());
    }
    let id = v["request"]["id"].as_str().unwrap_or_default().to_string();
    if !cli.json {
        eprintln!(
            "Asked the person in Clax to connect {} ({}); waiting for the answer…",
            v["request"]["name"].as_str().unwrap_or_default(),
            v["request"]["origin"].as_str().unwrap_or_default()
        );
    }
    if !clax_core::is_ulid(&id) {
        anyhow::bail!("the daemon answered no request ID");
    }
    let p = super::tools::with_daemon(c, session, |d| async move {
        d.remote_request_wait_up_to(&id, wait, true).await
    })?;
    super::print(cli, p, answer_line);
    Ok(())
}

/// A connection request's answer, as `clax remote request` prints it.
fn answer_line(j: &Value) -> String {
    match j["status"].as_str() {
        Some("connected") => format!(
            "connected: {} is remote {}",
            j["origin"].as_str().unwrap_or("the instance"),
            j["remote"].as_str().unwrap_or_default()
        ),
        Some("not_now") => format!(
            "not_now: the person said not now; ask again after {}",
            j["until"].as_str().unwrap_or("a day")
        ),
        Some("muted") => "muted: the person does not connect this instance".into(),
        Some("dismissed") => "dismissed: the person did not connect it".into(),
        Some("expired") => "expired: nobody answered within a day".into(),
        _ => format!(
            "open: still waiting for the person's answer (request {})",
            j["request_id"].as_str().unwrap_or_default()
        ),
    }
}

fn add(cli: &crate::Cli, c: &Client, a: &AddArgs) -> anyhow::Result<()> {
    let v = c.post(
        "/api/remotes",
        &json!({"url": a.url, "name": a.name, "machine": a.machine}),
    )?;
    let id = v["remote"]["id"].as_str().unwrap_or_default().to_string();
    let url = v["verification_url"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let code = v["user_code"].as_str().unwrap_or_default();
    let interval = Duration::from_secs(v["interval"].as_u64().unwrap_or(5).max(1));
    if !cli.json {
        eprintln!(
            "To sign this Clax in to {}, approve code {code} at:\n  {url}",
            v["remote"]["origin"].as_str().unwrap_or_default()
        );
        if !a.no_open && !clax_mcp::tools::open_in_browser(&url) {
            eprintln!("(could not open a browser: open the address yourself)");
        }
        eprintln!("Waiting for approval…");
    }
    // The codes last ten minutes; the remote answers `expired` past that.
    let deadline = Instant::now() + Duration::from_secs(11 * 60);
    loop {
        std::thread::sleep(interval);
        let p = c.post(&format!("/api/remotes/{id}/poll"), &json!({}))?;
        match p["status"].as_str() {
            Some("pending") if Instant::now() < deadline => continue,
            Some("connected") => {
                super::print(cli, p, |j| {
                    format!(
                        "signed in to {} as {}: publish with `clax push <artifact>`",
                        j["remote"]["origin"].as_str().unwrap_or_default(),
                        j["remote"]["person_name"].as_str().unwrap_or("someone")
                    )
                });
                return Ok(());
            }
            Some("denied") => anyhow::bail!("access_denied: the sign-in was denied there"),
            _ => anyhow::bail!("expired_token: the sign-in expired: add the remote again"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn the_session_is_the_harness_variables_else_an_ancestors() {
        let sessions = [
            json!({"id": "s_codex", "harness": "codex", "harness_session_id": "th-1", "parent_pid": 40}),
            json!({"id": "s_claude", "harness": "claude", "harness_session_id": "cc-1", "parent_pid": 30}),
            json!({"id": "s_far", "harness": "claude", "harness_session_id": "cc-2", "parent_pid": 20}),
        ];
        assert_eq!(
            session_among(&sessions, env(&[("CODEX_THREAD_ID", "th-1")]), &[30]).as_deref(),
            Some("s_codex")
        );
        assert_eq!(
            session_among(&sessions, env(&[("CLAUDE_CODE_SESSION_ID", "cc-1")]), &[]).as_deref(),
            Some("s_claude")
        );
        // A variable naming no live session falls back to the ancestors,
        // the nearest first.
        assert_eq!(
            session_among(&sessions, env(&[("GROK_SESSION_ID", "g-9")]), &[99, 20, 30]).as_deref(),
            Some("s_far")
        );
        assert_eq!(session_among(&sessions, env(&[]), &[7, 8]), None);
        // The harness must match too.
        assert_eq!(
            session_among(&sessions, env(&[("GROK_SESSION_ID", "th-1")]), &[]),
            None
        );
    }

    #[test]
    fn the_wait_is_the_tools() {
        assert_eq!(default_wait(env(&[])), 600);
        assert_eq!(default_wait(env(&[("CODEX_THREAD_ID", "t")])), 50);
        assert_eq!(default_wait(env(&[("CODEX_CI", "1")])), 50);
    }
}
