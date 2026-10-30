//! `clax comments`: the comment threads on artifacts, read and acted on as
//! the owner.
//!
//! Threads are numbered per artifact, `#1` upward in the order they were
//! created, resolved threads included, so a number stays put when threads
//! are resolved or reopened (it shifts only when an older thread is deleted).
//! A thread is named `<artifact>#<n>` (the artifact by ID or URL), or by its
//! thread ID alone (a ULID, unique across artifacts).
//!
//! Acting on a thread goes through the routes the owner's browser uses, with
//! the token, which makes the CLI the owner identity: the one viewer every
//! browser of the owner's is too. Replies, resolves and sends carry the
//! owner's name, the one `clax comments name` sets and the browser's "Your
//! name" field shows.
//!
//! An artifact's URL is resolved by the daemon (`GET /api/resolve`), as the
//! MCP tools resolve one: an artifact of a Clax instance this home is
//! connected to is listed, shown, replied to, resolved and reopened through
//! the daemon's remote artifact routes (`/api/remotes/<rid>/artifacts/<id>`),
//! with the agent token the daemon holds there, as the person it is the
//! token of. Sending a remote thread to an agent is not offered.

use crate::client::Client;
use crate::term::{self, Paint};
use clax_core::Home;
use reqwest::Method;
use serde_json::{Value, json};
use std::io::Read;

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
    /// An artifact (ID or URL) to list the threads of, or a thread
    /// (`<artifact>#<n>`, or a thread ID) to show. Every artifact when absent.
    pub target: Option<String>,
    /// Also list resolved threads.
    #[arg(long)]
    pub all: bool,
}

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Show a whole thread: anchor, quote, clip, comments, versions, state.
    Show(ThreadArg),
    /// Reply to a thread as you; a thread sent to the agent forwards the reply.
    Reply(ReplyArgs),
    /// Resolve a thread.
    Resolve(ThreadArg),
    /// Reopen a resolved thread.
    Reopen(ThreadArg),
    /// Send a thread to the agent, as the page's "Send to agent" button does.
    Send(SendArgs),
    /// Show or set the name your replies carry.
    Name(NameArgs),
}

#[derive(clap::Args)]
pub struct ThreadArg {
    /// `<artifact>#<n>` (artifact ID or URL, then the number listings show),
    /// or a thread ID.
    pub thread: String,
}

#[derive(clap::Args)]
pub struct ReplyArgs {
    /// `<artifact>#<n>` or a thread ID.
    pub thread: String,
    /// The reply; `-` reads it from stdin.
    pub text: String,
}

#[derive(clap::Args)]
pub struct SendArgs {
    /// `<artifact>#<n>` or a thread ID.
    pub thread: String,
    /// The agent handle (`a_...`) to send to. Default: the most recently
    /// active live agent on the artifact, as the page picks; with none live,
    /// the thread waits for the next agent that publishes or watches it.
    #[arg(long)]
    pub to: Option<String>,
}

#[derive(clap::Args)]
pub struct NameArgs {
    /// The name to set; prints the current one when absent, clears it when empty.
    pub name: Option<String>,
}

/// An artifact named on the command line: this daemon's, or one on a
/// connected remote.
#[derive(Clone, Debug, PartialEq)]
struct Art {
    /// Its ID (on its remote, for a remote one).
    id: String,
    /// The daemon's route of it: `/api/artifacts/<id>`, or
    /// `/api/remotes/<rid>/artifacts/<id>`.
    api: String,
    /// On a remote: its address there, and `{origin, name}`.
    remote: Option<(String, Value)>,
}

impl Art {
    fn local(id: &str) -> Art {
        Art {
            id: id.to_string(),
            api: format!("/api/artifacts/{id}"),
            remote: None,
        }
    }

    /// The artifact `GET /api/resolve`'s answer `v` for `raw` names.
    fn resolved(raw: &str, v: &Value) -> anyhow::Result<Art> {
        let text = |k: &str| v[k].as_str().unwrap_or_default();
        match text("kind") {
            "local" => Ok(Art::local(text("artifact_id"))),
            "remote" => Ok(Art {
                id: text("artifact_id").to_string(),
                api: format!(
                    "/api/remotes/{}/artifacts/{}",
                    text("remote"),
                    text("artifact_id")
                ),
                remote: Some((
                    text("url").to_string(),
                    json!({"origin": v["origin"], "name": v["name"]}),
                )),
            }),
            "sign_in_required" => anyhow::bail!(
                "{raw} is on {} ({}), a Clax instance this Clax is not connected to: connect it with `{}`",
                text("name"),
                text("origin"),
                v["request"]["command"].as_str().map_or_else(
                    || format!("clax remote add {}", text("origin")),
                    str::to_string
                )
            ),
            _ => anyhow::bail!("'{raw}' is a web page, not a Clax artifact"),
        }
    }

