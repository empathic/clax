//! Hook events: join and end the session the MCP shim registered, and hand
//! pending feedback to the agent from the Stop and prompt-submit hooks.

use crate::input::HookInput;
use crate::output::HookOutput;
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::time::Duration;

/// The pending-feedback request `session_start` makes after joining is
/// abandoned after this long at most, and sooner when less is left before
/// the hook's deadline ([`Daemon::time_left`]).
pub const START_FEEDBACK_TIMEOUT: Duration = Duration::from_secs(1);

/// A request that takes feedback is not made with less than this left
/// before the hook's deadline ([`Daemon::time_left`]): the daemon marks
/// what it hands over delivered before it answers, so a take whose answer
/// comes too late to print loses it. Left undelivered, it waits for the
/// next hook.
pub const MIN_TAKE_TIME: Duration = Duration::from_millis(500);

/// The daemon operations hooks need.
pub trait Daemon {
    /// How long a request may still take for its answer to be printed
    /// before the hook's deadline (the time kept for printing already taken
    /// off); `None` when the hook has no deadline.
    fn time_left(&self) -> Option<Duration> {
        None
    }
    fn browser_url(&self, path: &str) -> String;
    fn get(&self, path: &str) -> anyhow::Result<Value>;
    /// [`Daemon::get`], abandoned with an error after `timeout`.
    fn get_with_timeout(&self, path: &str, timeout: Duration) -> anyhow::Result<Value>;
    fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value>;
    fn patch(&self, path: &str, body: &Value) -> anyhow::Result<Value>;
    /// Feedback text as the agent is handed it. A remote daemon's names
    /// each clip by reference (spec §16): the agent side fetches it and
    /// rewrites the line ([`clax_core::clips::rewrite_text`]). The local
    /// daemon's text is handed over as it is.
    fn agent_text(&self, text: String) -> String {
        text
    }
}

/// Joins the harness's session ID to the session registered for the same
/// harness process (`parent_pid` is the hook's parent; `ancestor_pids`,
/// nearest first, cover a wrapper shell between the hook and the harness),
/// filling `cwd`, recording the harness's transcript file
/// (`transcript_path`, or Grok Build's `transcriptPath`) and `codex_home`
/// when given, and sending the `repo` and `branch` of `cwd` (spec §11, read
/// from its files with no `git` process) that name a session the join
/// makes. A daemon from before a field that refuses it as unknown is asked
/// again without it. Returns the daemon's answer, `{"session": …}`.
///
/// # Errors
/// When the input has no session ID or the daemon request fails.
pub fn join(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    codex_home: Option<&str>,
    daemon: &dyn Daemon,
) -> anyhow::Result<Value> {
    let Some(session_id) = input.session_id.as_deref().filter(|s| !s.is_empty()) else {
        bail!("hook input has no session_id");
    };
    let mut body = json!({
        "harness": harness,
        "parent_pid": parent_pid,
        "harness_session_id": session_id,
    });
    if !ancestor_pids.is_empty() {
        body["ancestor_pids"] = json!(ancestor_pids);
    }
    if let Some(cwd) = &input.cwd {
        body["cwd"] = json!(cwd);
        let (repo, branch) = clax_core::names::repo_and_branch(std::path::Path::new(cwd));
        if let Some(r) = repo {
            body["repo"] = json!(r);
        }
        if let Some(b) = branch {
            body["branch"] = json!(b);
        }
    }
    if let Some(t) = input.transcript_path.as_deref().filter(|t| !t.is_empty()) {
        body["transcript_path"] = json!(t);
    }
    if let Some(h) = codex_home {
        body["codex_home"] = json!(h);
    }
    loop {
        match daemon.post("/api/sessions/join", &body) {
            // A daemon from before one of these fields refuses it: join
            // without it.
            Err(e) => match refused_field(&e) {
                Some(f) if body.as_object_mut().is_some_and(|b| b.remove(f).is_some()) => {}
                _ => return Err(e),
            },
            r => return r,
        }
    }
}

/// The optional join field an older daemon refused as unknown
/// (`invalid_json`), if `e` is such a refusal and not one of a value.
fn refused_field(e: &anyhow::Error) -> Option<&'static str> {
    let msg = e.to_string();
    if !msg.starts_with("invalid_json:") {
        return None;
    }
    ["transcript_path", "repo", "branch"]
        .into_iter()
        .find(|f| msg.contains(&format!("unknown field `{f}`")))
}

/// Joins the harness's session ID to the session registered for the same
/// harness process (`parent_pid` is the hook's parent; `ancestor_pids`, nearest
/// first, cover a wrapper shell between the hook and the harness). The
/// context names the daemon and, when the joined session has pending
/// `prompt_hook` feedback, appends its rendered text. That feedback request is
/// bounded by [`START_FEEDBACK_TIMEOUT`] and by what is left before the
/// hook's deadline, and not made when that is less than [`MIN_TAKE_TIME`];
/// when it fails or times out the context is returned without it. `codex_home`, when given, is recorded for
/// the session so the daemon can run `codex queue` against that Codex home.
pub fn session_start(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    codex_home: Option<&str>,
    daemon: &dyn Daemon,
) -> anyhow::Result<HookOutput> {
    session_start_with(
        harness,
        parent_pid,
        ancestor_pids,
        input,
        codex_home,
        daemon,
        None,
    )
}

/// What the start of a session says about how it is woken: the clax binary
/// a follower runs, and whether the harness was launched with the Clax
/// channel (Claude Code; `None` when that could not be read, or for
/// another harness).
#[derive(Clone, Debug)]
pub struct StartWake {
    pub bin: String,
    pub channel: Option<bool>,
}

/// [`session_start`], with a `Push:` line after the daemon's
/// ([`clax_core::wake::session_line`]) when `wake` is given: whether a
/// comment wakes the idle session, from the channel's launch flag under
/// Claude Code or else the daemon's `push` (read within
/// [`START_FEEDBACK_TIMEOUT`]; no line when it cannot be read).
///
/// # Errors
/// When the input has no session ID or the join request fails.
pub fn session_start_with(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    codex_home: Option<&str>,
    daemon: &dyn Daemon,
    wake: Option<&StartWake>,
) -> anyhow::Result<HookOutput> {
    let joined = join(
        harness,
        parent_pid,
        ancestor_pids,
        input,
        codex_home,
        daemon,
    )?;
    let mut context = format!(
        "Clax daemon at {}; artifacts publish with the `publish` tool.",
        daemon.browser_url("/")
    );
    if let (Some(w), Some(sid)) = (wake, joined["session"]["id"].as_str())
        && let Some(line) = start_push_line(harness, sid, input, daemon, w)
    {
        context.push('\n');
        context.push_str(&line);
    }
    if let Some(sid) = joined["session"]["id"].as_str()
        && let Ok(Some(res)) = take(
            daemon,
            &format!("/api/sessions/{sid}/feedback?tier=prompt_hook"),
            Some(START_FEEDBACK_TIMEOUT),
        )
        && let Some(text) = rendered_text(&res)
    {
        context.push_str("\n\n");
        context.push_str(&daemon.agent_text(text));
    }
    Ok(HookOutput::additional_context("SessionStart", &context))
}

