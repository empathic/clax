use clax_core::Home;
use clax_core::daemon::{
    DaemonInfo, DaemonLock, FileLock, ServeLock, ServeRole, pid_alive, port_held, read_daemon_info,
    retry_interrupted, write_daemon_info,
};
use clax_server::serve::{EmbeddedControl, ServeConfig, StopOn, serve};
use clax_testkit::{recv_within, until, within};
use std::net::{IpAddr, Ipv4Addr};

#[test]
fn daemon_info_roundtrips_with_0600() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    home.ensure_dirs().unwrap();
    let info = DaemonInfo {
        port: 1,
        pid: 2,
        token: "t".into(),
        started_at: "now".into(),
        bind: "127.0.0.1".into(),
        version: "v".into(),
        exe: None,
        commit: None,
    };
    write_daemon_info(&home, &info).unwrap();
    assert_eq!(read_daemon_info(&home), Some(info));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(home.daemon_json())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        !home
            .root()
            .join(format!("daemon.json.{}.tmp", std::process::id()))
            .exists(),
        "temp file renamed away"
    );
    std::fs::write(home.daemon_json(), "garbage").unwrap();
    assert_eq!(read_daemon_info(&home), None);
}

#[test]
fn pid_alive_distinguishes_live_and_dead() {
    assert!(pid_alive(std::process::id()));
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(!pid_alive(pid));
}

#[test]
fn pid_alive_rejects_out_of_range_pids() {
    assert!(!pid_alive(0));
    assert!(!pid_alive(u32::MAX));
    assert!(!pid_alive(i32::MAX as u32 + 1));
}

#[tokio::test]
async fn lock_is_exclusive_and_released_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    home.ensure_dirs().unwrap();
    let a = DaemonLock::acquire(&home).unwrap();
    assert!(DaemonLock::try_acquire(&home).unwrap().is_none());
    drop(a);
    // flock locks belong to the open file description, and a child forked by a
    // concurrently running test (pid_alive_* spawns `true`) holds a duplicate of
    // every descriptor until it execs and O_CLOEXEC closes them. The lock is
    // therefore released once this process's descriptor and any such transient
    // copies are closed, so wait for that rather than asserting instantly.
    until("the lock free after its drop", || async {
        DaemonLock::try_acquire(&home).unwrap().map(drop)
    })
    .await;
}

#[test]
fn an_interrupted_lock_wait_is_retried() {
    // `retry_interrupted` retries an `EINTR` from a signal arriving while
    // a call blocks, and returns any other error.
    let mut calls = 0;
    let out = retry_interrupted(|| {
        calls += 1;
        if calls < 3 {
            Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
        } else {
            Ok(calls)
        }
    });
    assert_eq!(out.unwrap(), 3);
    let mut calls = 0;
    let e = retry_interrupted(|| -> std::io::Result<()> {
        calls += 1;
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    })
    .unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(calls, 1);
}

#[tokio::test]
async fn serve_picks_a_free_port_writes_info_and_shuts_down_on_request() {
    let _sigterm = no_sigterm().await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let cfg = ServeConfig {
        home: home.clone(),
        bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: busy_port,
        version: "test",
        stale_check_interval: std::time::Duration::from_secs(30),
        reap_interval: std::time::Duration::from_secs(60),
        // Push off: these daemons never reach the real `codex`.
        codex: Default::default(),
        sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
        web: clax_testkit::web_ui(),
        hosted: None,
        trusted_hosts: Vec::new(),
        tailscale_identity: false,
        require_credential: false,
        inherited: None,
        auto_started: false,
        stop: StopOn::Signals,
        embed: None,
        quickstart: false,
        owner_tailscale_login: None,
        trusted_host_frames: false,
    };
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    let info = rx.await.unwrap();
    assert_ne!(info.port, busy_port);
    assert_eq!(read_daemon_info(&home).unwrap().port, info.port);
    let client = reqwest::Client::new();
    let res = client
        .get(format!("http://127.0.0.1:{}/healthz", info.port))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = client
        .post(format!("http://127.0.0.1:{}/api/admin/shutdown", info.port))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);
    within("serve's return", handle).await.unwrap().unwrap();
    assert!(
        read_daemon_info(&home).is_none(),
        "daemon.json removed on clean exit"
    );
}

