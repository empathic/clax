//! `clax mcp --preflight`: the check the plugins' wrapper runs before it
//! execs `clax mcp`.

use assert_cmd::Command;

fn cmd(dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("clax").unwrap();
    c.env("CLAX_HOME", dir.join("ax")).env("HOME", dir);
    c
}

#[test]
fn preflight_passes_silently_and_starts_no_daemon() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("ax")).unwrap();
    std::fs::write(dir.path().join("ax/config.toml"), "[serve]\nport = 7481\n").unwrap();
    let out = cmd(dir.path())
        .args(["mcp", "--agent", "codex", "--preflight"])
        .write_stdin("")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    assert!(!dir.path().join("ax/daemon.json").exists());
}

#[test]
fn preflight_names_a_malformed_config_on_one_line() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("ax")).unwrap();
    std::fs::write(dir.path().join("ax/config.toml"), "[serve\nport = 7481\n").unwrap();
    let out = cmd(dir.path())
        .args(["mcp", "--agent", "claude", "--preflight"])
        .write_stdin("")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    let err = String::from_utf8(out.stderr).unwrap();
    let path = dir.path().join("ax/config.toml");
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(
        err.starts_with(&format!("error: {}", path.display())),
        "{err}"
    );
    assert!(!dir.path().join("ax/daemon.json").exists());
}
