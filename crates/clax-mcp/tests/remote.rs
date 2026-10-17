//! The shim's client in remote mode (spec §16, M26): a person's machine
//! registers its session on the owner's daemon and acts there, every
//! request signed with the grant's key (spec §6.6), uploads included.

use clax_core::remote::{Remote, RemoteConfig, SessionFiles, Signer};
use clax_core::secret::Secret;
use clax_core::sig::KeyPair;
use clax_mcp::DaemonClient;
use clax_testkit::{TestClient, TestServer};
use serde_json::json;
use std::sync::Arc;

/// Sam's machine as remote mode sees it: his grant and key, reaching the
/// daemon at its loopback address, reading `clock`, keeping its sessions'
/// credential files in `files`.
fn remote_at(
    ts: &TestServer,
    sam: &TestClient,
    clock: Option<i64>,
    files: SessionFiles,
) -> Arc<Remote> {
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
        sessions: files,
    })
}

/// [`remote_at`] with its credential files in `dir`.
fn remote_of(
    ts: &TestServer,
    sam: &TestClient,
    clock: Option<i64>,
    dir: &tempfile::TempDir,
) -> Arc<Remote> {
    remote_at(
        ts,
        sam,
        clock,
        SessionFiles::at(dir.path().join("remote-sessions")),
    )
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
    let dir = tempfile::tempdir().unwrap();
    let client = DaemonClient::managed_remote(remote_of(&ts, &sam, None, &dir), registration());
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

/// The shim signs the agent header values it sends trimmed, as the daemon
/// reads them: a padded value still verifies.
#[tokio::test(flavor = "multi_thread")]
async fn the_shim_signs_agent_headers_as_the_daemon_reads_them() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let dir = tempfile::tempdir().unwrap();
    let client = DaemonClient::with_channel(String::new(), String::new(), None, true)
        .with_remote(remote_of(&ts, &sam, None, &dir))
        .with_via(" cli\t");
    assert!(client.list().await.unwrap()["artifacts"].is_array());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_client_retries_a_stale_request_with_the_daemons_time() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let dir = tempfile::tempdir().unwrap();
    // This machine's clock is ten minutes behind the daemon's.
    let behind = ts.store.now_utc().timestamp() - 600;
    let remote = remote_of(&ts, &sam, Some(behind), &dir);
    let client = DaemonClient::with_channel(String::new(), String::new(), None, true)
        .with_remote(remote.clone());
    assert!(client.list().await.unwrap()["artifacts"].is_array());
    assert!(
        (remote.signer.offset() - 600).abs() <= 2,
        "{}",
        remote.signer.offset()
    );
}

/// The credential file a session of `harness` with harness session ID
/// `hsid` is kept in, as JSON.
fn credential_file(
    dir: &tempfile::TempDir,
    harness: &str,
    hsid: &str,
) -> Option<serde_json::Value> {
    let path = dir
        .path()
        .join("remote-sessions")
        .join(format!("{harness}-s-{hsid}.json"));
    let bytes = std::fs::read(path).ok()?;
    Some(serde_json::from_slice(&bytes).unwrap())
}

