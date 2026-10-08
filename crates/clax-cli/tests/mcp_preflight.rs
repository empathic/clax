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

/// Listeners holding 21 consecutive loopback ports, every port a daemon
/// asked for the first would try; returns them and the first port.
fn hold_range() -> (Vec<std::net::TcpListener>, u16) {
    let mut seed = std::process::id();
    for _ in 0..50 {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let base = 20_000 + (seed % 40_000) as u16;
        let held: Vec<_> = (base..base + 21)
            .map_while(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok())
            .collect();
        if held.len() == 21 {
            return (held, base);
        }
    }
    panic!("found no 21 consecutive free loopback ports in 50 tries");
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
fn preflight_passes_when_the_daemon_can_move_past_a_held_port() {
    let dir = tempfile::tempdir().unwrap();
    let (held, port) = holder();
    home_with_port(dir.path(), port);
    let out = preflight(&mut cmd(dir.path()));
    assert!(out.status.success(), "{out:?}");
    // The holder still has its port and accepts connections.
    let _c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    assert!(held.accept().is_ok());
}

#[test]
fn preflight_names_a_range_other_programs_hold_and_leaves_them_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (held, port) = hold_range();
    home_with_port(dir.path(), port);
    let err = failure(&preflight(&mut cmd(dir.path())));
    let config = dir.path().join("ax/config.toml");
    assert!(
        err.starts_with(&format!("error: ports {port}-{} are all in use", port + 20)),
        "{err}"
    );
    assert!(err.contains("not a Clax daemon for"), "{err}");
    assert!(
        err.contains("under `[serve]` in ") && err.contains(&config.display().to_string()),
        "{err}"
    );
    assert!(err.contains("CLAX_PORT="), "{err}");
    assert!(!dir.path().join("ax/daemon.json").exists());
    let _c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    assert!(held[0].accept().is_ok());
}

#[test]
fn preflight_fails_on_a_held_port_given_with_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    let (_held, port) = holder();
    std::fs::create_dir_all(dir.path().join("ax")).unwrap();
    let mut c = cmd(dir.path());
    c.args(["--port", &port.to_string()]);
    let err = failure(&preflight(&mut c));
    assert!(
        err.contains(&format!("port {port} is already in use")),
        "{err}"
    );
    assert!(err.contains("pass another port to --port"), "{err}");
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
    let (_held, port) = hold_range();
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
    let (_held, port) = hold_range();
    home_with_port(dir.path(), port);
    // The start lock is held while a daemon starts, before daemon.json exists.
    let lock = std::fs::File::create(dir.path().join("ax/daemon.lock")).unwrap();
    lock.lock().unwrap();
    let out = preflight(&mut cmd(dir.path()));
    assert!(out.status.success(), "{out:?}");
}
