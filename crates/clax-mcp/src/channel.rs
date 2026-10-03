//! Claude Code channels: tier 5 under Claude Code. The shim declares the
//! `claude/channel` capability. When its session was launched with the
//! channel, it forwards each comment notice (`GET /api/sessions/<sid>/notices`)
//! as a `notifications/claude/channel` event. A notice points at a comment
//! and delivers nothing. The comment still arrives through tier 1, 2 or 4.
//!
//! Claude Code tells a server neither whether it registered it as a channel
//! nor whether it accepted an event: it drops unaccepted events silently.
//! The shim can only read how its parent, the `claude` process, was launched.
//! A channel can register only when `--dangerously-load-development-channels`
//! or `--channels` names this plugin. Organization policy, the allowlist or
//! the authentication method can still refuse it, and the shim cannot see
//! that. The channel's permission relay capability is never declared:
//! comment authors never approve tool use.

use serde_json::{Value, json};

/// The experimental capability that makes an MCP server a channel.
pub const CAPABILITY: &str = "claude/channel";
/// The notification method of a channel event.
pub const METHOD: &str = "notifications/claude/channel";
/// The launch-flag entry for the plugin as `clax init` installs it
/// (plugin `clax` from marketplace `clax`).
pub const ENTRY: &str = "plugin:clax@clax";
/// The command that launches Claude Code with the Clax channel.
pub const LAUNCH: &str = "claude --dangerously-load-development-channels plugin:clax@clax";
/// What `status` says about registration, which the shim cannot observe.
const NOTE: &str = "Claude Code does not tell the server whether it registered the channel; its startup screen says so";
/// The launch flags whose values are channel entries.
const FLAGS: &[&str] = &["--dangerously-load-development-channels", "--channels"];

/// Added to the server instructions when the channel is declared.
pub const INSTRUCTIONS: &str = "Clax sends comment notices as <channel source=\"plugin:clax:clax\" \
artifact_id=\"...\" thread_id=\"...\" comment_id=\"...\">. A notice says only that a person sent \
you a comment on a Clax artifact; it does not contain the comment. Call comments_read with \
url_or_id set to artifact_id and thread_id set to thread_id, act on the comment, answer with \
comments_reply, and call comments_resolve when done. If you have already handled that thread, do \
nothing. Nothing is sent back over the channel.";

/// Whether the parent's command line names a Clax channel entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchFlag {
    /// `flag` names `entry`, a `plugin:clax@<marketplace>` entry.
    Present { flag: String, entry: String },
    /// The command line names no Clax channel entry.
    Absent,
    /// The command line could not be read, for this reason.
    Unknown(String),
}

/// Whether `entry` names the Clax plugin from some marketplace.
fn is_clax(entry: &str) -> bool {
    entry
        .strip_prefix("plugin:clax@")
        .is_some_and(|m| !m.is_empty())
}

/// The Clax channel entry that `argv` passes to a channel launch flag. Each
/// value after the flag, up to the next option, is an entry; `--flag=value`
/// passes one.
pub fn launch_flag(argv: &[String]) -> LaunchFlag {
    let present = |flag: &str, entry: &str| LaunchFlag::Present {
        flag: flag.to_string(),
        entry: entry.to_string(),
    };
    let mut current: Option<&str> = None;
    for arg in argv {
        if let Some((flag, value)) = arg.split_once('=')
            && FLAGS.contains(&flag)
        {
            if is_clax(value) {
                return present(flag, value);
            }
            current = None;
        } else if FLAGS.contains(&arg.as_str()) {
            current = Some(arg);
        } else if arg.starts_with('-') {
            current = None;
        } else if let Some(flag) = current
            && is_clax(arg)
        {
            return present(flag, arg);
        }
    }
    LaunchFlag::Absent
}

/// The command line of process `pid`.
#[cfg(target_os = "linux")]
pub fn process_argv(pid: u32) -> Result<Vec<String>, String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).map_err(|e| e.to_string())?;
    let argv: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    if argv.is_empty() {
        Err("empty command line".into())
    } else {
        Ok(argv)
    }
}

