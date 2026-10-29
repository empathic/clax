//! `clax queue`: a report queue's reports, for scripts and fixers, on the
//! triage routes the `queue_*` tools use (`docs/contract.md`, "Report
//! queue routes"). Listing and reading run the `queue_list` and
//! `queue_get` tools, so their JSON is the tools'; the clip, the
//! diagnostics, the writes and the watch make the tools' client calls
//! themselves, in the agent session the command names, so no result
//! carries (and so consumes) the session's waiting feedback.

use super::feedback::{self, Pace, Ready, SessionArgs, Target};
use super::tools::{self, InSession};
use crate::client::Client;
use crate::term;
use anyhow::{Context, anyhow, bail};
use clax_core::Home;
use clax_mcp::client::ClientError;
use clax_mcp::tools::{QueueListArgs, QueueReportArgs, ReportStateArg};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::time::Duration;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// List the queue's reports, oldest first (without a queue, your
    /// queues).
    ///
    /// Reports that are open or claimed by default; --state names one
    /// state, or `all`. Prints `ID  STATE  RELEASE  AGE  NOTE` columns;
    /// --json gives `{queue, reports}`, each report as `queue_list` gives
    /// it (its context replaced by `context_summary`).
    Ls(LsArgs),
    /// Show one report whole: its record, context, conversation, fixes,
    /// and its latest capture's diagnostics and clip.
    ///
    /// --json gives the `queue_get` tool's result (`{report, diagnostics,
    /// clip, note}`).
    Show(ReportArgs),
    /// Save a capture's clip PNG: the latest by default, or --capture N
    /// (0 the filing's, N the Nth reopen's).
    Clip(ClipArgs),
    /// Print a capture's diagnostics: the latest that has any by default,
    /// or --capture N (0 the filing's, N the Nth reopen's).
    ///
    /// Prints `AT  LEVEL  KIND  TEXT` lines; --json gives `{queue,
    /// report_id, capture, lines}`.
    Diagnostics(CaptureArgs),
    /// Claim a report for 10 minutes, as the agent session named.
    Claim(ActArgs),
    /// Extend your claim on a report to 10 minutes from now.
    Renew(ActArgs),
    /// Release your claim on a report.
    Release(ActArgs),
    /// Reply to a report you claimed (`-` reads the reply from stdin).
    Reply(ReplyArgs),
    /// Propose a fix for a report you claimed; it then waits in
    /// fix_proposed for the reporter's verdict, and your claim ends.
    Fix(FixArgs),
    /// Resolve a report you claimed, with a reason.
    Resolve(ResolveArgs),
    /// Watch the queue as the agent session named, and print one JSON line
    /// per report filed into it or reopened, until interrupted.
    ///
    /// Each line is `{event, queue, report_id, title, url}` (`event`
    /// `filed`, `reopened`, or `comment` for a comment on a report sent to
    /// the session), with `reopened_by` for a reopen by a triage person.
    /// The watch lasts until the session ends or `queue_watch` stops it.
    /// Once the queue is watched (and a report filed from then on reaches
    /// it), it says so once on stderr: `clax queue follow: watching queue
    /// <QID> as session <SID>`.
    /// Never starts a daemon; follows the session across daemon restarts,
    /// and exits 0 once the session has ended. A notice of anything else
    /// sent to the session is passed over: it still arrives through its
    /// delivery tiers.
    Follow(FollowArgs),
}

#[derive(clap::Args, Clone, Debug)]
pub struct QueueArg {
    /// The queue's ID (default: CLAX_QUEUE).
    #[arg(long, env = "CLAX_QUEUE")]
    pub queue: Option<String>,
}

#[derive(clap::Args)]
pub struct LsArgs {
    #[command(flatten)]
    pub queue: QueueArg,
    /// Only reports in this state (default `active`: open and claimed).
    #[arg(long, value_enum)]
    pub state: Option<StateArg>,
}

