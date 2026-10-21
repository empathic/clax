//! `clax connect` and remote mode (spec §16, §22, M26): the built `clax`
//! binary, in a scratch home, connects to a test daemon over loopback
//! standing for the owner's machine on the LAN, then runs the commands
//! spec §6.7 lists against it, signed, as the person it connected as.
//!
//! Test daemons listen on every interface (they may invite people); their
//! invites name the LAN address of `clax_testkit::LAN_LOCAL`, which
//! the tests rewrite to the loopback address the daemon also answers on.

use clax_core::Home;
use clax_core::remote::{RemoteConfig, save_key};
use clax_core::secret::Secret;
use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use clax_testkit::people::{self, World};
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
        let mut c = Command::new(crate::common::clax());
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
            clax_store::approvals::ScopeKind::Everywhere
        )
    );
    // Only remote.json holds a token; no file holds the key's bytes but
    // remote.key.
    let key = std::fs::read(home.join("remote.key")).unwrap();
    for (path, bytes) in m.files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name != "remote.json" {
            assert!(
                !String::from_utf8_lossy(&bytes).contains(&token),
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
    sams_machine_at(w, &loopback(&w.ts))
}

/// [`sams_machine`] reaching the daemon at `url`.
fn sams_machine_at(w: &World, url: &str) -> Machine {
    let m = Machine::new();
    let home = Home::at(m.home());
    let held = w.sam.held();
    RemoteConfig {
        url: url.to_string(),
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
            clax_store::approvals::Scope::Everywhere,
            clax_store::approvals::Via::Connect,
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

/// Spec §11, M20: a connected person renames their agents in their
/// browser; the CLI prints the People link there and changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn remote_agents_rename_prints_the_people_link() {
    let w = World::two_people().await;
    let m = sams_machine(&w);
    let sams = w.ts.store.get_session(&w.sam_session).unwrap().unwrap();
    let name = sams.name.clone().unwrap();
    for args in [
        vec!["agents", "rename", &name, "sams-task"],
        vec!["agents", "narrow", &name],
        vec!["approvals", "revoke", "a_1"],
    ] {
        let out = m.run(&args).await;
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        assert!(
            text(&out).contains(&format!("{}/people", loopback(&w.ts))),
            "{args:?}: {}",
            text(&out)
        );
    }
    let now = w.ts.store.get_session(&w.sam_session).unwrap().unwrap();
    assert_eq!(now.name, Some(name));
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
        vec!["tag", &aid, "--add", "x"],
        vec!["tags"],
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

impl Machine {
    /// Runs `clax hook --agent <agent> <event>` with `input` on stdin, to
    /// its end, off the test's runtime.
    async fn hook(&self, agent: &str, event: &str, input: &Value) -> Output {
        self.hook_with(agent, event, input, &[], false).await
    }

    /// [`Machine::hook`] with more environment `env`, and through a wrapper
    /// shell when `via_shell` (the hook is then a grandchild of this test,
    /// as when a harness runs its hooks through `sh -c`).
    async fn hook_with(
        &self,
        agent: &str,
        event: &str,
        input: &Value,
        env: &[(&str, &str)],
        via_shell: bool,
    ) -> Output {
        use std::io::Write;
        let mut c = self.cmd(&["hook", "--agent", agent, event]);
        if via_shell {
            let bin = crate::common::clax();
            let mut sh = Command::new("/bin/sh");
            for (k, v) in c.get_envs() {
                match v {
                    Some(v) => sh.env(k, v),
                    None => sh.env_remove(k),
                };
            }
            // Not `exec`: the shell stays the hook's parent.
            sh.args(["-c", "\"$0\" \"$@\"; exit $?"])
                .arg(bin)
                .args(["hook", "--agent", agent, event]);
            c = sh;
        }
        for (k, v) in env {
            c.env(k, v);
        }
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let input = input.to_string();
        let label = format!("hook {agent} {event}");
        tokio::time::timeout(
            LIMIT,
            tokio::task::spawn_blocking(move || {
                let mut child = c.spawn().unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(input.as_bytes())
                    .unwrap();
                child.wait_with_output().unwrap()
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("clax {label} did not finish"))
        .unwrap()
    }

    /// The session credential file `name` under `remote-sessions/`.
    fn credential(&self, name: &str) -> PathBuf {
        self.home().join("remote-sessions").join(name)
    }
}

/// Spec 2026-10-08 §6.8: a remote `SessionStart` hook's join makes the
/// session and keeps its secret in a 0600 file named for the harness
/// session; the later hooks find it there and act as that session; the
/// `SessionEnd` hook ends the session and removes the file.
#[tokio::test(flavor = "multi_thread")]
async fn remote_hooks_find_their_session_by_its_credential_file() {
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let gid = m.remote_json()["grant_id"].as_str().unwrap().to_string();
    let input = json!({"session_id": "cc-1", "cwd": "/w"});
    let out = m.hook("claude", "session-start", &input).await;
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let file = m.credential("claude-s-cc-1.json");
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&m.home().join("remote-sessions")), 0o700);
    let cred: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let sid = cred["session"].as_str().unwrap().to_string();
    let secret = Secret::new(cred["secret"].as_str().unwrap().to_string());
    let s = ts.store.get_session(&sid).unwrap().unwrap();
    assert_eq!(
        (s.grant_id.as_deref(), s.harness_session_id.as_deref()),
        (Some(gid.as_str()), Some("cc-1"))
    );
    assert!(
        ts.store
            .session_secret_matches(&sid, &gid, &secret)
            .unwrap()
    );
    // The Stop hook acts as the session: ending its working records takes
    // the session's proof.
    let out = m.hook("claude", "stop", &input).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    // The tool hook too.
    let out = m.hook("claude", "tool", &input).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let out = m.hook("claude", "session-end", &input).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    assert!(
        ts.store
            .get_session(&sid)
            .unwrap()
            .unwrap()
            .ended_at
            .is_some()
    );
    assert!(!file.exists());
}

/// Spec 2026-10-08 §6.8: a remote hook with no credential for its session,
/// or one it cannot read, fails saying which file, and acts as no session.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_hook_without_its_credential_fails_clearly() {
    use std::os::unix::fs::PermissionsExt;
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let input = json!({"session_id": "cc-2", "cwd": "/w"});
    let out = m.hook("claude", "stop", &input).await;
    let said = text(&out);
    assert!(said.contains("session_credential_missing"), "{said}");
    assert!(said.contains("claude-s-cc-2.json"), "{said}");
    assert!(
        said.contains(&m.home().join("remote-sessions").display().to_string()),
        "{said}"
    );
    // A file that is not a credential names itself.
    let file = m.credential("claude-s-cc-2.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"{\"not\": \"a credential\"}").unwrap();
    let out = m.hook("claude", "stop", &input).await;
    let said = text(&out);
    assert!(said.contains("session_credential_unreadable"), "{said}");
    assert!(said.contains(&file.display().to_string()), "{said}");
    // So does one that cannot be read.
    std::fs::write(&file, b"{}").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    let out = m.hook("claude", "stop", &input).await;
    let said = text(&out);
    assert!(said.contains("session_credential_unreadable"), "{said}");
    assert!(said.contains(&file.display().to_string()), "{said}");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    // A stale credential (its session ended) is refused, not acted on.
    let start = json!({"session_id": "cc-3", "cwd": "/w"});
    let out = m.hook("claude", "session-start", &start).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let cred: Value =
        serde_json::from_slice(&std::fs::read(m.credential("claude-s-cc-3.json")).unwrap())
            .unwrap();
    let sid = cred["session"].as_str().unwrap();
    ts.store
        .end_session(&clax_core::audit::AuditCtx::DAEMON, sid)
        .unwrap();
    let out = m.hook("claude", "stop", &start).await;
    assert!(
        text(&out).contains("invalid_session_proof"),
        "{}",
        text(&out)
    );
    // `clax connect --off` removes every credential.
    m.ok(&["connect", "--off"]).await;
    assert!(!m.home().join("remote-sessions").exists());
}

impl Machine {
    /// The credential file `name` under `remote-sessions/`, as JSON.
    fn credential_json(&self, name: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.credential(name)).unwrap()).unwrap()
    }
}

/// Spec 2026-10-08 §6.8: an agent started from another agent's shell
/// (`claude -p` in agent B's terminal) runs its `SessionStart` hook under
/// the same parent process before its own shim has written anything. It
/// must not find B's parent-PID file and become B: it gets a session of
/// its own, and B's row and files stay B's.
#[tokio::test(flavor = "multi_thread")]
async fn a_nested_agents_hook_never_becomes_its_parents_session() {
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let b_input = json!({"session_id": "b-h", "cwd": "/w"});
    let out = m.hook("claude", "session-start", &b_input).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let b = m.credential_json("claude-s-b-h.json");
    let b_sid = b["session"].as_str().unwrap().to_string();
    let b_row = ts.store.get_session(&b_sid).unwrap().unwrap();
    // B's parent-PID file names this process, the hooks' parent.
    let ppid = m.credential(&format!("claude-p-{}.json", std::process::id()));
    assert!(ppid.exists());
    let c_input = json!({"session_id": "c-h", "cwd": "/w"});
    let out = m.hook("claude", "session-start", &c_input).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let c = m.credential_json("claude-s-c-h.json");
    assert_ne!(c["session"], json!(b_sid), "the nested agent is not B");
    assert_ne!(c["secret"], b["secret"]);
    let after = ts.store.get_session(&b_sid).unwrap().unwrap();
    assert_eq!(
        (after.harness_session_id, after.parent_pid, after.ended_at),
        (b_row.harness_session_id, b_row.parent_pid, None)
    );
    assert_eq!(m.credential_json("claude-s-b-h.json"), b);
    // B's parent-PID file stays B's: another session of the same process
    // never replaces it, and its end never removes it.
    let b_ppid = m.credential_json(&format!("claude-p-{}.json", std::process::id()));
    assert_eq!(b_ppid["session"], json!(b_sid));
    // The nested agent ends: B's session stays live, with its files.
    let out = m.hook("claude", "session-end", &c_input).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    assert_eq!(
        m.credential_json(&format!("claude-p-{}.json", std::process::id()))["session"],
        json!(b_sid)
    );
    assert!(
        ts.store
            .get_session(&b_sid)
            .unwrap()
            .unwrap()
            .ended_at
            .is_none()
    );
}

/// Spec 2026-10-08 §6.8: Codex's shim registers without a harness session
/// ID and keeps its credential under its parent's PID; the Codex hooks,
/// children of that process, find it there and join that session. A file
/// written for an earlier process that had the PID is never taken.
#[tokio::test(flavor = "multi_thread")]
async fn a_codex_hook_finds_its_shims_session_by_its_parent_and_never_by_a_reused_pid() {
    use clax_core::remote::{SessionCredential, SessionFiles};
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let gid = m.remote_json()["grant_id"].as_str().unwrap().to_string();
    let me = std::process::id();
    let shim = |hsid: Option<&str>| clax_core::RegisterSession {
        harness: "codex".into(),
        harness_session_id: hsid.map(str::to_string),
        cwd: "/w".into(),
        parent_pid: Some(me),
        grant_id: Some(gid.clone()),
        ..Default::default()
    };
    let reg = ts
        .store
        .register_session_proved(
            &clax_core::audit::AuditCtx::DAEMON,
            shim(None),
            clax_store::Proved::Unchecked,
        )
        .unwrap();
    let files = SessionFiles::of(&Home::at(m.home()));
    files
        .write(
            "codex",
            None,
            Some(me),
            None,
            &SessionCredential {
                session: reg.session.id.clone(),
                secret: reg.secret.clone().unwrap(),
            },
        )
        .unwrap();
    let input = json!({"session_id": "x-1", "cwd": "/w"});
    // The hook's environment differs from the test's in locale and time
    // zone: the parent's start time it compares does not depend on them.
    let foreign = [
        ("LANG", "de_DE.UTF-8"),
        ("LC_TIME", "de_DE.UTF-8"),
        ("LC_ALL", "de_DE.UTF-8"),
        ("TZ", "Asia/Tokyo"),
    ];
    let out = m
        .hook_with("codex", "session-start", &input, &foreign, false)
        .await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    assert_eq!(
        m.credential_json("codex-s-x-1.json")["session"],
        json!(reg.session.id)
    );
    let row = ts.store.get_session(&reg.session.id).unwrap().unwrap();
    assert_eq!(row.harness_session_id.as_deref(), Some("x-1"));
    // The same parent-PID file, as if written for an earlier process that
    // had this PID: a new hook does not take it.
    ts.store
        .end_session(&clax_core::audit::AuditCtx::DAEMON, &reg.session.id)
        .unwrap();
    let other = ts
        .store
        .register_session_proved(
            &clax_core::audit::AuditCtx::DAEMON,
            shim(None),
            clax_store::Proved::Unchecked,
        )
        .unwrap();
    let ppid = m.credential(&format!("codex-p-{me}.json"));
    let mut v: Value = serde_json::from_slice(&std::fs::read(&ppid).unwrap()).unwrap();
    v["session"] = json!(other.session.id);
    v["secret"] = json!(other.secret.unwrap().expose());
    v["harness_session_id"] = Value::Null;
    v["parent_started_s"] = json!(1);
    std::fs::write(&ppid, v.to_string()).unwrap();
    let input = json!({"session_id": "x-2", "cwd": "/w"});
    let out = m.hook("codex", "stop", &input).await;
    assert!(
        text(&out).contains("session_credential_missing"),
        "{}",
        text(&out)
    );
}

/// Spec 2026-10-08 §6.8: a hook run through a wrapper shell is a grandchild
/// of the harness. It finds the shim's credential under its grandparent's
/// PID, as join matches a row by the hook's ancestors.
#[tokio::test(flavor = "multi_thread")]
async fn a_hook_behind_a_wrapper_shell_finds_its_shims_session_by_an_ancestor() {
    use clax_core::remote::{SessionCredential, SessionFiles};
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let gid = m.remote_json()["grant_id"].as_str().unwrap().to_string();
    let me = std::process::id();
    let reg = ts
        .store
        .register_session_proved(
            &clax_core::audit::AuditCtx::DAEMON,
            clax_core::RegisterSession {
                harness: "codex".into(),
                cwd: "/w".into(),
                parent_pid: Some(me),
                grant_id: Some(gid),
                ..Default::default()
            },
            clax_store::Proved::Unchecked,
        )
        .unwrap();
    SessionFiles::of(&Home::at(m.home()))
        .write(
            "codex",
            None,
            Some(me),
            None,
            &SessionCredential {
                session: reg.session.id.clone(),
                secret: reg.secret.clone().unwrap(),
            },
        )
        .unwrap();
    let input = json!({"session_id": "w-1", "cwd": "/w"});
    let out = m
        .hook_with("codex", "session-start", &input, &[], true)
        .await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    assert_eq!(
        m.credential_json("codex-s-w-1.json")["session"],
        json!(reg.session.id)
    );
    assert_eq!(
        ts.store
            .get_session(&reg.session.id)
            .unwrap()
            .unwrap()
            .harness_session_id
            .as_deref(),
        Some("w-1")
    );
}

/// The questions an `AskUserQuestion` call carries.
fn ask_input(hsid: &str) -> Value {
    json!({"session_id": hsid, "cwd": "/w", "hook_event_name": "PreToolUse",
           "tool_name": "AskUserQuestion", "tool_use_id": "toolu_1",
           "tool_input": {"questions": [{"question": "Which layout?", "header": "Layout",
               "multiSelect": false,
               "options": [{"label": "Grid", "description": "cards"},
                           {"label": "List", "description": "rows"}]}]}})
}

/// Spec §16, 2026-10-06-agent-questions-and-inbox-design §4.4: on a
/// connected machine, Claude Code's `AskUserQuestion` is mirrored into the
/// person's Clax as their agent's question, through the hook's session
/// and its proof; with no Clax surface of theirs open, the terminal
/// dialog is left to ask.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_hooks_ask_mirrors_the_question() {
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let start = json!({"session_id": "cc-ask", "cwd": "/w"});
    let out = m.hook("claude", "session-start", &start).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let sid = m.credential_json("claude-s-cc-ask.json")["session"]
        .as_str()
        .unwrap()
        .to_string();
    let out = m.hook("claude", "ask", &ask_input("cc-ask")).await;
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let log = std::fs::read_to_string(m.home().join("logs/hooks.log")).unwrap();
    let line = log.lines().rfind(|l| l.contains(" ask ")).unwrap();
    assert!(line.contains("outcome=terminal"), "{line}");
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    let asked: String = db
        .query_row(
            "SELECT questions_json FROM questions WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(asked.contains("Which layout?"), "{asked}");
}

/// Spec §16: a remote hook finds its session through the grant's own
/// listing (`GET /api/sessions`), named by its proof: the Stop hook hands
/// over what was sent to it.
#[tokio::test(flavor = "multi_thread")]
async fn remote_hooks_find_their_session() {
    let w = World::two_people().await;
    let m = sams_machine(&w);
    let start = json!({"session_id": "cc-stop", "cwd": "/w", "stop_hook_active": false});
    let out = m.hook("claude", "session-start", &start).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let cred = m.credential_json("claude-s-cc-stop.json");
    let (sid, secret) = (
        cred["session"].as_str().unwrap().to_string(),
        cred["secret"].as_str().unwrap().to_string(),
    );
    // Sam's agent watches a page of the owner's, where Sam's own comment
    // is sent to the agents.
    let aid =
        w.ts.publish("Notes", &[("index.html", "<h2>notes</h2>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string();
    let r = w
        .sam
        .agent_proving(
            "PUT",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            Some(&secret),
            None,
        )
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let sam = w.ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap();
    // Sam let the owner's pages reach his agents, as `clax connect --yes`
    // does, so the clip of the owner's page is not withheld.
    let owner = w.ts.store.owner_viewer(false).unwrap();
    w.ts.store
        .approve(
            &clax_core::audit::AuditCtx::DAEMON,
            Some(&sam.public_id),
            &owner.public_id,
            clax_store::approvals::Scope::Everywhere,
            clax_store::approvals::Via::Connect,
        )
        .unwrap();
    // Sam's comment, with a clip.
    let form = reqwest::multipart::Form::new()
        .text("anchor", clax_testkit::element_anchor().to_string())
        .text("body", "SAMSAYS tighten it")
        .text("version", "1")
        .part(
            "clip",
            reqwest::multipart::Part::bytes(clax_testkit::FAKE_PNG.to_vec())
                .file_name("clip.png")
                .mime_str("image/png")
                .unwrap(),
        );
    let r =
        w.ts.client
            .post(format!("{}/api/artifacts/{aid}/threads", w.ts.base))
            .header(
                "cookie",
                format!("clax_viewer={}", w.ts.browser_cookie(&sam.id)),
            )
            .multipart(form)
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 201);
    let tid = r.json::<Value>().await.unwrap()["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r =
        w.ts.client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/send",
                w.ts.base
            ))
            .header(
                "cookie",
                format!("clax_viewer={}", w.ts.browser_cookie(&sam.id)),
            )
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let out = m.hook("claude", "stop", &start).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(said.contains("SAMSAYS tighten it"), "{said}");
    // Its clip was fetched into this machine's cache, and the payload
    // names that file (spec §16).
    let cached = m
        .home()
        .join("remote-cache/clips")
        .join(format!("{aid}-{tid}.png"));
    let v: Value = serde_json::from_str(said.trim()).unwrap();
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.contains(&format!("\nClip: {}\n", cached.display())),
        "{reason}"
    );
    assert_eq!(std::fs::read(&cached).unwrap(), clax_testkit::FAKE_PNG);
    assert!(!reason.contains("clax-clip:"), "{reason}");
}

/// Spec 2026-10-06-agent-questions-and-inbox-design §4.4 on a connected
/// machine: while Sam has Clax open and his agent has a page, the hook
/// waits for his answer there and hands it to Claude Code.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_hooks_ask_waits_for_the_persons_answer() {
    use clax_testkit::EventReader;
    let w = World::two_people().await;
    let m = sams_machine(&w);
    let start = json!({"session_id": "cc-wait", "cwd": "/w"});
    let out = m.hook("claude", "session-start", &start).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let cred = m.credential_json("claude-s-cc-wait.json");
    let (sid, secret) = (
        cred["session"].as_str().unwrap().to_string(),
        cred["secret"].as_str().unwrap().to_string(),
    );
    let aid =
        w.ts.publish("Layouts", &[("index.html", "<h2>layouts</h2>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string();
    let r = w
        .sam
        .agent_proving(
            "PUT",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            Some(&secret),
            None,
        )
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    // Sam's browser holds his questions.
    let sam = w.ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap();
    let cookie = format!("clax_viewer={}", w.ts.browser_cookie(&sam.id));
    let res =
        w.ts.client
            .get(format!("{}/api/stream", w.ts.base))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
    let mut events = EventReader::from_response(res);
    let stream = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let r =
        w.ts.client
            .post(format!("{}/api/stream/{stream}", w.ts.base))
            .header("cookie", &cookie)
            .json(&json!({"subscribe": ["questions"]}))
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 200);
    let input = ask_input("cc-wait");
    let hook = m.hook("claude", "ask", &input);
    let answer = async {
        let qid = loop {
            let db = rusqlite::Connection::open(w.ts.home.db_path()).unwrap();
            let q: Option<String> = db
                .query_row(
                    "SELECT id FROM questions WHERE session_id = ?1",
                    [&sid],
                    |r| r.get(0),
                )
                .ok();
            if let Some(q) = q {
                break q;
            }
            tokio::task::yield_now().await;
        };
        w.ts.wait_question_waiters(&qid, 1).await;
        let r =
            w.ts.client
                .post(format!("{}/api/questions/{qid}/answer", w.ts.base))
                .header("cookie", &cookie)
                .json(&json!({"answers": [{"selected": ["Grid"]}]}))
                .send()
                .await
                .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    };
    let (out, ()) = tokio::join!(hook, answer);
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(said.contains("\"Which layout?\":\"Grid\""), "{said}");
    drop(events);
}

/// Spec 2026-10-08 §6.8, §16: `clax feedback follow` on a connected
/// machine names its session with the session's proof, found in the
/// session's credential file by the harness session or by the session.
#[tokio::test(flavor = "multi_thread")]
async fn remote_follow_names_its_session_with_its_proof() {
    let w = World::two_people().await;
    let m = sams_machine(&w);
    let start = json!({"session_id": "cc-follow", "cwd": "/w"});
    let out = m.hook("claude", "session-start", &start).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let cred = m.credential_json("claude-s-cc-follow.json");
    let (sid, secret) = (
        cred["session"].as_str().unwrap().to_string(),
        cred["secret"].as_str().unwrap().to_string(),
    );
    let aid =
        w.ts.publish("Notes", &[("index.html", "<h2>notes</h2>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string();
    let r = w
        .sam
        .agent_proving(
            "PUT",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            Some(&secret),
            None,
        )
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let sam = w.ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap();
    let send = |body: &'static str| {
        let (w, aid, cookie) = (&w, aid.clone(), w.ts.browser_cookie(&sam.id));
        async move {
            let tid = w.ts.thread_as(&aid, &cookie, body).await["id"]
                .as_str()
                .unwrap()
                .to_string();
            let r =
                w.ts.client
                    .post(format!(
                        "{}/api/artifacts/{aid}/threads/{tid}/send",
                        w.ts.base
                    ))
                    .header("cookie", format!("clax_viewer={cookie}"))
                    .send()
                    .await
                    .unwrap();
            assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
            tid
        }
    };
    let first = send("first").await;
    let out = m
        .run(&[
            "feedback",
            "follow",
            "--agent",
            "claude",
            "--harness-session",
            "cc-follow",
            "--once",
            "--poll-secs",
            "1",
        ])
        .await;
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains(&first), "{}", text(&out));
    let second = send("second").await;
    let out = m
        .run(&[
            "feedback",
            "follow",
            "--session",
            &sid,
            "--once",
            "--poll-secs",
            "1",
        ])
        .await;
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains(&second), "{}", text(&out));
    // A session this machine holds no credential for is refused at once.
    let out = m
        .run(&[
            "feedback",
            "follow",
            "--session",
            &clax_core::new_ulid(),
            "--once",
        ])
        .await;
    assert!(!out.status.success());
    assert!(
        text(&out).contains("session_credential_missing"),
        "{}",
        text(&out)
    );
}

/// A TCP proxy in front of `upstream` that passes requests through until
/// one asks for a clip (`…/clip HTTP/`): that request is never forwarded,
/// and its connection is held open unanswered. A daemon whose clips never
/// come.
async fn clip_holding_proxy(upstream: String) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((down, _)) = listener.accept().await {
            let upstream = upstream.clone();
            tokio::spawn(async move {
                let Ok(up) = tokio::net::TcpStream::connect(&upstream).await else {
                    return;
                };
                let (mut down_r, mut down_w) = down.into_split();
                let (mut up_r, mut up_w) = up.into_split();
                tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut up_r, &mut down_w).await;
                    // Held open while the request side holds.
                    std::future::pending::<()>().await;
                });
                let mut buf = vec![0u8; 16384];
                loop {
                    let Ok(n) = down_r.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    if String::from_utf8_lossy(&buf[..n]).contains("/clip HTTP/") {
                        std::future::pending::<()>().await;
                    }
                    if up_w.write_all(&buf[..n]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// Spec §16 with the hook's deadline (I1): a clip that does not come in
/// time never costs the comments the hook took. Two comments on threads
/// with clips, through a daemon whose clips never come: the prompt hook
/// prints both within its deadline, each clip `unavailable (timed out)`.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_hook_prints_its_comments_when_clips_do_not_come() {
    let w = World::two_people().await;
    let proxy = clip_holding_proxy(format!("127.0.0.1:{}", w.ts.addr.port())).await;
    let m = sams_machine_at(&w, &proxy);
    let start = json!({"session_id": "cc-slow", "cwd": "/w", "prompt": "go"});
    let out = m.hook("claude", "session-start", &start).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let cred = m.credential_json("claude-s-cc-slow.json");
    let (sid, secret) = (
        cred["session"].as_str().unwrap().to_string(),
        cred["secret"].as_str().unwrap().to_string(),
    );
    let aid =
        w.ts.publish("Notes", &[("index.html", "<h2>notes</h2>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string();
    let r = w
        .sam
        .agent_proving(
            "PUT",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            Some(&secret),
            None,
        )
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let sam = w.ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap();
    let owner = w.ts.store.owner_viewer(false).unwrap();
    w.ts.store
        .approve(
            &clax_core::audit::AuditCtx::DAEMON,
            Some(&sam.public_id),
            &owner.public_id,
            clax_store::approvals::Scope::Everywhere,
            clax_store::approvals::Via::Connect,
        )
        .unwrap();
    for body in ["FIRSTCLIP", "SECONDCLIP"] {
        let form = reqwest::multipart::Form::new()
            .text("anchor", clax_testkit::element_anchor().to_string())
            .text("body", body)
            .text("version", "1")
            .part(
                "clip",
                reqwest::multipart::Part::bytes(clax_testkit::FAKE_PNG.to_vec())
                    .file_name("clip.png")
                    .mime_str("image/png")
                    .unwrap(),
            );
        let r =
            w.ts.client
                .post(format!("{}/api/artifacts/{aid}/threads", w.ts.base))
                .header(
                    "cookie",
                    format!("clax_viewer={}", w.ts.browser_cookie(&sam.id)),
                )
                .multipart(form)
                .send()
                .await
                .unwrap();
        assert_eq!(r.status(), 201);
        let tid = r.json::<Value>().await.unwrap()["thread"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let r =
            w.ts.client
                .post(format!(
                    "{}/api/artifacts/{aid}/threads/{tid}/send",
                    w.ts.base
                ))
                .header(
                    "cookie",
                    format!("clax_viewer={}", w.ts.browser_cookie(&sam.id)),
                )
                .send()
                .await
                .unwrap();
        assert_eq!(r.status(), 200);
    }
    let out = m.hook("claude", "prompt", &start).await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        said.contains("FIRSTCLIP") && said.contains("SECONDCLIP"),
        "{said}"
    );
    assert_eq!(
        said.matches("Clip: unavailable (timed out)").count(),
        2,
        "{said}"
    );
}

/// Starts the hook session `hsid` on Sam's machine `m`, watching a page of
/// the owner's that Sam may read whole; the session, its secret and the
/// page.
async fn sams_watching_session(
    w: &World,
    m: &Machine,
    hsid: &str,
    aid: Option<&str>,
) -> (String, String, String) {
    let out = m
        .hook(
            "claude",
            "session-start",
            &json!({"session_id": hsid, "cwd": "/w"}),
        )
        .await;
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let cred = m.credential_json(&format!("claude-s-{hsid}.json"));
    let (sid, secret) = (
        cred["session"].as_str().unwrap().to_string(),
        cred["secret"].as_str().unwrap().to_string(),
    );
    let aid = match aid {
        Some(a) => a.to_string(),
        None => {
            w.ts.publish("Notes", &[("index.html", "<h2>notes</h2>")])
                .await["artifact"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let r = w
        .sam
        .agent_proving(
            "PUT",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            Some(&secret),
            None,
        )
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    (sid, secret, aid)
}

/// Sam's own comment on `aid`, sent to his agents; the thread.
async fn sam_sends(w: &World, aid: &str, body: &str) -> String {
    let sam = w.ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap();
    let tid =
        w.ts.thread_as(aid, &w.ts.browser_cookie(&sam.id), body)
            .await["id"]
            .as_str()
            .unwrap()
            .to_string();
    let r =
        w.ts.client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/send",
                w.ts.base
            ))
            .header(
                "cookie",
                format!("clax_viewer={}", w.ts.browser_cookie(&sam.id)),
            )
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    tid
}

/// Spec 2026-10-08 §6.8: a follower whose session ended holds a credential
/// that proves nothing (`invalid_session_proof`): the session is gone, and
/// a follower of that Clax session exits 0.
#[tokio::test(flavor = "multi_thread")]
async fn remote_follow_of_an_ended_session_exits() {
    let w = World::two_people().await;
    let m = sams_machine(&w);
    let (sid, _, _) = sams_watching_session(&w, &m, "cc-gone", None).await;
    w.ts.store
        .end_session(&clax_core::audit::AuditCtx::DAEMON, &sid)
        .unwrap();
    let out = m
        .run(&["feedback", "follow", "--session", &sid, "--poll-secs", "1"])
        .await;
    assert!(out.status.success(), "{}", text(&out));
}

/// A follower of a harness session follows it across a restart: its
/// session ends (its credential then proves nothing) and the harness
/// session registers again, with a new credential file; the follower takes
/// the new one and prints the comment sent to the new session.
#[tokio::test(flavor = "multi_thread")]
async fn remote_follow_follows_its_harness_session_across_a_restart() {
    let w = World::two_people().await;
    let m = sams_machine(&w);
    let (first, _, aid) = sams_watching_session(&w, &m, "cc-again", None).await;
    let mut c = m.cmd(&[
        "feedback",
        "follow",
        "--agent",
        "claude",
        "--harness-session",
        "cc-again",
        "--once",
        "--poll-secs",
        "1",
        "--grace-secs",
        "60",
    ]);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = c.spawn().unwrap();
    let pid = child.id();
    let waited = tokio::task::spawn_blocking(move || child.wait_with_output());
    w.ts.store
        .end_session(&clax_core::audit::AuditCtx::DAEMON, &first)
        .unwrap();
    let (second, _, _) = sams_watching_session(&w, &m, "cc-again", Some(&aid)).await;
    assert_ne!(second, first);
    let tid = sam_sends(&w, &aid, "after the restart").await;
    let out = match tokio::time::timeout(LIMIT, waited).await {
        Ok(out) => out.unwrap().unwrap(),
        Err(_) => {
            let _ = Command::new("kill").arg(pid.to_string()).status();
            panic!("the follower did not print the comment sent after the restart");
        }
    };
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains(&tid), "{}", text(&out));
}

/// `clax mcp --agent claude` on machine `m` for harness session `hsid`,
/// logging at `info`, with stderr closed at once when `close_stderr`;
/// returns once it has answered `initialize` and a `status` call.
struct Shim {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Shim {
    async fn start(m: &Machine, hsid: &str, close_stderr: bool) -> Shim {
        use std::io::Write;
        let mut child = m
            .cmd(&["mcp", "--agent", "claude"])
            .env("RUST_LOG", "info")
            .env("CLAUDE_CODE_SESSION_ID", hsid)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if close_stderr {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .spawn()
            .unwrap();
        drop(child.stderr.take());
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let job = tokio::task::spawn_blocking(move || {
            let mut line = String::new();
            for (id, msg) in [
                (
                    Some(0),
                    json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
                        "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": {"name": "test", "version": "0"}}}),
                ),
                (
                    None,
                    json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                ),
                (
                    Some(1),
                    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                           "params": {"name": "status", "arguments": {}}}),
                ),
            ] {
                writeln!(stdin, "{msg}").unwrap();
                if let Some(id) = id {
                    line.clear();
                    stdout.read_line(&mut line).unwrap();
                    assert!(line.contains(&format!("\"id\":{id}")), "{line}");
                }
            }
            (stdin, stdout)
        });
        let (stdin, stdout) = tokio::time::timeout(LIMIT, job)
            .await
            .expect("the shim did not answer")
            .unwrap();
        Shim {
            child,
            stdin,
            stdout,
        }
    }

    /// Closes its stdin, as a harness that exits does; its exit status.
    async fn close(self) -> std::process::ExitStatus {
        use std::io::Read;
        let Shim {
            mut child,
            stdin,
            mut stdout,
        } = self;
        let job = tokio::task::spawn_blocking(move || {
            drop(stdin);
            let mut rest = String::new();
            let _ = stdout.read_to_string(&mut rest);
            child.wait().unwrap()
        });
        tokio::time::timeout(LIMIT, job)
            .await
            .expect("the shim did not exit")
            .unwrap()
    }
}

/// Every session of grant `gid`, live or ended.
fn grant_sessions(ts: &TestServer, gid: &str) -> Vec<clax_core::model::Session> {
    ts.store
        .list_sessions(false)
        .unwrap()
        .into_iter()
        .filter(|s| s.grant_id.as_deref() == Some(gid))
        .collect()
}

/// The shim logs to stderr; a harness that goes away first leaves that pipe
/// closed. Logging into it never panics the shim: it serves, and when its
/// stdin closes it ends its remote session (which has no PID for the
/// daemon to probe) and removes the session's credential file.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_shim_whose_stderr_closed_still_ends_its_session() {
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let gid = m.remote_json()["grant_id"].as_str().unwrap().to_string();
    let shim = Shim::start(&m, "cc-gone", true).await;
    let status = shim.close().await;
    assert!(status.success(), "{status:?}");
    let sessions = grant_sessions(&ts, &gid);
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert!(sessions[0].ended_at.is_some(), "{sessions:?}");
    assert!(!m.credential("claude-s-cc-gone.json").exists());
}

/// A shim whose session already ended (the `SessionEnd` hook ran first, or
/// the person ended it in People) has nothing to end at exit: it registers
/// no new session to end, and leaves no credential file behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_shim_ending_after_its_session_ended_makes_no_session() {
    let ts = lan_server().await;
    let m = connected(&ts, "Sam").await;
    let gid = m.remote_json()["grant_id"].as_str().unwrap().to_string();
    let shim = Shim::start(&m, "cc-ended", false).await;
    let out = m
        .hook(
            "claude",
            "session-end",
            &json!({"session_id": "cc-ended", "cwd": "/w"}),
        )
        .await;
    assert!(out.status.success(), "{}", text(&out));
    assert!(shim.close().await.success());
    let sessions = grant_sessions(&ts, &gid);
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert!(sessions[0].ended_at.is_some(), "{sessions:?}");
    let left: Vec<_> = std::fs::read_dir(m.home().join("remote-sessions"))
        .map(|d| d.map(|e| e.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
}
