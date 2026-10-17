//! `clax versions <artifact>`: each version with its label, note, publisher,
//! time, and the threads it addressed.

use crate::client::Client;
use crate::term::{self, Paint};
use clax_core::Home;
use serde_json::{Value, json};

#[derive(clap::Args)]
pub struct Args {
    /// Artifact ID or URL.
    pub target: String,
}

/// Prints `{artifact_id, url, title, current_version, versions: [{n, label,
/// note, created_at, publisher: {agent, harness} | null, files, addressed:
/// [{thread_id, ref}]}]}` (`--json`; newest version first, `ref` null for a
/// thread since deleted), or one block per version.
pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let id = clax_mcp::tools::artifact_ref(&a.target)
        .map(|(id, _)| id)
        .map_err(|_| anyhow::anyhow!("'{}' is not an artifact ID or URL", a.target))?;
    let c = Client::connect(home, cli.port_for(home)?)?;
    let art = c.get(&format!("/api/artifacts/{id}"))?;
    let threads = super::comments::all_threads(&c, &id)?;
    let number = |tid: &str| threads.iter().position(|t| t["id"] == tid).map(|i| i + 1);
    let mut versions: Vec<Value> = art["versions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|v| {
            let addressed: Vec<Value> = v["addresses"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_str)
                .map(|tid| {
                    json!({"thread_id": tid, "ref": number(tid).map(|n| format!("{id}#{n}"))})
                })
                .collect();
            let publisher = if v["agent"].is_string() || v["agent_harness"].is_string() {
                json!({"agent": v["agent"], "harness": v["agent_harness"]})
            } else {
                Value::Null
            };
            json!({
                "n": v["n"],
                "label": v["label"],
                "note": v["note"],
                "created_at": v["created_at"],
                "publisher": publisher,
                "files": v["files"].as_object().map(|f| f.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                "addressed": addressed,
            })
        })
        .collect();
    versions.sort_by_key(|v| std::cmp::Reverse(v["n"].as_u64()));
    let out = json!({
        "artifact_id": id,
        "url": c.browser_url(&format!("/a/{id}")),
        "title": art["artifact"]["title"],
        "current_version": art["artifact"]["current_version"],
        "versions": versions,
    });
    if cli.json {
        println!("{out}");
        return Ok(());
    }
    let paint = Paint::stdout();
    let now = chrono::Utc::now();
    let mut lines = vec![format!(
        "{}  {}  {}",
        paint.bold(&term::snippet(out["title"].as_str().unwrap_or(""), 60)),
        paint.dim(&id),
        paint.dim(out["url"].as_str().unwrap_or(""))
    )];
    for v in out["versions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let mut head = format!("  {}", paint.cyan(&format!("v{}", v["n"])));
        if v["n"] == out["current_version"] {
            head.push_str(&paint.green(" (current)"));
        }
        if let Some(l) = v["label"].as_str() {
            head.push_str(&format!("  {}", paint.bold(&term::snippet(l, 60))));
        }
        let by = match &v["publisher"] {
            Value::Null => "the command line".to_string(),
            p => {
                let h = term::snippet(p["harness"].as_str().unwrap_or("agent"), 20);
                match p["agent"].as_str() {
                    Some(a) => format!("{h} ({})", term::snippet(a, 30)),
                    None => h,
                }
            }
        };
        head.push_str(&paint.dim(&format!(
            "  by {by} · {}",
            term::ago(v["created_at"].as_str().unwrap_or_default(), now)
        )));
        lines.push(head);
        if let Some(n) = v["note"].as_str() {
            lines.push(format!("      {}", term::snippet(n, 200)));
        }
        let addressed: Vec<String> = v["addressed"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|t| match t["ref"].as_str() {
                Some(r) => format!("#{}", r.rsplit_once('#').map_or(r, |(_, n)| n)),
                None => format!("{} (deleted)", t["thread_id"].as_str().unwrap_or("?")),
            })
            .collect();
        if !addressed.is_empty() {
            lines.push(format!(
                "      {} {}",
                paint.dim("addressed"),
                addressed.join(", ")
            ));
        }
    }
    println!("{}", lines.join("\n"));
    Ok(())
}