#[derive(clap::Args)]
pub struct ReportArgs {
    /// The report's ID.
    pub report: String,
    #[command(flatten)]
    pub queue: QueueArg,
}

#[derive(clap::Args)]
pub struct CaptureArgs {
    /// The report's ID.
    pub report: String,
    #[command(flatten)]
    pub queue: QueueArg,
    /// The capture: 0 the filing's, N the Nth reopen's (default: the
    /// latest).
    #[arg(long)]
    pub capture: Option<u64>,
}

#[derive(clap::Args)]
pub struct ClipArgs {
    #[command(flatten)]
    pub at: CaptureArgs,
    /// Where to write the PNG (default `<report>.png`; `-` is stdout).
    #[arg(short, long)]
    pub output: Option<String>,
}

/// The report a write names and the session it acts in.
#[derive(clap::Args)]
pub struct ActArgs {
    /// The report's ID.
    pub report: String,
    #[command(flatten)]
    pub queue: QueueArg,
    #[command(flatten)]
    pub who: SessionArgs,
}

#[derive(clap::Args)]
pub struct ReplyArgs {
    #[command(flatten)]
    pub act: ActArgs,
    /// The reply, as Markdown; `-` reads it from stdin.
    pub body: String,
}

#[derive(clap::Args)]
pub struct FixArgs {
    #[command(flatten)]
    pub act: ActArgs,
    /// What the fix changes, as plain text (at most 4 KiB); posted as your
    /// comment for the reporter.
    #[arg(long)]
    pub summary: String,
    /// A link to the change, such as its pull request.
    #[arg(long)]
    pub url: Option<String>,
    /// The commit that makes the fix.
    #[arg(long)]
    pub commit: Option<String>,
    /// The release that carries it.
    #[arg(long)]
    pub release: Option<String>,
    /// Where the reporter can check the fix.
    #[arg(long)]
    pub verify_url: Option<String>,
    /// When the fix can be checked, such as `release:0.6.1`.
    #[arg(long)]
    pub verify_after: Option<String>,
}

#[derive(clap::Args)]
pub struct ResolveArgs {
    #[command(flatten)]
    pub act: ActArgs,
    /// Why it is resolved (default `other`).
    #[arg(long, value_enum)]
    pub reason: Option<ReasonArg>,
}

#[derive(clap::Args)]
pub struct FollowArgs {
    /// The queue's ID (default: --queue, else CLAX_QUEUE).
    #[arg(value_name = "QUEUE")]
    pub queue_id: Option<String>,
    #[command(flatten)]
    pub queue: QueueArg,
    #[command(flatten)]
    pub who: SessionArgs,
    /// Seconds each daemon long-poll waits.
    #[arg(long, hide = true, default_value_t = 50)]
    pub poll_secs: u64,
    /// Seconds a followed harness session may have no live Clax session
    /// before the command exits.
    #[arg(long, hide = true, default_value_t = 60)]
    pub grace_secs: u64,
    /// Exit 0 after the first poll that printed at least one line, for a
    /// harness that wakes only when a background command exits.
    #[arg(long)]
    pub once: bool,
}

/// A report's state as `ls --state` takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum StateArg {
    Open,
    Claimed,
    #[value(name = "fix_proposed")]
    FixProposed,
    Verified,
    Resolved,
    Active,
    All,
}

impl From<StateArg> for ReportStateArg {
    fn from(s: StateArg) -> ReportStateArg {
        match s {
            StateArg::Open => ReportStateArg::Open,
            StateArg::Claimed => ReportStateArg::Claimed,
            StateArg::FixProposed => ReportStateArg::FixProposed,
            StateArg::Verified => ReportStateArg::Verified,
            StateArg::Resolved => ReportStateArg::Resolved,
            StateArg::Active => ReportStateArg::Active,
            StateArg::All => ReportStateArg::All,
        }
    }
}

