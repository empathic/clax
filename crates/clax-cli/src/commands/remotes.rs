//! `clax remote add|list|remove`: the public Clax instances this Clax
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

pub fn run(cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    match cmd {
        Cmd::Add(a) => add(cli, &c, a),
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
