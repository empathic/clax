//! `clax inbox`: the owner's inbox (spec 2026-10-06-agent-questions-and-inbox
//! §8.4): list and search what agents sent back, show an item, and mark
//! items read or unread.
//!
//! The CLI holds the token, which makes it the owner identity, so its marks
//! are the owner's. A listing numbers its lines from 1 and keeps their item
//! IDs in `<home>/run/inbox-last.json`, so `clax inbox show 1` names the
//! first line of the last listing; an item ID is accepted anywhere a number
//! is.

use crate::client::Client;
use crate::term::{self, Paint};
use clax_core::Home;
use clax_core::store::inbox::is_item_id;
use serde_json::{Value, json};
use std::path::PathBuf;

/// Lists, searches, shows and marks the owner's inbox.
#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
    /// Read items only.
    #[arg(long, conflicts_with = "all")]
    pub read: bool,
    /// Read and unread items.
    #[arg(long)]
    pub all: bool,
    #[command(flatten)]
    pub filters: Filters,
    /// How many items to list (at most 200).
    #[arg(short = 'n', default_value_t = 20)]
    pub limit: u32,
    /// Search text: every word must start a word of the item.
    pub search: Vec<String>,
}

/// The filters a listing and `read --all` share.
#[derive(clap::Args, Default)]
pub struct Filters {
    /// Only items of this kind (repeatable).
    #[arg(long = "kind", value_parser = ["reply", "version", "published", "question", "finished"])]
    pub kinds: Vec<String>,
    /// Only items about this artifact (ID or URL).
    #[arg(long)]
    pub artifact: Option<String>,
    /// Only items from this agent: a harness (`claude`) or an agent handle (`a_…`).
    #[arg(long)]
    pub agent: Option<String>,
    /// Only items made at or after this date (YYYY-MM-DD, a local day) or RFC 3339 time.
    #[arg(long)]
    pub since: Option<String>,
    /// Only items made before this time, or up to the end of this date (a local day).
    #[arg(long)]
    pub until: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Print an item in full and mark it read.
    Show {
        /// A number from the last `clax inbox`, or an item ID.
        item: String,
    },
    /// Mark items read; `--all` marks every unread item matching the
    /// filters and `--search` given here.
    Read {
        /// Numbers from the last `clax inbox`, or item IDs.
        items: Vec<String>,
        /// Every unread item that matches.
        #[arg(long, conflicts_with = "items")]
        all: bool,
        #[command(flatten)]
        filters: Filters,
        /// With --all: only the items this search text finds.
        #[arg(long, requires = "all")]
        search: Option<String>,
    },
    /// Mark items unread.
    Unread {
        /// Numbers from the last `clax inbox`, or item IDs.
        #[arg(required = true)]
        items: Vec<String>,
    },
}

/// Most item numbers a listing keeps.
const MAX_NUMBERED: usize = 200;

fn last_path(home: &Home) -> PathBuf {
    home.root().join("run").join("inbox-last.json")
}

/// Keeps the item IDs a listing printed, in order, readable only by the
/// owner: written beside the file and renamed over it.
fn save_numbers(home: &Home, ids: &[String]) -> anyhow::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    let file = last_path(home);
    let dir = file.parent().expect("a parent directory");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let tmp = dir.join(format!(".inbox-last.json.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let res = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(serde_json::to_string(ids)?.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &file)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(res?)
}

/// The item ID `raw` names: an item ID, or a number from the last listing.
fn resolve(home: &Home, raw: &str) -> anyhow::Result<String> {
    let raw = raw.trim();
    if is_item_id(raw) {
        return Ok(raw.to_string());
    }
    let n: usize = raw
        .parse()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| anyhow::anyhow!("'{raw}' is not an item number or an item ID"))?;
    let ids: Vec<String> = std::fs::read(last_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    ids.get(n - 1).cloned().ok_or_else(|| {
        anyhow::anyhow!("no item {n} in the last listing: run `clax inbox` to number the items")
    })
}

/// The filters and search text as the route's parameters.
fn params(a: &Filters, search: Option<String>) -> anyhow::Result<Vec<(&'static str, String)>> {
    let mut q: Vec<(&'static str, String)> = Vec::new();
    if let Some(t) = search.filter(|t| !t.trim().is_empty()) {
        q.push(("q", t));
    }
    if !a.kinds.is_empty() {
        q.push(("kind", a.kinds.join(",")));
    }
    if let Some(art) = &a.artifact {
        let id = clax_mcp::tools::artifact_ref(art)
            .map(|(id, _)| id)
            .map_err(|_| anyhow::anyhow!("'{art}' is not an artifact ID or URL"))?;
        q.push(("artifact", id));
    }
    if let Some(v) = &a.agent {
        q.push(("agent", v.clone()));
    }
    if let Some(v) = &a.since {
        q.push(("since", local_day(v, false)));
    }
    if let Some(v) = &a.until {
        q.push(("until", local_day(v, true)));
    }
    Ok(q)
}

/// A bare `YYYY-MM-DD` as the RFC 3339 time of that day's local midnight
/// (with `end`, the next day's), so dates are the user's days, not UTC's;
/// anything else as given, for the daemon to read or refuse.
fn local_day(raw: &str, end: bool) -> String {
    use chrono::{Local, NaiveDate, SecondsFormat};
    let Ok(d) = NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d") else {
        return raw.to_string();
    };
    let d = if end { d.succ_opt().unwrap_or(d) } else { d };
    d.and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(Local).earliest())
        .map_or_else(
            || raw.to_string(),
            |t| t.to_rfc3339_opts(SecondsFormat::Millis, false),
        )
}

