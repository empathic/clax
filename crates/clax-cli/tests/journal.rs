//! The audit journal across a killed daemon (spec
//! 2026-10-06-toolpath-audit-design §7.5, §14): a real `clax serve` killed
//! with SIGKILL while events are being recorded and journalled resumes
//! from its files on the next start, on another port, and its journal is
//! byte for byte the one a daemon writing the same events from scratch
//! makes: no step lost, none twice, and no line differing.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn clax_bin() -> PathBuf {
    std::env::var_os("CLAX_TEST_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| assert_cmd::cargo::cargo_bin("clax"))
}

/// A foreground daemon on `home`, on a port the OS picks.
struct Daemon {
    child: Child,
    port: u16,
    token: String,
}

/// A daemon left running when a test fails is killed and reaped.
impl Drop for Daemon {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Daemon {
    // The child is reaped by `Daemon`'s `Drop`, which the lint cannot see.
    #[allow(clippy::zombie_processes)]
    fn start(dir: &Path) -> Daemon {
        let home = dir.join("ax");
        let _ = std::fs::remove_file(home.join("daemon.json"));
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("daemon.log"))
            .unwrap();
        let child = Command::new(clax_bin())
            .args(["serve", "--foreground", "--port", "0"])
            .env("CLAX_HOME", &home)
            .env("HOME", dir)
            .env("CLAX_CODEX_BIN", "")
            .env("CLAX_NO_OPEN", "1")
            .env_remove("CLAX_CONFIG_DIR")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
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
                    child,
                };
            }
            if Instant::now() >= deadline {
                drop(Daemon {
                    child,
                    port: 0,
                    token: String::new(),
                });
                panic!("the daemon did not start");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// One HTTP request; the status and body. `None` when the daemon is
    /// gone.
    fn request(&self, method: &str, path: &str, body: &str) -> Option<(u16, String)> {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: localhost:{}\r\nAuthorization: Bearer {}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            self.token,
            body.len()
        )
        .ok()?;
        let mut out = String::new();
        s.read_to_string(&mut out).ok()?;
        let status = out.split(' ').nth(1)?.parse().ok()?;
        let body = out.split_once("\r\n\r\n").map(|(_, b)| b.to_string())?;
        Some((status, body))
    }

    fn publish(&self, i: usize) -> bool {
        let body = serde_json::json!({"title": format!("Page {i}"),
            "files": {"index.html": {"content": format!("<h1>{i}</h1>"), "encoding": "utf8"}}});
        matches!(
            self.request("POST", "/api/artifacts", &body.to_string()),
            Some((201, _))
        )
    }

    fn status(&self) -> serde_json::Value {
        let (code, body) = self.request("GET", "/api/toolpath/status", "").unwrap();
        assert_eq!(code, 200, "{body}");
        // A chunked body: the JSON object is its one chunk.
        let start = body.find('{').unwrap();
        let end = body.rfind('}').unwrap();
        serde_json::from_str(&body[start..=end]).unwrap()
    }

    /// Waits until the journal holds every recorded event.
    fn caught_up(&self) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let v = self.status();
            if v["journal"] == true && v["cursor"] == v["newest_seq"] {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "the journal did not catch up: {v}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// SIGTERM, the graceful stop: the journal closes its segment.
    fn stop(mut self) {
        let ok = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let status = self.child.wait().unwrap();
        assert!(status.success(), "{status}");
    }
}

/// Every journal file under `dir`, by path relative to it.
fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().display().to_string();
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_killed_daemon_resumes_its_journal_byte_for_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let journal = tmp.path().join("ax/toolpath/journal");

    // Events recorded and journalled, then a SIGKILL while a writer is
    // still publishing.
    let d = Daemon::start(tmp.path());
    for i in 0..20 {
        assert!(d.publish(i));
    }
    let before = d.caught_up()["newest_seq"].as_i64().unwrap();
    let port = d.port;
    let d = std::sync::Arc::new(d);
    let writer = {
        let d = d.clone();
        std::thread::spawn(move || {
            let mut n = 0;
            while d.publish(100 + n) {
                n += 1;
            }
            n
        })
    };
    // Kill once the journal is moving under the burst.
    let deadline = Instant::now() + Duration::from_secs(60);
    while d.status()["newest_seq"].as_i64().unwrap() < before + 20 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let pid = d.child.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-KILL", &pid])
            .status()
            .unwrap()
            .success()
    );
    let published = writer.join().unwrap();
    assert!(published > 0);
    std::sync::Arc::into_inner(d).unwrap().child.wait().unwrap();

    // What a crash leaves: the open segment may also end mid-line, and lose
    // its unsynced last lines.
    let (rel, bytes) = files(&journal).pop().unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    lines.truncate(lines.len().saturating_sub(4).max(1));
    let cut = lines.join("\n") + "\n{\"Step\":{\"change\":{\"cla";
    std::fs::write(journal.join(&rel), cut).unwrap();

    // The next start resumes the same segment, on another port.
    let d = Daemon::start(tmp.path());
    if d.port == port {
        eprintln!("the OS gave the restarted daemon the same port {port}");
    }
    assert!(d.publish(1000));
    let v = d.caught_up();
    assert_eq!(v["last_error"], serde_json::Value::Null);
    d.stop();
    let resumed = files(&journal);

    // A daemon writing the same events from scratch writes the same bytes,
    // file for file (more than one only if a UTC day began meanwhile).
    std::fs::rename(&journal, tmp.path().join("journal.resumed")).unwrap();
    let d = Daemon::start(tmp.path());
    d.caught_up();
    d.stop();
    let fresh = files(&journal);
    let names = |fs: &[(String, Vec<u8>)]| fs.iter().map(|f| f.0.clone()).collect::<Vec<_>>();
    assert_eq!(names(&resumed), names(&fresh));
    for ((name, a), (_, b)) in resumed.iter().zip(&fresh) {
        if a != b {
            let (a, b) = (String::from_utf8_lossy(a), String::from_utf8_lossy(b));
            let n = a.lines().zip(b.lines()).position(|(x, y)| x != y);
            panic!("{name}: the resumed journal differs from a fresh one at line {n:?}");
        }
    }
    let all: String = resumed
        .iter()
        .map(|f| String::from_utf8_lossy(&f.1).into_owned())
        .collect();
    let steps = all.lines().filter(|l| l.starts_with("{\"Step\":")).count();
    assert!(steps >= 41, "{steps} steps");
    let a = String::from_utf8_lossy(&resumed.last().unwrap().1).into_owned();
    assert!(a.ends_with("{\"PathClose\":{}}\n"));
}
