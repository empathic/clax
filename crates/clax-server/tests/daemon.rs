use clax_core::Home;
use clax_server::daemon::{
    DaemonInfo, DaemonLock, ServeConfig, pid_alive, read_daemon_info, serve, write_daemon_info,
};
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

#[test]
fn lock_is_exclusive_and_released_on_drop() {
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
    // copies are closed, so poll with a deadline rather than asserting instantly.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if DaemonLock::try_acquire(&home).unwrap().is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "lock still held 20 s after drop"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

extern "C" fn ignore_signal(_: libc::c_int) {}

#[test]
fn acquire_retries_when_interrupted_by_a_signal() {
    // A handler installed without SA_RESTART makes a blocked flock return EINTR.
    // SAFETY: installs a no-op handler for SIGUSR1, which nothing else in this
    // test binary uses.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = ignore_signal as *const () as libc::sighandler_t;
        sa.sa_flags = 0;
        libc::sigemptyset(&mut sa.sa_mask);
        assert_eq!(libc::sigaction(libc::SIGUSR1, &sa, std::ptr::null_mut()), 0);
    }
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    home.ensure_dirs().unwrap();
    let held = DaemonLock::acquire(&home).unwrap();
    let waiter = {
        let home = home.clone();
        std::thread::spawn(move || DaemonLock::acquire(&home).map(|_| ()))
    };
    use std::os::unix::thread::JoinHandleExt;
    let thread = waiter.as_pthread_t();
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        // SAFETY: the waiter thread has not been joined, so its handle is valid.
        unsafe { libc::pthread_kill(thread, libc::SIGUSR1) };
    }
    drop(held);
    waiter
        .join()
        .unwrap()
        .expect("acquire succeeds after EINTR once the holder releases");
}

#[tokio::test]
async fn serve_picks_a_free_port_writes_info_and_shuts_down_on_request() {
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
        sample: std::sync::Arc::new(clax_server::sample::Sampler::disabled()),
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
    tokio::time::timeout(std::time::Duration::from_secs(20), handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        read_daemon_info(&home).is_none(),
        "daemon.json removed on clean exit"
    );
}

#[tokio::test]
async fn shutdown_completes_while_an_sse_client_stays_connected() {
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
        sample: std::sync::Arc::new(clax_server::sample::Sampler::disabled()),
    };
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    let info = rx.await.unwrap();
    let client = reqwest::Client::new();
    let mut sse = client
        .get(format!("http://127.0.0.1:{}/api/events", info.port))
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
    tokio::time::timeout(std::time::Duration::from_secs(20), handle)
        .await
        .expect("serve exits while SSE client is still connected")
        .unwrap()
        .unwrap();
    drop(sse);
}

#[tokio::test]
async fn serve_exits_when_daemon_json_is_deleted() {
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
        sample: std::sync::Arc::new(clax_server::sample::Sampler::disabled()),
    };
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    rx.await.unwrap();
    std::fs::remove_file(home.daemon_json()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(20), handle)
        .await
        .expect("serve exits once daemon.json stays missing")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn reaper_ends_idle_sessions_whose_process_is_gone() {
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
        sample: std::sync::Arc::new(clax_server::sample::Sampler::disabled()),
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !ended(dead.clone()).await {
        assert!(std::time::Instant::now() < deadline, "reaper never ran");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
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
    let v: clax_server::daemon::DaemonInfo = serde_json::from_str(
        r#"{"port":1,"pid":2,"token":"t","started_at":"s","bind":"127.0.0.1","version":"0.2.0"}"#,
    )
    .unwrap();
    assert_eq!(v.exe, None);
}