/// `s` percent-encoded for a query string: every byte but the unreserved
/// characters of RFC 3986.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    match &a.cmd {
        None => list(cli, home, &c, a),
        Some(Cmd::Show { item }) => show(cli, &c, &resolve(home, item)?),
        Some(Cmd::Read {
            all: true,
            filters,
            search,
            ..
        }) => {
            let filter: serde_json::Map<String, Value> = params(filters, search.clone())?
                .into_iter()
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect();
            let r = c.post("/api/inbox/read", &json!({"all": true, "filter": filter}))?;
            print_marked(cli, &r);
            Ok(())
        }
        Some(Cmd::Read { items, .. }) => {
            if items.is_empty() {
                anyhow::bail!("name items to mark read, or --all");
            }
            let ids = items
                .iter()
                .map(|i| resolve(home, i))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let r = c.post("/api/inbox/read", &json!({"ids": ids}))?;
            print_marked(cli, &r);
            Ok(())
        }
        Some(Cmd::Unread { items }) => {
            let ids = items
                .iter()
                .map(|i| resolve(home, i))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mut responses = Vec::new();
            let mut changed = 0;
            for id in &ids {
                let was_read = c.get(&format!("/api/inbox/{id}"))?["item"]["read"] == true;
                let r = c.post(&format!("/api/inbox/{id}/unread"), &json!({}))?;
                changed += usize::from(was_read);
                responses.push(r);
            }
            if cli.json {
                println!("{}", Value::Array(responses));
            } else {
                let unread = responses
                    .last()
                    .map_or(Value::Null, |r| r["unread"].clone());
                println!("marked {changed} unread ({unread} unread)");
            }
            Ok(())
        }
    }
}

fn print_marked(cli: &crate::Cli, r: &Value) {
    if cli.json {
        println!("{r}");
    } else {
        println!("marked {} ({} unread)", r["marked"], r["unread"]);
    }
}

fn list(cli: &crate::Cli, home: &Home, c: &Client, a: &Args) -> anyhow::Result<()> {
    let read = if a.all {
        "all"
    } else if a.read {
        "read"
    } else {
        "unread"
    };
    let limit = a.limit.clamp(1, MAX_NUMBERED as u32);
    let mut q = vec![("read", read.to_string()), ("limit", limit.to_string())];
    q.extend(params(&a.filters, Some(a.search.join(" ")))?);
    let q: Vec<String> = q
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect();
    let v = c.get(&format!("/api/inbox?{}", q.join("&")))?;
    let items = v["items"].as_array().cloned().unwrap_or_default();
    let ids: Vec<String> = items
        .iter()
        .filter_map(|i| i["id"].as_str().map(str::to_string))
        .collect();
    save_numbers(home, &ids)?;
    if cli.json {
        println!("{v}");
        return Ok(());
    }
    if items.is_empty() {
        println!(
            "{}",
            match read {
                "unread" => "no unread items",
                "read" => "no read items",
                _ => "no items",
            }
        );
        return Ok(());
    }
    let paint = Paint::stdout();
    let now = chrono::Utc::now();
    let width = items.len().to_string().len();
    for (n, i) in items.iter().enumerate() {
        println!("{}", line(i, n + 1, width, paint, now));
    }
    if v["next_cursor"].is_string() {
        let shown = match &v["total"] {
            Value::Null => format!("{} shown", items.len()),
            t => format!(
                "{} of {} shown",
                items.len(),
                t.as_str().map_or(t.to_string(), str::to_string)
            ),
        };
        let hint = if limit < MAX_NUMBERED as u32 {
            format!("raise -n (at most {MAX_NUMBERED})")
        } else {
            "narrow with --since, --until, --kind or more search words".to_string()
        };
        println!("{}", paint.dim(&format!("… {shown}; {hint}")));
    }
    Ok(())
}

/// What an item says, in one line of agent text (not yet escaped).
fn text(i: &Value) -> String {
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let t = match i["kind"].as_str().unwrap_or_default() {
        "reply" => s(&i["reply"]["body"]),
        "version" => {
            let note = s(&i["version"]["note"]);
            let n = &i["version"]["n"];
            if note.is_empty() {
                format!("version {n}")
            } else {
                format!("version {n}: {note}")
            }
        }
        "published" => s(&i["published"]["description"]),
        "question" => {
            let q = &i["question"];
            let first = s(&q["questions"][0]["question"]);
            match q["status"].as_str() {
                Some("open") | None => first,
                Some(st) => format!("{first} ({st})"),
            }
        }
        "finished" => s(&i["work"]["message"]),
        _ => String::new(),
    };
    if i["gone"] == true {
        format!("{t} (gone)").trim().to_string()
    } else {
        t
    }
}