/// Why a report is resolved, as `resolve --reason` takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ReasonArg {
    Fixed,
    Duplicate,
    Wontfix,
    #[value(name = "cannot_reproduce")]
    CannotReproduce,
    Other,
}

impl ReasonArg {
    fn wire(self) -> &'static str {
        match self {
            ReasonArg::Fixed => "fixed",
            ReasonArg::Duplicate => "duplicate",
            ReasonArg::Wontfix => "wontfix",
            ReasonArg::CannotReproduce => "cannot_reproduce",
            ReasonArg::Other => "other",
        }
    }
}

pub fn run(cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    if let Cmd::Follow(a) = cmd {
        return follow(home, a);
    }
    let c = Client::connect(home, cli.port_for(home)?)?;
    match cmd {
        Cmd::Ls(a) => ls(cli, home, &c, a),
        Cmd::Show(a) => show(cli, home, &c, a),
        Cmd::Clip(a) => clip(cli, &c, a),
        Cmd::Diagnostics(a) => diagnostics(cli, &c, a),
        Cmd::Claim(a) => act(cli, &c, a, "claim", json!({})),
        Cmd::Renew(a) => act(cli, &c, a, "renew", json!({})),
        Cmd::Release(a) => act(cli, &c, a, "release", json!({})),
        Cmd::Reply(a) => {
            let body = text_or_stdin(&a.body)?;
            if body.trim().is_empty() {
                bail!("invalid_args: the reply must not be empty");
            }
            act(cli, &c, &a.act, "comments", json!({"body": body}))
        }
        Cmd::Fix(a) => {
            if a.summary.trim().is_empty() {
                bail!("invalid_args: --summary must not be empty");
            }
            let mut body = json!({"summary": a.summary});
            for (k, v) in [
                ("url", &a.url),
                ("commit", &a.commit),
                ("release", &a.release),
                ("verify_url", &a.verify_url),
                ("verify_after", &a.verify_after),
            ] {
                if let Some(v) = v {
                    body[k] = json!(v);
                }
            }
            act(cli, &c, &a.act, "fix", body)
        }
        Cmd::Resolve(a) => {
            let body = match a.reason {
                Some(r) => json!({"reason": r.wire()}),
                None => json!({}),
            };
            act(cli, &c, &a.act, "resolve", body)
        }
        Cmd::Follow(_) => unreachable!("handled above"),
    }
}

/// The queue `q` names, or `CLAX_QUEUE`'s; an error saying how to name one
/// when neither does, or when it is no queue ID.
fn queue_id(q: &QueueArg) -> anyhow::Result<String> {
    let Some(id) = q.queue.as_deref().filter(|s| !s.is_empty()) else {
        bail!(
            "no queue: pass --queue <ID> or set CLAX_QUEUE (`clax queue ls` without either lists your queues)"
        );
    };
    check_id(id, "queue")?;
    Ok(id.to_string())
}

fn check_id(id: &str, what: &str) -> anyhow::Result<()> {
    if clax_core::is_ulid(id) {
        Ok(())
    } else {
        bail!(
            "invalid_args: '{}' is not a {what} ID",
            term::clean_line(id)
        )
    }
}

/// `raw`, or stdin when it is `-`.
fn text_or_stdin(raw: &str) -> anyhow::Result<String> {
    if raw == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        Ok(raw.to_string())
    }
}