    /// Its address in a browser.
    fn url(&self, c: &Client) -> String {
        match &self.remote {
            Some((url, _)) => url.clone(),
            None => c.browser_url(&format!("/a/{}", self.id)),
        }
    }
}

/// One artifact's threads, every one of them, oldest first: a thread's
/// number is its index plus one.
struct Group {
    art: Art,
    artifact_id: String,
    title: String,
    files: Vec<String>,
    threads: Vec<Value>,
}

impl Group {
    fn find(&self, tid: &str) -> Option<usize> {
        self.threads.iter().position(|t| t["id"] == tid)
    }
}

/// A thread named on the command line, its artifact as given.
enum ThreadRef {
    /// `<artifact>#<n>`.
    Number(String, usize),
    /// A thread ID, with the artifact when given (`<artifact>#<ULID>`).
    Id(Option<String>, String),
}

/// The thread `raw` names, or `None` when it names no thread (an artifact,
/// or anything else). A URL's `#` fragment that is not a number or a thread
/// ID is not a thread.
fn thread_ref(raw: &str) -> anyhow::Result<Option<ThreadRef>> {
    let raw = raw.trim();
    if clax_core::is_ulid(raw) {
        return Ok(Some(ThreadRef::Id(None, raw.to_string())));
    }
    let Some((left, right)) = raw.rsplit_once('#') else {
        return Ok(None);
    };
    if let Ok(n) = right.parse::<usize>() {
        if n == 0 {
            anyhow::bail!("thread numbers start at #1");
        }
        return Ok(Some(ThreadRef::Number(artifact_named(left)?, n)));
    }
    if clax_core::is_ulid(right) {
        return Ok(Some(ThreadRef::Id(
            Some(artifact_named(left)?),
            right.to_string(),
        )));
    }
    Ok(None)
}

