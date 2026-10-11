//! The shim's client in remote mode (spec §16, M26): a person's machine
//! registers its session on the owner's daemon and acts there, every
//! request signed with the grant's key (spec §6.6), uploads included.

use clax_core::remote::{Remote, RemoteConfig, Signer};
use clax_core::secret::Secret;
use clax_core::sig::KeyPair;
use clax_mcp::DaemonClient;
use clax_server::testing::{TestClient, TestServer};
use serde_json::json;
use std::sync::Arc;

/// Sam's machine as remote mode sees it: his grant and key, reaching the
/// daemon at its loopback address, reading `clock`.
fn remote_of(ts: &TestServer, sam: &TestClient, clock: Option<i64>) -> Arc<Remote> {
    let held = sam.held();
    let url = format!("http://127.0.0.1:{}", ts.addr.port());
    let key = KeyPair::from_pkcs8(sam.key.pkcs8().expose()).unwrap();
    let mut signer = Signer::new(key, &url).unwrap();
    if let Some(at) = clock {
        signer = signer.with_clock(Arc::new(move || at));
    }
    Arc::new(Remote {
        config: RemoteConfig {
            url,
            grant_id: held.grant_id.clone().unwrap(),
            access_token: Secret::new(held.access.unwrap()),
            manage_token: Secret::new(held.manage.unwrap()),
            continue_token: Secret::new(held.continue_token.unwrap()),
            person_name: Some("Sam".into()),
            person_id: None,
            owner_name: None,
            own_machine: false,
            client_name: "test-machine".into(),
            connected_at: String::new(),
        },
        signer: Arc::new(signer),
    })
}

fn registration() -> clax_core::RegisterSession {
    clax_core::RegisterSession {
        harness: "claude".into(),
        harness_session_id: Some("sam-cc-1".into()),
        cwd: "/work".into(),
        pid: Some(4_000_000_000),
        parent_pid: Some(4_000_000_001),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shim_registers_and_uploads_signed_on_a_remote_daemon() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let client = DaemonClient::managed_remote(remote_of(&ts, &sam, None), registration());
    assert!(client.is_remote());
    client.ensure_session().await.unwrap();
    let session = client.session().unwrap();
    assert_eq!(session.grant_id.as_deref(), Some(sam.grant_id().as_str()));
    assert_eq!(
        session.pid, None,
        "a remote process is never the daemon's to probe"
    );
    let made = client
        .create(&json!({"title": "Sam's notes",
                        "files": {"index.html": {"content": "<p>notes</p>", "encoding": "utf8"}}}))
        .await
        .unwrap();
    let aid = made["artifact"]["id"].as_str().unwrap().to_string();
    // The upload's form is whole bytes, so its signature covers it.
    let up = client
        .upload_asset(&aid, "a \"b\".txt", "text/plain", b"asset bytes".to_vec())
        .await
        .unwrap();
    let asset = up["asset"]["id"].as_str().unwrap().to_string();
    let res = ts
        .authed(ts.client.get(format!("{}/_blob/{asset}", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.bytes().await.unwrap().as_ref(), b"asset bytes");
    assert!(client.list().await.unwrap()["artifacts"].is_array());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_client_retries_a_stale_request_with_the_daemons_time() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    // This machine's clock is ten minutes behind the daemon's.
    let behind = ts.store.now_utc().timestamp() - 600;
    let remote = remote_of(&ts, &sam, Some(behind));
    let client = DaemonClient::with_channel(String::new(), String::new(), None, true)
        .with_remote(remote.clone());
    assert!(client.list().await.unwrap()["artifacts"].is_array());
    assert!(
        (remote.signer.offset() - 600).abs() <= 2,
        "{}",
        remote.signer.offset()
    );
}