fn config_on(home: &Home, port: u16) -> ServeConfig {
    ServeConfig {
        home: home.clone(),
        bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port,
        version: "test",
        stale_check_interval: std::time::Duration::from_secs(30),
        reap_interval: std::time::Duration::from_secs(60),
        codex: Default::default(),
        sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
        web: clax_testkit::web_ui(),
        hosted: None,
        trusted_hosts: Vec::new(),
        tailscale_identity: false,
        require_credential: false,
        inherited: None,
        auto_started: false,
        stop: StopOn::Signals,
        embed: None,
        quickstart: false,
        owner_tailscale_login: None,
        trusted_host_frames: false,
    }
}

#[tokio::test]
async fn serve_serves_the_socket_it_was_handed_and_leaves_it_open() {
    let _sigterm = no_sigterm().await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    // launchd's socket: the daemon gets its own descriptor of it, and the
    // holder keeps it listening once the daemon has stopped.
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let mut cfg = config_on(&home, 0);
    cfg.inherited = Some(held.try_clone().unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    let info = rx.await.unwrap();
    assert_eq!(info.port, port);
    assert_eq!(read_daemon_info(&home).unwrap().port, port);
    let client = reqwest::Client::new();
    let res = client
        .get(format!("http://127.0.0.1:{port}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = client
        .post(format!("http://127.0.0.1:{port}/api/admin/shutdown"))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);
    by(20, "the daemon stops", handle).await.unwrap().unwrap();
    // A connection made after the daemon stopped waits for the next one.
    let _c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    assert!(held.accept().is_ok());
}

#[tokio::test]
async fn serve_refuses_a_handed_socket_off_its_bind_address() {
    let Ok(held) = std::net::TcpListener::bind("[::1]:0") else {
        return; // No IPv6 loopback here.
    };
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let mut cfg = config_on(&home, 0);
    cfg.inherited = Some(held);
    let err = serve(cfg, None).await.unwrap_err();
    assert!(err.to_string().contains("not 127.0.0.1"), "{err}");
    assert!(read_daemon_info(&home).is_none());
}

/// Starts a daemon asked for `port`, returns the port it took, and stops it.
async fn port_taken_for(port: u16) -> u16 {
    let _sigterm = no_sigterm().await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(serve(config_on(&home, port), Some(tx)));
    let info = rx.await.unwrap();
    assert_eq!(read_daemon_info(&home).unwrap().port, info.port);
    let res = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/api/admin/shutdown", info.port))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);
    within("serve's return", handle).await.unwrap().unwrap();
    info.port
}

// Linux only: a wildcard listener can raise the macOS firewall's dialog.
// On macOS the [::1] test below covers the same localhost shadowing without
// a non-loopback listener.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn serve_skips_a_port_a_wildcard_listener_holds() {
    // A 127.0.0.1 bind can succeed beside a wildcard listener (macOS), yet a
    // browser could reach that listener at localhost: the port is held.
    let holder = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = holder.local_addr().unwrap().port();
    assert!(port_held(port));
    let took = port_taken_for(port).await;
    assert_ne!(took, port);
    assert!(took > port && took <= port + 20, "{took}");
    // The holder is left alone.
    let _c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    assert!(holder.accept().is_ok());
}

#[tokio::test]
async fn serve_skips_a_port_held_on_ipv6_loopback_only() {
    let Ok(holder) = std::net::TcpListener::bind("[::1]:0") else {
        return; // No IPv6 loopback here.
    };
    let port = holder.local_addr().unwrap().port();
    assert!(port_held(port));
    let took = port_taken_for(port).await;
    assert_ne!(took, port);
}

#[tokio::test]
async fn serve_fails_when_every_port_it_would_try_is_held() {
    let mut seed = std::process::id();
    let (_held, base) = (0..50)
        .find_map(|_| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let base = 20_000 + (seed % 40_000) as u16;
            let held: Vec<_> = (base..base + 21)
                .map_while(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok())
                .collect();
            (held.len() == 21).then_some((held, base))
        })
        .expect("found no 21 consecutive free loopback ports in 50 tries");
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let e = serve(config_on(&home, base), None).await.unwrap_err();
    assert!(
        format!("{e:#}").contains("in use") || format!("{e:#}").contains("held"),
        "{e:#}"
    );
    assert!(read_daemon_info(&home).is_none());
}