/// `v` pretty-printed, as every `--json` output is.
fn print_json(v: &Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

/// A tool's result without its `feedback`, which is empty: these
/// commands run the tools in no session.
fn without_feedback(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut()
        && o.get("feedback")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    {
        o.remove("feedback");
    }
    v
}

fn ls(cli: &crate::Cli, home: &Home, c: &Client, a: &LsArgs) -> anyhow::Result<()> {
    if a.queue.queue.as_deref().is_none_or(str::is_empty) {
        if a.state.is_some() {
            bail!("--state lists one queue's reports: pass --queue <ID> or set CLAX_QUEUE");
        }
        let v = tools::call_on(c, home, |t| async move {
            t.queue_list(Parameters(QueueListArgs::default())).await
        })?;
        let v = without_feedback(v);
        if cli.json {
            return print_json(&v);
        }
        let queues = v["queues"].as_array().cloned().unwrap_or_default();
        if queues.is_empty() {
            println!("You are a member of no queue.");
            return Ok(());
        }
        let rows: Vec<Vec<String>> = queues
            .iter()
            .map(|q| {
                vec![
                    cell(&q["id"]),
                    cell(&q["role"]),
                    term::snippet(q["name"].as_str().unwrap_or(""), 60),
                ]
            })
            .collect();
        print_table(&["ID", "ROLE", "NAME"], &rows);
        return Ok(());
    }
    let qid = queue_id(&a.queue)?;
    let state: Option<ReportStateArg> = a.state.map(Into::into);
    let mut reports = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let args = QueueListArgs {
            queue: Some(qid.clone()),
            state,
            cursor: cursor.clone(),
        };
        let v = tools::call_on(
            c,
            home,
            |t| async move { t.queue_list(Parameters(args)).await },
        )?;
        reports.extend(v["reports"].as_array().cloned().unwrap_or_default());
        match v["next"].as_str() {
            Some(n) if Some(n) != cursor.as_deref() => cursor = Some(n.to_string()),
            _ => break,
        }
    }
    if cli.json {
        return print_json(&json!({"queue": qid, "reports": reports}));
    }
    if reports.is_empty() {
        println!("No reports.");
        return Ok(());
    }
    let now = chrono::Utc::now();
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|r| {
            vec![
                cell(&r["id"]),
                cell(&r["state"]),
                release_of(&r["context_summary"]["release"]),
                term::age(r["created_at"].as_str().unwrap_or(""), now),
                term::snippet(r["comments"][0]["body"].as_str().unwrap_or(""), 60),
            ]
        })
        .collect();
    print_table(&["ID", "STATE", "RELEASE", "AGE", "NOTE"], &rows);
    Ok(())
}

/// A string field of a daemon answer for a table cell, cleaned; `-` when
/// absent.
fn cell(v: &Value) -> String {
    match v.as_str() {
        Some(s) if !s.is_empty() => term::clean_line(s),
        _ => "-".into(),
    }
}

/// A context's `release` as one word: its version, else its ID, else its
/// commit's first 12 characters; `-` without one.
fn release_of(r: &Value) -> String {
    let pick = r["version"]
        .as_str()
        .or_else(|| r["id"].as_str())
        .map(str::to_string)
        .or_else(|| r["commit"].as_str().map(|c| c.chars().take(12).collect()));
    match pick {
        Some(s) if !s.is_empty() => term::snippet(&s, 24),
        _ => "-".into(),
    }
}

/// Prints `rows` under `header` in columns two spaces apart, the last
/// column unpadded.
fn print_table(header: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let last = cells.len() - 1;
        let mut out = String::new();
        for (i, c) in cells.into_iter().enumerate() {
            out.push_str(c);
            if i < last {
                let pad = widths[i] + 2 - c.chars().count();
                out.push_str(&" ".repeat(pad));
            }
        }
        println!("{}", out.trim_end());
    };
    line(header.to_vec());
    for r in rows {
        line(r.iter().map(String::as_str).collect());
    }
}