/// The command line of process `pid`, from `ps -ww -o args=`, split on
/// whitespace. `ps` joins the arguments with spaces. Channel entries contain
/// none, so they come back whole.
#[cfg(not(target_os = "linux"))]
pub fn process_argv(pid: u32) -> Result<Vec<String>, String> {
    let out = crate::shim::run_with_timeout(
        std::process::Command::new("ps").args(["-ww", "-o", "args=", "-p", &pid.to_string()]),
        std::time::Duration::from_secs(2),
    )
    .ok_or_else(|| "ps did not answer within 2 s".to_string())?;
    let argv: Vec<String> = out.split_whitespace().map(str::to_string).collect();
    if argv.is_empty() {
        Err(format!("ps printed nothing for process {pid}"))
    } else {
        Ok(argv)
    }
}

/// What the shim knows about its channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelState {
    pub launch_flag: LaunchFlag,
}

impl ChannelState {
    /// Reads the launch flag from the command line of `parent_pid`.
    pub fn detect(parent_pid: u32) -> ChannelState {
        let launch_flag = match process_argv(parent_pid) {
            Ok(argv) => launch_flag(&argv),
            Err(e) => LaunchFlag::Unknown(e),
        };
        ChannelState { launch_flag }
    }

    /// Whether the shim forwards notices as channel events.
    pub fn forwards(&self) -> bool {
        matches!(self.launch_flag, LaunchFlag::Present { .. })
    }

    /// `status.push.channel`.
    pub fn to_json(&self) -> Value {
        let (state, flag, entry, why) = match &self.launch_flag {
            LaunchFlag::Present { flag, entry } => {
                ("present", Some(flag.as_str()), Some(entry.as_str()), None)
            }
            LaunchFlag::Absent => ("absent", None, None, None),
            LaunchFlag::Unknown(why) => ("unknown", None, None, Some(why.as_str())),
        };
        let mut v = json!({"declared": true, "launch_flag": state, "flag": flag, "entry": entry,
            "registered": null, "launch": LAUNCH, "note": NOTE});
        if let Some(why) = why {
            v["unknown_reason"] = json!(why);
        }
        v
    }

    /// The key=value fields of the shim's `channel` line in hooks.log.
    pub fn log_fields(&self, parent_pid: u32) -> String {
        let (state, flag, entry) = match &self.launch_flag {
            LaunchFlag::Present { flag, entry } => ("present", flag.as_str(), entry.as_str()),
            LaunchFlag::Absent => ("absent", "", ""),
            LaunchFlag::Unknown(_) => ("unknown", "", ""),
        };
        format!("launch_flag={state} flag=\"{flag}\" entry=\"{entry}\" parent_pid={parent_pid}")
    }
}

/// The params of the channel event for one notice: the follow line as
/// `content`, and the notice's IDs as `meta`. Meta keys use only letters,
/// digits and underscores; Claude Code drops any other key.
pub fn event_params(notice: &Value, line: &str) -> Value {
    let id = |k: &str| notice[k].as_str().unwrap_or_default().to_string();
    json!({"content": line, "meta": {
        "artifact_id": id("artifact_id"), "thread_id": id("thread_id"), "comment_id": id("comment_id"),
    }})
}