/// Whether `raw` is an http(s) URL, which the daemon resolves.
fn is_url(raw: &str) -> bool {
    let lower = raw.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// `raw` when it names an artifact: an http(s) URL (which the daemon
/// resolves, [`artifact`]), or an artifact ID or Clax path, as the MCP
/// tools read one.
fn artifact_named(raw: &str) -> anyhow::Result<String> {
    let raw = raw.trim();
    if is_url(raw) || clax_mcp::tools::artifact_ref(raw).is_ok() {
        return Ok(raw.to_string());
    }
    anyhow::bail!("'{raw}' is not an artifact ID or URL, `<artifact>#<n>`, or a thread ID")
}

/// The artifact `raw` names: an http(s) URL as the daemon resolves it
/// (`GET /api/resolve`; a daemon from before it reads one as the MCP tools
/// read an ID), else the artifact ID it is or holds.
fn artifact(c: &Client, raw: &str) -> anyhow::Result<Art> {
    let raw = artifact_named(raw)?;
    if is_url(&raw) {
        let q = super::remotes::encode(&raw);
        match c.get(&format!("/api/resolve?url={q}")) {
            Ok(v) => return Art::resolved(&raw, &v),
            Err(e) if e.to_string().contains("not_found") => {}
            Err(e) => return Err(e),
        }
    }
    clax_mcp::tools::artifact_ref(&raw)
        .map(|(id, _)| Art::local(&id))
        .map_err(|_| {
            anyhow::anyhow!(
                "'{raw}' is not an artifact ID or URL, `<artifact>#<n>`, or a thread ID"
            )
        })
}

/// Every thread of every artifact that has any. On a remote daemon
/// (spec §6.7), each readable artifact's threads, through `GET
/// /api/artifacts` and each artifact's thread listing.
fn load_all(c: &Client) -> anyhow::Result<Vec<Group>> {
    if c.remote().is_some() {
        let list = c.get("/api/artifacts")?;
        let mut groups = Vec::new();
        for a in list["artifacts"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(id) = a["id"].as_str() else {
                continue;
            };
            let g = load_one(c, &Art::local(id))?;
            if !g.threads.is_empty() {
                groups.push(g);
            }
        }
        return Ok(groups);
    }
    let v = c.get("/api/threads?include_resolved=true")?;
    Ok(v["artifacts"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|g| Group {
            art: Art::local(g["artifact_id"].as_str().unwrap_or_default()),
            artifact_id: g["artifact_id"].as_str().unwrap_or_default().to_string(),
            title: g["title"].as_str().unwrap_or_default().to_string(),
            files: strings(&g["files"]),
            threads: g["threads"].as_array().cloned().unwrap_or_default(),
        })
        .collect())
}

/// Every thread of artifact `art`.
fn load_one(c: &Client, art: &Art) -> anyhow::Result<Group> {
    let a = c.get(&art.api)?;
    let n = a["artifact"]["current_version"].as_u64();
    let files = a["versions"]
        .as_array()
        .and_then(|vs| vs.iter().find(|v| v["n"].as_u64() == n))
        .and_then(|v| v["files"].as_object())
        .map(|f| f.keys().cloned().collect())
        .unwrap_or_default();
    Ok(Group {
        artifact_id: art.id.clone(),
        art: art.clone(),
        title: a["artifact"]["title"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        files,
        threads: threads_at(c, &art.api)?,
    })
}

/// Every thread view of artifact `id`, resolved ones included, oldest first:
/// the order thread numbers follow.
pub(crate) fn all_threads(c: &Client, id: &str) -> anyhow::Result<Vec<Value>> {
    threads_at(c, &format!("/api/artifacts/{id}"))
}

/// [`all_threads`] of the artifact at the daemon's route `api`.
fn threads_at(c: &Client, api: &str) -> anyhow::Result<Vec<Value>> {
    let mut threads = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut path = format!("{api}/threads?include_resolved=true&limit=200");
        if let Some(cur) = &cursor {
            // A remote's cursor is its own text: one query value here.
            path.push_str(&format!("&cursor={}", super::remotes::encode(cur)));
        }
        let page = c.get(&path)?;
        threads.extend(page["threads"].as_array().cloned().unwrap_or_default());
        match page["next_cursor"].as_str() {
            Some(n) => cursor = Some(n.to_string()),
            None => return Ok(threads),
        }
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect()
}

/// The group and index of the thread `r` names.
fn locate(c: &Client, r: ThreadRef) -> anyhow::Result<(Group, usize)> {
    match r {
        ThreadRef::Number(raw, n) => {
            let g = load_one(c, &artifact(c, &raw)?)?;
            let aid = &g.artifact_id;
            if n > g.threads.len() {
                anyhow::bail!(
                    "{aid} has no thread #{n}: it has {} (run `clax comments {raw} --all`)",
                    g.threads.len()
                );
            }
            Ok((g, n - 1))
        }
        ThreadRef::Id(Some(raw), tid) => {
            let g = load_one(c, &artifact(c, &raw)?)?;
            let aid = &g.artifact_id;
            let i = g
                .find(&tid)
                .ok_or_else(|| anyhow::anyhow!("{aid} has no thread {tid}"))?;
            Ok((g, i))
        }
        ThreadRef::Id(None, tid) => load_all(c)?
            .into_iter()
            .find_map(|g| g.find(&tid).map(|i| (g, i)))
            .ok_or_else(|| anyhow::anyhow!("no artifact has a thread {tid}")),
    }
}

/// The thread `raw` names, which must name one.
fn locate_raw(c: &Client, raw: &str) -> anyhow::Result<(Group, usize)> {
    let r = thread_ref(raw)?.ok_or_else(|| {
        anyhow::anyhow!(
            "'{raw}' names no thread: use `<artifact>#<n>` (as listings show) or a thread ID"
        )
    })?;
    locate(c, r)
}

/// When the thread was last touched: its newest comment, resolve, or creation.
fn last_activity(t: &Value) -> String {
    let mut at = t["created_at"].as_str().unwrap_or_default().to_string();
    let comments = t["comments"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    for s in comments
        .iter()
        .filter_map(|c| c["created_at"].as_str())
        .chain(t["resolved_at"].as_str())
    {
        if s > at.as_str() {
            at = s.to_string();
        }
    }
    at
}

/// An open thread whose anchor's page the current version lacks. A thread
/// about the whole artifact is on no page and never detached.
fn is_detached(g: &Group, t: &Value) -> bool {
    if t["anchor"]["kind"] == "artifact" {
        return false;
    }
    let file = t["anchor"]["file"]
        .as_str()
        .unwrap_or(clax_core::anchor::INDEX_FILE);
    t["status"] == "open" && !g.files.is_empty() && !g.files.iter().any(|f| f == file)
}

/// The roster entries working on thread `tid` of `aid`.
fn working_on<'a>(roster: &'a [Value], aid: &str, tid: &str) -> Vec<&'a Value> {
    roster
        .iter()
        .filter(|w| {
            w["artifact_id"] == aid
                && w["thread_ids"]
                    .as_array()
                    .is_some_and(|ts| ts.iter().any(|x| x == tid))
        })
        .collect()
}

/// The working roster, or empty when the daemon cannot say.
fn roster(c: &Client) -> Vec<Value> {
    c.get("/api/working")
        .ok()
        .and_then(|v| v["working"].as_array().cloned())
        .unwrap_or_default()
}

/// A thread as the CLI's JSON shows it: `comments_read`'s thread object plus
/// `ref`, `n`, `detached`, the times, the resolution, `addressed_in`, `sends`
/// and the agents `working` on it.
fn thread_json(g: &Group, i: usize, roster: &[Value]) -> Value {
    let t = &g.threads[i];
    let mut v = clax_mcp::tools::thread_summary(t);
    let tid = t["id"].as_str().unwrap_or_default();
    v["ref"] = json!(format!("{}#{}", g.artifact_id, i + 1));
    v["n"] = json!(i + 1);
    v["detached"] = json!(is_detached(g, t));
    v["created_at"] = t["created_at"].clone();
    v["last_activity_at"] = json!(last_activity(t));
    v["resolved_at"] = t["resolved_at"].clone();
    v["resolved_by"] = t["resolved_by"].clone();
    v["resolved_by_name"] = t["resolved_by_name"].clone();
    v["addressed_in"] = t["addressed_in"].clone();
    v["sends"] = t["sends"].clone();
    v["working"] = json!(
        working_on(roster, &g.artifact_id, tid)
            .iter()
            .map(|w| json!({
                "agent": w["agent"],
                "harness": w["harness"],
                "session_id": w["session_id"],
                "message": w["message"],
                "started_at": w["started_at"],
            }))
            .collect::<Vec<_>>()
    );
    v
}

fn group_json(c: &Client, g: &Group, picked: &[usize], roster: &[Value]) -> Value {
    let mut v = json!({
        "artifact_id": g.artifact_id,
        "url": g.art.url(c),
        "title": g.title,
        "threads": picked.iter().map(|&i| thread_json(g, i, roster)).collect::<Vec<_>>(),
        "next_cursor": null,
    });
    if let Some((_, instance)) = &g.art.remote {
        v["remote"] = instance.clone();
    }
    v
}

/// The indexes of `g`'s threads a listing shows, newest activity first.
fn pick(g: &Group, all: bool) -> Vec<usize> {
    let mut v: Vec<usize> = (0..g.threads.len())
        .filter(|&i| all || g.threads[i]["status"] == "open")
        .collect();
    v.sort_by_key(|&i| std::cmp::Reverse(last_activity(&g.threads[i])));
    v
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    match &a.cmd {
        Some(Cmd::Show(t)) => show(cli, home, &t.thread),
        Some(Cmd::Reply(r)) => reply(cli, home, r),
        Some(Cmd::Resolve(t)) => act(cli, home, &t.thread, Action::Resolve),
        Some(Cmd::Reopen(t)) => act(cli, home, &t.thread, Action::Reopen),
        Some(Cmd::Send(s)) => send(cli, home, s),
        Some(Cmd::Name(n)) => name(cli, home, n),
        None => match a.target.as_deref() {
            Some(t) if thread_ref(t)?.is_some() => {
                if a.all {
                    anyhow::bail!("--all lists threads; it does not apply to showing one");
                }
                show(cli, home, t)
            }
            Some(t) => list(cli, home, Some(&artifact_named(t)?), a.all),
            None => list(cli, home, None, a.all),
        },
    }
}

fn list(cli: &crate::Cli, home: &Home, only: Option<&str>, all: bool) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    let groups = match only {
        Some(raw) => vec![load_one(&c, &artifact(&c, raw)?)?],
        None => load_all(&c)?,
    };
    let roster = roster(&c);
    let mut shown: Vec<(&Group, Vec<usize>)> = groups
        .iter()
        .map(|g| (g, pick(g, all)))
        .filter(|(_, p)| !p.is_empty() || only.is_some())
        .collect();
    shown.sort_by_key(|(g, p)| {
        std::cmp::Reverse(
            p.first()
                .map(|&i| last_activity(&g.threads[i]))
                .unwrap_or_default(),
        )
    });
    if cli.json {
        let out = match only {
            Some(_) => {
                let (g, p) = &shown[0];
                let mut v = group_json(&c, g, p, &roster);
                v["note"] = json!(clax_core::feedback::UNTRUSTED_NOTE);
                v
            }
            None => json!({
                "artifacts": shown.iter().map(|(g, p)| group_json(&c, g, p, &roster)).collect::<Vec<_>>(),
                "note": clax_core::feedback::UNTRUSTED_NOTE,
            }),
        };
        println!("{out}");
        return Ok(());
    }
    let paint = Paint::stdout();
    let now = chrono::Utc::now();
    let mut out = Vec::new();
    for (g, picked) in &shown {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push(header(&c, g, paint));
        if picked.is_empty() {
            out.push(format!(
                "  {}",
                paint.dim(if all { "no threads" } else { "no open threads" })
            ));
        }
        for &i in picked {
            out.extend(thread_lines(g, i, &roster, paint, now));
        }
    }
    if out.is_empty() {
        out.push(if all {
            "no threads".to_string()
        } else {
            "no open threads".to_string()
        });
    }
    println!("{}", out.join("\n"));
    Ok(())
}