#[tokio::test]
async fn shutdown_completes_while_an_sse_client_stays_connected() {
    let _sigterm = no_sigterm().await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let cfg = ServeConfig {
        home: home.clone(),
        bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        version: "test",
        stale_check_interval: std::time::Duration::from_secs(30),
        reap_interval: std::time::Duration::from_secs(60),
        // Push off: these daemons never reach the real `codex`.
        codex: Default::default(),
        sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
        web: clax_testkit::web_ui(),
        hosted: None,
        trusted_hosts: Vec::new(),
        tailscale_identity: false,
        require_credential: false,
        inherited: None,
        auto_started: false,
        stop: StopOn::Signals,
        embed: None,
        quickstart: false,
        owner_tailscale_login: None,
        trusted_host_frames: false,
    };
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    let info = rx.await.unwrap();
    let client = reqwest::Client::new();
    let mut sse = client
        .get(format!("http://127.0.0.1:{}/api/events", info.port))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap();
    let first = sse.chunk().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("ready"));
    let res = client
        .post(format!("http://127.0.0.1:{}/api/admin/shutdown", info.port))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);
    within("serve exits while SSE client is still connected", handle)
        .await
        .unwrap()
        .unwrap();
    drop(sse);
}

#[tokio::test]
async fn serve_exits_when_daemon_json_is_deleted() {
    let _sigterm = no_sigterm().await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let cfg = ServeConfig {
        home: home.clone(),
        bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        version: "test",
        stale_check_interval: std::time::Duration::from_millis(100),
        reap_interval: std::time::Duration::from_secs(60),
        // Push off: these daemons never reach the real `codex`.
        codex: Default::default(),
        sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
        web: clax_testkit::web_ui(),
        hosted: None,
        trusted_hosts: Vec::new(),
        tailscale_identity: false,
        require_credential: false,
        inherited: None,
        auto_started: false,
        stop: StopOn::Signals,
        embed: None,
        quickstart: false,
        owner_tailscale_login: None,
        trusted_host_frames: false,
    };
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    rx.await.unwrap();
    std::fs::remove_file(home.daemon_json()).unwrap();
    within("serve exits once daemon.json stays missing", handle)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn reaper_ends_idle_sessions_whose_process_is_gone() {
    let _sigterm = no_sigterm().await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let cfg = ServeConfig {
        home: home.clone(),
        bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        version: "test",
        stale_check_interval: std::time::Duration::from_secs(30),
        reap_interval: std::time::Duration::from_millis(100),
        // Push off: these daemons never reach the real `codex`.
        codex: Default::default(),
        sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
        web: clax_testkit::web_ui(),
        hosted: None,
        trusted_hosts: Vec::new(),
        tailscale_identity: false,
        require_credential: false,
        inherited: None,
        auto_started: false,
        stop: StopOn::Signals,
        embed: None,
        quickstart: false,
        owner_tailscale_login: None,
        trusted_host_frames: false,
    };
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    let info = rx.await.unwrap();
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", info.port);
    let register = |pid: u32| {
        let client = client.clone();
        let base = base.clone();
        let token = info.token.clone();
        async move {
            let res = client
                .post(format!("{base}/api/sessions"))
                .bearer_auth(token)
                .json(&serde_json::json!({"harness": "codex", "cwd": "/", "pid": pid}))
                .send()
                .await
                .unwrap();
            res.json::<serde_json::Value>().await.unwrap()["session"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    // Beyond i32::MAX, so never a live process.
    let dead = register(4_000_000_000).await;
    let alive = register(std::process::id()).await;
    let recent = register(4_000_000_001).await;
    // Age two of the rows past the idle limit through a second connection.
    let conn = rusqlite::Connection::open(home.db_path()).unwrap();
    for id in [&dead, &alive] {
        conn.execute(
            "UPDATE sessions SET last_seen_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
            [id],
        )
        .unwrap();
    }
    let ended = |id: String| {
        let client = client.clone();
        let url = format!("{base}/api/sessions/{id}");
        let token = info.token.clone();
        async move {
            client
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()["session"]["ended_at"]
                .is_string()
        }
    };
    // The reaper runs every 100 ms: waited on, however long a run takes.
    until("the reaper ends the dead process's session", || async {
        ended(dead.clone()).await.then_some(())
    })
    .await;
    assert!(!ended(alive).await, "a live process keeps its session");
    assert!(!ended(recent).await, "a recently seen session is kept");
    client
        .post(format!("{base}/api/admin/shutdown"))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap();
    handle.await.unwrap().unwrap();
}

#[test]
fn a_record_without_exe_still_parses() {
    let v: clax_core::daemon::DaemonInfo = serde_json::from_str(
        r#"{"port":1,"pid":2,"token":"t","started_at":"s","bind":"127.0.0.1","version":"0.2.0"}"#,
    )
    .unwrap();
    assert_eq!(v.exe, None);
}

/// A SIGTERM goes to the whole test process. The tests that send one hold
/// this for writing, and every test running a daemon that stops on signals
/// holds it for reading, so the signal never stops another test's daemon.
static SIGTERM: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Holds off the tests that send SIGTERM while the caller's daemon runs.
async fn no_sigterm() -> tokio::sync::RwLockReadGuard<'static, ()> {
    SIGTERM.read().await
}

/// Sends SIGTERM to this process.
fn sigterm_this_process() {
    nix::sys::signal::kill(nix::unistd::Pid::this(), nix::sys::signal::Signal::SIGTERM).unwrap();
}

/// A daemon on a free port that stops only on its control and the shutdown
/// route, and the control's two senders.
struct Embedded {
    stop: tokio::sync::watch::Sender<bool>,
    trim: tokio::sync::mpsc::Sender<std::sync::mpsc::Sender<clax_store::TrimReport>>,
    info: DaemonInfo,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}

async fn embedded(home: &Home) -> Embedded {
    let (d, ready) = start_embedded(config_on(home, 0));
    d.served(ready.await.unwrap())
}

/// An [`Embedded`] daemon that has not answered yet.
struct Pending {
    stop: tokio::sync::watch::Sender<bool>,
    trim: tokio::sync::mpsc::Sender<std::sync::mpsc::Sender<clax_store::TrimReport>>,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Pending {
    fn served(self, info: DaemonInfo) -> Embedded {
        Embedded {
            stop: self.stop,
            trim: self.trim,
            info,
            handle: self.handle,
        }
    }
}

/// Starts the daemon `cfg` describes, stopped by its control in place of
/// `cfg.stop`; the receiver gets its record once it serves.
fn start_embedded(cfg: ServeConfig) -> (Pending, tokio::sync::oneshot::Receiver<DaemonInfo>) {
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let (trim, trim_rx) = tokio::sync::mpsc::channel(1);
    let cfg = ServeConfig {
        stop: StopOn::Embedded(EmbeddedControl {
            stop: stop_rx,
            trim: trim_rx,
        }),
        ..cfg
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    (Pending { stop, trim, handle }, rx)
}

async fn healthz(port: u16) -> reqwest::StatusCode {
    reqwest::get(format!("http://127.0.0.1:{port}/healthz"))
        .await
        .unwrap()
        .status()
}

/// Asks `d` to stop through its control and waits for `serve` to return.
async fn stop_embedded(d: Embedded) {
    d.stop.send(true).unwrap();
    within("serve returns once its control asks it to stop", d.handle)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn an_embedded_daemon_stops_on_its_control_and_removes_daemon_json() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let d = embedded(&home).await;
    assert_eq!(healthz(d.info.port).await, 200);
    assert_eq!(read_daemon_info(&home).unwrap().port, d.info.port);
    stop_embedded(d).await;
    assert!(
        read_daemon_info(&home).is_none(),
        "daemon.json removed on clean exit"
    );
}

#[tokio::test]
async fn a_second_daemon_on_a_home_exits_before_opening_its_database() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let first = embedded(&home).await;
    let (_stop, stop_rx) = tokio::sync::watch::channel(false);
    let (_trim, trim_rx) = tokio::sync::mpsc::channel(1);
    let second = ServeConfig {
        stop: StopOn::Embedded(EmbeddedControl {
            stop: stop_rx,
            trim: trim_rx,
        }),
        ..config_on(&home, 0)
    };
    let e = by(60, "the second daemon gives up", serve(second, None))
        .await
        .expect_err("the second daemon refuses the home");
    let e = format!("{e:#}");
    assert!(e.contains("another clax daemon"), "{e}");
    assert!(e.contains(&format!("pid {}", first.info.pid)), "{e}");
    assert_eq!(
        read_daemon_info(&home).map(|i| i.port),
        Some(first.info.port),
        "daemon.json still names the first"
    );
    assert_eq!(healthz(first.info.port).await, 200);
    stop_embedded(first).await;
}

#[tokio::test]
async fn a_daemon_starts_once_the_one_before_it_has_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let first = embedded(&home).await;
    // The successor starts while its predecessor still serves, as under a
    // supervisor's restart, and waits for it.
    let h = home.clone();
    let next = tokio::spawn(async move { embedded(&h).await });
    lull(500).await;
    assert!(!next.is_finished(), "the successor waits for the lock");
    stop_embedded(first).await;
    let next = by(30, "the successor starts", next).await.unwrap();
    assert_eq!(read_daemon_info(&home).unwrap().port, next.info.port);
    stop_embedded(next).await;
}

/// `fut`'s output within `secs` seconds of real time, or a panic naming
/// `what`: for a bound a test asserts (how soon one daemon hands the home
/// to another), not a hang guard ([`within`]).
#[expect(
    clippy::disallowed_methods,
    reason = "the bound on a handover between two daemons is what the test checks"
)]
async fn by<F: std::future::IntoFuture>(secs: u64, what: &str, fut: F) -> F::Output {
    let bound = std::time::Duration::from_secs(secs);
    tokio::time::timeout(bound, fut)
        .await
        .unwrap_or_else(|_| panic!("{what}: not within {bound:?}"))
}

/// Lets `ms` of real time pass so that something which must NOT happen has
/// had its chance to: there is no condition to wait on for an absence.
#[expect(
    clippy::disallowed_methods,
    reason = "an absence has no condition to wait on: the test lets real time pass and then checks nothing happened"
)]
async fn lull(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

/// The config of a daemon handed `held` as launchd's socket.
fn supervised_on(home: &Home, held: &std::net::TcpListener) -> ServeConfig {
    ServeConfig {
        inherited: Some(held.try_clone().unwrap()),
        ..config_on(home, 0)
    }
}

#[tokio::test]
async fn a_supervised_daemon_takes_the_home_from_an_auto_started_one() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    // A client's stray holds the home first, as when it won the lock while
    // the supervised daemon restarted.
    let (stray, ready) = start_embedded(ServeConfig {
        auto_started: true,
        ..config_on(&home, 0)
    });
    let stray_info = ready.await.unwrap();
    assert_eq!(
        ServeLock::holder(&home).map(|h| (h.role, h.port)),
        Some((ServeRole::AutoStarted, Some(stray_info.port)))
    );
    let Pending {
        stop: _stray_stop,
        trim: _stray_trim,
        handle: stray_handle,
    } = stray;
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let (supervised, ready) = start_embedded(supervised_on(&home, &held));
    let info = by(15, "the supervised daemon serves", ready).await.unwrap();
    assert_eq!(info.port, port);
    by(5, "the stray exits", stray_handle)
        .await
        .unwrap()
        .expect("the stray stopped cleanly");
    assert_eq!(healthz(port).await, 200);
    assert_eq!(read_daemon_info(&home).unwrap().port, port);
    assert_eq!(
        ServeLock::holder(&home).map(|h| (h.role, h.port)),
        Some((ServeRole::Supervised, Some(port)))
    );
    stop_embedded(supervised.served(info)).await;
}

