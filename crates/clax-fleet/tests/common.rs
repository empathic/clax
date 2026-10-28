//! A fleet's processes in this test process: each a [`Fleet`] on a port of
//! its own, over one catalog, with the instances' homes under a directory
//! of its own (or one the test shares).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clax_core::embedded::MemTree;
use clax_core::working::ManualClock;
use clax_fleet::fleet::{Fleet, FleetConfig, Placement, Storage};

/// The peer key every process of a test fleet shares.
pub const PEER_KEY: &str = "test-peer-key";

/// A catalog of the named instances, each at `https://<name>.test` with its
/// sites under `<name>-sites.test` and its setup secret in
/// `CLAX_SETUP_SECRET_<NAME>`, on the shard its entry gives (two shards,
/// and each instance may make its home, when any names one).
pub fn catalog(instances: &[(&str, Option<u32>)]) -> String {
    let mut out = String::new();
    if instances.iter().any(|(_, s)| s.is_some()) {
        out.push_str("shards = 2\n\n");
    }
    for (name, shard) in instances {
        let shard = shard
            .map(|s| format!("shard = {s}\ncreate = true\n"))
            .unwrap_or_default();
        let env = format!(
            "CLAX_SETUP_SECRET_{}",
            name.to_uppercase().replace('-', "_")
        );
        out.push_str(&format!(
            "[[instance]]\nname = \"{name}\"\n{shard}config = '''\n[hosted]\npublic_origin = \"https://{name}.test\"\nsite_domain = \"{name}-sites.test\"\nsetup_secret_env = \"{env}\"\n'''\n\n"
        ));
    }
    out
}

/// One process of a test fleet: its fleet, where it listens, the clock its
/// start backoff is timed by, and the task serving it.
pub struct Process {
    pub fleet: Arc<Fleet>,
    pub addr: SocketAddr,
    pub clock: Arc<ManualClock>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Process {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// A port bound for a process that is told its address before it starts.
pub async fn listener() -> (tokio::net::TcpListener, SocketAddr) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    (l, addr)
}

/// Serves a fleet named `pod` on `listener`, its instances' homes under
/// `data_dir`, its catalog at `catalog`.
pub async fn process(
    pod: &str,
    (listener, addr): (tokio::net::TcpListener, SocketAddr),
    data_dir: &Path,
    catalog: &Path,
    placement: Placement,
) -> Process {
    let clock = Arc::new(ManualClock::at("2026-01-01T00:00:00Z"));
    let fleet = Fleet::new(FleetConfig {
        pod: pod.to_string(),
        advertise: addr.to_string(),
        data_dir: data_dir.to_path_buf(),
        catalog: catalog.to_path_buf(),
        version: "test",
        web: Arc::new(MemTree(Default::default())),
        placement,
        storage: Storage::Disk,
        kek: Some([7; 32]),
        peer_key: PEER_KEY.to_string(),
        db_limits: clax_pagevfs::Limits::default(),
        port: addr.port(),
        clock: clock.clone(),
    })
    .unwrap();
    let app = fleet
        .router()
        .into_make_service_with_connect_info::<clax_http::auth::Conn>();
    let server = tokio::spawn(async move {
        let _ = axum::serve(clax_http::auth::TunedListener(listener), app).await;
    });
    Process {
        fleet,
        addr,
        clock,
        server,
    }
}

/// Writes `text` as the catalog in `dir`.
pub fn write_catalog(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("fleet.toml");
    std::fs::write(&path, text).unwrap();
    path
}

/// `GET http://<addr>/healthz` with `Host: <host>`: its status, and the
/// instance's start time from its body when it answered.
pub async fn healthz(addr: SocketAddr, host: &str) -> (u16, Option<String>) {
    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/healthz"))
        .header("host", host)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Option<serde_json::Value> = resp.json().await.ok();
    let started = body
        .as_ref()
        .and_then(|b| b.get("started_at"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    (status, started)
}