fn header(c: &Client, g: &Group, paint: Paint) -> String {
    format!(
        "{}  {}  {}",
        paint.bold(&term::snippet(&g.title, 60)),
        paint.dim(&g.artifact_id),
        paint.dim(&g.art.url(c))
    )
}

/// The feedback state of a sent thread in a few words.
fn delivery(t: &Value) -> String {
    let state = t["feedback_state"]["state"].as_str();
    match state {
        Some("sent") => "sent to agent, not picked up yet".into(),
        Some("delivered") => "sent to agent, delivered".into(),
        Some("acknowledged") => "sent to agent, read".into(),
        Some("agent_ended") => "sent to agent, whose session ended".into(),
        Some("no_access") => "sent to agent, held: the agent no longer has access".into(),
        Some("withheld") => {
            let wait = match t["feedback_state"]["approver"].as_str() {
                Some(who) => format!(
                    "sent to agent, waiting for {}'s approval",
                    term::snippet(who, 40)
                ),
                None => "sent to agent, waiting for an approval".into(),
            };
            if t["feedback_state"]["no_access"] == true {
                format!("{wait}; the agent no longer has access")
            } else {
                wait
            }
        }
        _ => "sent to agent".into(),
    }
}

fn working_text(w: &Value, now: chrono::DateTime<chrono::Utc>) -> String {
    let who = term::snippet(w["harness"].as_str().unwrap_or("agent"), 20);
    let since = term::age(w["started_at"].as_str().unwrap_or_default(), now);
    match w["message"].as_str() {
        Some(m) => format!("{who} working: {} ({since})", term::snippet(m, 80)),
        None => format!("{who} working ({since})"),
    }
}