#[tokio::test]
async fn a_supervised_daemon_does_not_take_the_home_from_another() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let (first, ready) = start_embedded(supervised_on(&home, &held));
    let first = first.served(ready.await.unwrap());
    let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (second, _ready) = start_embedded(supervised_on(&home, &other));
    let e = within("the second supervised daemon gives up", second.handle)
        .await
        .unwrap()
        .expect_err("the second supervised daemon refuses the home");
    let e = format!("{e:#}");
    assert!(e.contains("another clax daemon (supervised"), "{e}");
    assert_eq!(healthz(port).await, 200);
    assert_eq!(read_daemon_info(&home).unwrap().port, port);
    stop_embedded(first).await;
}

#[tokio::test]
async fn an_auto_started_daemon_gives_up_at_once_to_a_supervised_one() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let (first, ready) = start_embedded(supervised_on(&home, &held));
    let first = first.served(ready.await.unwrap());
    let (stray, _ready) = start_embedded(ServeConfig {
        auto_started: true,
        ..config_on(&home, 0)
    });
    // Well inside the 10 s a daemon waits out a holder that is draining.
    let e = by(
        5,
        "the stray gives up without waiting out the lock",
        stray.handle,
    )
    .await
    .unwrap()
    .expect_err("the stray refuses the home");
    assert!(format!("{e:#}").contains("supervised"), "{e:#}");
    assert_eq!(healthz(port).await, 200);
    stop_embedded(first).await;
}

