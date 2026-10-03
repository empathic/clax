//! `clax feedback follow`: prints one line per comment sent to an agent
//! session, for a harness that turns a command's output lines into wake-ups
//! (Grok Build's `monitor` tool). Each line is a notice that points at the
//! comment; it delivers nothing (see `Store::take_notices`). The command
//! never starts a daemon: it waits for one, follows the session across
//! daemon restarts, and exits 0 once the session has ended.

use crate::client::Client;
use clax_core::Home;
use serde_json::Value;
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Print one line per comment sent to an agent session, as it arrives.
    ///
    /// Each line names the artifact and thread and says to call
    /// comments_read; it never contains the comment. Following does not
    /// deliver: the comment still reaches the agent through its next clax
    /// tool call, its Stop hook, or wait_for_feedback. The session is
    /// --session (a Clax session ID), or --agent with --harness-session (the
    /// harness's own ID), or, inside Grok Build, GROK_SESSION_ID. Exits 0
    /// once the session has ended. With --once, it exits after the first
    /// comment, for a harness that wakes when a background command exits
    /// (Claude Code).
    Follow(FollowArgs),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum FollowAgent {
    Claude,
    Codex,
    Grok,
}

impl FollowAgent {
    fn harness(self) -> &'static str {
        match self {
            FollowAgent::Claude => "claude",
            FollowAgent::Codex => "codex",
            FollowAgent::Grok => "grok",
        }
    }
}

#[derive(clap::Args)]
pub struct FollowArgs {
    /// The Clax session to follow (`status` reports it as session.id).
    #[arg(long, conflicts_with_all = ["agent", "harness_session"])]
    pub session: Option<String>,
    /// The harness whose session --harness-session names.
    #[arg(long, value_enum, requires = "harness_session")]
    pub agent: Option<FollowAgent>,
    /// The harness's own session ID (`status` reports it as
    /// session.harness_session_id).
    #[arg(long, requires = "agent")]
    pub harness_session: Option<String>,
    /// Seconds each daemon long-poll waits.
    #[arg(long, hide = true, default_value_t = 50)]
    pub poll_secs: u64,
    /// Seconds a followed harness session may have no live Clax session
    /// before the command exits.
    #[arg(long, hide = true, default_value_t = 60)]
    pub grace_secs: u64,
    /// Exit 0 after the first poll that printed at least one line, so a
    /// harness that wakes when a background command exits (Claude Code) is
    /// woken by the first comment. The agent starts it again after handling
    /// the comment.
    #[arg(long)]
    pub once: bool,
}