fn title(i: &Value) -> String {
    i["artifact"]["title"]
        .as_str()
        .map(str::to_string)
        .or_else(|| i["artifact"]["id"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A listing line: number, read mark, harness, kind, title, text, age.
fn line(
    i: &Value,
    n: usize,
    width: usize,
    paint: Paint,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let unread = i["read"] != true;
    let mark = if unread { "●" } else { "○" };
    let harness = term::snippet(i["agent"]["harness"].as_str().unwrap_or("agent"), 12);
    format!(
        "{}  {}  {:<8} {:<10} {:<20} {}   {}",
        paint.cyan(&format!("{n:>width$}")),
        if unread {
            paint.bold(mark)
        } else {
            paint.dim(mark)
        },
        harness,
        i["kind"].as_str().unwrap_or_default(),
        term::snippet(&title(i), 20),
        term::snippet(&text(i), 80),
        paint.dim(&term::ago(
            i["created_at"].as_str().unwrap_or_default(),
            now
        )),
    )
}

fn show(cli: &crate::Cli, c: &Client, id: &str) -> anyhow::Result<()> {
    let r = c.post(&format!("/api/inbox/{id}/read"), &json!({}))?;
    if cli.json {
        println!("{r}");
        return Ok(());
    }
    let i = &r["item"];
    let paint = Paint::stdout();
    let now = chrono::Utc::now();
    let field = |k: &str, v: String| format!("  {:<10}{v}", paint.dim(k));
    let mut out = vec![format!(
        "{}  {}  {}",
        paint.bold(i["kind"].as_str().unwrap_or_default()),
        term::snippet(&title(i), 60),
        paint.dim(&term::ago(
            i["created_at"].as_str().unwrap_or_default(),
            now
        ))
    )];
    out.push(field(
        "item",
        term::clean_line(i["id"].as_str().unwrap_or_default()),
    ));
    if let Some(h) = i["agent"]["harness"].as_str() {
        let handle = i["agent"]["handle"].as_str().unwrap_or_default();
        let project = i["agent"]["project"].as_str().unwrap_or_default();
        out.push(field(
            "agent",
            term::clean_line(&format!("{h} {handle} ({project})")),
        ));
    }
    if let Some(s) = i["thread"]["summary"].as_str() {
        out.push(field("thread", term::snippet(s, 160)));
    }
    if i["gone"] == true {
        out.push(field("gone", "its source no longer exists".into()));
    }
    let url = i["url"].as_str().unwrap_or("/");
    out.push(field("url", term::clean_line(&c.browser_url(url))));
    out.push(String::new());
    match i["kind"].as_str().unwrap_or_default() {
        "question" => {
            let q = &i["question"];
            for x in q["questions"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                out.push(format!(
                    "{} {}",
                    paint.bold(&term::snippet(x["header"].as_str().unwrap_or_default(), 40)),
                    term::clean_text(x["question"].as_str().unwrap_or_default())
                ));
                for o in x["options"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    out.push(format!(
                        "  - {}",
                        term::snippet(o["label"].as_str().unwrap_or_default(), 120)
                    ));
                }
            }
            out.push(format!(
                "status: {}",
                term::clean_line(q["status"].as_str().unwrap_or_default())
            ));
            if q["status"] == "open" {
                out.push(format!(
                    "answer it at {}",
                    term::clean_line(&c.browser_url(url))
                ));
            }
        }
        "finished" => {
            out.push(term::clean_text(
                i["work"]["message"].as_str().unwrap_or_default(),
            ));
            for t in i["work"]["threads"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                out.push(format!(
                    "  thread {}",
                    term::snippet(t["summary"].as_str().unwrap_or_default(), 120)
                ));
            }
        }
        "version" => {
            out.push(term::clean_text(&text(i)));
            for t in i["version"]["addressed"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                out.push(format!(
                    "  addressed {}",
                    term::snippet(t["summary"].as_str().unwrap_or_default(), 120)
                ));
            }
        }
        _ => out.push(term::clean_text(&text(i))),
    }
    println!("{}", out.join("\n"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Local, NaiveDate};

    #[test]
    fn a_bare_date_is_a_local_day() {
        let local = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&Local)
        };
        let since = local(&local_day("2026-10-07", false));
        let until = local(&local_day("2026-10-07", true));
        assert_eq!(
            since.date_naive(),
            NaiveDate::from_ymd_opt(2026, 10, 7).unwrap()
        );
        assert_eq!(
            until.date_naive(),
            NaiveDate::from_ymd_opt(2026, 10, 8).unwrap()
        );
        assert_eq!(since.format("%H:%M:%S").to_string(), "00:00:00");
        assert_eq!(
            local_day("2026-10-07T10:00:00Z", true),
            "2026-10-07T10:00:00Z"
        );
        assert_eq!(local_day("yesterday", false), "yesterday");
    }
}
