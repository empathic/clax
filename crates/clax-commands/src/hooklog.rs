//! `logs/hooks.log`: one line per `clax hook` run, and one per failed
//! launcher resolution (written by `scripts/ensure-clax.sh`), so a hook
//! failure the harness only reports as an exit code can be inspected.
//!
//! Lines are `<RFC 3339 UTC> <source> key=value ...`; `agent=<harness>` is on
//! every line. The file is renamed to `hooks.log.1` once it passes
//! [`MAX_BYTES`]. Logging never fails the caller.

use clax_core::Home;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// Past this size the log is rotated to `hooks.log.1` before the next append.
pub const MAX_BYTES: u64 = 1 << 20;
/// How much of a hook's stderr or error a line keeps.
const STDERR_CHARS: usize = 200;

/// `text` on one line, quotes and backslashes escaped, cut to [`STDERR_CHARS`].
fn one_line(text: &str) -> String {
    let cut: String = text.chars().take(STDERR_CHARS).collect();
    cut.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ")
}

/// The line recorded for one hook run.
pub fn hook_line(
    at: chrono::DateTime<chrono::Utc>,
    agent: &str,
    event: &str,
    bin: &Path,
    took: Duration,
    exit: i32,
    stderr: Option<&str>,
) -> String {
    format!(
        "{} hook agent={agent} event={event} bin={} duration_ms={} exit={exit} stderr=\"{}\"",
        at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        bin.display(),
        took.as_millis(),
        one_line(stderr.unwrap_or_default()),
    )
}

/// Appends `line` to the home's hooks.log, creating `logs/` and rotating the
/// file past [`MAX_BYTES`]. Errors are ignored.
pub fn append(home: &Home, line: &str) {
    let _ = try_append(home, line);
}

fn try_append(home: &Home, line: &str) -> std::io::Result<()> {
    let path = home.hooks_log_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        std::fs::rename(&path, path.with_extension("log.1"))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{line}")
}

/// The last `n` lines of hooks.log (then hooks.log.1) naming `agent=<agent>`,
/// oldest first. Stand-down lines (`standdown … host=grok`) are left out:
/// they are not that harness's hooks.
pub fn tail_for(home: &Home, agent: &str, n: usize) -> Vec<String> {
    let path = home.hooks_log_path();
    let needle = format!(" agent={agent} ");
    let mut lines: Vec<String> = [path.with_extension("log.1"), path]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .flat_map(|t| t.lines().map(str::to_string).collect::<Vec<_>>())
        .filter(|l| l.contains(&needle) && !l.contains(" standdown "))
        .collect();
    let skip = lines.len().saturating_sub(n);
    lines.drain(..skip);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-09-29T10:10:00Z")
            .unwrap()
            .into()
    }

    #[test]
    fn a_hook_line_names_the_run_and_keeps_200_chars_of_stderr_on_one_line() {
        let long = format!("first \"line\"\nsecond {}", "x".repeat(300));
        let line = hook_line(
            at(),
            "codex",
            "stop",
            Path::new("/b/clax"),
            Duration::from_millis(42),
            0,
            Some(&long),
        );
        assert!(
            line.starts_with(
                "2026-09-29T10:10:00Z hook agent=codex event=stop bin=/b/clax duration_ms=42 exit=0 stderr=\"first \\\"line\\\" second xxx"
            ),
            "{line}"
        );
        assert!(!line.contains('\n'));
        let kept = line.split_once("stderr=\"").unwrap().1;
        assert_eq!(
            kept.matches('x').count(),
            200 - "first \"line\"\nsecond ".len()
        );
        let quiet = hook_line(
            at(),
            "claude",
            "prompt",
            Path::new("/b"),
            Duration::ZERO,
            0,
            None,
        );
        assert!(quiet.ends_with("stderr=\"\""), "{quiet}");
    }

    #[test]
    fn append_creates_the_logs_dir_and_rotates_past_one_mib() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        append(&home, "a agent=codex x");
        assert_eq!(
            std::fs::read_to_string(home.hooks_log_path()).unwrap(),
            "a agent=codex x\n"
        );
        std::fs::write(home.hooks_log_path(), vec![b'.'; MAX_BYTES as usize + 1]).unwrap();
        append(&home, "b agent=codex y");
        assert_eq!(
            std::fs::read_to_string(home.hooks_log_path()).unwrap(),
            "b agent=codex y\n"
        );
        assert_eq!(
            std::fs::metadata(home.hooks_log_path().with_extension("log.1"))
                .unwrap()
                .len(),
            MAX_BYTES + 1
        );
    }

    #[test]
    fn append_ignores_an_unwritable_home() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        append(&Home::at(file), "x agent=codex y");
    }

    #[test]
    fn tail_keeps_the_agents_last_lines_across_the_rotated_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().to_path_buf());
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        std::fs::write(
            home.hooks_log_path().with_extension("log.1"),
            "1 hook agent=codex a\n2 hook agent=claude b\n",
        )
        .unwrap();
        std::fs::write(
            home.hooks_log_path(),
            "3 launcher mode=hook agent=codex c\n4 hook agent=codex d\n",
        )
        .unwrap();
        assert_eq!(
            tail_for(&home, "codex", 2),
            vec!["3 launcher mode=hook agent=codex c", "4 hook agent=codex d"]
        );
        assert_eq!(tail_for(&home, "codex", 9).len(), 3);
        assert!(tail_for(&home, "pi", 5).is_empty());
    }

    #[test]
    fn tail_leaves_out_standdown_lines() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().to_path_buf());
        append(
            &home,
            "2026-10-01T00:00:00Z standdown mode=hook agent=claude host=grok",
        );
        append(&home, "2026-10-01T00:00:01Z hook agent=claude event=stop x");
        assert_eq!(
            tail_for(&home, "claude", 5),
            vec!["2026-10-01T00:00:01Z hook agent=claude event=stop x"]
        );
    }
}
