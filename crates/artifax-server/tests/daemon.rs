use artifax_core::Home;
use artifax_server::daemon::{
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
    assert!(DaemonLock::try_acquire(&home).unwrap().is_some());
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
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
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
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
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
    };
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    rx.await.unwrap();
    std::fs::remove_file(home.daemon_json()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), handle)
        .await
        .expect("serve exits once daemon.json stays missing")
        .unwrap()
        .unwrap();
}
