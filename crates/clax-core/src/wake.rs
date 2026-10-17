//! How a comment sent to an agent session wakes it while it is idle: the
//! `wake` block of the `watch` result, the Stop hook's nudge and the
//! SessionStart line all read the session's `push` (as `status` reports
//! it) through [`block`], so they say the same thing.
//!
//! Without a wake path, a comment waits for the agent's next clax tool
//! call, the end of its turn, the person's next message, or
//! `wait_for_feedback`; an idle session gets none of those by itself.

use serde_json::{Value, json};

/// `s` as one single-quoted shell word.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `clax feedback follow` of harness session `hs` under `harness`, with
/// `bin` (the clax binary) and `hs` each one shell word; `once` adds
/// `--once`, which exits after the first comment.
pub fn follow_command(bin: &str, harness: &str, hs: &str, once: bool) -> String {
    format!(
        "{} feedback follow{} --agent {} --harness-session {}",
        shell_quote(bin),
        if once { " --once" } else { "" },
        harness,
        shell_quote(hs)
    )
}

/// The longest a Claude Code `Monitor` runs, in milliseconds; it is re-armed
/// when it expires.
pub const MONITOR_TIMEOUT_MS: u64 = 1_800_000;

/// What the agent does when nothing wakes it and nothing it can start
/// would.
const TELL_THE_PERSON: &str = "Nothing can wake this session while it is idle. Tell the person \
that their comments reach you only when you next act (your next clax tool call, the end of a \
turn, or their next message); while they comment, call wait_for_feedback in a loop instead of \
ending your turn.";

/// Why a channel's wake is not sure: Claude Code tells a server neither
/// whether it registered it as a channel nor whether it accepted an event.
const CHANNEL_UNSURE: &str = "Claude Code was launched with the Clax channel, but it does not \
tell Clax whether it registered it: organization policy (channelsEnabled), the --channels \
allowlist or the sign-in method can refuse it, and then it drops each notice silently";

/// What the person is told, once, about a channel that may have been
/// refused: only they saw Claude Code's startup screen.
pub const CHANNEL_ASK: &str = "If Claude Code said the Clax channel was blocked when it \
started, relaunch without the channel flag so I can be woken.";

/// What to do about a channel that may have been refused.
const CHANNEL_DO: &str = "Start nothing: a comment sent to you should wake this session through \
the Clax channel, but Claude Code does not confirm the channel loaded. Tell the person once, in \
chat: \"If Claude Code said the Clax channel was blocked when it started, relaunch without the \
channel flag so I can be woken.\" Then end your turn when you are done.";

