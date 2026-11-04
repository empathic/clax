//! A real `clax serve` stops without waiting out a store call that does not
//! finish: the call is left to the process exit, so the write it would
//! have made is never made.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A foreground daemon on `home`, on a port the OS picks.
struct Daemon {
    /// `None` once a test takes it to wait on.
    child: Option<Child>,
    port: u16,
    token: String,
}

/// A daemon left running when a test fails is killed and reaped.
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(c) = self.child.as_mut() {
            if matches!(c.try_wait(), Ok(None)) {
                let _ = c.kill();
            }
            let _ = c.wait();
        }
    }
}

impl Daemon {
    /// [`Daemon::spawn`] with its stdout and stderr appended to
    /// `dir/daemon.log`.
    fn start(dir: &Path) -> Daemon {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("daemon.log"))
            .unwrap();
        Daemon::spawn(dir, log.try_clone().unwrap().into(), log.into())
    }

    /// A foreground daemon on `dir/ax` with its stdout and stderr as given,
    /// once its daemon.json names it.
    // The child is reaped by `Daemon`'s `Drop`, which the lint cannot see.
    #[allow(clippy::zombie_processes)]
    fn spawn(dir: &Path, stdout: Stdio, stderr: Stdio) -> Daemon {
        let home = dir.join("ax");
        let _ = std::fs::remove_file(home.join("daemon.json"));
        let child = Command::new(crate::common::clax())
            .args(["serve", "--foreground", "--port", "0"])
            .env("CLAX_HOME", &home)
            .env("HOME", dir)
            .env("CLAX_CODEX_BIN", "")
            .env("CLAX_NO_OPEN", "1")
            .env_remove("CLAX_CONFIG_DIR")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(text) = std::fs::read_to_string(home.join("daemon.json"))
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
                && v["pid"] == child.id()
            {
                return Daemon {
                    port: v["port"].as_u64().unwrap() as u16,
                    token: v["token"].as_str().unwrap().to_string(),
                    child: Some(child),
                };
            }
            if Instant::now() >= deadline {
                drop(Daemon {
                    child: Some(child),
                    port: 0,
                    token: String::new(),
                });
                panic!("the daemon did not start");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// `method path` with the token, written on a connection of its own;
    /// the connection, for its response to be read with [`answer`].
    fn send(&self, method: &str, path: &str) -> std::net::TcpStream {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: localhost:{}\r\nAuthorization: Bearer {}\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n",
            self.port, self.token
        )
        .unwrap();
        s
    }
}

/// The status and body of the response on `s`; `None` when the connection
/// ended without one.
fn answer(mut s: std::net::TcpStream) -> Option<(u16, String)> {
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    let status = out.split(' ').nth(1)?.parse().ok()?;
    Some((status, out.split_once("\r\n\r\n")?.1.to_string()))
}

/// What `/api/_test/slow_publish` logs once its store call has begun.
const STARTED: &str = "slow publish started";

#[test]
fn shutdown_leaves_a_store_call_that_does_not_finish_to_the_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let mut d = Daemon::start(tmp.path());
    // A store call that sleeps a minute, then publishes "slow".
    let slow = d.send("POST", "/api/_test/slow_publish/60000");
    // The call is in the store before the shutdown is asked for.
    let log = tmp.path().join("daemon.log");
    let deadline = Instant::now() + Duration::from_secs(120);
    let started = || std::fs::read_to_string(&log).is_ok_and(|t| t.contains(STARTED));
    while !started() {
        assert!(Instant::now() < deadline, "the slow publish never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (code, body) = answer(d.send("POST", "/api/admin/shutdown")).unwrap();
    assert_eq!(code, 202, "{body}");
    // The hang guard only: the exit itself comes from the shutdown's bounds.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut child = d.child.take().unwrap();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });
    let status = rx
        .recv_timeout(Duration::from_secs(300))
        .expect("the daemon did not exit")
        .unwrap();
    assert!(status.success(), "{status}");
    drop(slow);
    drop(d);
    // The call was cut short with the process: nothing was published.
    let again = Daemon::start(tmp.path());
    let (code, body) = answer(again.send("GET", "/api/artifacts")).unwrap();
    assert_eq!(code, 200, "{body}");
    assert!(!body.contains("\"slow\""), "{body}");
}

/// A daemon whose stdout and stderr are gone (the terminal or the pipes
/// that held them closed) goes on answering and stops as asked: a line it
/// can no longer write is dropped, where reporting the failure on the
/// closed stderr would panic the thread that logged it.
#[test]
fn a_daemon_whose_stdout_and_stderr_closed_logs_without_failing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut d = Daemon::spawn(tmp.path(), Stdio::piped(), Stdio::piped());
    let child = d.child.as_mut().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let stderr = child.stderr.take().unwrap();
    // Read up to the line saying it listens, then both pipes closed.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut line = String::new();
        while stdout.read_line(&mut line).is_ok_and(|n| n > 0) {
            if line.contains("clax daemon listening") {
                let _ = tx.send(stdout);
                return;
            }
            line.clear();
        }
    });
    let stdout = rx
        .recv_timeout(Duration::from_secs(120))
        .expect("no listening line (hang guard)");
    drop((stdout, stderr));
    // Ending the owner's browser sessions logs a line at info.
    let (code, body) = answer(d.send("DELETE", "/api/browser-login")).expect("no answer");
    assert_eq!(code, 200, "{body}");
    let (code, body) = answer(d.send("POST", "/api/admin/shutdown")).expect("no answer");
    assert_eq!(code, 202, "{body}");
    // The hang guard only: the exit itself comes from the shutdown's bounds.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut child = d.child.take().unwrap();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });
    let status = rx
        .recv_timeout(Duration::from_secs(300))
        .expect("the daemon did not exit (hang guard)")
        .unwrap();
    assert!(status.success(), "{status}");
}
