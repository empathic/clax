//! `clax remote add|list|remove|comments|unlink|delete`: the public Clax instances this Clax
//! publishes to (the daemon's `/api/remotes`). Adding one signs this Clax
//! in there by device code: the command opens the public instance's
//! approval page, the person signed in there approves this machine, and
//! the daemon receives an agent token of theirs. Not `clax connect`, which
//! joins another machine's daemon as a person's machine.

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
    /// Delete a public copy there, after keeping its git history here.
    ///
    /// For a copy whose page here is retired: refused while a page here
    /// (not deleted) is linked to it. Its git repository (every version, a
    /// commit tagged vN) is mirrored into `<archive>/<id>.git` first, with a
    /// short-lived read token the daemon mints there; nothing is deleted
    /// unless that mirror holds the copy's main branch.
    Delete(DeleteArgs),
}

#[derive(clap::Args)]
pub struct DeleteArgs {
    /// The public copy (its ID, or its URL there).
    pub target: String,
    /// The directory to keep its git mirror in (`<archive>/<id>.git`).
    #[arg(long)]
    pub archive: std::path::PathBuf,
    /// The public Clax (name, origin or ID); the only one when there is one.
    #[arg(long)]
    pub remote: Option<String>,
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
        Cmd::Delete(a) => delete(cli, &c, a),
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
        _ => format!(
            "{name}\t{origin}\twaiting for approval (code {})",
            r["user_code"].as_str().unwrap_or("?")
        ),
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
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

/// The connected remote `named` names (name, origin or ID), or the only one.
fn remote_id(c: &Client, named: Option<&str>) -> anyhow::Result<String> {
    let v = c.get("/api/remotes")?;
    let rs: Vec<Value> = v["remotes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r["status"] == "connected")
        .collect();
    let hit: Vec<&Value> = match named {
        Some(n) => rs
            .iter()
            .filter(|r| {
                [&r["name"], &r["origin"], &r["id"]]
                    .iter()
                    .any(|x| x.as_str() == Some(n))
            })
            .collect(),
        None => rs.iter().collect(),
    };
    match hit.as_slice() {
        [one] => Ok(one["id"].as_str().unwrap_or_default().to_string()),
        [] => anyhow::bail!("no_remote: no connected public Clax by that name"),
        _ => anyhow::bail!("remote_required: several public Clax: name one with --remote"),
    }
}

fn git(dir: Option<&std::path::Path>, token: &str) -> std::process::Command {
    let mut g = std::process::Command::new("git");
    if let Some(d) = dir {
        g.arg("-C").arg(d);
    }
    // The token goes to git in its environment only: never argv, never a config file.
    g.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "http.extraHeader")
        .env(
            "GIT_CONFIG_VALUE_0",
            format!("Authorization: Bearer {token}"),
        );
    g
}

fn delete(cli: &crate::Cli, c: &Client, a: &DeleteArgs) -> anyhow::Result<()> {
    let raid = super::open::id_from(&a.target)?;
    let rid = remote_id(c, a.remote.as_deref())?;
    // Refused while a page here is linked to it: deleting a live page's copy is unpublishing, not pruning.
    let list = c.get("/api/artifacts")?;
    for art in list["artifacts"].as_array().cloned().unwrap_or_default() {
        let Some(id) = art["id"].as_str() else {
            continue;
        };
        let links = c.get(&format!("/api/artifacts/{id}/remotes"))?;
        let live = links["links"].as_array().is_some_and(|ls| {
            ls.iter()
                .any(|l| l["remote_id"] == rid.as_str() && l["remote_artifact_id"] == raid.as_str())
        });
        if live {
            anyhow::bail!(
                "linked: page {id} here is linked to {raid}: delete or unlink it here first"
            );
        }
    }
    let t = c.post(
        &format!("/api/remotes/{}/artifacts/{raid}/git-token", encode(&rid)),
        &json!({}),
    )?;
    let (url, token) = (
        t["remote"].as_str().unwrap_or_default().to_string(),
        t["token"].as_str().unwrap_or_default().to_string(),
    );
    anyhow::ensure!(
        !url.is_empty() && !token.is_empty(),
        "no_token: the public Clax gave no git read token"
    );
    std::fs::create_dir_all(&a.archive)?;
    let dest = a.archive.join(format!("{raid}.git"));
    let status = if dest.exists() {
        git(Some(&dest), &token)
            .args(["fetch", "-q", "--prune", &url, "+refs/*:refs/*"])
            .status()?
    } else {
        git(None, &token)
            .args(["clone", "-q", "--mirror", &url])
            .arg(&dest)
            .status()?
    };
    anyhow::ensure!(
        status.success(),
        "archive_failed: git could not mirror {raid}: nothing was deleted"
    );
    let main = std::process::Command::new("git")
        .arg("-C")
        .arg(&dest)
        .args(["rev-parse", "-q", "--verify", "refs/heads/main"])
        .output()?;
    anyhow::ensure!(
        main.status.success(),
        "archive_failed: the mirror of {raid} has no main branch: nothing was deleted"
    );
    let v = c.send(
        reqwest::Method::DELETE,
        &format!("/api/remotes/{}/artifacts/{raid}", encode(&rid)),
        None,
    )?;
    let out = json!({
        "deleted": v["deleted"], "unlinked": v["unlinked"],
        "archive": dest.to_string_lossy(), "main": String::from_utf8_lossy(&main.stdout).trim(),
    });
    super::print(cli, out, |j| {
        if j["deleted"] == true {
            format!(
                "deleted {raid} there; its history is in {}",
                j["archive"].as_str().unwrap_or_default()
            )
        } else {
            format!(
                "{raid} was already gone there; its history is in {}",
                j["archive"].as_str().unwrap_or_default()
            )
        }
    });
    Ok(())
}
