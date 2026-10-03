//! Whether Grok Build started this run of the Claude Code plugin's copy.
//! Grok loads Claude Code plugins too; in a Grok session only `--agent
//! grok` acts (spec D17), so a Claude Code copy that Grok runs stands down.

use clax_core::Home;
use clax_hooks::input::HookInput;

fn var(env: &impl Fn(&str) -> Option<String>, k: &str) -> Option<String> {
    env(k).filter(|v| !v.is_empty())
}

/// True when Grok started this MCP server: `GROK_SESSION_ID` is set and
/// `CLAUDE_PID` is not `parent_pid`. Claude Code sets `CLAUDE_PID` to its
/// own PID and is the parent of the servers it starts, so a Claude Code
/// session started from a Grok shell, which inherits `GROK_SESSION_ID`,
/// still acts; a Grok started from a Claude Code shell inherits a
/// `CLAUDE_PID` that is not its server's parent.
pub fn grok_runs_mcp(env: impl Fn(&str) -> Option<String>, parent_pid: u32) -> bool {
    var(&env, "GROK_SESSION_ID").is_some()
        && var(&env, "CLAUDE_PID").and_then(|p| p.trim().parse::<u32>().ok()) != Some(parent_pid)
}

/// True when Grok runs this hook: `GROK_HOOK_EVENT` is set (Grok sets it
/// for every hook it runs, and its shell tool does not pass it on), or the
/// input is Grok's envelope (`hookEventName`).
pub fn grok_runs_hook(env: impl Fn(&str) -> Option<String>, input: &HookInput) -> bool {
    var(&env, "GROK_HOOK_EVENT").is_some() || input.is_grok_envelope()
}

/// Appends `<time> standdown mode=<mode> agent=claude host=grok` to
/// hooks.log, the line the wrapper writes for the same event.
pub fn log_standdown(home: &Home, mode: &str) {
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    crate::hooklog::append(
        home,
        &format!("{at} standdown mode={mode} agent=claude host=grok"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn grok_runs_an_mcp_server_it_gave_a_session_id_and_did_not_get_from_claude() {
        assert!(grok_runs_mcp(env(&[("GROK_SESSION_ID", "g")]), 500));
        assert!(
            grok_runs_mcp(env(&[("GROK_SESSION_ID", "g"), ("CLAUDE_PID", "77")]), 500),
            "Grok started from a Claude Code shell: CLAUDE_PID is inherited"
        );
        assert!(
            !grok_runs_mcp(env(&[("GROK_SESSION_ID", "g"), ("CLAUDE_PID", "500")]), 500),
            "Claude Code started from a Grok shell: Claude Code is the parent"
        );
        assert!(!grok_runs_mcp(env(&[("CLAUDE_PID", "500")]), 500));
        assert!(!grok_runs_mcp(env(&[("GROK_SESSION_ID", "")]), 500));
        assert!(!grok_runs_mcp(env(&[]), 500));
    }

    #[test]
    fn grok_runs_a_hook_with_its_event_variable_or_its_envelope() {
        let claude = HookInput::parse(r#"{"session_id":"s","hook_event_name":"Stop"}"#);
        let grok = HookInput::parse(r#"{"session_id":"s","sessionId":"s","hookEventName":"Stop"}"#);
        assert!(grok_runs_hook(env(&[("GROK_HOOK_EVENT", "Stop")]), &claude));
        assert!(grok_runs_hook(env(&[]), &grok));
        assert!(!grok_runs_hook(env(&[]), &claude));
        assert!(
            !grok_runs_hook(env(&[("GROK_SESSION_ID", "g")]), &claude),
            "a Claude Code hook in a session started from a Grok shell acts"
        );
    }
}