/// Spec 2026-10-08 §6.8: the shim keeps the secret its registration was
/// answered and writes it, 0600, for the hooks; a shim restarted in the
/// same harness session proves the session from the file and keeps it; a
/// sibling without it cannot take the session over; ending the session
/// removes the file.
#[tokio::test(flavor = "multi_thread")]
async fn the_shim_keeps_its_session_secret_and_its_credential_file() {
    use std::os::unix::fs::PermissionsExt;
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let dir = tempfile::tempdir().unwrap();
    let client = DaemonClient::managed_remote(remote_of(&ts, &sam, None, &dir), registration());
    client.ensure_session().await.unwrap();
    let sid = client.session().unwrap().id;
    let file = credential_file(&dir, "claude", "sam-cc-1").expect("the credential file");
    assert_eq!(file["session"], json!(sid));
    let secret = file["secret"].as_str().unwrap().to_string();
    assert!(
        ts.store
            .session_secret_matches(&sid, &sam.grant_id(), &Secret::new(secret.clone()))
            .unwrap()
    );
    let path = dir.path().join("remote-sessions/claude-s-sam-cc-1.json");
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&dir.path().join("remote-sessions")), 0o700);
    // Its requests prove the session: a publish is credited to it.
    let made = client
        .create(&json!({"title": "Sam's notes",
                        "files": {"index.html": {"content": "<p>notes</p>", "encoding": "utf8"}}}))
        .await
        .unwrap();
    let aid = made["artifact"]["id"].as_str().unwrap();
    let a = ts
        .store
        .get_artifact(&clax_core::ArtifactId::parse(aid).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(a.owner_session_id.as_deref(), Some(sid.as_str()));
    // A sibling registering with the same harness session ID, without the
    // file, cannot take the session over.
    let other = tempfile::tempdir().unwrap();
    let sibling = DaemonClient::managed_remote(remote_of(&ts, &sam, None, &other), registration());
    let e = sibling.ensure_session().await.unwrap_err();
    assert!(e.to_string().contains("invalid_session_proof"), "{e}");
    // The shim restarted in the same harness session finds its file and
    // keeps its session.
    let restarted = DaemonClient::managed_remote(remote_of(&ts, &sam, None, &dir), registration());
    restarted.ensure_session().await.unwrap();
    assert_eq!(restarted.session().unwrap().id, sid);
    // Ending the session removes its file.
    restarted.end_session().await.unwrap();
    assert!(credential_file(&dir, "claude", "sam-cc-1").is_none());
    assert!(
        !dir.path()
            .join("remote-sessions/claude-p-4000000001.json")
            .exists()
    );
}

/// Spec 2026-10-08 §6.8: a session ended under the shim (its secret
/// cleared) is registered again on the next call, with a new secret
/// written over the stale file; the old secret proves nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_ended_under_a_remote_shim_is_registered_again() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let dir = tempfile::tempdir().unwrap();
    let client = DaemonClient::managed_remote(remote_of(&ts, &sam, None, &dir), registration());
    client.ensure_session().await.unwrap();
    let first = client.session().unwrap().id;
    let old = credential_file(&dir, "claude", "sam-cc-1").unwrap()["secret"].clone();
    ts.store
        .end_session(&clax_core::audit::AuditCtx::DAEMON, &first)
        .unwrap();
    assert!(client.list().await.unwrap()["artifacts"].is_array());
    let second = client.session().unwrap().id;
    assert_ne!(second, first);
    let file = credential_file(&dir, "claude", "sam-cc-1").unwrap();
    assert_eq!(file["session"], json!(second));
    assert_ne!(file["secret"], old);
    assert!(
        !ts.store
            .session_secret_matches(
                &second,
                &sam.grant_id(),
                &Secret::new(old.as_str().unwrap().to_string())
            )
            .unwrap()
    );
}

/// Spec 2026-10-08 §6.8 Clients: a credential file naming a session the
/// daemon has no row for (written for another daemon, or one reset since)
/// is passed over as a refused one is: the shim registers anew and writes
/// its new credential over it.
#[tokio::test(flavor = "multi_thread")]
async fn a_credential_for_a_session_the_daemon_does_not_know_is_passed_over() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let dir = tempfile::tempdir().unwrap();
    let remote = remote_of(&ts, &sam, None, &dir);
    let unknown = clax_core::new_ulid();
    remote
        .sessions
        .write(
            "claude",
            Some("sam-cc-1"),
            None,
            None,
            &clax_core::remote::SessionCredential {
                session: unknown.clone(),
                secret: clax_core::secret::token(clax_core::secret::SESSION_PREFIX),
            },
        )
        .unwrap();
    let client = DaemonClient::managed_remote(remote, registration());
    client.ensure_session().await.unwrap();
    let sid = client.session().unwrap().id;
    assert_ne!(sid, unknown);
    assert_eq!(
        credential_file(&dir, "claude", "sam-cc-1").unwrap()["session"],
        json!(sid)
    );
}