fn author(c: &Value) -> String {
    let name = term::snippet(c["author_name"].as_str().unwrap_or("?"), 40);
    if c["author_kind"] == "agent" {
        format!("{name} (agent)")
    } else {
        name
    }
}

/// A listed thread: number, anchor and age; its latest comment; its state.
fn thread_lines(
    g: &Group,
    i: usize,
    roster: &[Value],
    paint: Paint,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<String> {
    let t = &g.threads[i];
    let s = clax_mcp::tools::thread_summary(t);
    let anchor = term::snippet(s["anchor"]["summary"].as_str().unwrap_or(""), 70);
    let mut lines = vec![format!(
        "  {}  {}  {}",
        paint.cyan(&format!("#{}", i + 1)),
        anchor,
        paint.dim(&term::age(&last_activity(t), now))
    )];
    if let Some(c) = t["comments"].as_array().and_then(|c| c.last()) {
        lines.push(format!(
            "      {} {}",
            paint.bold(&format!("{}:", author(c))),
            term::snippet(c["body"].as_str().unwrap_or_default(), 100)
        ));
    }
    let mut state = Vec::new();
    if t["status"] == "resolved" {
        state.push(paint.green("resolved"));
    }
    if is_detached(g, t) {
        state.push(paint.yellow("detached"));
    }
    if t["sent_to_agent"] == true {
        state.push(paint.magenta(&delivery(t)));
    }
    let tid = t["id"].as_str().unwrap_or_default();
    for w in working_on(roster, &g.artifact_id, tid) {
        state.push(paint.magenta(&working_text(w, now)));
    }
    if !state.is_empty() {
        lines.push(format!("      {}", state.join(" · ")));
    }
    lines
}

fn show(cli: &crate::Cli, home: &Home, raw: &str) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    let (g, i) = locate_raw(&c, raw)?;
    let roster = roster(&c);
    if cli.json {
        let mut v = group_json(&c, &g, &[i], &roster);
        v["note"] = json!(clax_core::feedback::UNTRUSTED_NOTE);
        println!("{v}");
        return Ok(());
    }
    let paint = Paint::stdout();
    let now = chrono::Utc::now();
    let t = &g.threads[i];
    let s = clax_mcp::tools::thread_summary(t);
    let mut out = vec![header(&c, &g, paint)];
    let mut state = vec![if t["status"] == "resolved" {
        paint.green("resolved")
    } else {
        "open".to_string()
    }];
    if is_detached(&g, t) {
        state.push(paint.yellow("detached (its page is not in the current version)"));
    }
    if t["sent_to_agent"] == true {
        state.push(paint.magenta(&delivery(t)));
    }
    out.push(format!(
        "{}  {}",
        paint.cyan(&paint.bold(&format!("{}#{}", g.artifact_id, i + 1))),
        state.join(" · ")
    ));
    let field = |k: &str, v: String| format!("  {:<10}{v}", paint.dim(k));
    out.push(field(
        "thread",
        t["id"].as_str().unwrap_or_default().to_string(),
    ));
    // The summary ends in the quote, which has its own line here.
    let summary = s["anchor"]["summary"].as_str().unwrap_or("");
    let target = summary.rsplit_once("  «").map_or(summary, |(t, _)| t);
    out.push(field("anchor", term::snippet(target, 160)));
    if let Some(q) = t["anchor"]["quote"]
        .as_str()
        .filter(|q| !q.trim().is_empty())
    {
        out.push(field("quote", format!("«{}»", term::snippet(q, 300))));
    }
    if let Some(p) = t["clip_path"].as_str() {
        out.push(field("clip", term::clean_line(p)));
    }
    let mut versions = format!("left on v{}", t["version_n"]);
    let addressed: Vec<String> = t["addressed_in"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|n| format!("v{n}"))
        .collect();
    if !addressed.is_empty() {
        versions.push_str(&format!(" · addressed in {}", addressed.join(", ")));
    }
    out.push(field("versions", versions));
    if let Some(fs) = t["feedback_state"].as_object() {
        let mut f = fs["state"].as_str().unwrap_or("").to_string();
        if let Some(tier) = fs.get("tier").and_then(Value::as_str) {
            f.push_str(&format!(" via {tier}"));
        }
        if let Some(since) = fs.get("since").and_then(Value::as_str) {
            f.push_str(&format!(" · {}", term::ago(since, now)));
        }
        if fs.get("resends").and_then(Value::as_u64).unwrap_or(0) > 0 {
            f.push_str(&format!(" · resent {}", fs["resends"]));
        }
        out.push(field("feedback", f));
    }
    for send in t["sends"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let mut line = format!(
            "batch of {} by {}",
            send["size"],
            term::snippet(send["sent_by"].as_str().unwrap_or("?"), 40)
        );
        if let Some(n) = send["note"].as_str() {
            line.push_str(&format!(": {}", term::snippet(n, 120)));
        }
        out.push(field("sent", line));
    }
    if t["status"] == "resolved" {
        let by = t["resolved_by_name"]
            .as_str()
            .or(t["resolved_by"].as_str())
            .unwrap_or("?");
        out.push(field(
            "resolved",
            format!(
                "by {} · {}",
                term::snippet(by, 40),
                term::ago(t["resolved_at"].as_str().unwrap_or_default(), now)
            ),
        ));
    }
    for w in working_on(
        &roster,
        &g.artifact_id,
        t["id"].as_str().unwrap_or_default(),
    ) {
        out.push(field("working", paint.magenta(&working_text(w, now))));
    }
    for cm in t["comments"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        out.push(String::new());
        let mut head = format!(
            "  {}  {}",
            paint.bold(&author(cm)),
            paint.dim(&term::ago(
                cm["created_at"].as_str().unwrap_or_default(),
                now
            ))
        );
        if cm["via_page"] == true {
            head.push_str(&paint.dim("  (written by the page)"));
        }
        out.push(head);
        for line in term::clean_text(cm["body"].as_str().unwrap_or_default()).lines() {
            out.push(format!("    {line}"));
        }
    }
    println!("{}", out.join("\n"));
    Ok(())
}

