//! `clax connect` and remote mode (spec §16, §22, M26): the built `clax`
//! binary, in a scratch home, connects to a test daemon over loopback
//! standing for the owner's machine on the LAN, then runs the commands
//! spec §6.7 lists against it, signed, as the person it connected as.
//!
//! Test daemons listen on every interface (they may invite people); their
//! invites name the LAN address of `clax_server::testing::LAN_LOCAL`, which
//! the tests rewrite to the loopback address the daemon also answers on.

use crate::common::TestServer;
use crate::common::people::{self, World};
use clax_core::Home;
use clax_core::remote::{RemoteConfig, save_key};
use clax_core::secret::Secret;
use clax_core::working::ManualClock;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

/// Longest a `clax` run may take.
const LIMIT: Duration = Duration::from_secs(60);

/// The harness environment a test inherits from the agent running it.
const HARNESS_ENV: [&str; 10] = [
    "CLAX_CHANNEL",
    "CLAUDECODE",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX",
    "CODEX_CI",
    "PI_CODING_AGENT",
    "CLAX_REMOTE",
    "CLAX_PORT",
    "CLAUDE_PLUGIN_ROOT",
    "CLAX_SESSION_ID",
];

/// The `clax` binary: `CLAX_TEST_BIN` (an absolute path to a copy kept by
/// content), else this build's.
fn bin() -> PathBuf {
    match std::env::var_os("CLAX_TEST_BIN") {
        Some(b) => PathBuf::from(b),
        None => PathBuf::from(env!("CARGO_BIN_EXE_clax")),
    }
}

/// A scratch home of another machine.
struct Machine {
    dir: tempfile::TempDir,
}

impl Machine {
    fn new() -> Machine {
        Machine {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("ax")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(bin());
        for v in HARNESS_ENV {
            c.env_remove(v);
        }
        c.env("CLAX_HOME", self.home())
            .env("HOME", self.dir.path())
            .env("CLAX_CODEX_BIN", "")
            .env("CLAX_NO_OPEN", "1")
            .env("RUST_LOG", "error")
            .env("NO_COLOR", "1")
            .args(args)
            .stdin(Stdio::null());
        c
    }

    /// Runs `clax args` to its end, off the test's runtime.
    async fn run(&self, args: &[&str]) -> Output {
        let mut c = self.cmd(args);
        let label = args.join(" ");
        tokio::time::timeout(
            LIMIT,
            tokio::task::spawn_blocking(move || c.output().unwrap()),
        )
        .await
        .unwrap_or_else(|_| panic!("clax {label} did not finish"))
        .unwrap()
    }

    /// Runs `clax args`, asserting it succeeds; its stdout.
    async fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args).await;
        assert!(
            out.status.success(),
            "clax {}: {}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn remote_json(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.home().join("remote.json")).unwrap()).unwrap()
    }