#[tokio::test]
async fn a_supervised_daemon_ends_a_stray_whose_lock_has_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    home.ensure_dirs().unwrap();
    // A stray of a build that wrote no record in serve.lock: a `clax
    // serve` by its command line (a script named clax), which daemon.json
    // names on a port where nothing answers, so only SIGTERM ends it. The
    // test holds the empty lock for it and lets go once it has exited.
    let script = dir.path().join("clax");
    std::fs::write(&script, "trap 'exit 0' TERM\nwhile :; do sleep 0.1; done\n").unwrap();
    let mut stray = std::process::Command::new("/bin/sh")
        .arg(&script)
        .args(["serve", "--foreground", "--port", "7480"])
        .spawn()
        .unwrap();
    let dead_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    write_daemon_info(
        &home,
        &DaemonInfo {
            port: dead_port,
            pid: stray.id(),
            token: "t".into(),
            started_at: "now".into(),
            bind: "127.0.0.1".into(),
            version: "0.0.1".into(),
            exe: None,
            commit: None,
        },
    )
    .unwrap();
    let lock = FileLock::try_exclusive(std::fs::File::create(home.serve_lock()).unwrap())
        .unwrap()
        .expect("the stray's lock");
    assert_eq!(ServeLock::holder(&home), None, "no record");
    let exited = tokio::task::spawn_blocking(move || {
        let status = stray.wait().unwrap();
        drop(lock);
        status
    });
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let (supervised, ready) = start_embedded(supervised_on(&home, &held));
    let info = by(15, "the supervised daemon serves", ready).await.unwrap();
    assert_eq!(info.port, port);
    assert!(
        exited.await.unwrap().success(),
        "the stray ended on SIGTERM"
    );
    assert_eq!(read_daemon_info(&home).unwrap().port, port);
    stop_embedded(supervised.served(info)).await;
}