fn show(cli: &crate::Cli, home: &Home, c: &Client, a: &ReportArgs) -> anyhow::Result<()> {
    let qid = queue_id(&a.queue)?;
    check_id(&a.report, "report")?;
    let args = QueueReportArgs {
        queue: qid,
        report_id: a.report.clone(),
    };
    let v = without_feedback(tools::call_on(c, home, |t| async move {
        t.queue_get(Parameters(args)).await
    })?);
    if cli.json {
        return print_json(&v);
    }
    let r = &v["report"];
    let now = chrono::Utc::now();
    println!(
        "{}  {}",
        term::snippet(r["artifact"]["title"].as_str().unwrap_or(""), 80),
        cell(&r["artifact"]["url"])
    );
    let mut state = format!("report {}  {}", cell(&r["id"]), cell(&r["state"]));
    if let Some(reason) = r["resolve_reason"].as_str() {
        state.push_str(&format!(" ({})", term::clean_line(reason)));
    }
    if let Some(holder) = r["claim"]["holder_name"].as_str() {
        state.push_str(&format!(
            "  claimed by {} until {}",
            term::clean_line(holder),
            cell(&r["claim"]["expires_at"])
        ));
    }
    println!("{state}");
    println!(
        "filed {} by {}",
        term::ago(r["created_at"].as_str().unwrap_or(""), now),
        cell(&r["reporter_id"])
    );
    let ctx = &r["context"];
    if ctx.is_object() {
        let env = &ctx["environment"];
        let mut parts = vec![format!("release {}", release_of(&ctx["release"]))];
        if let Some(u) = env["url"].as_str() {
            parts.push(term::clean_line(u));
        }
        if let (Some(w), Some(h)) = (env["viewport"]["w"].as_f64(), env["viewport"]["h"].as_f64()) {
            parts.push(format!("{w}x{h}"));
        }
        if let Some(m) = ctx["capture"]["mode"].as_str() {
            parts.push(format!("{} capture", term::clean_line(m)));
        }
        println!("context: {}", parts.join("  "));
        if let Some(flags) = ctx["flags"].as_object().filter(|f| !f.is_empty()) {
            let f: Vec<String> = flags
                .iter()
                .map(|(k, v)| term::clean_line(&format!("{k}={v}")))
                .collect();
            println!("flags: {}", f.join(" "));
        }
    }
    println!();
    for cm in r["comments"].as_array().into_iter().flatten() {
        let mut head = format!(
            "{} ({}, {})",
            term::clean_line(cm["author_name"].as_str().unwrap_or("")),
            cell(&cm["author_kind"]),
            term::ago(cm["created_at"].as_str().unwrap_or(""), now)
        );
        if let Some(n) = cm["capture"].as_u64() {
            head.push_str(&format!(" [reopen, capture {n}]"));
        }
        println!("{head}");
        for l in term::clean_text(cm["body"].as_str().unwrap_or("")).lines() {
            println!("  {l}");
        }
    }
    let fixes = r["fixes"].as_array().cloned().unwrap_or_default();
    if !fixes.is_empty() {
        println!();
        println!("fixes:");
        for f in &fixes {
            let mut line = format!(
                "  #{} {}: {}",
                f["n"],
                cell(&f["outcome"]),
                term::snippet(f["summary"].as_str().unwrap_or(""), 80)
            );
            if let Some(u) = f["url"].as_str() {
                line.push_str(&format!("  {}", term::clean_line(u)));
            }
            println!("{line}");
        }
    }
    println!();
    let d = &v["diagnostics"];
    match d["capture"].as_u64() {
        Some(n) => println!(
            "diagnostics: {} lines, capture {n} (clax queue diagnostics {})",
            d["lines"].as_array().map_or(0, Vec::len),
            cell(&r["id"])
        ),
        None => println!("diagnostics: none"),
    }
    if let Some(note) = d["note"].as_str() {
        println!("  {}", term::clean_line(note));
    }
    match v["clip"]["capture"].as_u64() {
        Some(n) => println!("clip: capture {n} (clax queue clip {})", cell(&r["id"])),
        None => println!("clip: none"),
    }
    println!();
    println!("{}", cell(&v["note"]));
    Ok(())
}

/// Report `tid` of queue `qid`, through the tools' client.
fn report(c: &Client, qid: &str, tid: &str) -> anyhow::Result<Value> {
    let (q, t) = (qid.to_string(), tid.to_string());
    let v = tools::daemon(c, None, |d| async move { d.queue_report(&q, &t).await })?
        .map_err(tools::client_error)?;
    Ok(v["report"].clone())
}

