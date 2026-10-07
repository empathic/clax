//! Handling of harness lifecycle hooks (Claude Code, Codex): parse the hook
//! input, talk to the daemon, and shape the output the harness expects.

pub mod ask;
pub mod events;
pub mod input;
pub mod output;