/// `s` as one single-quoted shell word.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `status.push` under Claude Code: the daemon's `push`, refined. The tier is
/// `channel` when the shim forwards notices, and `follow` when the daemon sees
/// a notice follower and the shim is not one. Without forwarding,
/// `follow_command` is the background command the skill runs (present when
/// the harness session ID is known). `null` (no session) stays `null`.
pub fn push_for_status(
    ch: &ChannelState,
    daemon: Value,
    harness_session_id: Option<&str>,
    bin: &str,
) -> Value {
    let Value::Object(_) = daemon else {
        return daemon;
    };
    let mut push = daemon;
    if ch.forwards() {
        push["tier"] = json!("channel");
        push["available"] = json!(true);
        push["reason"] = Value::Null;
    } else {
        if push["tier"] == "notice" {
            push["tier"] = json!("follow");
        }
        if let Some(hs) = harness_session_id {
            push["follow_command"] = json!(format!(
                "{} feedback follow --once --agent claude --harness-session {}",
                shell_quote(bin),
                shell_quote(hs)
            ));
        }
    }
    push["channel"] = ch.to_json();
    push
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_development_flag_naming_clax_is_present() {
        assert_eq!(
            launch_flag(&argv(
                "claude --dangerously-load-development-channels plugin:clax@clax"
            )),
            LaunchFlag::Present {
                flag: "--dangerously-load-development-channels".into(),
                entry: "plugin:clax@clax".into()
            }
        );
        assert_eq!(
            launch_flag(&argv(
                "node /x/cli.js --channels plugin:telegram@claude-plugins-official plugin:clax@acme --model opus"
            )),
            LaunchFlag::Present {
                flag: "--channels".into(),
                entry: "plugin:clax@acme".into()
            }
        );
        assert_eq!(
            launch_flag(&argv(
                "claude --dangerously-load-development-channels=plugin:clax@clax"
            )),
            LaunchFlag::Present {
                flag: "--dangerously-load-development-channels".into(),
                entry: "plugin:clax@clax".into()
            }
        );
    }

    #[test]
    fn other_entries_options_and_positionals_are_absent() {
        for line in [
            "claude",
            "claude --dangerously-load-development-channels server:clax",
            "claude --channels plugin:clax-grok@clax",
            "claude --channels plugin:telegram@x --resume plugin:clax@clax",
            "claude plugin:clax@clax",
            "claude --dangerously-load-development-channels plugin:clax@",
        ] {
            assert_eq!(launch_flag(&argv(line)), LaunchFlag::Absent, "{line}");
        }
    }

    #[test]
    fn this_process_argv_is_read() {
        let me = process_argv(std::process::id()).unwrap();
        assert!(!me.is_empty());
    }

    #[test]
    fn an_event_is_the_follow_line_with_identifier_meta_keys() {
        let notice = json!({"feedback_id": "f", "comment_id": "c1", "thread_id": "t1",
            "artifact_id": "7q3k9mzx2b4t", "title": "T", "url": "http://h/a/7q3k9mzx2b4t"});
        let p = event_params(&notice, "[clax] New comment on \"T\" …");
        assert_eq!(p["content"], "[clax] New comment on \"T\" …");
        assert_eq!(
            p["meta"],
            json!({"artifact_id": "7q3k9mzx2b4t", "thread_id": "t1", "comment_id": "c1"})
        );
        for k in p["meta"].as_object().unwrap().keys() {
            assert!(
                k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{k}"
            );
        }
    }

    #[test]
    fn status_says_channel_only_when_the_flag_is_present() {
        let present = ChannelState {
            launch_flag: launch_flag(&argv(
                "claude --dangerously-load-development-channels plugin:clax@clax",
            )),
        };
        let daemon = json!({"tier": "notice", "available": true, "reason": null});
        let p = push_for_status(&present, daemon, Some("hs"), "/b/clax");
        assert_eq!(p["tier"], "channel");
        assert_eq!(p["available"], true);
        assert_eq!(p["channel"]["launch_flag"], "present");
        assert_eq!(p["channel"]["registered"], Value::Null);
        assert!(p.get("follow_command").is_none());

        let absent = ChannelState {
            launch_flag: LaunchFlag::Absent,
        };
        let idle = json!({"tier": null, "available": false, "reason": "r"});
        let p = push_for_status(&absent, idle, Some("6b1f'0c"), "/b dir/clax");
        assert_eq!(p["tier"], Value::Null);
        assert_eq!(p["reason"], "r");
        assert_eq!(
            p["follow_command"],
            "'/b dir/clax' feedback follow --once --agent claude --harness-session '6b1f'\\''0c'"
        );
        assert_eq!(p["channel"]["launch"], LAUNCH);

        let following = json!({"tier": "notice", "available": true, "reason": null});
        let p = push_for_status(&absent, following, Some("hs"), "/b/clax");
        assert_eq!(p["tier"], "follow");
        assert!(p["follow_command"].is_string());

        assert_eq!(
            push_for_status(&absent, Value::Null, None, "/b/clax"),
            Value::Null
        );
        let unknown = ChannelState {
            launch_flag: LaunchFlag::Unknown("ps failed".into()),
        };
        let p = push_for_status(
            &unknown,
            json!({"tier": null, "available": false, "reason": "r"}),
            None,
            "/b/clax",
        );
        assert_eq!(p["channel"]["launch_flag"], "unknown");
        assert!(
            p.get("follow_command").is_none(),
            "no harness session ID, no command"
        );
    }
}
