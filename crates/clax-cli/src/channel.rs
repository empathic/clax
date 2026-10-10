//! Whether this CLI runs as an agent channel (spec §8.5, §16, M8): under a
//! harness, its requests carry `X-Clax-Channel: agent`, so the daemon serves
//! them as the owner's agent and not as the owner by hand.
//!
//! The decision reads the environment, first match wins:
//!
//! 1. `CLAX_CHANNEL=owner`: not an agent channel (the owner's own scripts).
//! 2. `CLAX_CHANNEL=agent`: an agent channel. The plugins' wrapper
//!    (`ensure-clax.sh exec`) exports it for every mode but `serve`, `stop`,
//!    `doctor` and `init`; the Pi extension sets it in Pi's process, which
//!    Pi's bash tool inherits.
//! 3. A variable a harness sets in its shell tool's environment, as
//!    `scripts/probe-harness-env.sh` observed it (`docs/contract.md`,
//!    "Harness shell environments"): [`HARNESS_VARIABLES`]. A variable no
//!    probe confirmed is not listed.
//!
//! Otherwise the CLI is the owner, used by hand.

use std::ffi::OsString;

/// What the agent-channel header says on every request of an agent channel.
pub const HEADER: &str = "x-clax-channel";
/// The value [`HEADER`] carries.
pub const AGENT: &str = "agent";

/// The variables a harness sets in its shell tool's environment, each with
/// the value it must have (`None`: any value but empty), as the probe saw
/// them: Claude Code 2.1.294 sets `CLAUDECODE=1`; Codex 0.162.0 sets
/// `CODEX_THREAD_ID` and `CODEX_CI=1`, and `CODEX_SANDBOX` inside its
/// sandbox. Grok Build was not available to probe, so `GROK_SESSION_ID` is
/// not read; Pi 0.73.1 sets `PI_CODING_AGENT=true` in its own process,
/// which its bash tool inherits (the Clax extension also sets
/// `CLAX_CHANNEL`).
pub const HARNESS_VARIABLES: &[(&str, Option<&str>)] = &[
    ("CLAUDECODE", Some("1")),
    ("CODEX_THREAD_ID", None),
    ("CODEX_SANDBOX", None),
    ("CODEX_CI", None),
    ("PI_CODING_AGENT", Some("true")),
];

/// Whether a CLI whose environment `env` reads runs under a harness.
pub fn under_harness(env: impl Fn(&str) -> Option<OsString>) -> bool {
    match env("CLAX_CHANNEL").as_deref().and_then(|v| v.to_str()) {
        Some("owner") => return false,
        Some("agent") => return true,
        _ => {}
    }
    HARNESS_VARIABLES.iter().any(|(name, want)| {
        env(name).is_some_and(|v| match want {
            Some(w) => v == *w,
            None => !v.is_empty(),
        })
    })
}

/// [`under_harness`] for this process's environment.
pub fn from_process() -> bool {
    under_harness(|k| std::env::var_os(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(pairs: &[(&str, &str)]) -> bool {
        under_harness(|k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(*v))
        })
    }

    #[test]
    fn each_confirmed_harness_variable_makes_an_agent_channel() {
        assert!(!with(&[]));
        assert!(with(&[("CLAUDECODE", "1")]));
        assert!(!with(&[("CLAUDECODE", "0")]));
        assert!(with(&[("CODEX_THREAD_ID", "019a")]));
        assert!(with(&[("CODEX_SANDBOX", "seatbelt")]));
        assert!(!with(&[("CODEX_SANDBOX", "")]));
        assert!(with(&[("CODEX_CI", "1")]));
        assert!(with(&[("PI_CODING_AGENT", "true")]));
        assert!(!with(&[("PI_CODING_AGENT_DIR", "/p")]));
        assert!(with(&[("CLAX_CHANNEL", "agent")]));
        // Variables no probe confirmed are not read.
        for unconfirmed in [
            "GROK_SESSION_ID",
            "CLAUDE_CODE_ENTRYPOINT",
            "CODEX_COMPANION_SESSION_ID",
        ] {
            assert!(!with(&[(unconfirmed, "1")]), "{unconfirmed}");
        }
    }

    #[test]
    fn the_owner_marker_wins_and_the_agent_marker_needs_no_harness() {
        assert!(!with(&[("CLAX_CHANNEL", "owner"), ("CLAUDECODE", "1")]));
        assert!(!with(&[
            ("CLAX_CHANNEL", "owner"),
            ("CODEX_THREAD_ID", "x")
        ]));
        assert!(with(&[("CLAX_CHANNEL", "agent")]));
        // Any other value is no marker: the harness variables decide.
        assert!(with(&[("CLAX_CHANNEL", "people"), ("CLAUDECODE", "1")]));
        assert!(!with(&[("CLAX_CHANNEL", "people")]));
    }
}
