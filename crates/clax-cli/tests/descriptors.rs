//! A daemon closes the descriptors it inherits. A test binary apart from
//! `tests/integration.rs`, its tests holding [`FORKS`] while they run: the
//! check reads whether any other copy of a socket is still open, and under
//! `cargo test`, where the tests of one binary are threads of one process,
//! another test's fork could catch a copy of it (see [`other_ends_closed`]).

mod common;
use common::Env;

/// Held by each test for as long as it relies on no other fork in this
/// process.
static FORKS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Whether every other end of `ours` is closed now: a read that does not
/// wait finds EOF, where an end still open somewhere would leave nothing to
/// read yet. Asked once the closing must already have happened, so the
/// answer does not depend on how fast the machine is.
///
/// It relies on no other fork in this process meanwhile: under nextest each
/// test is a process of its own, and under `cargo test` these tests hold
/// [`FORKS`]. Otherwise another test's fork can catch a copy of the pair
/// before std marks it close-on-exec (macOS has no `socketpair` flag for
/// that), and a child still running then would read as an end left open.
fn other_ends_closed(ours: &std::os::unix::net::UnixStream) -> bool {
    use std::io::Read;
    ours.set_nonblocking(true).unwrap();
    let mut buf = [0u8; 64];
    loop {
        match (&*ours).read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
}

/// A command running `clax` with `theirs` as an extra inherited descriptor
/// (9): the state a concurrent fork catches a std pipe in on platforms
/// without `pipe2`. A shell takes `theirs` as its stderr and moves it to
/// descriptor 9 as it execs `clax` (stderr goes to `/dev/null`), so only this
/// child inherits it and other tests' children cannot hold it. It is one end
/// of a socket pair rather than a pipe so the test can read the other end
/// without waiting ([`other_ends_closed`]); the daemon closes either alike.
fn clax_inheriting(theirs: std::os::unix::net::UnixStream) -> std::process::Command {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.args(["-c", r#"exec "$0" "$@" 9>&2 2>/dev/null"#])
        .arg(assert_cmd::cargo::cargo_bin("clax"))
        .stderr(std::os::fd::OwnedFd::from(theirs));
    cmd
}

#[test]
fn an_auto_started_daemon_does_not_hold_inherited_descriptors() {
    let _forks = FORKS.lock().unwrap_or_else(|e| e.into_inner());
    let e = Env::new();
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut cmd = clax_inheriting(theirs);
    cmd.env("CLAX_HOME", e.dir.path().join("ax"))
        .env("CLAX_CODEX_BIN", "")
        .env("HOME", e.dir.path())
        .args(["status", "--start", "--json", "--port", "0"])
        .stdin(std::process::Stdio::null());
    let out = cmd.output().unwrap();
    // The command has been dropped with this process's copy of the other end.
    drop(cmd);
    assert!(out.status.success(), "{out:?}");
    // `status --start` returns once the daemon answers, and the daemon closes
    // what it inherited before it serves: the client has exited, so no one
    // holds the other end any more.
    let closed = other_ends_closed(&ours);
    e.stop();
    assert!(closed, "the daemon kept an inherited descriptor open");
}

#[test]
fn a_foreground_daemon_closes_inherited_descriptors() {
    use std::io::BufRead;
    let _forks = FORKS.lock().unwrap_or_else(|e| e.into_inner());
    let e = Env::new();
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut cmd = clax_inheriting(theirs);
    cmd.env("CLAX_HOME", e.dir.path().join("ax"))
        .env("CLAX_CODEX_BIN", "")
        .env("HOME", e.dir.path())
        .args(["serve", "--foreground", "--port", "0"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    // Drops this process's copy of the other end.
    drop(cmd);
    // The daemon closes inherited descriptors before anything else, then logs
    // to stdout, and goes on logging after it has written daemon.json. Its log
    // is read a line at a time until daemon.json names it, or to its end if it
    // exits first, so the wait is on the daemon, not on a clock. A watchdog
    // kills it only if it neither does so nor exits within HANG_GUARD, which
    // ends the log too, so a daemon that never serves fails the test rather
    // than hanging it.
    const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(90);
    let home = e.dir.path().join("ax");
    let pid = child.id();
    let names_child = || {
        std::fs::read_to_string(home.join("daemon.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["pid"].as_u64())
            == Some(u64::from(pid))
    };
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let guard = std::thread::spawn(move || {
        if done_rx.recv_timeout(HANG_GUARD) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            use nix::sys::signal::{Signal, kill};
            let _ = kill(nix::unistd::Pid::from_raw(pid as i32), Signal::SIGKILL);
            return true;
        }
        false
    });
    let mut log = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    let mut seen = String::new();
    let ready = loop {
        line.clear();
        if log.read_line(&mut line).unwrap() == 0 {
            break false;
        }
        seen.push_str(&line);
        if names_child() {
            break true;
        }
    };
    let closed = other_ends_closed(&ours);
    let alive = child.try_wait().unwrap().is_none();
    // Stops the watchdog before the daemon is killed and reaped, so it can
    // never signal a reused pid.
    drop(done_tx);
    let _ = child.kill();
    let _ = child.wait();
    let hung = guard.join().unwrap();
    assert!(
        !hung,
        "the daemon neither wrote daemon.json naming it nor exited within {HANG_GUARD:?}; its log:\n{seen}"
    );
    assert!(
        ready,
        "the daemon ended before daemon.json named it; its log:\n{seen}"
    );
    assert!(alive, "the daemon kept running after closing descriptors");
    assert!(closed, "the daemon kept an inherited descriptor open");
}