/// The capture of `report` to read: `asked` when given, else the latest
/// for which `has` holds of the reopen's comment (or, for the filing,
/// of the report); `None` when none does.
fn latest_capture(report: &Value, has: impl Fn(&Value) -> bool) -> Option<u64> {
    let reopen = report["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| has(c))
        .filter_map(|c| c["capture"].as_u64())
        .max();
    reopen.or_else(|| has(report).then_some(0))
}

/// The route below the report's of capture `n`'s clip of `report`:
/// `/clip` for the filing, `/comments/<cid>/clip` for a reopen; `None`
/// when that capture has no clip.
fn clip_tail(report: &Value, n: u64) -> Option<String> {
    if n == 0 {
        return (report["has_clip"] == true).then(|| "/clip".into());
    }
    report["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["capture"].as_u64() == Some(n) && c["has_clip"] == true)
        .and_then(|c| c["id"].as_str())
        .map(|cid| format!("/comments/{cid}/clip"))
}

fn clip(cli: &crate::Cli, c: &Client, a: &ClipArgs) -> anyhow::Result<()> {
    let qid = queue_id(&a.at.queue)?;
    let tid = &a.at.report;
    check_id(tid, "report")?;
    let r = report(c, &qid, tid)?;
    let n = match a.at.capture {
        Some(n) => n,
        None => latest_capture(&r, |v| v["has_clip"] == true)
            .ok_or_else(|| anyhow!("no_clip: report {tid} has no clip"))?,
    };
    let tail = clip_tail(&r, n)
        .ok_or_else(|| anyhow!("no_clip: capture {n} of report {tid} has no clip"))?;
    let path = format!("/api/queues/{qid}/reports/{tid}{tail}");
    let png = tools::daemon(c, None, |d| async move { d.png_at(&path).await })?
        .map_err(tools::client_error)?;
    let out = a.output.clone().unwrap_or_else(|| format!("{tid}.png"));
    if out == "-" {
        let mut stdout = std::io::stdout();
        stdout.write_all(&png)?;
        stdout.flush()?;
        return Ok(());
    }
    std::fs::write(&out, &png).with_context(|| format!("writing {out}"))?;
    if cli.json {
        return print_json(&json!({
            "queue": qid, "report_id": tid, "capture": n, "path": out, "bytes": png.len()
        }));
    }
    println!("wrote {out} (capture {n}, {} bytes)", png.len());
    Ok(())
}

fn diagnostics(cli: &crate::Cli, c: &Client, a: &CaptureArgs) -> anyhow::Result<()> {
    let qid = queue_id(&a.queue)?;
    let tid = &a.report;
    check_id(tid, "report")?;
    let n = match a.capture {
        Some(n) => Some(n),
        None => latest_capture(&report(c, &qid, tid)?, |v| {
            v["diagnostics_lines"].as_u64().unwrap_or(0) > 0
        }),
    };
    let lines = match n {
        Some(n) => {
            let (q, t) = (qid.clone(), tid.clone());
            let v = tools::daemon(
                c,
                None,
                |d| async move { d.queue_diagnostics(&q, &t, n).await },
            )?
            .map_err(tools::client_error)?;
            v["lines"].as_array().cloned().unwrap_or_default()
        }
        None => Vec::new(),
    };
    if cli.json {
        return print_json(&json!({
            "queue": qid, "report_id": tid, "capture": n, "lines": lines
        }));
    }
    for l in &lines {
        println!(
            "{}  {}  {}  {}",
            cell(&l["at"]),
            cell(&l["level"]),
            cell(&l["kind"]),
            term::clean_line(l["text"].as_str().unwrap_or(""))
        );
    }
    Ok(())
}

/// The session `who` names (or `GROK_SESSION_ID`, inside Grok Build), as
/// the daemon `c` knows it; `None` when nothing names one.
fn session(c: &Client, who: &SessionArgs) -> anyhow::Result<Option<InSession>> {
    let Some(target) = feedback::target(who, |k| std::env::var(k).ok()) else {
        return Ok(None);
    };
    let cred = feedback::credential(c, &target)?;
    let id = match &target {
        Target::Session(s) => s.clone(),
        Target::Harness(h, id) => {
            let found = if c.remote().is_some() && cred.is_none() {
                None
            } else {
                feedback::live_session(c, h, id, cred.as_ref())?
            };
            found.ok_or_else(|| {
                anyhow!(
                    "no_session: no live Clax session for {h} session {}",
                    term::clean_line(id)
                )
            })?
        }
    };
    Ok(Some(InSession::new(id, cred.as_ref())))
}

/// `action` (`claim`, `renew`, `release`, `comments`, `fix`, `resolve`)
/// with `body` on the report `a` names, in the session it names; prints
/// the route's answer (`--json`) or a line saying what changed.
fn act(cli: &crate::Cli, c: &Client, a: &ActArgs, action: &str, body: Value) -> anyhow::Result<()> {
    let qid = queue_id(&a.queue)?;
    check_id(&a.report, "report")?;
    let s = session(c, &a.who)?;
    let (q, t, act) = (qid.clone(), a.report.clone(), action.to_string());
    let v = tools::daemon(c, s.as_ref(), |d| async move {
        d.queue_report_action(&q, &t, &act, &body).await
    })?
    .map_err(|e| {
        let no_session =
            matches!(&e, ClientError::Api { error, .. } if error["code"] == "no_session");
        let e = tools::client_error(e);
        if no_session {
            e.context(
                "this needs an agent session: pass --session, or --agent with --harness-session",
            )
        } else {
            e
        }
    })?;
    if cli.json {
        return print_json(&v);
    }
    let r = &v["report"];
    let tid = term::clean_line(&a.report);
    match action {
        "claim" => println!(
            "claimed {tid} until {} (fence {})",
            cell(&v["expires_at"]),
            v["fence"]
        ),
        "renew" => println!("renewed {tid} until {}", cell(&v["expires_at"])),
        "release" => println!("released {tid}: {}", cell(&r["state"])),
        "comments" => println!("replied on {tid} (comment {})", cell(&v["comment"]["id"])),
        "fix" => println!("proposed a fix for {tid}: {}", cell(&r["state"])),
        _ => println!(
            "resolved {tid}: {} ({})",
            cell(&r["state"]),
            cell(&r["resolve_reason"])
        ),
    }
    Ok(())
}

/// The JSON line `queue follow` prints for notice `n` when it is of queue
/// `qid`: `{event, queue, report_id, title, url}` and `reopened_by` when
/// present; `None` for a notice of anything else.
fn event_line(n: &Value, qid: &str) -> Option<Value> {
    if n["queue_id"].as_str() != Some(qid) {
        return None;
    }
    let mut e = json!({
        "event": n["event"].as_str().unwrap_or("comment"),
        "queue": qid,
        "report_id": n["thread_id"],
        "title": n["title"],
        "url": n["url"],
    });
    if let Some(by) = n["reopened_by"].as_str() {
        e["reopened_by"] = json!(by);
    }
    Some(e)
}

fn follow(home: &Home, a: &FollowArgs) -> anyhow::Result<()> {
    let qid = match a.queue_id.as_deref().filter(|s| !s.is_empty()) {
        Some(q) => {
            check_id(q, "queue")?;
            q.to_string()
        }
        None => queue_id(&a.queue)?,
    };
    let Some(target) = feedback::target(&a.who, |k| std::env::var(k).ok()) else {
        eprintln!(
            "clax queue follow: no session: pass --session, or --agent with --harness-session, or run it inside Grok Build (GROK_SESSION_ID)"
        );
        std::process::exit(2);
    };
    let pace = Pace {
        poll: Duration::from_secs(a.poll_secs.max(1)),
        grace: Duration::from_secs(a.grace_secs),
    };
    let mut stdout = std::io::stdout();
    let mut told = false;
    let followed = feedback::follow(
        home,
        &target,
        &pace,
        |c, sid, cred| {
            let q = qid.clone();
            let s = InSession::new(sid.to_string(), cred);
            match tools::daemon(
                c,
                Some(&s),
                |d| async move { d.queue_watch(&q, true).await },
            )? {
                Ok(()) => {
                    // Once: from here a report filed or reopened reaches
                    // this follower.
                    if !told {
                        eprintln!(
                            "clax queue follow: watching queue {} as session {}",
                            term::clean_line(&qid),
                            term::clean_line(sid)
                        );
                        told = true;
                    }
                    Ok(Ready::Yes)
                }
                Err(ClientError::Api { error, .. })
                    if feedback::gone_code(error["code"].as_str().unwrap_or("")) =>
                {
                    Ok(Ready::Gone)
                }
                Err(e @ ClientError::Api { .. }) => Err(tools::client_error(e)),
                Err(_) => Ok(Ready::Retry),
            }
        },
        |v| {
            let mut printed = 0;
            for n in v["notices"].as_array().into_iter().flatten() {
                if let Some(e) = event_line(n, &qid) {
                    writeln!(stdout, "{}", serde_json::to_string(&e)?)?;
                    printed += 1;
                }
            }
            stdout.flush()?;
            Ok(a.once && printed > 0)
        },
    );
    if let Err(e) = followed {
        eprintln!("clax queue follow: {e:#}");
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_capture_prefers_the_newest_reopen_then_the_filing() {
        let r = json!({
            "has_clip": true,
            "diagnostics_lines": 3,
            "comments": [
                {"id": "c0"},
                {"id": "c1", "capture": 1, "has_clip": true},
                {"id": "c2", "capture": 2, "diagnostics_lines": 4},
            ]
        });
        assert_eq!(latest_capture(&r, |v| v["has_clip"] == true), Some(1));
        assert_eq!(
            latest_capture(&r, |v| v["diagnostics_lines"].as_u64().unwrap_or(0) > 0),
            Some(2)
        );
        assert_eq!(clip_tail(&r, 0).as_deref(), Some("/clip"));
        assert_eq!(clip_tail(&r, 1).as_deref(), Some("/comments/c1/clip"));
        assert_eq!(clip_tail(&r, 2), None);
        let bare = json!({"has_clip": false, "comments": []});
        assert_eq!(latest_capture(&bare, |v| v["has_clip"] == true), None);
    }

    #[test]
    fn follow_prints_only_its_queues_notices() {
        let q = "01JQUEUE00000000000000000Q";
        let wake = json!({"queue_id": q, "event": "reopened", "thread_id": "t1",
            "title": "Invoice", "url": "https://x/a", "reopened_by": "Ada for its reporter"});
        assert_eq!(
            event_line(&wake, q).unwrap(),
            json!({"event": "reopened", "queue": q, "report_id": "t1", "title": "Invoice",
                "url": "https://x/a", "reopened_by": "Ada for its reporter"})
        );
        let comment = json!({"queue_id": q, "thread_id": "t1", "title": "I", "url": "u"});
        assert_eq!(event_line(&comment, q).unwrap()["event"], "comment");
        assert!(event_line(&json!({"thread_id": "t2"}), q).is_none());
        assert!(event_line(&json!({"queue_id": "other", "event": "filed"}), q).is_none());
    }

    #[test]
    fn release_of_names_the_version_then_the_id_then_the_commit() {
        assert_eq!(release_of(&json!({"version": "0.6.1", "id": "r"})), "0.6.1");
        assert_eq!(release_of(&json!({"id": "r9"})), "r9");
        assert_eq!(
            release_of(&json!({"commit": "0123456789abcdef"})),
            "0123456789ab"
        );
        assert_eq!(release_of(&Value::Null), "-");
    }
}
