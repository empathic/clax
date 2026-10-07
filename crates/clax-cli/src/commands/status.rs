use crate::client::Client;
use crate::term::{self, Paint};
use clax_core::Home;
use serde_json::{Value, json};

#[derive(clap::Args)]
pub struct Args {
    /// Start a daemon if none is running.
    #[arg(long)]
    pub start: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let c = if a.start {
        Some(Client::connect(home, cli.port_for(home)?)?)
    } else {
        Client::discover(home)
    };
    match c {
        Some(c) => {
            let mut out = super::daemon_json(&c);
            let hold = crate::client::upgrade_hold_for(home, &c.info.version);
            if let Some(h) = &hold {
                out["upgrade_held"] = h.to_json(home);
            }
            out["working"] = json!(working(&c));
            super::print(cli, out, |j| {
                let mut text = format!(
                    "running at {} (pid {}, v{}, commit {})",
                    j["url"].as_str().unwrap(),
                    j["pid"],
                    j["version"].as_str().unwrap(),
                    j["commit"]
                        .as_str()
                        .map_or("unknown", |c| c.get(..7).unwrap_or(c))
                );
                if let Some(h) = &hold {
                    text.push_str(&format!(
                        "\nupgrade held: {}\nwhy: {}",
                        h.line(home, &c.info.version),
                        h.reason
                    ));
                }
                let roster = j["working"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                if roster.is_empty() {
                    text.push_str("\nno agent is working on an artifact");
                } else {
                    text.push_str("\nworking now:");
                    let paint = Paint::stdout();
                    for w in roster {
                        text.push_str(&format!("\n{}", roster_line(w, paint)));
                    }
                }
                text
            })
        }
        None => match clax_server::daemon::read_starting_info(home)
            .filter(|s| clax_server::daemon::pid_alive(s.pid))
        {
            // A daemon that has not answered yet: say what it is doing (a
            // first start may record the audit backfill for a while).
            Some(s) => super::print(
                cli,
                json!({"running": false, "starting": s, "home": home.root()}),
                |_| format!("starting: {}", s.line()),
            ),
            None => super::print(
                cli,
                serde_json::json!({"running": false, "home": home.root()}),
                |_| format!("not running (home {})", home.root().display()),
            ),
        },
    }
    // A config.toml that would stop the next start is named here, where a
    // person looking into "not running" will see it.
    if let Err(e) = cli.port_for(home) {
        eprintln!("error: {e}");
    }
    Ok(())
}

/// The working roster: each agent working on an artifact now, newest first,
/// as `{agent, harness, session_id, artifact_id, title, url, thread_ids,
/// threads, message, started_at, for_s}`; `threads` holds each thread's
/// `<artifact>#<n>` (null for one not found). Empty when the daemon cannot
/// say.
fn working(c: &Client) -> Vec<Value> {
    let Ok(r) = c.get("/api/working") else {
        return Vec::new();
    };
    let records = r["working"].as_array().cloned().unwrap_or_default();
    if records.is_empty() {
        return Vec::new();
    }
    let titles: std::collections::HashMap<String, String> = c
        .get("/api/artifacts")
        .ok()
        .and_then(|v| v["artifacts"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|a| {
            Some((
                a["id"].as_str()?.to_string(),
                a["title"].as_str()?.to_string(),
            ))
        })
        .collect();
    let mut numbers: std::collections::HashMap<String, Vec<Value>> = Default::default();
    let now = chrono::Utc::now();
    records
        .iter()
        .map(|w| {
            let aid = w["artifact_id"].as_str().unwrap_or_default().to_string();
            let thread_ids = w["thread_ids"].as_array().cloned().unwrap_or_default();
            let all = if thread_ids.is_empty() {
                &Vec::new()
            } else {
                &*numbers
                    .entry(aid.clone())
                    .or_insert_with(|| super::comments::all_threads(c, &aid).unwrap_or_default())
            };
            let threads: Vec<Value> = thread_ids
                .iter()
                .map(|tid| {
                    all.iter()
                        .position(|t| &t["id"] == tid)
                        .map_or(Value::Null, |i| json!(format!("{aid}#{}", i + 1)))
                })
                .collect();
            let for_s = w["started_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| (now - t.with_timezone(&chrono::Utc)).num_seconds().max(0));
            json!({
                "agent": w["agent"],
                "harness": w["harness"],
                "session_id": w["session_id"],
                "artifact_id": aid,
                "title": titles.get(&aid),
                "url": c.browser_url(&format!("/a/{aid}")),
                "thread_ids": thread_ids,
                "threads": threads,
                "message": w["message"],
                "started_at": w["started_at"],
                "for_s": for_s,
            })
        })
        .collect()
}

fn roster_line(w: &Value, paint: Paint) -> String {
    let who = format!(
        "{} {}",
        term::snippet(w["harness"].as_str().unwrap_or("agent"), 20),
        paint.dim(&format!(
            "({})",
            term::snippet(w["agent"].as_str().unwrap_or("?"), 30)
        ))
    );
    let title = term::snippet(w["title"].as_str().unwrap_or("(untitled)"), 50);
    let mut line = format!(
        "  {} on {} {}",
        paint.magenta(&who),
        paint.bold(&title),
        paint.dim(w["artifact_id"].as_str().unwrap_or(""))
    );
    let refs: Vec<String> = w["threads"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(|r| format!("#{}", r.rsplit_once('#').map_or(r, |(_, n)| n)))
        .collect();
    if !refs.is_empty() {
        line.push_str(&format!(" {}", paint.cyan(&refs.join(", "))));
    }
    if let Some(m) = w["message"].as_str() {
        line.push_str(&format!(": {}", term::snippet(m, 100)));
    }
    line.push_str(&paint.dim(&format!(
        " · {} · session {}",
        w["for_s"].as_i64().map(term::span).unwrap_or_default(),
        w["session_id"].as_str().unwrap_or("?")
    )));
    line
}
