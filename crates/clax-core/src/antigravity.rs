//! Google's Antigravity CLI (`agy`) as a Clax harness: the names its
//! plugin, MCP server and session metadata go by, and whether a run is
//! headless, decided from process command lines and whether the `agy`
//! process's standard input is a terminal.
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
    agy_args(argv).is_some()
}

/// Whether an `agy` command line asks for a headless run. Scans the words
/// after `agy` left to right: `-p`, `--print`, `--prompt` (or `--print=…`,
/// `--prompt=…`) first means it does; `-i` or `--prompt-interactive` (or
/// `--prompt-interactive=…`) first, or neither, means it does not.
pub fn asks_to_print(argv: &[String]) -> bool {
    let Some(args) = agy_args(argv) else {
        return false;
    };
    for word in args {
        match word.as_str() {
            "-p" | "--print" | "--prompt" => return true,
            "-i" | "--prompt-interactive" => return false,
            w if w.starts_with("--prompt-interactive=") => return false,
            w if w.starts_with("--print=") || w.starts_with("--prompt=") => return true,
            _ => {}
        }
    }
    false
}

/// Whether a run is headless. `command_lines` are a process's parent's
/// first, then its ancestors' (`None` for one that could not be read). The
/// nearest `agy` among them decides: the run is interactive only when that
/// command line does not ask to print ([`asks_to_print`]) and
/// `stdin_is_terminal`, given its index in `command_lines`, says that
/// process's standard input is a terminal. `None` from it (not known), or
/// no `agy` among them, means headless. `stdin_is_terminal` is called at
/// most once, and only when the answer depends on it.
pub fn run_is_headless(
    command_lines: &[Option<Vec<String>>],
    stdin_is_terminal: impl FnOnce(usize) -> Option<bool>,
) -> bool {
    let Some((i, argv)) = command_lines
        .iter()
        .enumerate()
        .find_map(|(i, l)| l.as_ref().filter(|a| is_agy(a)).map(|a| (i, a)))
    else {
        return true;
    };
    asks_to_print(argv) || stdin_is_terminal(i) != Some(true)
}

/// The words after `agy` in `argv`, or `None` when neither of its first two
/// words is `agy`.
fn agy_args(argv: &[String]) -> Option<&[String]> {
    let is_agy_word = |w: &String| Path::new(w).file_name() == Some(OsStr::new("agy"));
    argv.iter()
        .take(2)
        .position(is_agy_word)
        .map(|i| &argv[i + 1..])
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

    #[test]
    fn print_flags_mean_headless() {
        for c in [
            "agy -p hi",
            "agy --print hi",
            "agy --prompt hi",
            "agy --print=hi",
            "agy --prompt=hi",
            "agy --model m -p hi",
            "/bin/bash /t/agy -p hi",
        ] {
            assert!(asks_to_print(&argv(c)), "{c}");
        }
        for c in [
            "agy",
            "agy --model m",
            "agy -c",
            "agy --conversation abc",
            "/bin/bash /t/agy",
        ] {
            assert!(!asks_to_print(&argv(c)), "{c}");
        }
        assert!(!asks_to_print(&argv("zsh")));
    }

    #[test]
    fn an_interactive_prompt_that_mentions_p_is_interactive() {
        assert!(!asks_to_print(&argv("agy -i explain the -p flag")));
        assert!(!asks_to_print(&argv(
            "agy --prompt-interactive use --print here"
        )));
        assert!(!asks_to_print(&argv("agy --prompt-interactive=x -p")));
        assert!(asks_to_print(&argv("agy -p explain the -i flag")));
    }

    #[test]
    fn the_nearest_agy_decides_and_unknown_is_headless() {
        let l = |s: &str| Some(argv(s));
        let tty = |_| Some(true);
        assert!(!run_is_headless(&[l("agy")], tty));
        assert!(run_is_headless(&[l("agy -p x")], tty));
        assert!(!run_is_headless(
            &[l("sh -c ./scripts/tool-hook.sh"), l("agy")],
            tty
        ));
        assert!(!run_is_headless(&[None, l("agy")], tty));
        assert!(run_is_headless(&[None], tty));
        assert!(run_is_headless(&[], tty));
        assert!(run_is_headless(&[l("zsh")], tty));
    }

    #[test]
    fn an_agy_without_a_print_flag_is_interactive_only_on_a_terminal() {
        let l = |s: &str| Some(argv(s));
        assert!(!run_is_headless(&[l("agy")], |_| Some(true)), "terminal");
        assert!(run_is_headless(&[l("agy")], |_| Some(false)), "pipe");
        assert!(run_is_headless(&[l("agy")], |_| None), "unknown");
        assert!(
            run_is_headless(&[l("agy -p x")], |_| Some(true)),
            "print flag on a terminal"
        );
    }

    #[test]
    fn the_terminal_is_asked_of_the_nearest_agy_only_when_it_decides() {
        let l = |s: &str| Some(argv(s));
        let mut asked = Vec::new();
        run_is_headless(&[l("sh -c x"), None, l("agy"), l("agy")], |i| {
            asked.push(i);
            Some(true)
        });
        assert_eq!(asked, [2]);
        run_is_headless(&[l("agy -p x")], |_| panic!("asked about agy -p"));
        run_is_headless(&[l("zsh")], |_| panic!("asked with no agy"));
    }
}