/// The `Push:` line of [`session_start_with`] for session `sid`.
fn start_push_line(
    harness: &str,
    sid: &str,
    input: &HookInput,
    daemon: &dyn Daemon,
    w: &StartWake,
) -> Option<String> {
    let hs = input.session_id.as_deref();
    let push = match (harness, w.channel) {
        ("claude", Some(true)) => json!({"tier": "channel", "available": true}),
        ("claude", None) => {
            return Some(
                "Push: unknown in this session (how Claude Code was launched could not be read); \
the `watch` result's `wake` says whether a comment wakes it while idle."
                    .to_string(),
            );
        }
        _ => daemon
            .get_with_timeout(&format!("/api/sessions/{sid}"), START_FEEDBACK_TIMEOUT)
            .ok()?["push"]
            .clone(),
    };
    Some(clax_core::wake::session_line(&clax_core::wake::block(
        harness, &push, &w.bin, hs,
    )))
}

/// [`session_start`] for a harness that ignores `SessionStart` output
/// (Grok Build): joins and prints nothing. It does not ask for
/// `prompt_hook` feedback: that request marks comments delivered, and
/// they would never reach the agent.
///
/// # Errors
/// When the input has no session ID or the join request fails.
pub fn session_start_quiet(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    daemon: &dyn Daemon,
) -> anyhow::Result<HookOutput> {
    join(harness, parent_pid, ancestor_pids, input, None, daemon)?;
    Ok(HookOutput::none())
}

/// Ends the live session for `(harness, session_id)`, if there is one.
pub fn session_end(
    harness: &str,
    input: &HookInput,
    daemon: &dyn Daemon,
) -> anyhow::Result<HookOutput> {
    let Some(session_id) = input.session_id.as_deref().filter(|s| !s.is_empty()) else {
        bail!("hook input has no session_id");
    };
    let listed = daemon.get("/api/sessions?live=true")?;
    let found = listed["sessions"]
        .as_array()
        .context("sessions listing is not an array")?
        .iter()
        .filter(|s| s["harness"] == harness && s["harness_session_id"].as_str() == Some(session_id))
        .filter_map(|s| s["id"].as_str());
    for id in found {
        daemon.patch(&format!("/api/sessions/{id}"), &json!({"ended": true}))?;
    }
    Ok(HookOutput::none())
}

/// The ID of the live Clax session for `(harness, input.session_id)`, if any.
///
/// # Errors
/// When the input has no `session_id` or the daemon cannot be asked.
pub(crate) fn live_session(
    harness: &str,
    input: &HookInput,
    daemon: &dyn Daemon,
) -> anyhow::Result<Option<String>> {
    let Some(hsid) = input.session_id.as_deref().filter(|s| !s.is_empty()) else {
        bail!("hook input has no session_id");
    };
    let listed = daemon.get("/api/sessions?live=true")?;
    Ok(listed["sessions"]
        .as_array()
        .context("sessions listing is not an array")?
        .iter()
        .find(|s| s["harness"] == harness && s["harness_session_id"].as_str() == Some(hsid))
        .and_then(|s| s["id"].as_str())
        .map(str::to_string))
}

/// `GET path` for a request that takes feedback: made only while at least
/// [`MIN_TAKE_TIME`] is left before the hook's deadline, and abandoned
/// once what is left, or `cap`, runs out, so an answer that arrives is
/// printed. `Ok(None)` when it was not made.
fn take(daemon: &dyn Daemon, path: &str, cap: Option<Duration>) -> anyhow::Result<Option<Value>> {
    let timeout = match (daemon.time_left(), cap) {
        (Some(left), _) if left < MIN_TAKE_TIME => return Ok(None),
        (Some(left), Some(c)) => Some(left.min(c)),
        (Some(left), None) => Some(left),
        (None, c) => c,
    };
    match timeout {
        Some(t) => daemon.get_with_timeout(path, t),
        None => daemon.get(path),
    }
    .map(Some)
}

/// The rendered feedback the daemon hands over for `query`, as the agent
/// is handed it ([`Daemon::agent_text`]: a remote daemon's clips fetched
/// and their lines rewritten; otherwise the daemon's text as it is):
/// `Some(None)` when none is pending, `None` when too little time was left
/// to ask ([`take`]).
fn feedback_text(
    daemon: &dyn Daemon,
    sid: &str,
    query: &str,
) -> anyhow::Result<Option<Option<String>>> {
    let Some(res) = take(
        daemon,
        &format!("/api/sessions/{sid}/feedback?{query}"),
        None,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(rendered_text(&res).map(|t| daemon.agent_text(t))))
}

/// The non-empty `text` of a feedback response.
fn rendered_text(res: &Value) -> Option<String> {
    res["text"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// Tier 2. Blocks the stop with the pending feedback as the reason; allows it
/// (prints nothing) when nothing is pending. Only watches with replies armed
/// count. While `stop_hook_active` is set, only never-delivered rows can block,
/// so a stop is blocked at most once per new comment. When it allows the
/// stop, it ends the session's working records (the turn is over). For Grok
/// Build, a Stop whose reason is not `end_turn` allows the stop without asking
/// the daemon.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn stop(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    stop_with(harness, input, daemon, None)
}

/// What the Stop hook remembers of one harness session between stops:
/// the watch armings (`<artifact ID>@<created_at>`) it has blocked a stop
/// about, and whether a comment could wake the session at a stop since.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Nudged {
    #[serde(default)]
    pub armings: Vec<String>,
    #[serde(default)]
    pub woken: bool,
}

