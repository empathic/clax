//! `clax feedback follow`: prints one line per comment sent to an agent
//! session, for a harness that turns a command's output lines into wake-ups
//! (Grok Build's `monitor` tool). Each line is a notice that points at the
//! comment; it delivers nothing (see `Store::take_notices`). The command
//! never starts a daemon: it waits for one, follows the session across
//! daemon restarts, and exits 0 once the session has ended.

use crate::client::Client;
use clax_core::Home;
use clax_core::remote::{SESSION_SECRET_HEADER, SessionCredential, SessionKey};
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
    /// once the session has ended.
    ///
    /// In Claude Code, run it without --once under the Monitor tool
    /// (timeout_ms 1800000, started again when it expires): each line is
    /// an event that wakes the session, and nothing needs starting again
    /// per comment. In Grok Build, run it under the monitor tool with
    /// persistent: true. With --once it exits after the first comment, for
    /// a harness that wakes only when a background command exits (Claude
    /// Code's Bash run_in_background, started again after each comment).
    /// The `watch` tool's result gives the command for this session as
    /// `wake.follow_command`.
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

/// The flags that name the agent session a command acts in: a Clax
/// session ID, or a harness and the harness's own session ID.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct SessionArgs {
    /// The Clax session (`status` reports it as session.id).
    #[arg(long, conflicts_with_all = ["agent", "harness_session"])]
    pub session: Option<String>,
    /// The harness whose session --harness-session names.
    #[arg(long, value_enum, requires = "harness_session")]
    pub agent: Option<FollowAgent>,
    /// The harness's own session ID (`status` reports it as
    /// session.harness_session_id).
    #[arg(long, requires = "agent")]
    pub harness_session: Option<String>,
}

#[derive(clap::Args)]
pub struct FollowArgs {
    #[command(flatten)]
    pub who: SessionArgs,
    /// Seconds each daemon long-poll waits.
    #[arg(long, hide = true, default_value_t = 50)]
    pub poll_secs: u64,
    /// Seconds a followed harness session may have no live Clax session
    /// before the command exits.
    #[arg(long, hide = true, default_value_t = 60)]
    pub grace_secs: u64,
    /// Exit 0 after the first poll that printed at least one line, so a
    /// harness that wakes only when a background command exits (Claude
    /// Code's Bash run_in_background) is woken by the first comment. The
    /// agent starts it again after handling the comment. Under Claude
    /// Code's Monitor tool, leave it off.
    #[arg(long)]
    pub once: bool,
}