#[tokio::test]
async fn an_embedded_daemon_installs_no_signal_handler() {
    let _only = SIGTERM.write().await;
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let d = embedded(&home).await;
    sigterm_this_process();
    within("the test's own handler hears the SIGTERM", term.recv()).await;
    assert_eq!(
        healthz(d.info.port).await,
        200,
        "the SIGTERM did not stop the daemon"
    );
    assert!(!d.handle.is_finished());
    stop_embedded(d).await;
}

#[tokio::test]
async fn a_signals_daemon_stops_on_sigterm() {
    let _only = SIGTERM.write().await;
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(serve(config_on(&home, 0), Some(tx)));
    let info = rx.await.unwrap();
    assert_eq!(healthz(info.port).await, 200);
    sigterm_this_process();
    within("serve returns on SIGTERM", handle)
        .await
        .unwrap()
        .unwrap();
    assert!(read_daemon_info(&home).is_none());
    within("the test's own handler hears the SIGTERM too", term.recv()).await;
}

#[tokio::test]
async fn an_embedded_daemon_trims_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let d = embedded(&home).await;
    let (tx, rx) = std::sync::mpsc::channel();
    d.trim.send(tx).await.unwrap();
    let report = tokio::task::spawn_blocking(move || recv_within("the daemon's trim report", &rx))
        .await
        .unwrap();
    assert_eq!(report.checkpointed, report.wal_frames, "{report:?}");
    stop_embedded(d).await;
}

