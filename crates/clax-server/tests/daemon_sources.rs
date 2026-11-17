//! A scan of the daemon's sources, a test target of its own: it reads
//! the sources of crates `clax-server` does not depend on (`clax-cli`,
//! `clax-commands`, `clax-worker` and others), which Bazel gives this
//! target alone (`test_data_of` in `crates/clax-server/BUILD.bazel`), so
//! an edit to them reruns it and no other test of the crate.

/// The daemon's crates above the run loop's (which `clax-bot`'s own scan
/// reads: `crates/clax-bot/tests/outside.rs`).
const DAEMON_CRATES: [&str; 9] = [
    "clax-bot-js",
    "clax-cli",
    "clax-commands",
    "clax-hooks",
    "clax-http",
    "clax-mcp",
    "clax-realtime",
    "clax-server",
    "clax-worker",
];

fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether `text` holds `word` as a word of its own.
fn names(text: &str, word: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(part)
            && !text[at + word.len()..].chars().next().is_some_and(part)
    })
}

/// The files and directories that are modules of a crate's tests alone:
/// each `mod <name>;` a file declares under `#[cfg(test)]`.
fn test_modules(files: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let dir = path.parent().unwrap();
        let base = if ["lib", "main", "mod"].contains(&stem) {
            dir.to_path_buf()
        } else {
            dir.join(stem)
        };
        let mut lines = text.lines().map(str::trim).peekable();
        while let Some(line) = lines.next() {
            if line != "#[cfg(test)]" {
                continue;
            }
            let declared = lines.peek().and_then(|next| {
                next.rsplit_once("mod ")
                    .and_then(|(_, name)| name.strip_suffix(';'))
            });
            if let Some(name) = declared {
                out.push(base.join(format!("{name}.rs")));
                out.push(base.join(name));
            }
        }
    }
    out
}

/// No source of the daemon names a host's surface: neither the
/// fixture's `desk`, anywhere in a file's code and comments, nor
/// ClaxShell's `shell` as a surface (`clax.shell`, or an operation or a
/// kind of it in a string, `"shell.…"`) in a line of code. Clax's own
/// web UI is called the shell, so the word alone says nothing; a
/// comment may give `shell.d.ts` or `shell.triage` as an example. Tests
/// are left out: a module declared under `#[cfg(test)]`, a file under a
/// `tests` directory, and what follows `#[cfg(test)]` in any file. A
/// surface reaches the daemon from its host's registration and from
/// nowhere else.
#[test]
fn no_source_of_the_daemon_names_a_hosts_surface() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut read = 0;
    for name in DAEMON_CRATES {
        let mut files = Vec::new();
        sources(&crates.join(name).join("src"), &mut files);
        assert!(!files.is_empty(), "{name} has no sources here");
        let tests = test_modules(&files);
        for path in files {
            let shown = path.display().to_string();
            let test = path.components().any(|c| c.as_os_str() == "tests")
                || tests.iter().any(|t| path.starts_with(t));
            if test {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            read += 1;
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            assert!(
                !names(&code.to_lowercase(), "desk"),
                "{shown} names the fixture's surface"
            );
            for (n, line) in code.lines().enumerate() {
                let line = line.trim();
                if line.starts_with("//") {
                    continue;
                }
                let as_surface = line.contains("clax.shell")
                    || line.match_indices("\"shell.").any(|(at, m)| {
                        let rest = &line[at + m.len()..];
                        rest.starts_with(|c: char| c.is_ascii_lowercase())
                            && !rest.starts_with("d.ts")
                            && !rest.starts_with("json")
                    });
                assert!(!as_surface, "{shown}:{}: {line}", n + 1);
            }
        }
    }
    assert!(read > 200, "{read} sources were read");
    assert!(names("a desk.", "desk") && !names("desktop", "desk") && !names("_desk", "desk"));
}