/// Where the Stop hook keeps [`Nudged`]. A ledger that cannot be written
/// never blocks a stop, so a stop is never blocked twice for one reason.
pub trait Ledger {
    /// What was kept, or the default when nothing was (or it is unreadable).
    fn load(&self) -> Nudged;
    fn save(&self, n: &Nudged) -> anyhow::Result<()>;
}

/// A [`Ledger`] in a JSON file.
pub struct FileLedger(pub std::path::PathBuf);

impl Ledger for FileLedger {
    fn load(&self) -> Nudged {
        std::fs::read(&self.0)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(&self, n: &Nudged) -> anyhow::Result<()> {
        if let Some(dir) = self.0.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.0.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(n)?)?;
        std::fs::rename(&tmp, &self.0)?;
        Ok(())
    }
}

/// How the Stop hook checks that a watching session can be woken: the
/// clax binary a follower runs, and where it remembers what it said.
pub struct StopWake<'a> {
    pub bin: String,
    pub ledger: &'a dyn Ledger,
    /// Whether Claude Code was launched with the Clax channel (`None`:
    /// unknown, or another harness). A channel-launched session relies on
    /// the channel: a lapse of the shim's poll is no reason to start a
    /// follower.
    pub channel: Option<bool>,
}

/// [`stop`], which, when `wake` is given and nothing is pending, also
/// blocks the stop once when the session watches with replies armed but
/// nothing can wake it while idle (no channel, follower, queue or
/// extension): its reason is the `wake` block's `do`
/// ([`clax_core::wake::block`]) with the follower's commands. It blocks at
/// most once per watch arming, and once more after a session that could be
/// woken at an earlier stop no longer can (its follower stopped). Any
/// failure of that check allows the stop.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn stop_with(
    harness: &str,
    input: &HookInput,
    daemon: &dyn Daemon,
    wake: Option<&StopWake<'_>>,
) -> anyhow::Result<HookOutput> {
    // Grok Build fires Stop again at session end (`channel_closed`,
    // `shutdown`) and ignores that run's output; only `end_turn` (or an
    // input without a reason) is the end of a turn.
    if harness == "grok" && input.stop_reason().is_some_and(|r| r != "end_turn") {
        return Ok(HookOutput::none());
    }
    let Some(sid) = live_session(harness, input, daemon)? else {
        return Ok(HookOutput::none());
    };
    let resends = !input.stop_hook_active.unwrap_or(false);
    match feedback_text(daemon, &sid, &format!("tier=stop_hook&resends={resends}"))? {
        // Too late to ask: the stop goes ahead, the turn's working records
        // and any pending feedback left for the next hook.
        None => Ok(HookOutput::none()),
        Some(Some(text)) => Ok(HookOutput::block(&text)),
        Some(None) => {
            if let Some(w) = wake
                && let Ok(Some(reason)) = unwoken(harness, &sid, daemon, w)
            {
                return Ok(HookOutput::block(&reason));
            }
            // The turn really ends: its working records end with it.
            daemon.post(&format!("/api/sessions/{sid}/working/end"), &json!({}))?;
            Ok(HookOutput::none())
        }
    }
}

/// The reason [`stop_with`] blocks a stop of session `sid` for, when it
/// does: its armed watches, unwoken, that it has not said so about.
fn unwoken(
    harness: &str,
    sid: &str,
    daemon: &dyn Daemon,
    w: &StopWake<'_>,
) -> anyhow::Result<Option<String>> {
    // Pi has no Stop hook, and its extension wakes it.
    if !matches!(harness, "claude" | "codex" | "grok") {
        return Ok(None);
    }
    let info = daemon.get(&format!("/api/sessions/{sid}"))?;
    let push = &info["push"];
    let mut kept = w.ledger.load();
    if push["available"] == true {
        if !kept.woken {
            kept.woken = true;
            w.ledger.save(&kept)?;
        }
        return Ok(None);
    }
    if harness == "claude" && w.channel == Some(true) {
        return Ok(None);
    }
    let listed = daemon.get(&format!("/api/sessions/{sid}/watches"))?;
    let armings: Vec<String> = listed["watches"]
        .as_array()
        .context("watches listing is not an array")?
        .iter()
        .filter(|w| w["replies_armed"] == true)
        .filter_map(|w| {
            Some(format!(
                "{}@{}",
                w["artifact_id"].as_str()?,
                w["created_at"].as_str()?
            ))
        })
        .collect();
    if armings.is_empty() {
        return Ok(None);
    }
    let fresh = armings.iter().any(|a| !kept.armings.contains(a));
    let lost = kept.woken;
    if !fresh && !lost {
        return Ok(None);
    }
    // Saved before blocking: a ledger that cannot be written never blocks.
    w.ledger.save(&Nudged {
        armings: armings.clone(),
        woken: false,
    })?;
    let hs = info["session"]["harness_session_id"].as_str();
    let wake = clax_core::wake::block(harness, push, &w.bin, hs);
    let n = armings.len();
    let mut text = format!(
        "[clax] {}You watch {} for comments sent to you. {}",
        if lost {
            "The follower that woke this session has stopped. "
        } else {
            ""
        },
        if n == 1 {
            "1 artifact or page".to_string()
        } else {
            format!("{n} artifacts or pages")
        },
        wake["do"].as_str().unwrap_or_default()
    );
    for k in ["follow_command", "follow_once_command"] {
        if let Some(c) = wake[k].as_str() {
            text.push_str(&format!("\n{k}: {c}"));
        }
    }
    text.push_str("\nThis is said once; end your turn when done.");
    Ok(Some(text))
}

/// `PostToolUse`: renews the session's working records. Prints nothing.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn tool(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    if let Some(sid) = live_session(harness, input, daemon)? {
        daemon.post(&format!("/api/sessions/{sid}/working/renew"), &json!({}))?;
    }
    Ok(HookOutput::none())
}