/// The owner identity the CLI acts as (the token makes it the owner's, as
/// the owner's browsers are).
fn owner(c: &Client) -> anyhow::Result<Value> {
    Ok(c.send(Method::GET, "/api/viewers/me", None)?["viewer"].clone())
}

fn reply(cli: &crate::Cli, home: &Home, r: &ReplyArgs) -> anyhow::Result<()> {
    let text = if r.text == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        r.text.clone()
    };
    if text.trim().is_empty() {
        anyhow::bail!("the reply is empty");
    }
    let c = Client::connect(home, cli.port_for(home)?)?;
    let (g, i) = locate_raw(&c, &r.thread)?;
    // A remote person writes in their browser; their replies here are
    // refused for want of a session (spec §6.7), and they have no owner
    // identity to read.
    let me = match c.remote() {
        Some(_) => json!({"display_name": "remote"}),
        None => owner(&c)?,
    };
    let tid = g.threads[i]["id"].as_str().unwrap_or_default();
    let res = write(
        &c,
        &g,
        &format!("{}/threads/{tid}/comments", g.art.api),
        Some(&json!({"body": text})),
    )?;
    let reference = format!("{}#{}", g.artifact_id, i + 1);
    let forwarded = res["thread"]["sent_to_agent"] == true;
    let out = json!({
        "thread_id": tid,
        "ref": reference,
        "replied": true,
        "comment_id": res["comment"]["id"],
        "author_name": res["comment"]["author_name"],
        "sent_to_agent": forwarded,
    });
    super::print(cli, out, |j| {
        let mut s = format!(
            "replied to {reference} as {}",
            term::snippet(j["author_name"].as_str().unwrap_or("?"), 40)
        );
        if forwarded {
            s.push_str("; forwarded to the agent");
        }
        s
    });
    if !cli.json && me["display_name"].is_null() {
        eprintln!("hint: set the name your replies carry with `clax comments name <name>`");
    }
    Ok(())
}