/// Whether a comment wakes an idle session with push `push` (as `status`
/// reports it: the daemon's, refined by the shim), by which path, and what
/// the agent does about it. `bin` is the clax binary a follower runs and
/// `hs` the harness's own session ID, when known. The block:
///
/// - `when_idle`: whether a comment wakes the idle session now.
/// - `via`: `channel` (Claude Code channel), `follower` (a `clax feedback
///   follow` is connected), `queue` (Codex's `codex queue`), `inject` (the
///   Pi extension) or `monitor` (Grok's monitor running the follower); null
///   when nothing wakes it.
/// - `follow_command` (and under Claude Code `follow_once_command`): the
///   background command that would wake it, when one would.
/// - `sure`: false when Clax cannot tell whether the path works (a
///   channel Claude Code may have refused); `reason` then says why.
/// - `reason`: why it is not woken, when it is not.
/// - `do`: one plain instruction.
pub fn block(harness: &str, push: &Value, bin: &str, hs: Option<&str>) -> Value {
    let available = push["available"].as_bool().unwrap_or(false);
    if available {
        let via = match push["tier"].as_str() {
            Some("channel") => "channel",
            Some("queue") => "queue",
            Some("inject") => "inject",
            Some("monitor") => "monitor",
            _ => "follower",
        };
        let how = match via {
            "channel" => "through the Clax channel",
            "queue" => "through codex queue",
            "inject" => "through the Clax Pi extension",
            "monitor" => "through the follower your monitor runs",
            _ => "through the clax feedback follow already running",
        };
        if via == "channel" {
            return json!({
                "when_idle": true,
                "via": via,
                "sure": false,
                "reason": CHANNEL_UNSURE,
                "do": CHANNEL_DO,
            });
        }
        return json!({
            "when_idle": true,
            "via": via,
            "sure": true,
            "do": format!("Start nothing: a comment sent to you wakes this session {how}. End your turn when you are done."),
        });
    }
    let reason = push["reason"].clone();
    let mut out = json!({"when_idle": false, "via": null, "sure": true, "reason": reason});
    match (harness, hs) {
        ("claude", Some(hs)) => {
            out["follow_command"] = json!(follow_command(bin, "claude", hs, false));
            out["follow_once_command"] = json!(follow_command(bin, "claude", hs, true));
            out["do"] = json!(format!(
                "A comment will not wake this session while it is idle. Start follow_command now \
with the Monitor tool (timeout_ms {MONITOR_TIMEOUT_MS}; start it again when it expires): each line \
it prints is a comment notice that wakes you. Without Monitor, run follow_once_command with the \
Bash tool and run_in_background: true, and run it again after each comment. Keep exactly one \
running; if you already started one this turn, start nothing."
            ));
        }
        ("grok", Some(hs)) => {
            out["follow_command"] = json!(follow_command(bin, "grok", hs, false));
            out["do"] = json!(
                "A comment will not wake this session while it is idle. Start follow_command now \
with Grok's monitor tool (persistent: true), exactly as given: each line it prints is a comment \
notice that wakes you. Keep exactly one running."
            );
        }
        ("codex", _) if push["other_machine"] == true => {
            out["do"] = json!(
                "Nothing can wake this session while it is idle. Tell the person: \"Codex can't be \
woken here; it runs on another machine than Clax, whose codex queue cannot reach it. Your \
comments reach me only when I next act.\" While they comment, call wait_for_feedback in a loop \
instead of ending your turn."
            );
        }
        ("codex", _) => {
            let why = push["reason"]
                .as_str()
                .unwrap_or("the Clax daemon cannot run codex queue");
            out["do"] = json!(format!(
                "Nothing can wake this session while it is idle ({why}). Tell the person: \
\"Codex can't be woken here; running `codex queue` to wake it needs `codex` on the Clax \
daemon's PATH (or CLAX_CODEX_BIN naming it) and a daemon restart. Until then your comments reach me only when I \
next act.\" While they comment, call wait_for_feedback in a loop instead of ending your turn."
            ));
        }
        _ => out["do"] = json!(TELL_THE_PERSON),
    }
    out
}