/// The session a command names.
#[derive(Debug, PartialEq)]
pub(crate) enum Target {
    /// A Clax session ID: following ends when it ends.
    Session(String),
    /// A harness session, resolved to its live Clax session on each poll,
    /// so a session re-registered after a daemon restart is followed too.
    Harness(&'static str, String),
}

/// The session `a` names: --session, else --agent with
/// --harness-session, else, inside Grok Build, `GROK_SESSION_ID` (read
/// through `env`).
pub(crate) fn target(a: &SessionArgs, env: impl Fn(&str) -> Option<String>) -> Option<Target> {
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

/// The headers that name session credential `c` (spec 2026-10-08 §6.8):
/// none without one.
fn proof(c: Option<&SessionCredential>) -> Vec<(&str, &str)> {
    c.map(|c| {
        vec![
            ("x-clax-session", c.session.as_str()),
            (SESSION_SECRET_HEADER, c.secret.expose()),
        ]
    })
    .unwrap_or_default()
}

/// What a command acting in a session on a remote daemon proves it with:
/// the credential in the session's file (spec 2026-10-08 §6.8). `Ok(None)`
/// on the local daemon. A harness session whose file is not there yet is
/// waited for as the session is; a Clax session must have it.
pub(crate) fn credential(
    client: &Client,
    target: &Target,
) -> anyhow::Result<Option<SessionCredential>> {
    let Some(remote) = client.remote() else {
        return Ok(None);
    };
    let files = &remote.sessions;
    match target {
        Target::Harness(h, id) => Ok(files.read(&SessionKey::Harness { harness: h, id })?),
        Target::Session(sid) => files.by_session(sid)?.map(Some).ok_or_else(|| {
            anyhow::anyhow!(
                "session_credential_missing: no credential for session {sid} in {}: it has not registered from this machine, or has ended",
                files.dir().display()
            )
        }),
    }
}

/// The live Clax session of `(harness, id)`, if any, asked as `cred`'s
/// session on a remote daemon.
pub(crate) fn live_session(
    c: &Client,
    harness: &str,
    id: &str,
    cred: Option<&SessionCredential>,
) -> anyhow::Result<Option<String>> {
    let v = c.request_with(
        reqwest::Method::GET,
        "/api/sessions?live=true",
        None,
        None,
        &proof(cred),
    )?;
    Ok(v["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["harness"] == harness && s["harness_session_id"].as_str() == Some(id))
        .and_then(|s| s["id"].as_str())
        .map(str::to_string))
}

/// Whether a daemon error says the session is gone: on a remote daemon,
/// a proof refused because its session ended too.
pub(crate) fn session_gone(e: &anyhow::Error) -> bool {
    gone_code(&e.to_string())
}

/// Whether a refusal's `<code>: <message>` text (or its code alone) says
/// the session is gone.
pub(crate) fn gone_code(m: &str) -> bool {
    ["unknown_session", "not_found", "invalid_session_proof"]
        .iter()
        .any(|c| m == *c || m.starts_with(&format!("{c}:")))
}

/// The shortest time between the starts of two notices polls when the
/// first answered empty.
const EARLY_PACE: Duration = Duration::from_secs(1);

/// Sleeps `backoff`, then doubles it up to 30 s.
fn back_off(backoff: &mut Duration) {
    std::thread::sleep(*backoff);
    *backoff = (*backoff * 2).min(Duration::from_secs(30));
}

/// What a follower's preparation of a session (the `ready` of [`follow`])
/// found.
pub(crate) enum Ready {
    /// The session is ready to follow.
    Yes,
    /// The daemon could not be asked: try again after a pause.
    Retry,
    /// The session is gone, as [`session_gone`] says.
    Gone,
}

/// How long a follower's polls wait, and how long a harness session may
/// have no live Clax session before following ends.
pub(crate) struct Pace {
    pub poll: Duration,
    pub grace: Duration,
}

/// Follows `target`'s notices: finds the daemon (never starting one) and
/// the session, across daemon restarts and re-registrations, calls
/// `ready` for each session it finds before its first poll (and again
/// after a failed poll, as the daemon may have restarted), and hands each
/// answer of `GET /api/sessions/<sid>/notices` to `on_poll`, which says
/// whether to stop. Returns once `on_poll` says stop, or the session has
/// ended (a Clax session at once; a harness session after `pace.grace`
/// with no live Clax session). An error of `ready` or `on_poll` ends it.
pub(crate) fn follow(
    home: &Home,
    target: &Target,
    pace: &Pace,
    mut ready: impl FnMut(&Client, &str, Option<&SessionCredential>) -> anyhow::Result<Ready>,
    mut on_poll: impl FnMut(&Value) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    let (poll, grace) = (pace.poll, pace.grace);
    let mut backoff = Duration::from_secs(1);
    let mut missing_since: Option<Instant> = None;
    // The session `ready` last said yes for.
    let mut prepared: Option<String> = None;
    loop {
        let Some(client) = Client::discover(home).map(Client::agent_channel) else {
            // No daemon: wait for one; another client starts it.
            prepared = None;
            back_off(&mut backoff);
            continue;
        };
        let cred = credential(&client, target)?;
        let remote_without_file = client.remote().is_some() && cred.is_none();
        let sid = match target {
            Target::Session(s) => s.clone(),
            Target::Harness(h, id) => {
                let found = match live_session(&client, h, id, cred.as_ref()) {
                    // On a remote daemon the session is not this machine's
                    // until its credential file is written.
                    _ if remote_without_file => None,
                    Ok(found) => found,
                    // A credential whose session ended proves nothing: the
                    // session is gone until one registers again.
                    Err(e) if session_gone(&e) => None,
                    Err(_) => {
                        back_off(&mut backoff);
                        continue;
                    }
                };
                let Some(s) = found else {
                    let since = *missing_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= grace {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_secs(1).min(grace));
                    continue;
                };
                missing_since = None;
                s
            }
        };
        if prepared.as_deref() != Some(sid.as_str()) {
            match ready(&client, &sid, cred.as_ref())? {
                Ready::Yes => prepared = Some(sid.clone()),
                Ready::Retry => {
                    back_off(&mut backoff);
                    continue;
                }
                Ready::Gone => {
                    if let Target::Session(_) = target {
                        return Ok(());
                    }
                    // A harness session may be registered again; resolve it anew.
                    std::thread::sleep(Duration::from_secs(1).min(grace));
                    continue;
                }
            }
        }
        let path = format!("/api/sessions/{sid}/notices?wait={}", poll.as_secs());
        let polled = Instant::now();
        match client.request_with(
            reqwest::Method::GET,
            &path,
            None,
            Some(poll + Duration::from_secs(10)),
            &proof(cred.as_ref()),
        ) {
            Ok(v) => {
                backoff = Duration::from_secs(1);
                // An empty answer that did not wait (the session is inside
                // wait_for_feedback, or the daemon is stopping): poll again
                // no sooner than a second after this poll began.
                if v["lines"].as_array().is_none_or(|l| l.is_empty()) {
                    std::thread::sleep(EARLY_PACE.saturating_sub(polled.elapsed()));
                }
                if on_poll(&v)? {
                    return Ok(());
                }
            }
            Err(e) if session_gone(&e) => {
                if let Target::Session(_) = target {
                    return Ok(());
                }
                // A harness session may be registered again; resolve it anew.
                prepared = None;
            }
            Err(_) => {
                prepared = None;
                back_off(&mut backoff);
            }
        }
    }
}

pub fn run(_cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    let Cmd::Follow(a) = cmd;
    let Some(target) = target(&a.who, |k| std::env::var(k).ok()) else {
        eprintln!(
            "clax feedback follow: no session: pass --session, or --agent with --harness-session, or run it inside Grok Build (GROK_SESSION_ID)"
        );
        std::process::exit(2);
    };
    let pace = Pace {
        poll: Duration::from_secs(a.poll_secs.max(1)),
        grace: Duration::from_secs(a.grace_secs),
    };
    let mut stdout = std::io::stdout();
    let followed = follow(
        home,
        &target,
        &pace,
        |_, _, _| Ok(Ready::Yes),
        |v| {
            let lines: Vec<&str> = v["lines"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            for line in &lines {
                writeln!(stdout, "{line}")?;
            }
            stdout.flush()?;
            Ok(a.once && !lines.is_empty())
        },
    );
    if let Err(e) = followed {
        eprintln!("clax feedback follow: {e:#}");
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::client::tests::recv_within;
    use crate::remote::tests::{Answer, FakeDaemon};
    use clap::Parser;
    use serde_json::json;

    #[derive(clap::Parser)]
    struct T {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn args(argv: &[&str]) -> SessionArgs {
        let mut full = vec!["t", "follow"];
        full.extend_from_slice(argv);
        let Cmd::Follow(a) = T::try_parse_from(full).unwrap().cmd;
        a.who
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

    /// Records `daemon` in `home`'s `daemon.json` as a live daemon (this
    /// test's own process).
    fn record(home: &Home, daemon: &FakeDaemon) {
        let port = daemon.base.rsplit(':').next().unwrap().parse().unwrap();
        clax_core::daemon::write_daemon_info(
            home,
            &clax_core::daemon::DaemonInfo {
                port,
                pid: std::process::id(),
                token: "t".into(),
                started_at: "2026-11-01T00:00:00Z".into(),
                bind: "127.0.0.1".into(),
                version: "0.0.1".into(),
                exe: None,
                commit: None,
            },
        )
        .unwrap();
    }

    /// One round of [`follow`] against a [`FakeDaemon`]: `/healthz`, then
    /// the notices poll answering `lines`.
    fn round(lines: &[&str]) -> [Answer; 2] {
        [
            (200, vec![], json!({"version": "0.0.1"})),
            (200, vec![], json!({"notices": [], "lines": lines})),
        ]
    }

    /// Follows session `s1` of `home` on a thread of its own until a poll
    /// prints `last`: follow's result and the answers it handed on, in
    /// order, bounded by the hang guard.
    fn follow_until(home: &Home, last: &'static str) -> (anyhow::Result<()>, Vec<Value>) {
        let home = home.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            let pace = Pace {
                poll: Duration::from_secs(50),
                grace: Duration::from_secs(60),
            };
            let r = follow(
                &home,
                &Target::Session("s1".into()),
                &pace,
                |_, _, _| Ok(Ready::Yes),
                |v| {
                    seen.push(v.clone());
                    Ok(v["lines"].as_array().unwrap().iter().any(|l| l == last))
                },
            );
            let _ = tx.send((r, seen));
        });
        recv_within("the follow's end", &rx)
    }

    /// The paths of `daemon`'s requests.
    fn paths(daemon: &FakeDaemon) -> Vec<String> {
        daemon.seen().into_iter().map(|r| r.path).collect()
    }

    /// Each answer is handed on in turn, an empty one that came at once as
    /// well as those that printed, each from a poll of its own.
    #[test]
    fn follow_hands_on_each_answer_from_a_poll_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let daemon = FakeDaemon::start(
            [round(&[]), round(&["a"]), round(&["b"])]
                .into_iter()
                .flatten()
                .collect(),
        );
        record(&home, &daemon);
        let (r, seen) = follow_until(&home, "b");
        r.unwrap();
        let lines: Vec<&Value> = seen.iter().map(|v| &v["lines"]).collect();
        assert_eq!(lines, [&json!([]), &json!(["a"]), &json!(["b"])]);
        let polls: Vec<String> = paths(&daemon)
            .into_iter()
            .filter(|p| p.contains("/notices"))
            .collect();
        assert_eq!(polls, vec!["/api/sessions/s1/notices?wait=50"; 3]);
    }

    /// A daemon that does not answer `/healthz` is no daemon to follow: it
    /// looks again, and follows the one that answers.
    #[test]
    fn follow_looks_again_while_no_daemon_answers() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let mut answers = vec![(503, vec![], json!({}))];
        answers.extend(round(&["a"]));
        let daemon = FakeDaemon::start(answers);
        record(&home, &daemon);
        let (r, seen) = follow_until(&home, "a");
        r.unwrap();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(
            paths(&daemon),
            ["/healthz", "/healthz", "/api/sessions/s1/notices?wait=50"]
        );
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