/// The tasks alive on this runtime, once those `Core::stop` aborted have
/// been dropped: a bounded number of yields lets the scheduler drop them.
async fn alive_tasks() -> usize {
    let metrics = tokio::runtime::Handle::current().metrics();
    let mut alive = metrics.num_alive_tasks();
    for _ in 0..100 {
        tokio::task::yield_now().await;
        alive = metrics.num_alive_tasks();
    }
    alive
}

#[tokio::test]
async fn a_serve_that_fails_after_its_core_started_stops_the_core() {
    // A started and stopped core leaves no task behind: its background
    // tasks, its listeners and the linked threads' sync all end.
    let dir = tempfile::tempdir().unwrap();
    let before = alive_tasks().await;
    let core = clax_server::core::Core::start(
        clax_server::core::CoreConfig {
            home: Home::at(dir.path().join("stopped")),
            version: "test",
            reap_interval: std::time::Duration::from_secs(60),
            codex: Default::default(),
            sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
            web: clax_testkit::web_ui(),
            hosted: None,
            trusted_hosts: Vec::new(),
            tailscale_identity: false,
            owner_tailscale_login: None,
            trusted_host_frames: false,
            require_credential: false,
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            embed: None,
            quickstart: false,
            catalog_url: None,
        },
        &mut |_| {},
    )
    .await
    .unwrap();
    core.stop().await;
    assert_eq!(alive_tasks().await, before, "a stopped core leaves no task");

    let home = Home::at(dir.path().join("ax"));
    home.ensure_dirs().unwrap();
    // daemon.json is a directory that is not empty, so writing it fails
    // after the core has started.
    std::fs::create_dir_all(home.daemon_json().join("x")).unwrap();
    // It stops on signals: a SIGTERM another test sends while it stops
    // would end this process.
    let _sigterm = no_sigterm().await;
    let served = serve(config_on(&home, 0), None).await;
    assert!(served.is_err(), "daemon.json cannot be written");
    assert_eq!(
        alive_tasks().await,
        before,
        "the failed serve's core leaves no task either"
    );
}

#[tokio::test]
async fn a_stopped_core_leaves_no_task_of_its_linked_sync() {
    // A connected remote with a linked artifact: the sync keeps a task for
    // it (its remote unreachable, the task waits out its backoff), which a
    // stopped core has ended, not only aborted, by the time stop returns.
    use clax_core::Secret;
    use clax_store::remotes::{PendingSignIn, SignedInAs};
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("linked"));
    home.ensure_dirs().unwrap();
    let rid = {
        let st = clax_store::Store::open(&home).unwrap();
        let sign_in = PendingSignIn {
            device_code: Secret::new("cxd_1".into()),
            user_code: "WXYZ-4821".into(),
            verification_url: "http://127.0.0.1:9/device".into(),
            expires_at: "2026-10-12T10:10:00.000Z".into(),
            interval_s: 5,
        };
        let r = st
            .add_pending_remote("http://127.0.0.1:9", "unreachable", &sign_in)
            .unwrap();
        st.connect_remote(
            &r.id,
            &SignedInAs {
                token: Secret::new("cxa_1".into()),
                person_public_id: "p1".into(),
                person_name: None,
                grant_id: "g1".into(),
            },
        )
        .unwrap();
        st.record_remote_push("a1", &r.id, "b1", "http://127.0.0.1:9/a/b1", (1, 1), None)
            .unwrap();
        st.shutdown();
        r.id
    };
    let before = alive_tasks().await;
    let core = clax_server::core::Core::start(
        clax_server::core::CoreConfig {
            home,
            version: "test",
            reap_interval: std::time::Duration::from_secs(60),
            codex: Default::default(),
            sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
            web: clax_testkit::web_ui(),
            hosted: None,
            trusted_hosts: Vec::new(),
            tailscale_identity: false,
            owner_tailscale_login: None,
            trusted_host_frames: false,
            require_credential: false,
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            embed: None,
            quickstart: false,
            catalog_url: None,
        },
        &mut |_| {},
    )
    .await
    .unwrap();
    while !core.state().linked.running(&rid) {
        tokio::task::yield_now().await;
    }
    core.stop().await;
    // Counted at once: every task of the core has ended as stop returns.
    let left = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();
    assert_eq!(left, before, "a task of the stopped core's sync still runs");
}