/// `PostToolUse` on a Clax MCP tool (Claude Code): reports the harness's
/// call ID for the call to the live session's
/// `POST /api/sessions/<sid>/tool-call-ids` as `{tool_use_id, tool_name,
/// args_sha256}`, the hash computed from `tool_input` as passed (an absent
/// or null one is `{}`) by the canonical argument hash the shim uses (spec
/// 2026-10-06-toolpath-audit-design §6.7, §12.2). Prints nothing; with no
/// live session, reports nothing.
///
/// # Errors
/// When the input has no `session_id` or `tool_use_id`, its `tool_name` is
/// not a Clax tool, or a daemon request fails.
pub fn tool_call_id(
    harness: &str,
    input: &HookInput,
    daemon: &dyn Daemon,
) -> anyhow::Result<HookOutput> {
    let field = |k: &str| {
        input
            .rest
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let Some(tool_use_id) = field("tool_use_id") else {
        bail!("hook input has no tool_use_id");
    };
    let tool_name = field("tool_name").unwrap_or_default();
    if clax_core::audit::bare_clax_tool(tool_name).is_none() {
        bail!("hook input's tool_name is not a Clax tool");
    }
    // Missing arguments, absent or null, are `{}` (§12.2), as for the shim.
    let args_sha256 = match input.rest.get("tool_input") {
        Some(args) if !args.is_null() => clax_core::audit::args::args_sha256(args),
        _ => clax_core::audit::args::object_sha256(&serde_json::Map::new()),
    };
    if let Some(sid) = live_session(harness, input, daemon)? {
        daemon.post(
            &format!("/api/sessions/{sid}/tool-call-ids"),
            &json!({"tool_use_id": tool_use_id, "tool_name": tool_name, "args_sha256": args_sha256}),
        )?;
    }
    Ok(HookOutput::none())
}

/// Tier 3. Reports the person's prompt (it is terminal activity, spec
/// 2026-10-16-ask-where-attention-is-design §1) and adds pending feedback,
/// and where to ask, to the prompt as additional context, in one request.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn prompt(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    let Some(sid) = live_session(harness, input, daemon)? else {
        return Ok(HookOutput::none());
    };
    let text = feedback_text(daemon, &sid, "tier=prompt_hook&prompt=true")?;
    Ok(match text {
        Some(Some(text)) => HookOutput::additional_context("UserPromptSubmit", &text),
        Some(None) | None => HookOutput::none(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<(String, String, Value)>>,
    }
    impl Daemon for Fake {
        fn browser_url(&self, path: &str) -> String {
            format!("http://h:1{path}")
        }
        fn get(&self, path: &str) -> anyhow::Result<Value> {
            self.calls
                .borrow_mut()
                .push(("GET".into(), path.into(), Value::Null));
            Ok(json!({"sessions": [
                {"id": "a", "harness": "claude", "harness_session_id": "s1"},
                {"id": "b", "harness": "codex", "harness_session_id": "s1"},
                {"id": "c", "harness": "claude", "harness_session_id": "s2"},
                {"id": "g", "harness": "grok", "harness_session_id": "s1"},
            ]}))
        }
        fn get_with_timeout(&self, path: &str, _: Duration) -> anyhow::Result<Value> {
            self.get(path)
        }
        fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
            self.calls
                .borrow_mut()
                .push(("POST".into(), path.into(), body.clone()));
            Ok(json!({}))
        }
        fn patch(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
            self.calls
                .borrow_mut()
                .push(("PATCH".into(), path.into(), body.clone()));
            Ok(json!({}))
        }
    }

    fn input(id: &str) -> HookInput {
        HookInput::parse(&format!(r#"{{"session_id":"{id}","cwd":"/w"}}"#))
    }

    #[test]
    fn start_joins_and_reports_url() {
        let d = Fake::default();
        let out = session_start("claude", 42, &[7, 1], &input("s1"), None, &d).unwrap();
        let calls = d.calls.borrow();
        assert_eq!(calls[0].1, "/api/sessions/join");
        assert_eq!(
            calls[0].2,
            json!({"harness": "claude", "parent_pid": 42, "ancestor_pids": [7, 1], "harness_session_id": "s1", "cwd": "/w"})
        );
        let text = out.value().unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("http://h:1/"), "{text}");
    }

    #[test]
    fn a_quiet_start_joins_and_prints_nothing() {
        let d = Fake::default();
        let out = session_start_quiet("grok", 42, &[7], &input("s1"), &d).unwrap();
        assert_eq!(out, HookOutput::none());
        let calls = d.calls.borrow();
        assert_eq!(calls.len(), 1, "no prompt_hook request: {calls:?}");
        assert_eq!(calls[0].1, "/api/sessions/join");
        assert_eq!(calls[0].2["harness"], "grok");
    }

    #[test]
    fn join_stores_transcript_path_for_claude_codex_and_grok() {
        for (harness, stdin) in [
            (
                "claude",
                r#"{"session_id":"s1","cwd":"/w","transcript_path":"/t/claude.jsonl"}"#,
            ),
            (
                "codex",
                r#"{"session_id":"s1","cwd":"/w","transcript_path":"/t/codex.jsonl"}"#,
            ),
            (
                "grok",
                r#"{"hookEventName":"SessionStart","sessionId":"s1","cwd":"/w","transcriptPath":"/t/grok.jsonl"}"#,
            ),
        ] {
            let d = Fake::default();
            join(harness, 42, &[], &HookInput::parse(stdin), None, &d).unwrap();
            let calls = d.calls.borrow();
            assert_eq!(calls[0].1, "/api/sessions/join");
            assert_eq!(
                calls[0].2["transcript_path"],
                format!("/t/{harness}.jsonl"),
                "{harness}"
            );
        }
        // Without one, or with an empty one, the join names none.
        for stdin in [
            r#"{"session_id":"s1"}"#,
            r#"{"session_id":"s1","transcript_path":""}"#,
        ] {
            let d = Fake::default();
            join("claude", 42, &[], &HookInput::parse(stdin), None, &d).unwrap();
            assert!(d.calls.borrow()[0].2.get("transcript_path").is_none());
        }
    }

    /// A daemon that refuses the join body as an older one does when it
    /// carries `transcript_path` (or with `invalid` as a newer one refuses
    /// a bad value).
    struct OldDaemon {
        refusal: &'static str,
        bodies: RefCell<Vec<Value>>,
    }
    impl Daemon for OldDaemon {
        fn browser_url(&self, path: &str) -> String {
            format!("http://h:1{path}")
        }
        fn get(&self, _: &str) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
        fn get_with_timeout(&self, path: &str, _: Duration) -> anyhow::Result<Value> {
            self.get(path)
        }
        fn post(&self, _: &str, body: &Value) -> anyhow::Result<Value> {
            self.bodies.borrow_mut().push(body.clone());
            if body.get("transcript_path").is_some() {
                bail!("{}", self.refusal);
            }
            Ok(json!({"session": {"id": "S"}}))
        }
        fn patch(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
    }

    #[test]
    fn a_join_retries_without_the_transcript_path_an_older_daemon_refuses() {
        let input =
            HookInput::parse(r#"{"session_id":"s1","cwd":"/w","transcript_path":"/t/a.jsonl"}"#);
        let d = OldDaemon {
            refusal: "invalid_json: Failed to deserialize the JSON body into the target type: transcript_path: unknown field `transcript_path`, expected one of `harness`, `parent_pid`, `harness_session_id`, `cwd`, `ancestor_pids`, `codex_home` at line 1 column 120",
            bodies: RefCell::new(vec![]),
        };
        assert_eq!(
            join("claude", 42, &[], &input, None, &d).unwrap()["session"]["id"],
            "S"
        );
        let bodies = d.bodies.borrow();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["transcript_path"], "/t/a.jsonl");
        assert!(bodies[1].get("transcript_path").is_none());
        assert_eq!(bodies[1]["cwd"], "/w");
        // A refusal of the value itself is not retried.
        let d = OldDaemon {
            refusal: "invalid_session: transcript_path must be at most 4096 bytes, without control characters",
            bodies: RefCell::new(vec![]),
        };
        assert!(join("claude", 42, &[], &input, None, &d).is_err());
        assert_eq!(d.bodies.borrow().len(), 1);
    }

    /// Spec §11: the join names the repository and branch of the hook's
    /// `cwd`, read from its files, so the session is named after them.
    #[test]
    fn a_join_sends_the_repository_and_branch_of_its_cwd() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("web");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/fix/chart-axis\n").unwrap();
        let input =
            HookInput::parse(&json!({"session_id": "s1", "cwd": repo.join("src")}).to_string());
        let f = Fake::default();
        join("claude", 42, &[], &input, None, &f).unwrap();
        let body = f.calls.borrow()[0].2.clone();
        assert_eq!(
            (&body["repo"], &body["branch"]),
            (&json!("web"), &json!("fix/chart-axis")),
            "{body}"
        );
        // An older daemon that refuses the fields is asked again without
        // them.
        let old = OldDaemon {
            refusal: "invalid_json: Failed to deserialize the JSON body into the target type: unknown field `repo`, expected one of `harness`",
            bodies: RefCell::new(vec![]),
        };
        struct NoRepo(OldDaemon);
        impl Daemon for NoRepo {
            fn browser_url(&self, path: &str) -> String {
                self.0.browser_url(path)
            }
            fn get(&self, path: &str) -> anyhow::Result<Value> {
                self.0.get(path)
            }
            fn get_with_timeout(&self, path: &str, t: Duration) -> anyhow::Result<Value> {
                self.0.get_with_timeout(path, t)
            }
            fn post(&self, _: &str, body: &Value) -> anyhow::Result<Value> {
                self.0.bodies.borrow_mut().push(body.clone());
                if body.get("repo").is_some() {
                    bail!("{}", self.0.refusal);
                }
                if body.get("branch").is_some() {
                    bail!("{}", self.0.refusal.replace("`repo`", "`branch`"));
                }
                Ok(json!({"session": {"id": "S"}}))
            }
            fn patch(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
                self.0.patch(path, body)
            }
        }
        let d = NoRepo(old);
        assert_eq!(
            join("claude", 42, &[], &input, None, &d).unwrap()["session"]["id"],
            "S"
        );
        let bodies = d.0.bodies.borrow();
        assert_eq!(bodies.len(), 3);
        assert!(bodies[2].get("repo").is_none() && bodies[2].get("branch").is_none());
    }

    /// Spec §16: the feedback a hook hands over passes through the
    /// daemon's agent-side text, which rewrites a remote daemon's clips.
    #[test]
    fn hooks_hand_over_feedback_through_the_agent_side_text() {
        struct Rewriting(FeedbackFake);
        impl Daemon for Rewriting {
            fn browser_url(&self, path: &str) -> String {
                self.0.browser_url(path)
            }
            fn get(&self, path: &str) -> anyhow::Result<Value> {
                self.0.get(path)
            }
            fn get_with_timeout(&self, path: &str, t: Duration) -> anyhow::Result<Value> {
                self.0.get_with_timeout(path, t)
            }
            fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
                self.0.post(path, body)
            }
            fn patch(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
                self.0.patch(path, body)
            }
            fn agent_text(&self, text: String) -> String {
                text.replace("Clip: clax-clip:a/b", "Clip: /cache/a-b.png")
            }
        }
        let d = Rewriting(fake(Some("[clax] x\nClip: clax-clip:a/b\nMia: hi")));
        for out in [
            stop("claude", &input("s1"), &d).unwrap(),
            prompt("claude", &input("s1"), &d).unwrap(),
            session_start("claude", 42, &[], &input("s1"), None, &d).unwrap(),
        ] {
            let line = out.to_line().unwrap();
            assert!(line.contains("Clip: /cache/a-b.png"), "{line}");
            assert!(!line.contains("clax-clip:"), "{line}");
        }
    }

    #[test]
    fn start_without_session_id_errors() {
        let d = Fake::default();
        assert!(session_start("claude", 1, &[], &HookInput::default(), None, &d).is_err());
        assert!(d.calls.borrow().is_empty());
    }

    #[test]
    fn start_sends_codex_home_when_known() {
        let d = Fake::default();
        session_start("codex", 42, &[], &input("s1"), Some("/cx"), &d).unwrap();
        assert_eq!(d.calls.borrow()[0].2["codex_home"], "/cx");
    }

    #[test]
    fn end_ends_only_the_matching_session() {
        let d = Fake::default();
        let out = session_end("claude", &input("s1"), &d).unwrap();
        assert_eq!(out, HookOutput::none());
        let calls = d.calls.borrow();
        let patches: Vec<_> = calls.iter().filter(|c| c.0 == "PATCH").collect();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].1, "/api/sessions/a");
    }

    struct FeedbackFake {
        text: Option<&'static str>,
        seen: RefCell<Vec<String>>,
        posts: RefCell<Vec<String>>,
        /// What [`Daemon::time_left`] answers.
        left: Option<Duration>,
        /// Each bounded request's path and timeout.
        bounded: RefCell<Vec<(String, Duration)>>,
    }
    impl Daemon for FeedbackFake {
        fn time_left(&self) -> Option<Duration> {
            self.left
        }
        fn browser_url(&self, path: &str) -> String {
            format!("http://h:1{path}")
        }
        fn get(&self, path: &str) -> anyhow::Result<Value> {
            self.seen.borrow_mut().push(path.to_string());
            if path.starts_with("/api/sessions?") {
                return Ok(json!({"sessions": [
                    {"id": "S", "harness": "claude", "harness_session_id": "s1"},
                    {"id": "G", "harness": "grok", "harness_session_id": "s1"},
                ]}));
            }
            Ok(
                json!({"feedback": if self.text.is_some() { json!([{}]) } else { json!([]) }, "text": self.text, "waited_s": 0}),
            )
        }
        fn get_with_timeout(&self, path: &str, t: Duration) -> anyhow::Result<Value> {
            self.bounded.borrow_mut().push((path.to_string(), t));
            self.get(path)
        }
        fn post(&self, path: &str, _: &Value) -> anyhow::Result<Value> {
            self.posts.borrow_mut().push(path.to_string());
            Ok(json!({"session": {"id": "S"}}))
        }
        fn patch(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
    }
    fn fake(text: Option<&'static str>) -> FeedbackFake {
        FeedbackFake {
            text,
            seen: RefCell::new(vec![]),
            posts: RefCell::new(vec![]),
            left: None,
            bounded: RefCell::new(vec![]),
        }
    }

    fn takes(d: &FeedbackFake) -> Vec<String> {
        d.seen
            .borrow()
            .iter()
            .filter(|p| p.contains("/feedback?"))
            .cloned()
            .collect()
    }

    #[test]
    fn no_hook_takes_feedback_with_too_little_time_left_to_print_it() {
        let short = MIN_TAKE_TIME - Duration::from_millis(1);
        let late = || FeedbackFake {
            left: Some(short),
            ..fake(Some("PENDING"))
        };
        let d = late();
        assert_eq!(
            prompt("claude", &input("s1"), &d).unwrap(),
            HookOutput::none()
        );
        assert!(takes(&d).is_empty(), "{:?}", d.seen.borrow());
        let d = late();
        assert_eq!(
            stop("claude", &input("s1"), &d).unwrap(),
            HookOutput::none()
        );
        assert!(takes(&d).is_empty(), "{:?}", d.seen.borrow());
        assert!(
            d.posts.borrow().is_empty(),
            "a stop that could not ask ends no working records"
        );
        let d = late();
        let out = session_start("claude", 1, &[], &input("s1"), None, &d).unwrap();
        assert!(takes(&d).is_empty(), "{:?}", d.seen.borrow());
        let text = out.value().unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            text.contains("http://h:1/") && !text.contains("PENDING"),
            "{text}"
        );
    }

    #[test]
    fn a_hook_takes_feedback_bounded_by_the_time_it_has_left() {
        let left = Duration::from_millis(700);
        let d = FeedbackFake {
            left: Some(left),
            ..fake(Some("PENDING"))
        };
        let out = prompt("claude", &input("s1"), &d).unwrap();
        assert!(out.to_line().unwrap().contains("PENDING"));
        assert_eq!(
            *d.bounded.borrow(),
            [(
                "/api/sessions/S/feedback?tier=prompt_hook&prompt=true".to_string(),
                left
            )]
        );
        // At session start, the shorter of its own bound and what is left.
        let d = FeedbackFake {
            left: Some(Duration::from_secs(3)),
            ..fake(Some("PENDING"))
        };
        session_start("claude", 1, &[], &input("s1"), None, &d).unwrap();
        assert_eq!(
            *d.bounded.borrow(),
            [(
                "/api/sessions/S/feedback?tier=prompt_hook".to_string(),
                START_FEEDBACK_TIMEOUT
            )]
        );
    }

    #[test]
    fn stop_ends_the_turn_only_when_it_allows_the_stop() {
        let d = fake(Some("[clax] 1 comment sent to you:\nX"));
        stop("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap();
        assert!(
            d.posts.borrow().is_empty(),
            "a blocked stop continues the turn"
        );
        let d = fake(None);
        assert_eq!(
            stop(
                "claude",
                &HookInput::parse(r#"{"session_id":"s1","stop_hook_active":true}"#),
                &d
            )
            .unwrap(),
            HookOutput::none()
        );
        assert_eq!(*d.posts.borrow(), ["/api/sessions/S/working/end"]);
    }

    #[test]
    fn tool_renews_and_prints_nothing() {
        let d = fake(None);
        assert_eq!(
            tool("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap(),
            HookOutput::none()
        );
        assert_eq!(*d.posts.borrow(), ["/api/sessions/S/working/renew"]);
        let d = fake(None);
        assert_eq!(
            tool("claude", &HookInput::parse(r#"{"session_id":"other"}"#), &d).unwrap(),
            HookOutput::none()
        );
        assert!(
            d.posts.borrow().is_empty(),
            "no live session: nothing to renew"
        );
    }

    #[test]
    fn stop_blocks_with_the_payload_and_excludes_resends_when_active() {
        let d = fake(Some("[clax] 1 comment sent to you:\nX"));
        let out = stop(
            "claude",
            &HookInput::parse(r#"{"session_id":"s1","stop_hook_active":false}"#),
            &d,
        )
        .unwrap();
        assert_eq!(out, HookOutput::block("[clax] 1 comment sent to you:\nX"));
        assert!(
            d.seen
                .borrow()
                .iter()
                .any(|p| p == "/api/sessions/S/feedback?tier=stop_hook&resends=true")
        );
        let d = fake(None);
        let out = stop(
            "claude",
            &HookInput::parse(r#"{"session_id":"s1","stop_hook_active":true}"#),
            &d,
        )
        .unwrap();
        assert_eq!(out, HookOutput::none());
        assert!(
            d.seen
                .borrow()
                .iter()
                .any(|p| p == "/api/sessions/S/feedback?tier=stop_hook&resends=false")
        );
    }

    #[test]
    fn unknown_sessions_and_other_harnesses_print_nothing() {
        let d = fake(Some("x"));
        assert_eq!(
            stop("codex", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap(),
            HookOutput::none()
        );
        assert_eq!(
            prompt("claude", &HookInput::parse(r#"{"session_id":"nope"}"#), &d).unwrap(),
            HookOutput::none()
        );
        assert!(
            stop("claude", &HookInput::default(), &d).is_err(),
            "no session_id"
        );
    }

    #[test]
    fn a_grok_stop_acts_only_at_the_end_of_a_turn() {
        let d = fake(Some("[clax] hi"));
        let grok = |reason: &str| {
            HookInput::parse(&format!(r#"{{"sessionId":"s1","reason":"{reason}"}}"#))
        };
        assert_eq!(
            stop("grok", &grok("channel_closed"), &d).unwrap(),
            HookOutput::none()
        );
        assert_eq!(
            stop("grok", &grok("shutdown"), &d).unwrap(),
            HookOutput::none()
        );
        assert!(
            d.seen.borrow().is_empty(),
            "no daemon request at session end"
        );
        assert_eq!(
            stop("grok", &grok("end_turn"), &d).unwrap(),
            HookOutput::block("[clax] hi")
        );
        let no_reason = HookInput::parse(r#"{"sessionId":"s1"}"#);
        assert_eq!(
            stop("grok", &no_reason, &d).unwrap(),
            HookOutput::block("[clax] hi")
        );
    }

    #[test]
    fn a_reason_does_not_filter_other_harnesses() {
        let d = fake(Some("[clax] hi"));
        let i = HookInput::parse(r#"{"session_id":"s1","reason":"anything"}"#);
        assert_eq!(
            stop("claude", &i, &d).unwrap(),
            HookOutput::block("[clax] hi")
        );
    }

    #[test]
    fn grok_stop_hook_active_turns_resends_off() {
        let d = fake(Some("[clax] hi"));
        let i = HookInput::parse(r#"{"sessionId":"s1","reason":"end_turn","stopHookActive":true}"#);
        stop("grok", &i, &d).unwrap();
        assert!(
            d.seen
                .borrow()
                .iter()
                .any(|p| p == "/api/sessions/G/feedback?tier=stop_hook&resends=false"),
            "{:?}",
            d.seen.borrow()
        );
    }

    #[test]
    fn prompt_adds_context_and_reports_the_prompt() {
        let d = fake(Some("P"));
        assert_eq!(
            prompt("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap(),
            HookOutput::additional_context("UserPromptSubmit", "P")
        );
        assert!(
            d.seen
                .borrow()
                .iter()
                .any(|p| p == "/api/sessions/S/feedback?tier=prompt_hook&prompt=true"),
            "{:?}",
            d.seen.borrow()
        );
    }

    #[test]
    fn session_start_appends_pending_feedback() {
        let d = fake(Some("PENDING"));
        let out = session_start("claude", 1, &[], &input("s1"), None, &d).unwrap();
        let text = out.value().unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.ends_with("\n\nPENDING"), "{text}");
    }

    /// A daemon whose feedback route answers only after `FEEDBACK_DELAY`; like
    /// the real client, a bounded request gives up after its timeout.
    struct SlowFeedback;
    const FEEDBACK_DELAY: Duration = Duration::from_secs(5);
    impl Daemon for SlowFeedback {
        fn browser_url(&self, path: &str) -> String {
            format!("http://h:1{path}")
        }
        fn get(&self, path: &str) -> anyhow::Result<Value> {
            self.get_with_timeout(path, Duration::MAX)
        }
        fn get_with_timeout(&self, _: &str, timeout: Duration) -> anyhow::Result<Value> {
            std::thread::sleep(timeout.min(FEEDBACK_DELAY));
            if timeout < FEEDBACK_DELAY {
                bail!("timed out");
            }
            Ok(json!({"feedback": [{}], "text": "LATE", "waited_s": 0}))
        }
        fn post(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
            Ok(json!({"session": {"id": "S"}}))
        }
        fn patch(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
    }

    #[test]
    fn session_start_returns_the_join_context_when_feedback_is_slow() {
        let started = std::time::Instant::now();
        let out = session_start("claude", 1, &[], &input("s1"), None, &SlowFeedback).unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "{:?}",
            started.elapsed()
        );
        let text = out.value().unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("http://h:1/"), "{text}");
        assert!(!text.contains("LATE"), "{text}");
    }

    /// A daemon with no pending feedback, a session `S` of `harness` whose
    /// push is `available`, and the watches `watches`.
    struct WakeFake {
        harness: &'static str,
        available: RefCell<bool>,
        watches: RefCell<Value>,
        posts: RefCell<Vec<String>>,
    }
    impl Daemon for WakeFake {
        fn browser_url(&self, path: &str) -> String {
            format!("http://h:1{path}")
        }
        fn get(&self, path: &str) -> anyhow::Result<Value> {
            if path.starts_with("/api/sessions?") {
                return Ok(json!({"sessions": [
                    {"id": "S", "harness": self.harness, "harness_session_id": "s1"},
                ]}));
            }
            if path == "/api/sessions/S" {
                let available = *self.available.borrow();
                return Ok(json!({
                    "session": {"id": "S", "harness": self.harness, "harness_session_id": "s1"},
                    "push": {"tier": if available { json!("notice") } else { Value::Null },
                             "available": available, "reason": "no follower"},
                }));
            }
            if path == "/api/sessions/S/watches" {
                return Ok(json!({"watches": self.watches.borrow().clone()}));
            }
            Ok(json!({"feedback": [], "text": null, "waited_s": 0}))
        }
        fn get_with_timeout(&self, path: &str, _: Duration) -> anyhow::Result<Value> {
            self.get(path)
        }
        fn post(&self, path: &str, _: &Value) -> anyhow::Result<Value> {
            self.posts.borrow_mut().push(path.to_string());
            Ok(json!({"session": {"id": "S"}}))
        }
        fn patch(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
    }
    fn wake_fake(harness: &'static str, available: bool, watches: Value) -> WakeFake {
        WakeFake {
            harness,
            available: RefCell::new(available),
            watches: RefCell::new(watches),
            posts: RefCell::new(vec![]),
        }
    }
    #[derive(Default)]
    struct MemLedger {
        kept: RefCell<Nudged>,
        broken: bool,
    }
    impl Ledger for MemLedger {
        fn load(&self) -> Nudged {
            self.kept.borrow().clone()
        }
        fn save(&self, n: &Nudged) -> anyhow::Result<()> {
            if self.broken {
                bail!("read-only");
            }
            *self.kept.borrow_mut() = n.clone();
            Ok(())
        }
    }
    fn armed(aid: &str, at: &str) -> Value {
        json!({"session_id": "S", "artifact_id": aid, "replies_armed": true, "created_at": at})
    }
    fn stop_once(d: &WakeFake, l: &MemLedger, active: bool) -> HookOutput {
        let w = StopWake {
            bin: "/b/clax".into(),
            ledger: l,
            channel: None,
        };
        let i = HookInput::parse(&format!(
            r#"{{"session_id":"s1","stop_hook_active":{active}}}"#
        ));
        stop_with(d.harness, &i, d, Some(&w)).unwrap()
    }
    fn reason(o: &HookOutput) -> String {
        let v = o.value().expect("blocked");
        assert_eq!(v["decision"], "block", "{v}");
        v["reason"].as_str().unwrap().to_string()
    }

    #[test]
    fn an_unwoken_watch_blocks_the_stop_once_per_arming() {
        let d = wake_fake("claude", false, json!([armed("a1", "t1")]));
        let l = MemLedger::default();
        let r = reason(&stop_once(&d, &l, false));
        assert!(r.starts_with("[clax] You watch 1 artifact or page for comments sent to you. A comment will not wake this session while it is idle."), "{r}");
        assert!(r.contains("Monitor"), "{r}");
        assert!(
            r.contains(
                "follow_command: '/b/clax' feedback follow --agent claude --harness-session 's1'"
            ),
            "{r}"
        );
        assert!(
            r.contains("follow_once_command: '/b/clax' feedback follow --once"),
            "{r}"
        );
        assert!(
            d.posts.borrow().is_empty(),
            "a blocked stop continues the turn"
        );
        // The stop the block caused, and every later one, are allowed.
        assert_eq!(stop_once(&d, &l, true), HookOutput::none());
        assert_eq!(stop_once(&d, &l, false), HookOutput::none());
        assert_eq!(d.posts.borrow().len(), 2, "each allowed stop ends the turn");
        // A new arming (another watch, or a watch started again) says it again.
        d.watches
            .replace(json!([armed("a1", "t1"), armed("a2", "t2")]));
        assert!(reason(&stop_once(&d, &l, false)).contains("You watch 2 artifacts or pages"));
        assert_eq!(stop_once(&d, &l, true), HookOutput::none());
    }

    #[test]
    fn a_live_follower_or_push_never_blocks_and_a_lost_one_blocks_once() {
        let d = wake_fake("claude", true, json!([armed("a1", "t1")]));
        let l = MemLedger::default();
        assert_eq!(stop_once(&d, &l, false), HookOutput::none());
        assert!(l.kept.borrow().woken);
        assert_eq!(stop_once(&d, &l, false), HookOutput::none());
        // The follower stopped (a Monitor expired): said once.
        d.available.replace(false);
        let r = reason(&stop_once(&d, &l, false));
        assert!(
            r.starts_with("[clax] The follower that woke this session has stopped."),
            "{r}"
        );
        assert_eq!(stop_once(&d, &l, false), HookOutput::none());
        // Pi's extension wakes it: never asked.
        let d = wake_fake("pi", false, json!([armed("a1", "t1")]));
        assert_eq!(
            stop_once(&d, &MemLedger::default(), false),
            HookOutput::none()
        );
    }

    #[test]
    fn watches_without_replies_none_and_an_unwritable_ledger_allow_the_stop() {
        let off = json!([{"artifact_id": "a1", "replies_armed": false, "created_at": "t1"}]);
        for watches in [off, json!([])] {
            let d = wake_fake("claude", false, watches);
            assert_eq!(
                stop_once(&d, &MemLedger::default(), false),
                HookOutput::none()
            );
        }
        let d = wake_fake("claude", false, json!([armed("a1", "t1")]));
        let broken = MemLedger {
            broken: true,
            ..Default::default()
        };
        assert_eq!(stop_once(&d, &broken, false), HookOutput::none());
        assert_eq!(stop_once(&d, &broken, false), HookOutput::none());
        // Without a wake check, the stop is as before.
        let i = HookInput::parse(r#"{"session_id":"s1"}"#);
        assert_eq!(stop("claude", &i, &d).unwrap(), HookOutput::none());
    }

    #[test]
    fn a_channel_launched_session_is_not_told_to_start_a_follower_when_the_shim_lapses() {
        // The shim's channel poll lapsed (a daemon restart): push reads
        // unavailable, but Claude Code was launched with the channel, which
        // the watch result already said to rely on.
        let d = wake_fake("claude", false, json!([armed("a1", "t1")]));
        let l = MemLedger::default();
        let w = StopWake {
            bin: "/b/clax".into(),
            ledger: &l,
            channel: Some(true),
        };
        let i = HookInput::parse(r#"{"session_id":"s1","stop_hook_active":false}"#);
        assert_eq!(
            stop_with("claude", &i, &d, Some(&w)).unwrap(),
            HookOutput::none()
        );
        assert_eq!(
            *l.kept.borrow(),
            Nudged::default(),
            "nothing said, nothing kept"
        );
    }

    #[test]
    fn a_codex_session_nothing_wakes_is_told_to_tell_the_person() {
        let d = wake_fake("codex", false, json!([armed("a1", "t1")]));
        let l = MemLedger::default();
        let r = reason(&stop_once(&d, &l, false));
        assert!(r.contains("Tell the person"), "{r}");
        assert!(!r.contains("follow_command"), "{r}");
        assert_eq!(stop_once(&d, &l, true), HookOutput::none());
    }

    #[test]
    fn a_file_ledger_keeps_what_it_said() {
        let dir = tempfile::tempdir().unwrap();
        let l = FileLedger(dir.path().join("run/wake/claude-s1.json"));
        assert_eq!(l.load(), Nudged::default());
        let n = Nudged {
            armings: vec!["a1@t1".into()],
            woken: true,
        };
        l.save(&n).unwrap();
        assert_eq!(FileLedger(l.0.clone()).load(), n);
    }

    #[test]
    fn session_start_says_whether_push_is_available() {
        let line = |harness: &'static str, available: bool, channel: Option<bool>| {
            let d = wake_fake(harness, available, json!([]));
            let w = StartWake {
                bin: "/b/clax".into(),
                channel,
            };
            let out =
                session_start_with(harness, 1, &[], &input("s1"), None, &d, Some(&w)).unwrap();
            out.value().unwrap()["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let t = line("claude", false, Some(false));
        assert!(t.contains("\nPush: unavailable in this session;"), "{t}");
        assert!(
            t.contains("feedback follow --agent claude --harness-session 's1'"),
            "{t}"
        );
        let t = line("claude", false, Some(true));
        assert!(
            t.contains("\nPush: probably available in this session (via channel)"),
            "{t}"
        );
        assert!(line("claude", false, None).contains("\nPush: unknown"));
        let t = line("codex", false, None);
        assert!(
            t.contains("Push: unavailable") && t.contains("Tell the person"),
            "{t}"
        );
        assert!(line("codex", true, None).contains("Push: available"));
    }
}