/// A thread Sam's browser (cookie `cookie`) starts on `aid` with a clip.
async fn thread_with_clip(ts: &TestServer, aid: &str, cookie: &str, body: &str) -> String {
    let form = reqwest::multipart::Form::new()
        .text("anchor", clax_testkit::element_anchor().to_string())
        .text("body", body.to_string())
        .text("version", "1")
        .part(
            "clip",
            reqwest::multipart::Part::bytes(clax_testkit::FAKE_PNG.to_vec())
                .file_name("clip.png")
                .mime_str("image/png")
                .unwrap(),
        );
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    res.json::<serde_json::Value>().await.unwrap()["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Spec §16: a remote session's payload names its clip `clax-clip:…`; the
/// shim fetches it, signed, into `<home>/remote-cache/clips/` and hands
/// the agent that file. A clip withheld from the session stays `Clip:
/// withheld`, fetched by no one.
#[tokio::test(flavor = "multi_thread")]
async fn remote_clip_lines_are_rewritten_to_cached_files() {
    use clax_mcp::ClaxTools;
    use clax_mcp::tools::WaitArgs;
    use rmcp::handler::server::wrapper::Parameters;
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let dir = tempfile::tempdir().unwrap();
    let client = DaemonClient::managed_remote(remote_of(&ts, &sam, None, &dir), registration());
    let tools = ClaxTools::new(client.clone(), String::new(), None, ts.home.log_path());
    let made = client
        .create(&json!({"title": "Sam's notes",
                        "files": {"index.html": {"content": "<p>notes</p>", "encoding": "utf8"}}}))
        .await
        .unwrap();
    let aid = made["artifact"]["id"].as_str().unwrap().to_string();
    let sam_cookie = ts.browser_cookie(&ts.store.viewer_of_grant(&sam.grant_id()).unwrap().id);
    let mia_cookie = ts.browser_cookie(&ts.store.viewer_of_grant(&mia.grant_id()).unwrap().id);
    let send = |tid: String| {
        let (ts, aid, cookie) = (&ts, aid.clone(), sam_cookie.clone());
        async move {
            let r = ts
                .client
                .post(format!(
                    "{}/api/artifacts/{aid}/threads/{tid}/send",
                    ts.base
                ))
                .header("cookie", format!("clax_viewer={cookie}"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        }
    };
    let mine = thread_with_clip(&ts, &aid, &sam_cookie, "Sam's own words").await;
    send(mine.clone()).await;
    // Mia's thread, whose clip is hers, with Sam's visible reply.
    let hers = thread_with_clip(&ts, &aid, &mia_cookie, "Mia's words").await;
    ts.reply_as(&aid, &hers, &sam_cookie, "Sam replies under it")
        .await;
    send(hers.clone()).await;
    let r = tools
        .wait_for_feedback(Parameters(WaitArgs {
            url_or_id: None,
            timeout_s: Some(1),
        }))
        .await
        .unwrap();
    let text: Vec<String> = r
        .content
        .iter()
        .map(|c| c.as_text().unwrap().text.clone())
        .collect();
    let trailing = text.get(1).expect("the payload's prose");
    let cached = dir
        .path()
        .join("remote-cache/clips")
        .join(format!("{aid}-{mine}.png"));
    assert!(
        trailing.contains(&format!("\nClip: {}\n", cached.display())),
        "{trailing}"
    );
    assert_eq!(std::fs::read(&cached).unwrap(), clax_testkit::FAKE_PNG);
    assert!(!trailing.contains("clax-clip:"), "{trailing}");
    assert!(trailing.contains("\nClip: withheld\n"), "{trailing}");
    let first: serde_json::Value = serde_json::from_str(&text[0]).unwrap();
    let item = first["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["thread_id"] == json!(mine))
        .unwrap();
    assert_eq!(item["clip_path"], json!(cached.display().to_string()));
    assert!(
        !dir.path()
            .join("remote-cache/clips")
            .join(format!("{aid}-{hers}.png"))
            .exists(),
        "a withheld clip is not fetched"
    );
}

/// Spec §16, §7.5: `status` on a remote daemon reports the agent's name,
/// its person and rights and that the daemon is remote; `/healthz` is
/// refused to another machine's agents, so the daemon's version is not
/// asked.
#[tokio::test(flavor = "multi_thread")]
async fn status_on_a_remote_daemon_reports_the_person_and_rights() {
    use clax_mcp::ClaxTools;
    use clax_mcp::tools::StatusArgs;
    use rmcp::handler::server::wrapper::Parameters;
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:reader").await;
    let dir = tempfile::tempdir().unwrap();
    let client = DaemonClient::managed_remote(remote_of(&ts, &sam, None, &dir), registration());
    let tools = ClaxTools::new(client, String::new(), None, ts.home.log_path());
    let r = tools
        .status(Parameters(StatusArgs {
            name: Some("sams-review".into()),
        }))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    assert!(r.is_error != Some(true), "{v}");
    assert_eq!(v["remote"], true, "{v}");
    assert_eq!(v["name"], "sams-review", "{v}");
    assert_eq!(v["person"]["name"], "Sam", "{v}");
    assert_eq!(
        v["rights"],
        clax_core::rights::Rights::preset(clax_core::rights::PRESET_READER)
            .unwrap()
            .to_json()
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

/// I1 in the shim: clips that do not come within the fetch bound never hold
/// the feedback; each reads `Clip: unavailable (timed out)`.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_clip_that_does_not_come_never_holds_the_feedback() {
    use clax_mcp::ClaxTools;
    use clax_mcp::tools::WaitArgs;
    use rmcp::handler::server::wrapper::Parameters;
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let dir = tempfile::tempdir().unwrap();
    let proxy = clip_holding_proxy(format!("127.0.0.1:{}", ts.addr.port())).await;
    let mut remote = Arc::into_inner(remote_of(&ts, &sam, None, &dir)).unwrap();
    let key = KeyPair::from_pkcs8(sam.key.pkcs8().expose()).unwrap();
    remote.config.url = proxy.clone();
    remote.signer = Arc::new(Signer::new(key, &proxy).unwrap());
    let client = DaemonClient::managed_remote(Arc::new(remote), registration())
        .with_clip_fetch_time(std::time::Duration::from_millis(300));
    let tools = ClaxTools::new(client.clone(), String::new(), None, ts.home.log_path());
    let made = client
        .create(&json!({"title": "Sam's notes",
                        "files": {"index.html": {"content": "<p>notes</p>", "encoding": "utf8"}}}))
        .await
        .unwrap();
    let aid = made["artifact"]["id"].as_str().unwrap().to_string();
    let cookie = ts.browser_cookie(&ts.store.viewer_of_grant(&sam.grant_id()).unwrap().id);
    for body in ["FIRSTCLIP", "SECONDCLIP"] {
        let tid = thread_with_clip(&ts, &aid, &cookie, body).await;
        let r = ts
            .client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/send",
                ts.base
            ))
            .header("cookie", format!("clax_viewer={cookie}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
    let r = tools
        .wait_for_feedback(Parameters(WaitArgs {
            url_or_id: None,
            timeout_s: Some(1),
        }))
        .await
        .unwrap();
    let trailing = r.content[1].as_text().unwrap().text.clone();
    assert!(
        trailing.contains("FIRSTCLIP") && trailing.contains("SECONDCLIP"),
        "{trailing}"
    );
    assert_eq!(
        trailing.matches("Clip: unavailable (timed out)").count(),
        2,
        "{trailing}"
    );
}