/// A `POST` of `body` to `path`, a thread route of `g`'s artifact: on a
/// remote one with an `Idempotency-Key` of its own, so the write sent again
/// is answered there with what the first made.
fn write(c: &Client, g: &Group, path: &str, body: Option<&Value>) -> anyhow::Result<Value> {
    match &g.art.remote {
        Some(_) => c.send_keyed(Method::POST, path, body, &clax_core::new_ulid()),
        None => c.send(Method::POST, path, body),
    }
}

enum Action {
    Resolve,
    Reopen,
}

fn act(cli: &crate::Cli, home: &Home, raw: &str, action: Action) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    let (g, i) = locate_raw(&c, raw)?;
    let tid = g.threads[i]["id"].as_str().unwrap_or_default();
    let (verb, key) = match action {
        Action::Resolve => ("resolve", "resolved"),
        Action::Reopen => ("reopen", "reopened"),
    };
    let res = write(&c, &g, &format!("{}/threads/{tid}/{verb}", g.art.api), None)?;
    let reference = format!("{}#{}", g.artifact_id, i + 1);
    let mut out = json!({"thread_id": tid, "ref": reference, "status": res["thread"]["status"]});
    out[key] = json!(true);
    super::print(cli, out, |_| format!("{key} {reference}"));
    Ok(())
}

fn send(cli: &crate::Cli, home: &Home, a: &SendArgs) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    let (g, i) = locate_raw(&c, &a.thread)?;
    if let Some((url, _)) = &g.art.remote {
        anyhow::bail!(
            "{url} is on a connected Clax instance: send its threads to an agent there, in its page"
        );
    }
    let tid = g.threads[i]["id"].as_str().unwrap_or_default();
    let art = c.get(&format!("/api/artifacts/{}", g.artifact_id))?;
    let agents: Vec<&Value> = art["artifact"]["participants"]["agents"]
        .as_array()
        .map(|a| a.iter().filter(|x| x["live"] == true).collect())
        .unwrap_or_default();
    let to = match &a.to {
        Some(h) => Some(h.clone()),
        None => agents
            .first()
            .and_then(|x| x["handle"].as_str())
            .map(str::to_string),
    };
    let harness = to.as_deref().and_then(|h| {
        agents
            .iter()
            .find(|x| x["handle"] == h)
            .and_then(|x| x["harness"].as_str())
            .map(str::to_string)
    });
    let body = to.as_ref().map(|h| json!({"to": h}));
    let res = c.send(
        Method::POST,
        &format!("/api/artifacts/{}/threads/{tid}/send", g.artifact_id),
        body.as_ref(),
    )?;
    let reference = format!("{}#{}", g.artifact_id, i + 1);
    let out = json!({
        "thread_id": tid,
        "ref": reference,
        "sent": true,
        "to": to,
        "harness": harness,
        "feedback_state": res["thread"]["feedback_state"],
    });
    super::print(cli, out, |j| match j["to"].as_str() {
        Some(h) => format!(
            "sent {reference} to {} ({h})",
            term::snippet(j["harness"].as_str().unwrap_or("agent"), 20)
        ),
        None => format!(
            "sent {reference}; no agent is live on this artifact, so it waits for the next one that publishes or watches it"
        ),
    });
    Ok(())
}