    /// Every file under the home, with its bytes.
    fn files(&self) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(p: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                let path = e.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.home(), &mut out);
        out
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The daemon's loopback address.
fn loopback(ts: &TestServer) -> String {
    format!("http://127.0.0.1:{}", ts.addr.port())
}

/// `cmd`'s invite URL, at the daemon's loopback address.
fn invite_url(ts: &TestServer, cmd: &str) -> String {
    let code = cmd.rsplit_once("/i/").expect("an invite command").1;
    format!("{}/i/{code}", loopback(ts))
}

async fn lan_server() -> TestServer {
    TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await
}

/// Names the owner `name`, as their browser does.
async fn name_owner(ts: &TestServer, name: &str) {
    let r = ts
        .authed(ts.client.put(format!("{}/api/viewers/me", ts.base)))
        .json(&json!({"display_name": name}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

/// A machine connected to `ts` by an invite for a new person `name`.
async fn connected(ts: &TestServer, name: &str) -> Machine {
    let m = Machine::new();
    let url = invite_url(ts, &ts.invite_command(name).await);
    m.ok(&[
        "connect",
        &url,
        "--yes",
        "--no-browser",
        "--name",
        "sam-mbp",
    ])
    .await;
    m
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_with_an_invite_writes_remote_json_and_key_0600() {
    let ts = lan_server().await;
    name_owner(&ts, "Alex").await;
    let m = Machine::new();
    let url = invite_url(&ts, &ts.invite_command("Sam").await);
    let out = m
        .ok(&[
            "connect",
            &url,
            "--yes",
            "--no-browser",
            "--name",
            "sam-mbp",
        ])
        .await;
    assert!(out.contains("Connected to Alex's Clax as Sam"), "{out}");
    assert!(out.contains("key SHA256:"), "{out}");
    assert!(out.contains("/connect/b/"), "the browser link: {out}");
    let home = m.home();
    assert_eq!(mode(&home.join("remote.json")), 0o600);
    assert_eq!(mode(&home.join("remote.key")), 0o600);
    assert!(!home.join("remote-pending.json").exists());
    let rj = m.remote_json();
    assert_eq!(rj["url"], loopback(&ts));
    let grant = ts.store.grant(rj["grant_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        (grant.status.as_str(), grant.client_name.as_str()),
        ("approved", "sam-mbp")
    );
    let token = rj["access_token"].as_str().unwrap().to_string();
    assert!(token.starts_with("cxg_"));
    assert!(!out.contains(&token), "the token is never printed");
    // `--yes` recorded Sam's approval of the owner, everywhere.
    let sam = ts.store.viewer_of_grant(&grant.id).unwrap();
    let owner = ts.store.owner().unwrap().unwrap();
    let live = ts.store.live_approvals(Some(&sam.public_id)).unwrap();
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(
        (
            live[0].author.as_str(),
            live[0].created_via.as_str(),
            live[0].scope
        ),
        (
            owner.public_id.as_str(),
            "connect",
            clax_core::store::approvals::ScopeKind::Everywhere
        )
    );
    // Only remote.json holds a token; no file holds the key's bytes but
    // remote.key.
    let key = std::fs::read(home.join("remote.key")).unwrap();
    for (path, bytes) in m.files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name != "remote.json" {
            assert!(
                !String::from_utf8_lossy(&bytes).contains("cx"),
                "{name} holds a token"
            );
        }
        if name != "remote.key" {
            assert!(!bytes.windows(key.len()).any(|w| w == key), "{name}");
        }
    }
    // Connected already: a second connect says so.
    let again = m.run(&["connect", &url, "--yes", "--no-browser"]).await;
    assert!(!again.status.success());
    assert!(
        text(&again).contains("already connected"),
        "{}",
        text(&again)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_without_yes_off_a_terminal_approves_nothing() {
    let ts = lan_server().await;
    let m = Machine::new();
    let url = invite_url(&ts, &ts.invite_command("Sam").await);
    let out = m.ok(&["connect", &url, "--no-browser"]).await;
    assert!(out.contains("as Sam"), "{out}");
    let grant = m.remote_json()["grant_id"].as_str().unwrap().to_string();
    let sam = ts.store.viewer_of_grant(&grant).unwrap();
    assert!(
        ts.store
            .live_approvals(Some(&sam.public_id))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_mode_signs_every_request_and_never_starts_a_daemon() {
    let ts = lan_server().await;
    let r = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Plan", "files": {"index.html": {"content": "<p>plan</p>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(r.status(), 201);
    let m = connected(&ts, "Sam").await;
    let out = m.ok(&["--json", "list"]).await;
    assert!(out.contains("Plan"), "{out}");
    assert!(!m.home().join("daemon.json").exists(), "no daemon here");
    // A token the daemon does not know signs nothing.
    let mut rj = m.remote_json();
    let real = rj["access_token"].clone();
    rj["access_token"] = json!("cxg_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    std::fs::write(m.home().join("remote.json"), rj.to_string()).unwrap();
    let out = m.run(&["list"]).await;
    assert!(!out.status.success());
    assert!(text(&out).contains("invalid_token"), "{}", text(&out));
    rj["access_token"] = real;
    std::fs::write(m.home().join("remote.json"), rj.to_string()).unwrap();
    // A key that is not the grant's: its signatures do not verify.
    let key = std::fs::read(m.home().join("remote.key")).unwrap();
    save_key(&Home::at(m.home()), &clax_core::sig::KeyPair::generate()).unwrap();
    let out = m.run(&["list"]).await;
    assert!(text(&out).contains("invalid_signature"), "{}", text(&out));
    // No readable key: every command says so; nothing falls back.
    std::fs::write(m.home().join("remote.key"), b"").unwrap();
    for args in [["list"].as_slice(), &["comments"], &["status"]] {
        let out = m.run(args).await;
        assert!(!out.status.success(), "{args:?}");
        assert!(
            text(&out).contains("remote_key_unreadable"),
            "{args:?}: {}",
            text(&out)
        );
    }
    std::fs::write(m.home().join("remote.key"), key).unwrap();
    // The daemon down: unreachable, named; still nothing starts here.
    let mut rj = m.remote_json();
    rj["url"] = json!(format!("http://127.0.0.1:{}", free_port()));
    std::fs::write(m.home().join("remote.json"), rj.to_string()).unwrap();
    let out = m.run(&["list"]).await;
    assert!(!out.status.success());
    assert!(text(&out).contains("daemon_unreachable"), "{}", text(&out));
    assert!(!m.home().join("daemon.json").exists(), "no daemon here");
    // CLAX_REMOTE=0 is the local daemon again.
    let out = m.cmd(&["status"]).env("CLAX_REMOTE", "0").output().unwrap();
    assert!(text(&out).contains("not running"), "{}", text(&out));
}

/// A port nothing listens on.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// A connect waiting for the owner, its stdout read line by line.
struct Waiting {
    child: Child,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Waiting {
    fn start(m: &Machine, args: &[&str]) -> Waiting {
        let mut child = m
            .cmd(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, lines) = std::sync::mpsc::channel();
        let out = child.stdout.take().unwrap();
        let err = child.stderr.take().unwrap();
        for r in [
            Box::new(out) as Box<dyn std::io::Read + Send>,
            Box::new(err),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for l in BufReader::new(r).lines().map_while(Result::ok) {
                    let _ = tx.send(l);
                }
            });
        }
        Waiting { child, lines }
    }

    /// The first line holding `what`, within [`LIMIT`].
    async fn line_with(&self, what: &str) -> String {
        let deadline = std::time::Instant::now() + LIMIT;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!left.is_zero(), "no line with {what}");
            match self
                .lines
                .recv_timeout(left.min(Duration::from_millis(200)))
            {
                Ok(l) if l.contains(what) => return l,
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => tokio::task::yield_now().await,
                Err(e) => panic!("no line with {what}: {e}"),
            }
        }
    }

    /// Its exit status, within [`LIMIT`].
    async fn wait(mut self) -> std::process::ExitStatus {
        let deadline = std::time::Instant::now() + LIMIT;
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "connect did not finish"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// The user code in a "approve request WXYZ-4821" line.
fn user_code(line: &str) -> String {
    line.split_whitespace()
        .map(|w| w.trim_end_matches([':', ',', '.']))
        .find(|w| w.len() == 9 && w.as_bytes()[4] == b'-')
        .expect("a user code")
        .to_string()
}

/// The pending grant's ID.
fn pending_grant(ts: &TestServer) -> String {
    ts.store
        .list_grants()
        .unwrap()
        .into_iter()
        .find(|g| g.status == "pending")
        .expect("a pending request")
        .id
}

async fn approve(ts: &TestServer, gid: &str, code: &str, name: &str) {
    let r = ts
        .owner_browser()
        .post(
            &format!("/api/grants/{gid}/approve"),
            json!({"user_code": code, "name": name}),
        )
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
}

/// A daemon on every interface whose clock the test moves (the
/// continuation's `wait` is 5 s of it), set to now so signed times fall
/// within its window.
async fn clocked_server() -> (TestServer, Arc<ManualClock>) {
    let now = chrono::Utc::now().to_rfc3339();
    let clock = Arc::new(ManualClock::at(&now));
    let ts = TestServer::spawn_on_with_clock("0.0.0.0".parse().unwrap(), clock.clone()).await;
    (ts, clock)
}

#[tokio::test(flavor = "multi_thread")]
async fn adhoc_connect_waits_and_completes_on_approval() {
    let (ts, clock) = clocked_server().await;
    let m = Machine::new();
    let url = loopback(&ts);
    let w = Waiting::start(
        &m,
        &[
            "connect",
            &url,
            "--yes",
            "--no-browser",
            "--poll-interval-ms",
            "20",
        ],
    );
    let line = w.line_with("approve request").await;
    let code = user_code(&line);
    assert!(m.home().join("remote-pending.json").exists());
    assert_eq!(mode(&m.home().join("remote-pending.json")), 0o600);
    let gid = pending_grant(&ts);
    approve(&ts, &gid, &code, "Sam").await;
    clock.advance(5);
    w.line_with("as Sam").await;
    assert!(w.wait().await.success());
    assert!(!m.home().join("remote-pending.json").exists());
    assert_eq!(m.remote_json()["grant_id"], gid.as_str());
    assert_eq!(mode(&m.home().join("remote.json")), 0o600);
    let sam = ts.store.viewer_of_grant(&gid).unwrap();
    assert_eq!(
        ts.store.live_approvals(Some(&sam.public_id)).unwrap().len(),
        1
    );
}

/// Task 2's lost continuation: a connect that never finishes would hold
/// its key and its peer's request slot until the request lapses. A killed
/// connect leaves its request in `remote-pending.json`, which the next
/// connect resumes; an interrupted one cancels it; `--off` cancels one
/// left behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_adhoc_connect_resumes_or_is_cancelled() {
    let (ts, clock) = clocked_server().await;
    let m = Machine::new();
    let url = loopback(&ts);
    let args = [
        "connect",
        url.as_str(),
        "--yes",
        "--no-browser",
        "--poll-interval-ms",
        "20",
    ];
    // Killed while waiting: the next run resumes the same request.
    let mut w = Waiting::start(&m, &args);
    let code = user_code(&w.line_with("approve request").await);
    w.child.kill().unwrap();
    assert!(!w.wait().await.success());
    assert!(m.home().join("remote-pending.json").exists());
    let gid = pending_grant(&ts);
    let w = Waiting::start(&m, &args);
    let line = w.line_with("approve request").await;
    assert_eq!(user_code(&line), code, "the same request: {line}");
    approve(&ts, &gid, &code, "Sam").await;
    clock.advance(5);
    assert!(w.wait().await.success());
    assert_eq!(m.remote_json()["grant_id"], gid.as_str());
    m.ok(&["connect", "--off"]).await;
    // Interrupted: the request is cancelled at once, its key free.
    let w = Waiting::start(&m, &args);
    w.line_with("approve request").await;
    let gid = pending_grant(&ts);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(w.child.id() as i32),
        nix::sys::signal::Signal::SIGINT,
    )
    .unwrap();
    assert!(!w.wait().await.success());
    assert!(!m.home().join("remote-pending.json").exists());
    assert!(!m.home().join("remote.key").exists());
    assert_ne!(ts.store.grant(&gid).unwrap().status, "pending");
    // Left behind by a kill: `--off` cancels it.
    let mut w = Waiting::start(&m, &args);
    w.line_with("approve request").await;
    let gid = pending_grant(&ts);
    w.child.kill().unwrap();
    w.wait().await;
    let out = m.ok(&["connect", "--off"]).await;
    assert!(out.contains("cancelled"), "{out}");
    assert_ne!(ts.store.grant(&gid).unwrap().status, "pending");
    assert!(!m.home().join("remote-pending.json").exists());
}

/// Left behind, then connecting with an invite instead: the waiting
/// request is cancelled on its daemon, not only forgotten here. (A daemon
/// of its own: a peer may make three requests in ten minutes.)
#[tokio::test(flavor = "multi_thread")]
async fn connecting_elsewhere_cancels_a_waiting_request() {
    let (ts, _clock) = clocked_server().await;
    let m = Machine::new();
    let url = loopback(&ts);
    let args = [
        "connect",
        url.as_str(),
        "--no-browser",
        "--poll-interval-ms",
        "20",
    ];
    let mut w = Waiting::start(&m, &args);
    w.line_with("approve request").await;
    let gid = pending_grant(&ts);
    w.child.kill().unwrap();
    w.wait().await;
    let url = invite_url(&ts, &ts.invite_command("Sam").await);
    m.ok(&["connect", &url, "--no-browser"]).await;
    assert_ne!(ts.store.grant(&gid).unwrap().status, "pending");
    assert_ne!(m.remote_json()["grant_id"], gid.as_str());
}

/// Connecting is the person's: on an agent channel `clax connect` refuses,
/// as a route for people does, and answers nothing for them; it may still
/// show the connection.
#[tokio::test(flavor = "multi_thread")]
async fn connect_refuses_on_an_agent_channel() {
    let ts = lan_server().await;
    let m = Machine::new();
    let url = invite_url(&ts, &ts.invite_command("Sam").await);
    for args in [
        vec!["connect", url.as_str(), "--yes", "--json"],
        vec!["connect", "--off"],
        vec!["connect", "--url", "http://127.0.0.1:7480"],
    ] {
        let out = m.cmd(&args).env("CLAX_CHANNEL", "agent").output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        assert!(
            text(&out).contains("for people, not agents"),
            "{args:?}: {}",
            text(&out)
        );
    }
    assert!(!m.home().join("remote.json").exists());
    assert!(
        ts.store
            .list_grants()
            .unwrap()
            .iter()
            .all(|g| g.status != "approved"),
        "nothing connected"
    );
    let out = m
        .cmd(&["connect", "--status"])
        .env("CLAX_CHANNEL", "agent")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
}

/// `clax connect`'s own subcommands read remote.json whatever
/// `CLAX_REMOTE` says.
#[tokio::test(flavor = "multi_thread")]
async fn connect_subcommands_see_the_connection_under_clax_remote_0() {
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let out = m
        .cmd(&["connect", "--status"])
        .env("CLAX_REMOTE", "0")
        .output()
        .unwrap();
    assert!(text(&out).contains("connected to"), "{}", text(&out));
    let out = m
        .cmd(&["connect", "--url", &loopback(&ts)])
        .env("CLAX_REMOTE", "0")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("now connecting to"), "{}", text(&out));
}

/// A daemon that answers a grant request with `answer` and records nothing
/// else: a stranger playing the owner's Clax.
fn fake_grant_daemon(answer: (u16, Value)) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            // The head, then the body its content-length names.
            loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let body = answer.1.to_string();
            let out = format!(
                "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                answer.0,
                body.len()
            );
            let _ = stream.write_all(out.as_bytes());
        }
    });
    base
}

/// What a stranger's daemon sends is never opened or printed raw: a
/// browser link that is not the connected daemon's own, a user code
/// carrying terminal escapes.
#[tokio::test(flavor = "multi_thread")]
async fn a_strangers_answers_are_not_opened_or_printed_raw() {
    let issued = json!({
        "access_token": {"value": "cxg_x", "manage": {"access_token": {"value": "cxm_x"}}},
        "continue": {"access_token": {"value": "cxc_x"}, "uri": "/api/grants/g1/continue"},
        "clax": {"person": {"display_name": "Sam"}, "owner": {"display_name": "Alex"},
                 "browser_link": "file:///Applications/Evil.app"},
    });
    let base = fake_grant_daemon((200, issued));
    let m = Machine::new();
    let out = m
        .run(&[
            "connect",
            &format!("{base}/i/K7QF-M2XD-9PLA"),
            "--no-browser",
        ])
        .await;
    assert!(out.status.success(), "{}", text(&out));
    assert!(!text(&out).contains("Evil.app"), "{}", text(&out));
    assert!(
        text(&out).contains("no usable browser link"),
        "{}",
        text(&out)
    );
    let pending = json!({
        "continue": {"access_token": {"value": "cxc_x"}, "uri": "/api/grants/g1/continue", "wait": 5},
        "interact": {"user_code": "\u{1b}]0;owned\u{7}\u{1b}[31mWXYZ-4821"},
    });
    let base = fake_grant_daemon((201, pending));
    let m = Machine::new();
    let out = m.run(&["connect", &base, "--no-browser"]).await;
    assert!(!out.status.success());
    assert!(!text(&out).contains('\u{1b}'), "{:?}", text(&out));
    assert!(text(&out).contains("no usable user code"), "{}", text(&out));
    assert!(!m.home().join("remote-pending.json").exists());
    assert!(!m.home().join("remote.key").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_off_revokes_and_removes_the_files() {
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let gid = m.remote_json()["grant_id"].as_str().unwrap().to_string();
    std::fs::create_dir_all(m.home().join("remote-cache/clips")).unwrap();
    let out = m.ok(&["connect", "--off"]).await;
    assert!(out.contains("Disconnected"), "{out}");
    let g = ts.store.grant(&gid).unwrap();
    assert_eq!(
        (g.status.as_str(), g.ended_by.as_deref()),
        ("revoked", Some("person"))
    );
    for f in ["remote.json", "remote.key", "remote-cache"] {
        assert!(!m.home().join(f).exists(), "{f}");
    }
    let out = m.ok(&["connect", "--status"]).await;
    assert!(out.contains("not connected"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn url_rewrites_the_address_and_keeps_the_grant() {
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let gid = m.remote_json()["grant_id"].clone();
    // The owner's machine got a new address: requests fail as unreachable.
    let mut rj = m.remote_json();
    rj["url"] = json!(format!("http://127.0.0.1:{}", free_port()));
    std::fs::write(m.home().join("remote.json"), rj.to_string()).unwrap();
    let status = m.run(&["connect", "--status"]).await;
    assert!(text(&status).contains("not reachable"), "{}", text(&status));
    let out = m.ok(&["connect", "--url", &loopback(&ts)]).await;
    assert!(out.contains(&loopback(&ts)), "{out}");
    assert_eq!(m.remote_json()["grant_id"], gid);
    assert_eq!(m.remote_json()["url"], loopback(&ts));
    assert_eq!(mode(&m.home().join("remote.json")), 0o600);
    m.ok(&["list"]).await;
    // Only an IP address is taken.
    let out = m
        .run(&["connect", "--url", "http://alex-mbp.local:7480"])
        .await;
    assert!(!out.status.success());
    assert!(text(&out).contains("IP address"), "{}", text(&out));
}

/// Sam's machine, holding the grant the world connected him with.
fn sams_machine(w: &World) -> Machine {
    let m = Machine::new();
    let home = Home::at(m.home());
    let held = w.sam.held();
    RemoteConfig {
        url: loopback(&w.ts),
        grant_id: held.grant_id.clone().unwrap(),
        access_token: Secret::new(held.access.clone().unwrap()),
        manage_token: Secret::new(held.manage.clone().unwrap()),
        continue_token: Secret::new(held.continue_token.clone().unwrap()),
        person_name: Some("Sam".into()),
        person_id: Some(
            w.ts.store
                .viewer_of_grant(&held.grant_id.unwrap())
                .unwrap()
                .public_id,
        ),
        owner_name: Some("Alex".into()),
        own_machine: false,
        client_name: "test-machine".into(),
        connected_at: "2026-10-11T10:00:00Z".into(),
    }
    .save(&home)
    .unwrap();
    save_key(&home, &w.sam.key).unwrap();
    m
}

/// Asserts no canary of Mia's is in `out`.
fn clean(label: &str, out: &str) {
    for c in people::CANARIES {
        assert!(!out.contains(c), "{label} leaked {c}: {out}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_cli_commands_work_as_listed() {
    let w = World::two_people().await;
    let m = sams_machine(&w);
    let aid = w.read.artifact.clone();
    // Sam let the owner's pages reach his agents (as `clax connect --yes`
    // does); the owner publishes one.
    let sam = w.ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap();
    let owner = w.ts.store.owner_viewer(false).unwrap();
    w.ts.store
        .approve(
            &clax_core::audit::AuditCtx::DAEMON,
            Some(&sam.public_id),
            &owner.public_id,
            clax_core::store::approvals::Scope::Everywhere,
            clax_core::store::approvals::Via::Connect,
        )
        .unwrap();
    let r = w
        .ts
        .post_json(
            "/api/artifacts",
            json!({"title": "The owner's plan", "files": {"index.html": {"content": "<p>owner words</p>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(r.status(), 201);
    let mine = r.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut runs: Vec<(String, String)> = Vec::new();
    for args in [
        vec!["list"],
        vec!["--json", "list"],
        vec!["list", "--files", &aid],
        vec!["read", &mine],
        vec!["--json", "read", &mine],
        vec!["--json", "open", &aid],
        vec!["versions", &aid],
        vec!["--json", "versions", &aid],
        vec!["db", "list", &aid, "notes"],
        vec!["comments"],
        vec!["--json", "comments"],
        vec!["comments", &aid, "--all"],
        vec!["--json", "comments", &aid, "--all"],
        vec!["comments", "show", &w.read.thread],
        vec!["comments", "name"],
        vec!["agents"],
        vec!["--json", "agents"],
        vec!["approvals"],
        vec!["--json", "approvals"],
        vec!["status"],
        vec!["--json", "status"],
        vec!["connect", "--status"],
    ] {
        let out = m.ok(&args).await;
        clean(&args.join(" "), &out);
        runs.push((args.join(" "), out));
    }
    let out = |label: &str| runs.iter().find(|(l, _)| l == label).unwrap().1.clone();
    assert!(out(&format!("read {mine}")).contains("owner words"));
    assert!(out("list").contains("The owner's plan"), "{}", out("list"));
    // Mia's own files are withheld from Sam, by name too.
    for path in ["index.html", people::FILE_NAME] {
        let r = m.run(&["read", &aid, "--path", path]).await;
        assert!(!r.status.success());
        assert!(text(&r).contains("withheld"), "{}", text(&r));
        clean("read", &text(&r));
    }
    // `clax comments` with no artifact lists every readable artifact's
    // threads, through each artifact (not GET /api/threads).
    let all: Value = serde_json::from_str(&out("--json comments")).unwrap();
    let listed: Vec<&str> = all["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["artifact_id"].as_str().unwrap())
        .collect();
    assert!(listed.contains(&aid.as_str()), "{listed:?}");
    // The name is the person's, from remote.json.
    assert!(
        out("comments name").contains("Sam"),
        "{}",
        out("comments name")
    );
    // Agents and approvals are Sam's own.
    let agents: Value = serde_json::from_str(&out("--json agents")).unwrap();
    let handles: Vec<&str> = agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["handle"].as_str().unwrap())
        .collect();
    let sams = w.ts.store.get_session(&w.sam_session).unwrap().unwrap();
    assert_eq!(handles, [sams.agent_handle.as_str()]);
    // Other people's agents are not listed, even by name.
    for other in [&w.owner_session, &w.mia_session] {
        let s = w.ts.store.get_session(other).unwrap().unwrap();
        assert!(!out("agents").contains(&s.agent_handle));
    }
    assert!(
        out("agents").contains("waiting on you since"),
        "{}",
        out("agents")
    );
    let approvals: Value = serde_json::from_str(&out("--json approvals")).unwrap();
    assert!(approvals["approvals"].is_array() && approvals["requests"].is_array());
    // Status prints the connection and the agents.
    let status = out("status");
    assert!(
        status.contains("connected to") && status.contains(&loopback(&w.ts)),
        "{status}"
    );
    assert!(status.contains("as Sam"), "{status}");
    let st: Value = serde_json::from_str(&out("--json status")).unwrap();
    assert_eq!(st["remote"]["url"], loopback(&w.ts));
    assert_eq!(st["agents"].as_array().unwrap().len(), 1);
    // The writes the list names act, as Sam, on a thread he may read.
    let r = w.owner_thread_on_mias_page().await;
    for args in [
        vec!["comments", "resolve", &r],
        vec!["comments", "reopen", &r],
        vec!["comments", "send", &r],
    ] {
        let out = m.ok(&args).await;
        clean(&args.join(" "), &out);
    }
    // Nothing was started on Sam's machine.
    assert!(!m.home().join("daemon.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_comment_reply_without_a_session_is_no_session() {
    let w = World::two_people().await;
    let m = sams_machine(&w);
    let out = m
        .run(&[
            "comments",
            "reply",
            &format!("{}#1", w.read.artifact),
            "hello",
        ])
        .await;
    assert!(!out.status.success());
    assert!(text(&out).contains("no_session"), "{}", text(&out));
    clean("comments reply", &text(&out));
}

#[tokio::test(flavor = "multi_thread")]
async fn other_commands_refuse_remotely() {
    let w = World::two_people().await;
    let m = sams_machine(&w);
    let page = m.dir.path().join("p.html");
    std::fs::write(&page, "<title>x</title>").unwrap();
    let aid = w.read.artifact.clone();
    for args in [
        vec!["publish", page.to_str().unwrap()],
        vec!["asset", "upload", &aid, page.to_str().unwrap()],
        vec!["delete", &aid],
        vec!["pin", &aid],
        vec!["unpin", &aid],
        vec!["inbox"],
        vec!["toolpath", "status"],
        vec!["extension", "status"],
        vec!["comments", "name", "Samuel"],
    ] {
        let out = m.run(&args).await;
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        assert!(
            text(&out).contains("not available on a remote Clax"),
            "{args:?}: {}",
            text(&out)
        );
    }
    // Nothing reached the daemon or started here.
    assert!(!m.home().join("daemon.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_checks_the_remote_connection() {
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let checks = |out: &Output| -> Vec<(String, bool)> {
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["name"].as_str().unwrap().to_string(), c["ok"] == true))
            .collect()
    };
    let out = m.run(&["--json", "doctor"]).await;
    assert!(out.status.success(), "{}", text(&out));
    let got = checks(&out);
    for name in [
        "remote_json",
        "remote_key",
        "remote_ip_literal",
        "remote_reachable",
        "remote_clock",
    ] {
        assert!(got.contains(&(name.to_string(), true)), "{name}: {got:?}");
    }
    assert!(
        !got.iter().any(|(n, _)| n == "db_integrity"),
        "no local store: {got:?}"
    );
    assert!(!m.home().join("clax.db").exists());
    // A loose key and a daemon that does not answer fail.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        m.home().join("remote.key"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let mut rj = m.remote_json();
    rj["url"] = json!(format!("http://127.0.0.1:{}", free_port()));
    std::fs::write(m.home().join("remote.json"), rj.to_string()).unwrap();
    let out = m.run(&["--json", "doctor"]).await;
    assert!(!out.status.success());
    let got = checks(&out);
    for name in ["remote_key", "remote_reachable"] {
        assert!(got.contains(&(name.to_string(), false)), "{name}: {got:?}");
    }
}
