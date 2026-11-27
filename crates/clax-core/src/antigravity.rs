//! Google's Antigravity CLI (`agy`) as a Clax harness: the names its
//! plugin, MCP server and session metadata go by, and whether a process
//! command line runs `agy`.
//!
//! A command line is a process's argv as `clax_mcp::channel::process_argv`
//! returns it: on macOS `ps` output split on whitespace, so a quoted prompt
//! arrives as several words. Nothing here reads a process.

use std::ffi::OsStr;
use std::path::Path;

pub const HARNESS: &str = "antigravity";
pub const PLUGIN: &str = "clax-antigravity";
pub const SERVER: &str = "clax-antigravity_clax";
pub const PERMISSION_RULE: &str = "mcp(clax-antigravity_clax/*)";
pub const MIN_AGY: (u64, u64, u64) = (1, 5, 18);
pub const META_ROOT: &str = "antigravity.google/root_conversation_id";
pub const META_CONVERSATION: &str = "antigravity.google/conversation_id";

/// Whether `argv` runs `agy`: the file name of its first word is `agy`,
/// or of its second (a script named `agy` run by an interpreter).
pub fn is_agy(argv: &[String]) -> bool {
    let is_agy_word = |w: &String| Path::new(w).file_name() == Some(OsStr::new("agy"));
    argv.iter().take(2).any(is_agy_word)
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn agy_is_recognised_by_its_program_name() {
        assert!(is_agy(&argv("agy")));
        assert!(is_agy(&argv("/Users/x/.local/bin/agy --model m")));
        assert!(is_agy(&argv("/bin/bash /tmp/t/agy -p x")));
        assert!(!is_agy(&argv("node /x/agy.js")));
        assert!(!is_agy(&argv(
            "bash ./scripts/ensure-clax.sh exec hook --agent antigravity stop"
        )));
        assert!(!is_agy(&[]));
    }
}
