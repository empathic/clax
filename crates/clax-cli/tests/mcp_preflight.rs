//! `clax mcp --preflight`: the check the plugins' wrapper runs before it
//! execs `clax mcp`.

use assert_cmd::Command;

fn cmd(dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("clax").unwrap();
    c.env("CLAX_HOME", dir.join("ax"))
        .env("HOME", dir)
        .env_remove("CLAX_PORT");
    c
}

/// A loopback port nothing listens on (the OS's pick, released again).
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// A fake listener holding a loopback port, as another program would.
fn holder() -> (std::net::TcpListener, u16) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

fn home_with_port(dir: &std::path::Path, port: u16) {
    std::fs::create_dir_all(dir.join("ax")).unwrap();
    std::fs::write(
        dir.join("ax/config.toml"),
        format!("[serve]\nport = {port}\n"),
    )
    .unwrap();
}

fn preflight(c: &mut Command) -> std::process::Output {
    c.args(["mcp", "--agent", "claude", "--preflight"])
        .write_stdin("")
        .output()
        .unwrap()
}

/// The one-line reason of a failed preflight.
fn failure(out: &std::process::Output) -> String {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    let err = String::from_utf8(out.stderr.clone()).unwrap();
    assert_eq!(err.lines().count(), 1, "{err}");
    err
}

#[test]
fn preflight_passes_silently_and_starts_no_daemon() {
    let dir = tempfile::tempdir().unwrap();
    home_with_port(dir.path(), free_port());
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

#[test]
fn preflight_names_a_port_another_program_holds_and_leaves_it_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (held, port) = holder();
    home_with_port(dir.path(), port);
    let err = failure(&preflight(&mut cmd(dir.path())));
    let config = dir.path().join("ax/config.toml");
    assert!(
        err.starts_with(&format!("error: port {port} is already in use")),
        "{err}"
    );
    assert!(err.contains("not a Clax daemon for"), "{err}");
    assert!(
        err.contains("under `[serve]` in ") && err.contains(&config.display().to_string()),
        "{err}"
    );
    assert!(err.contains("CLAX_PORT="), "{err}");
    assert!(!dir.path().join("ax/daemon.json").exists());
    // The holder still has its port and accepts connections.
    held.set_nonblocking(false).unwrap();
    let _c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    assert!(held.accept().is_ok());
}

#[test]
fn preflight_checks_a_home_that_does_not_exist_yet_without_creating_it() {
    let dir = tempfile::tempdir().unwrap();
    let (_held, port) = holder();
    let err = failure(&preflight(
        cmd(dir.path()).env("CLAX_PORT", port.to_string()),
    ));
    assert!(err.contains(&format!("port {port} ")), "{err}");
    assert!(!dir.path().join("ax").exists());
}

#[test]
fn clax_port_overrides_the_config_and_the_fix_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let (_held, port) = holder();
    // The config's port is held, but CLAX_PORT names a free one.
    home_with_port(dir.path(), port);
    let out = preflight(cmd(dir.path()).env("CLAX_PORT", free_port().to_string()));
    assert!(out.status.success(), "{out:?}");
    // CLAX_PORT names the held port, over a config whose port is free.
    home_with_port(dir.path(), free_port());
    let err = failure(&preflight(
        cmd(dir.path()).env("CLAX_PORT", port.to_string()),
    ));
    assert!(err.contains(&format!("port {port} ")), "{err}");
    assert!(
        err.contains(&format!("set CLAX_PORT (it is {port} now)")),
        "{err}"
    );
    assert!(!err.contains("[serve]"), "{err}");
    // --port overrides both.
    let mut c = cmd(dir.path());
    c.env("CLAX_PORT", port.to_string())
        .args(["--port", &free_port().to_string()]);
    assert!(preflight(&mut c).status.success());
    // A CLAX_PORT that is not a port is named.
    let err = failure(&preflight(cmd(dir.path()).env("CLAX_PORT", "http")));
    assert!(
        err.contains("CLAX_PORT") && err.contains("not a port"),
        "{err}"
    );
}

#[test]
fn preflight_passes_when_the_holder_is_this_homes_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let (_held, port) = holder();
    home_with_port(dir.path(), port);
    // daemon.json naming a live process: this home's daemon.
    let info = serde_json::json!({
        "port": port, "pid": std::process::id(), "token": "t",
        "started_at": "2026-10-08T00:00:00Z", "bind": "127.0.0.1", "version": "0.0.0"
    });
    std::fs::write(dir.path().join("ax/daemon.json"), info.to_string()).unwrap();
    let out = preflight(&mut cmd(dir.path()));
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn preflight_passes_while_a_daemon_of_this_home_is_starting() {
    let dir = tempfile::tempdir().unwrap();
    let (_held, port) = holder();
    home_with_port(dir.path(), port);
    // The start lock is held while a daemon starts, before daemon.json exists.
    let lock = std::fs::File::create(dir.path().join("ax/daemon.lock")).unwrap();
    lock.lock().unwrap();
    let out = preflight(&mut cmd(dir.path()));
    assert!(out.status.success(), "{out:?}");
}