/// One line for the start of a session: whether comments wake it while
/// idle, from [`block`].
pub fn session_line(wake: &Value) -> String {
    if wake["when_idle"] == true && wake["sure"] == false {
        return format!(
            "Push: probably available in this session (via {}): {}. {}",
            wake["via"].as_str().unwrap_or("push"),
            wake["reason"].as_str().unwrap_or_default(),
            wake["do"].as_str().unwrap_or_default()
        );
    }
    if wake["when_idle"] == true {
        return format!(
            "Push: available in this session (via {}); a comment sent to you wakes it while idle.",
            wake["via"].as_str().unwrap_or("push")
        );
    }
    match wake["follow_command"].as_str() {
        Some(cmd) => format!(
            "Push: unavailable in this session; a comment sent to you will not wake it while idle. \
After you publish or watch, start `{cmd}` in the background (in Claude Code with the Monitor \
tool) to be woken; the `watch` result's `wake` says the same."
        ),
        None => format!("Push: unavailable in this session. {TELL_THE_PERSON}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_available_tier_names_its_path_and_starts_nothing() {
        for (tier, via) in [
            ("channel", "channel"),
            ("follow", "follower"),
            ("notice", "follower"),
            ("queue", "queue"),
            ("inject", "inject"),
            ("monitor", "monitor"),
        ] {
            let w = block(
                "claude",
                &json!({"tier": tier, "available": true}),
                "/b",
                Some("h"),
            );
            assert_eq!(w["when_idle"], true, "{tier}");
            assert_eq!(w["via"], via, "{tier}");
            assert!(w.get("follow_command").is_none(), "{tier}");
            assert!(
                w["do"].as_str().unwrap().starts_with("Start nothing"),
                "{w}"
            );
        }
    }

    #[test]
    fn a_channel_is_unsure_and_every_other_path_is_sure() {
        let w = block(
            "claude",
            &json!({"tier": "channel", "available": true}),
            "/b",
            Some("h"),
        );
        assert_eq!(
            (&w["when_idle"], &w["sure"]),
            (&json!(true), &json!(false)),
            "{w}"
        );
        let d = w["do"].as_str().unwrap();
        assert!(d.contains("Tell the person once, in chat"), "{d}");
        assert!(d.contains(CHANNEL_ASK), "{d}");
        assert!(
            !d.contains("startup screen"),
            "the agent cannot see it: {d}"
        );
        assert!(
            session_line(&w).starts_with("Push: probably available"),
            "{}",
            session_line(&w)
        );
        for tier in ["follow", "notice", "queue", "inject", "monitor"] {
            let w = block(
                "claude",
                &json!({"tier": tier, "available": true}),
                "/b",
                Some("h"),
            );
            assert_eq!(w["sure"], true, "{tier}");
        }
        let off = block("claude", &json!({"available": false}), "/b", Some("h"));
        assert_eq!(off["sure"], true);
    }

    #[test]
    fn claude_without_push_is_told_to_start_the_streaming_follower_under_monitor() {
        let w = block(
            "claude",
            &json!({"tier": null, "available": false, "reason": "r"}),
            "/b dir/clax",
            Some("6b1f'0c"),
        );
        assert_eq!(w["when_idle"], false);
        assert_eq!(w["via"], Value::Null);
        assert_eq!(w["reason"], "r");
        assert_eq!(
            w["follow_command"],
            "'/b dir/clax' feedback follow --agent claude --harness-session '6b1f'\\''0c'"
        );
        assert_eq!(
            w["follow_once_command"],
            "'/b dir/clax' feedback follow --once --agent claude --harness-session '6b1f'\\''0c'"
        );
        let d = w["do"].as_str().unwrap();
        assert!(
            d.contains("Monitor") && d.contains("run_in_background"),
            "{d}"
        );
    }

    #[test]
    fn grok_without_a_monitor_is_told_to_start_one() {
        let w = block(
            "grok",
            &json!({"tier": "monitor", "available": false}),
            "/b",
            Some("g"),
        );
        assert_eq!(w["when_idle"], false);
        assert_eq!(
            w["follow_command"],
            "'/b' feedback follow --agent grok --harness-session 'g'"
        );
        assert!(w["do"].as_str().unwrap().contains("monitor tool"));
    }

    #[test]
    fn a_session_nothing_can_wake_is_told_to_tell_the_person() {
        for (harness, push, hs) in [
            (
                "codex",
                json!({"tier": null, "available": false, "reason": "codex is not on the daemon's PATH"}),
                Some("c"),
            ),
            ("claude", json!({"tier": null, "available": false}), None),
            ("other", json!({"available": false}), Some("x")),
            ("claude", Value::Null, None),
        ] {
            let w = block(harness, &push, "/b", hs);
            assert_eq!(w["when_idle"], false, "{harness}");
            assert!(w.get("follow_command").is_none(), "{harness}");
            assert!(w["do"].as_str().unwrap().contains("Tell the person"), "{w}");
        }
    }

    #[test]
    fn a_codex_session_without_codex_queue_gives_the_person_the_fix() {
        let w = block(
            "codex",
            &json!({"tier": null, "available": false, "reason": "codex is not on the daemon's PATH; native push disabled"}),
            "/b",
            Some("c"),
        );
        let d = w["do"].as_str().unwrap();
        assert!(
            d.contains("Tell the person: \"Codex can't be woken here; running `codex queue`"),
            "{d}"
        );
        assert!(d.contains("codex is not on the daemon's PATH"), "{d}");
        assert!(d.contains("CLAX_CODEX_BIN"), "{d}");
    }

    #[test]
    fn a_codex_session_on_another_machine_is_told_so() {
        let w = block(
            "codex",
            &json!({"tier": null, "available": false, "other_machine": true,
                    "reason": "this Codex session runs on another machine than this Clax"}),
            "/b",
            Some("c"),
        );
        let d = w["do"].as_str().unwrap();
        assert!(
            d.contains("Codex can't be woken here; it runs on another machine than Clax"),
            "{d}"
        );
        assert!(!d.contains("PATH"), "{d}");
    }

    #[test]
    fn the_session_line_says_whether_push_is_available() {
        let on = block(
            "pi",
            &json!({"tier": "inject", "available": true}),
            "/b",
            None,
        );
        assert!(session_line(&on).starts_with("Push: available"));
        let off = block("claude", &json!({"available": false}), "/b", Some("h"));
        let l = session_line(&off);
        assert!(l.starts_with("Push: unavailable in this session;"), "{l}");
        assert!(l.contains("feedback follow --agent claude"), "{l}");
        let none = block("codex", &json!({"available": false}), "/b", Some("h"));
        assert!(session_line(&none).contains("Tell the person"));
    }
}