fn name(cli: &crate::Cli, home: &Home, a: &NameArgs) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    if let Some(r) = c.remote() {
        // Remotely, the person's name, as `remote.json` holds it; names are
        // set in the person's browser.
        if a.name.is_some() {
            return Err(crate::client::NotRemote::new(home, "comments name <name>").into());
        }
        let out = json!({"display_name": r.config.person_name, "public_id": r.config.person_id});
        super::print(cli, out, |j| {
            j["display_name"]
                .as_str()
                .map_or_else(|| "no name".to_string(), term::clean_line)
        });
        return Ok(());
    }
    let me = match &a.name {
        Some(n) => c.send(
            Method::PUT,
            "/api/viewers/me",
            Some(&json!({"display_name": n})),
        )?["viewer"]
            .clone(),
        None => owner(&c)?,
    };
    let out = json!({"display_name": me["display_name"], "public_id": me["public_id"]});
    super::print(cli, out, |j| match j["display_name"].as_str() {
        Some(n) => term::clean_line(n),
        None => {
            "no name set: replies show as \"Viewer\" (set one with `clax comments name <name>`)"
                .to_string()
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_whose_agent_lost_access_says_both() {
        let t = serde_json::json!({"feedback_state": {"state": "withheld", "approver": "Sam", "no_access": true}});
        assert_eq!(
            delivery(&t),
            "sent to agent, waiting for Sam's approval; the agent no longer has access"
        );
    }

    #[test]
    fn a_thread_held_for_lost_access_reads_held() {
        let t = serde_json::json!({"feedback_state": {"state": "no_access"}});
        assert_eq!(
            delivery(&t),
            "sent to agent, held: the agent no longer has access"
        );
    }

    /// What the daemon resolves a URL to is the artifact the commands
    /// act on: here, at a remote's routes, or nothing at all.
    #[test]
    fn a_resolved_url_names_a_local_or_remote_artifact() {
        let raw = "https://clax.example/a/k3m9x2a7b8c4";
        assert_eq!(
            Art::resolved(
                raw,
                &json!({"kind": "local", "artifact_id": "k3m9x2a7b8c4"})
            )
            .unwrap(),
            Art::local("k3m9x2a7b8c4")
        );
        let r = Art::resolved(
            raw,
            &json!({"kind": "remote", "remote": "r_1", "origin": "https://clax.example",
                    "name": "acme", "artifact_id": "k3m9x2a7b8c4", "url": raw}),
        )
        .unwrap();
        assert_eq!(r.api, "/api/remotes/r_1/artifacts/k3m9x2a7b8c4");
        assert_eq!(
            r.remote,
            Some((
                raw.to_string(),
                json!({"origin": "https://clax.example", "name": "acme"})
            ))
        );
        let e = Art::resolved(
            raw,
            &json!({"kind": "sign_in_required", "origin": "https://clax.example", "name": "Acme",
                    "request": {"tool": "connect_instance",
                                "command": format!("clax remote request '{raw}'")}}),
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("not connected") && e.contains("clax remote request"),
            "{e}"
        );
        assert!(Art::resolved(raw, &json!({"kind": "page", "url": raw})).is_err());
    }

    #[test]
    fn thread_references_name_a_number_or_an_id() {
        let aid = "k3m9x2a7b8c4";
        let ulid = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
        assert!(matches!(
            thread_ref(&format!("{aid}#3")).unwrap(),
            Some(ThreadRef::Number(a, 3)) if a == aid
        ));
        assert!(matches!(
            thread_ref(&format!("http://localhost:7480/a/{aid}#12")).unwrap(),
            Some(ThreadRef::Number(a, 12)) if a == format!("http://localhost:7480/a/{aid}")
        ));
        assert!(matches!(
            thread_ref(ulid).unwrap(),
            Some(ThreadRef::Id(None, t)) if t == ulid
        ));
        assert!(matches!(
            thread_ref(&format!("{aid}#{ulid}")).unwrap(),
            Some(ThreadRef::Id(Some(a), t)) if a == aid && t == ulid
        ));
        assert!(thread_ref(aid).unwrap().is_none());
        assert!(
            thread_ref(&format!("http://localhost:7480/a/{aid}#top"))
                .unwrap()
                .is_none()
        );
        assert!(thread_ref(&format!("{aid}#0")).is_err());
        assert!(thread_ref("nope#2").is_err());
    }
}