/// What to follow.
#[derive(Debug, PartialEq)]
enum Target {
    /// A Clax session ID: following ends when it ends.
    Session(String),
    /// A harness session, resolved to its live Clax session on each poll,
    /// so a session re-registered after a daemon restart is followed too.
    Harness(&'static str, String),
}

fn target(a: &FollowArgs, env: impl Fn(&str) -> Option<String>) -> Option<Target> {
    if let Some(s) = &a.session {
        return Some(Target::Session(s.clone()));
    }
    if let (Some(agent), Some(id)) = (a.agent, &a.harness_session) {
        return Some(Target::Harness(agent.harness(), id.clone()));
    }
    env("GROK_SESSION_ID")
        .filter(|v| !v.is_empty())
        .map(|id| Target::Harness("grok", id))
}

/// The live Clax session of `(harness, id)`, if any.
fn live_session(c: &Client, harness: &str, id: &str) -> anyhow::Result<Option<String>> {
    let v = c.get("/api/sessions?live=true")?;
    Ok(v["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["harness"] == harness && s["harness_session_id"].as_str() == Some(id))
        .and_then(|s| s["id"].as_str())
        .map(str::to_string))
}

/// Whether a daemon error says the session is gone.
fn session_gone(e: &anyhow::Error) -> bool {
    let m = e.to_string();
    m.starts_with("unknown_session:") || m.starts_with("not_found:")
}

/// Sleeps `backoff`, then doubles it up to 30 s.
fn back_off(backoff: &mut Duration) {
    std::thread::sleep(*backoff);
    *backoff = (*backoff * 2).min(Duration::from_secs(30));
}

pub fn run(_cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    let Cmd::Follow(a) = cmd;
    let Some(target) = target(a, |k| std::env::var(k).ok()) else {
        eprintln!(
            "clax feedback follow: no session: pass --session, or --agent with --harness-session, or run it inside Grok Build (GROK_SESSION_ID)"
        );
        std::process::exit(2);
    };
    let poll = Duration::from_secs(a.poll_secs.max(1));
    let grace = Duration::from_secs(a.grace_secs);
    let mut backoff = Duration::from_secs(1);
    let mut missing_since: Option<Instant> = None;
    let mut stdout = std::io::stdout();
    loop {
        let Some(client) = Client::discover(home) else {
            // No daemon: wait for one; another client starts it.
            back_off(&mut backoff);
            continue;
        };
        let sid = match &target {
            Target::Session(s) => s.clone(),
            Target::Harness(h, id) => match live_session(&client, h, id) {
                Ok(Some(s)) => {
                    missing_since = None;
                    s
                }
                Ok(None) => {
                    let since = *missing_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= grace {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_secs(1).min(grace));
                    continue;
                }
                Err(_) => {
                    back_off(&mut backoff);
                    continue;
                }
            },
        };
        let path = format!("/api/sessions/{sid}/notices?wait={}", poll.as_secs());
        match client.get_with_timeout(&path, poll + Duration::from_secs(10)) {
            Ok(v) => {
                backoff = Duration::from_secs(1);
                for line in v["lines"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    writeln!(stdout, "{line}")?;
                }
                stdout.flush()?;
                if a.once && v["lines"].as_array().is_some_and(|l| !l.is_empty()) {
                    return Ok(());
                }
            }
            Err(e) if session_gone(&e) => {
                if let Target::Session(_) = target {
                    return Ok(());
                }
                // A harness session may be registered again; resolve it anew.
            }
            Err(_) => back_off(&mut backoff),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(clap::Parser)]
    struct T {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn args(argv: &[&str]) -> FollowArgs {
        let mut full = vec!["t", "follow"];
        full.extend_from_slice(argv);
        let Cmd::Follow(a) = T::try_parse_from(full).unwrap().cmd;
        a
    }

    fn env(v: Option<&str>) -> impl Fn(&str) -> Option<String> {
        let v = v.map(str::to_string);
        move |k| (k == "GROK_SESSION_ID").then(|| v.clone()).flatten()
    }

    #[test]
    fn feedback_target_flags_win_over_the_environment() {
        assert_eq!(
            target(&args(&["--session", "s1"]), env(Some("g9"))),
            Some(Target::Session("s1".into()))
        );
        assert_eq!(
            target(
                &args(&["--agent", "claude", "--harness-session", "c1"]),
                env(Some("g9"))
            ),
            Some(Target::Harness("claude", "c1".into()))
        );
    }

    #[test]
    fn feedback_target_falls_back_to_grok_session_id() {
        assert_eq!(
            target(&args(&[]), env(Some("g9"))),
            Some(Target::Harness("grok", "g9".into()))
        );
        assert_eq!(target(&args(&[]), env(Some(""))), None);
        assert_eq!(target(&args(&[]), env(None)), None);
    }

    #[test]
    fn feedback_flags_conflict_and_pair() {
        let parse = |argv: &[&str]| {
            let mut full = vec!["t", "follow"];
            full.extend_from_slice(argv);
            T::try_parse_from(full).is_ok()
        };
        assert!(!parse(&[
            "--session",
            "s",
            "--agent",
            "grok",
            "--harness-session",
            "g"
        ]));
        assert!(!parse(&["--agent", "grok"]));
        assert!(!parse(&["--harness-session", "g"]));
        assert!(!parse(&["--agent", "pi", "--harness-session", "g"]));
    }
}
