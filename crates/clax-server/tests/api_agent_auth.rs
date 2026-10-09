//! Agents of other machines at the routes (spec §6.1, §6.6, §6.7, M4, M7,
//! M8): one `Caller` per request, the header checks before any body is
//! read, the signature verified before every handler within bounded
//! unverified reads, and every session a request names bound to its
//! credential.

use crate::common::TestServer;
use clax_server::testing::{Call, LAN_HEADER, TestClient};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MIB: usize = 1024 * 1024;

async fn lan_server() -> TestServer {
    TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await
}

async fn code(res: reqwest::Response) -> String {
    let v: Value = res.json().await.unwrap_or_default();
    v["error"]["code"].as_str().unwrap_or_default().to_string()
}

async fn status_code(res: reqwest::Response) -> (u16, String) {
    (res.status().as_u16(), code(res).await)
}

/// A request written by hand on its own connection, so a test decides when
/// each byte of its body is sent.
struct Raw {
    stream: tokio::net::TcpStream,
}

impl Raw {
    /// Opens a connection to `ts` from the LAN and writes the head of
    /// `method path` with `headers`, `Host` the client's.
    async fn open(
        ts: &TestServer,
        c: &TestClient,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
    ) -> Raw {
        let addr = format!("127.0.0.1:{}", ts.addr.port());
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nhost: {}\r\n{LAN_HEADER}: 1\r\nconnection: close\r\n",
            c.target().1
        );
        for (k, v) in headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await.unwrap();
        Raw { stream }
    }

    /// An agent request of `c` declaring a `len`-byte body, signed for
    /// `body` (or with a bad signature).
    async fn agent(
        ts: &TestServer,
        c: &TestClient,
        path: &str,
        body: &[u8],
        len: usize,
        good: bool,
    ) -> Raw {
        let headers = [
            ("authorization", format!("Clax-Grant {}", c.access())),
            ("clax-signature", c.signature("POST", path, body, !good)),
            ("content-type", "application/octet-stream".to_string()),
            ("content-length", len.to_string()),
        ];
        Raw::open(ts, c, "POST", path, &headers).await
    }

    async fn send(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
    }

    /// The response's status and error code.
    async fn response(mut self) -> (u16, String) {
        let mut buf = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.stream.read_to_end(&mut buf))
            .await
            .expect("a response within 10 s");
        let text = String::from_utf8_lossy(&buf);
        let status = text
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("no status in {text:?}"));
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
        let v: Value = serde_json::from_str(body).unwrap_or_default();
        (
            status,
            v["error"]["code"].as_str().unwrap_or_default().to_string(),
        )
    }
}

/// A multipart body of one `file` field of `n` bytes, and its content
/// type.
fn upload_body(n: usize) -> (Vec<u8>, String) {
    let boundary = "clax-test-boundary";
    let mut b = format!(
        "--{boundary}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\ncontent-type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    b.extend(std::iter::repeat_n(b'x', n));
    b.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    (b, format!("multipart/form-data; boundary={boundary}"))
}

/// A signed upload of an `n`-byte asset to `aid` by `c`.
async fn upload(c: &TestClient, aid: &str, sid: &str, n: usize) -> reqwest::Response {
    let (body, ty) = upload_body(n);
    let path = format!("/api/artifacts/{aid}/assets");
    c.call(Call {
        method: "POST",
        path: &path,
        authorization: Some(format!("Clax-Grant {}", c.access())),
        body: Some(body),
        headers: vec![("x-clax-session", sid.to_string()), ("content-type", ty)],
        ..Default::default()
    })
    .await
}

#[tokio::test]
async fn a_persons_agent_registers_under_its_grant_and_reaches_only_its_sessions() {
    let ts = lan_server().await;
    let owner_sid = ts.register_session_pid("claude", "h1", 4242).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let s = sam.register_pid("claude", "h1", 4242).await;
    let sid = s["id"].as_str().unwrap().to_string();
    assert_ne!(
        sid, owner_sid,
        "the same harness ID and PID never match another credential's"
    );
    assert_eq!(s["grant_id"], sam.grant_id().as_str());
    assert!(s["person"].is_string());
    let list: Value = sam
        .get(None, "/api/sessions?live=true")
        .await
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![sid.as_str()]);
    // A join of the same harness session ID finds the grant's own row.
    let r = sam
        .agent(
            "POST",
            "/api/sessions/join",
            None,
            Some(
                json!({"harness": "claude", "parent_pid": 4242, "harness_session_id": "h1",
                        "codex_home": "/elsewhere"}),
            ),
        )
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<Value>().await.unwrap()["session"]["id"],
        sid.as_str()
    );
    // The token sees only its own sessions.
    let mine: Value = ts
        .get_authed("/api/sessions?live=true")
        .await
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = mine["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![owner_sid.as_str()]);
    // Its own session is reachable with its header, and a heartbeat works.
    let r = sam
        .agent(
            "PATCH",
            &format!("/api/sessions/{sid}"),
            Some(&sid),
            Some(json!({"heartbeat": true})),
        )
        .await;
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn path_session_must_belong_to_the_grant() {
    let ts = lan_server().await;
    let owner_sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let sid = sam.register("claude", "h2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mia_sid = mia.register("claude", "h2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for other in [
        &owner_sid,
        &mia_sid,
        &"01J9Z3K4M5N6P7Q8R9S0T1V2W3".to_string(),
    ] {
        for p in [
            format!("/api/sessions/{other}/feedback"),
            format!("/api/sessions/{other}"),
            format!("/api/sessions/{other}/working"),
            format!("/api/sessions/{other}/watches"),
        ] {
            let r = sam.get(Some(&sid), &p).await;
            assert_eq!(status_code(r).await, (403, "foreign_session".into()), "{p}");
        }
    }
    // A header naming a foreign session.
    for other in [&owner_sid, &mia_sid] {
        let r = sam.get(Some(other), "/api/artifacts").await;
        assert_eq!(status_code(r).await, (403, "foreign_session".into()));
    }
    // The token naming a grant's session, in the path or the header.
    let r = ts.get_authed(&format!("/api/sessions/{sid}/watches")).await;
    assert_eq!(status_code(r).await, (403, "foreign_session".into()));
    let r = ts
        .authed(ts.client.get(format!("{}/api/sessions", ts.base)))
        .header("x-clax-session", &sid)
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(r).await, (403, "foreign_session".into()));
    // Publishing as a grant's session with the token.
    let aid = ts.any_artifact().await;
    let r = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .header("x-clax-session", &sid)
        .json(&json!({"files": {"index.html": {"content": "<p>2", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(r).await, (403, "foreign_session".into()));
    // The grant's own session passes.
    let r = sam
        .get(Some(&sid), &format!("/api/sessions/{sid}/watches"))
        .await;
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn authentication_precedes_the_body() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = ts.any_artifact().await;
    let path = format!("/api/artifacts/{aid}/assets");
    let read_before = ts.agent_gate.state().read;
    // A forged token declaring a 20 MiB body: refused with nothing read
    // (the request's body is never sent).
    let forged = [
        ("authorization", "Clax-Grant cxg_forged".to_string()),
        ("clax-signature", sam.signature("POST", &path, b"", false)),
        ("x-clax-session", sid.clone()),
        ("content-length", (20 * MIB).to_string()),
    ];
    let raw = Raw::open(&ts, &sam, "POST", &path, &forged).await;
    assert_eq!(raw.response().await, (401, "invalid_token".into()));
    // A valid token with a stale `t`, before reading.
    let stale = {
        use sha2::Digest;
        let nonce = clax_core::sig::new_nonce();
        sam.key
            .sign(&clax_core::sig::SignInput {
                method: "POST",
                path_and_query: &path,
                host: sam.target().1,
                t: ts.store.now_utc().timestamp() - 200,
                nonce: &nonce,
                body_sha256: sha2::Sha256::digest(b"").into(),
                token_sha256: Some(clax_core::secret::sha256(&sam.access())),
            })
            .unwrap()
            .to_string()
    };
    let headers = [
        ("authorization", format!("Clax-Grant {}", sam.access())),
        ("clax-signature", stale),
        ("x-clax-session", sid.clone()),
        ("content-length", (20 * MIB).to_string()),
    ];
    let raw = Raw::open(&ts, &sam, "POST", &path, &headers).await;
    assert_eq!(raw.response().await, (401, "stale_request".into()));
    // No signature at all.
    let headers = [
        ("authorization", format!("Clax-Grant {}", sam.access())),
        ("content-length", (20 * MIB).to_string()),
    ];
    let raw = Raw::open(&ts, &sam, "POST", &path, &headers).await;
    assert_eq!(raw.response().await, (401, "invalid_signature".into()));
    let g = ts.gate().await;
    assert_eq!(
        (g["read"].as_u64(), g["reserved"].as_u64()),
        (Some(read_before), Some(0)),
        "nothing of the refused bodies was read"
    );
}

#[tokio::test]
async fn a_bad_signature_is_refused_on_get_delete_and_post() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = ts.any_artifact().await;
    for (m, p) in [
        ("GET", "/api/artifacts".to_string()),
        ("GET", format!("/api/sessions/{sid}/watches")),
        ("DELETE", format!("/api/sessions/{sid}/watches/{aid}")),
        ("POST", format!("/api/sessions/{sid}/feedback/ack")),
    ] {
        let r = sam.call_with_bad_signature(&sid, m, &p).await;
        assert_eq!(
            status_code(r).await,
            (401, "invalid_signature".into()),
            "{m} {p}"
        );
    }
    // The same calls signed are answered by their handlers.
    let r = sam
        .agent(
            "DELETE",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            None,
        )
        .await;
    assert_eq!(r.status(), 204);
}

#[tokio::test]
async fn unverified_bodies_are_bounded() {
    let (ts, timer) = TestServer::spawn_agent_timer().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let aid = ts.any_artifact().await;
    let assets = format!("/api/artifacts/{aid}/assets");
    // One large unverified body per key: a second waits in the queue, and
    // its wait ends after 30 s with 429.
    let r0 = ts.agent_gate.state().read;
    let mut first = Raw::agent(&ts, &sam, &assets, b"", 2 * MIB, false).await;
    // Enough of it to stay above the rate for 30 s.
    first.send(&vec![b'x'; 2 * MIB - 32 * 1024]).await;
    ts.wait_gate("the first body read", |g| {
        g.read == r0 + (2 * MIB - 32 * 1024) as u64
    })
    .await;
    let second = Raw::agent(&ts, &sam, &assets, b"", 2 * MIB, false).await;
    ts.wait_gate("the second queued", |g| g.queued == 1 && g.held == 1)
        .await;
    // Another key is not held up by Sam's.
    let other = Raw::agent(&ts, &mia, &assets, b"", 2 * MIB, false).await;
    ts.wait_gate("Mia's read under her own permit", |g| g.held == 2)
        .await;
    timer.advance(Duration::from_secs(30));
    assert_eq!(second.response().await, (429, "limit_reached".into()));
    first.send(&vec![b'x'; 32 * 1024]).await;
    assert_eq!(first.response().await, (401, "invalid_signature".into()));
    drop(other);
    // The budget: 90 MiB reserved up front by one body, so a 40 MiB one
    // waits for room, and after 30 s is 503 busy, having read nothing.
    let publish = format!("/api/artifacts/{aid}/versions");
    ts.wait_gate("every permit back", |g| g.held == 0 && g.reserved == 0)
        .await;
    let read_before = ts.agent_gate.state().read;
    let mut big = Raw::agent(&ts, &sam, &publish, b"", 90 * MIB, false).await;
    big.send(&vec![b'x'; 2 * MIB]).await;
    ts.wait_gate("90 MiB reserved", |g| g.reserved == 90 * MIB)
        .await;
    let waits = Raw::agent(&ts, &mia, &publish, b"", 40 * MIB, false).await;
    ts.wait_gate("the second waits for budget", |g| g.budget_waiting == 1)
        .await;
    timer.advance(Duration::from_secs(30));
    assert_eq!(waits.response().await, (503, "busy".into()));
    assert_eq!(
        ts.agent_gate.state().read,
        read_before + (2 * MIB) as u64,
        "the refused body was never read"
    );
    drop(big);
    ts.wait_gate("the dropped body's permit and budget back", |g| {
        g.held == 0 && g.reserved == 0 && g.queued == 0 && g.budget_waiting == 0
    })
    .await;
    let g = ts.gate().await;
    assert_eq!(
        (g["reserved"].as_u64(), g["held"].as_u64()),
        (Some(0), Some(0))
    );
}

#[tokio::test]
async fn concurrent_large_uploads_from_one_machine_succeed() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:full").await;
    let s1 = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let s2 = sam.register("claude", "h2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = ts.any_artifact().await;
    // Both are verified (one waits for the key's permit) and reach the
    // handler, which answers 503 `not_yet` until the route's filter is live.
    let (a, b) = tokio::join!(
        upload(&sam, &aid, &s1, 5 * MIB),
        upload(&sam, &aid, &s2, 5 * MIB)
    );
    for r in [a, b] {
        assert_eq!(status_code(r).await, (503, "not_yet".into()));
    }
    assert!(ts.agent_gate.state().read as usize >= 10 * MIB);

    // The queue holds 4: a sixth upload is refused at once.
    let (ts, timer) = TestServer::spawn_agent_timer().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let aid = ts.any_artifact().await;
    let path = format!("/api/artifacts/{aid}/assets");
    let r0 = ts.agent_gate.state().read;
    let mut holder = Raw::agent(&ts, &sam, &path, b"", 5 * MIB, false).await;
    // Enough of it to stay above the rate past the queue's 30 s.
    holder.send(&vec![b'x'; 2 * MIB]).await;
    ts.wait_gate("the holder's read", |g| {
        g.held == 1 && g.read == r0 + (2 * MIB) as u64
    })
    .await;
    let mut queued = Vec::new();
    for i in 1..=4 {
        queued.push(Raw::agent(&ts, &sam, &path, b"", 5 * MIB, false).await);
        ts.wait_gate("queued", |g| g.queued == i).await;
    }
    let sixth = Raw::agent(&ts, &sam, &path, b"", 5 * MIB, false).await;
    assert_eq!(sixth.response().await, (429, "limit_reached".into()));
    // The queue's 30 s wait ends on the timer; the holder reads on.
    timer.advance(Duration::from_secs(30));
    for q in queued {
        assert_eq!(q.response().await, (429, "limit_reached".into()));
    }
    holder.send(&vec![b'x'; 3 * MIB]).await;
    assert_eq!(holder.response().await, (401, "invalid_signature".into()));
    ts.wait_gate("all released", |g| {
        g.held == 0 && g.queued == 0 && g.reserved == 0
    })
    .await;
}

#[tokio::test]
async fn a_trickled_unverified_body_is_dropped() {
    let (ts, timer) = TestServer::spawn_agent_timer().await;
    let sam = ts.connect_person("Sam", "clax:full").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = ts.any_artifact().await;
    let path = format!("/api/artifacts/{aid}/assets");
    let (body, _) = upload_body(2 * MIB);
    let r0 = ts.agent_gate.state().read;
    let mut raw = Raw::agent(&ts, &sam, &path, &body, body.len(), true).await;
    raw.send(&body[..1024]).await;
    ts.wait_gate("a KiB read", |g| g.read == r0 + 1024).await;
    // 9 s in, still within the grace.
    timer.advance(Duration::from_secs(9));
    raw.send(&body[1024..2048]).await;
    ts.wait_gate("two KiB read", |g| g.read == r0 + 2048).await;
    timer.advance(Duration::from_secs(1));
    assert_eq!(raw.response().await, (408, "timeout".into()));
    let g = ts.gate().await;
    assert_eq!(
        (g["reserved"].as_u64(), g["held"].as_u64()),
        (Some(0), Some(0))
    );
    // A real upload then proceeds: verified, it reaches the handler.
    let r = upload(&sam, &aid, &sid, 2 * MIB).await;
    assert_eq!(status_code(r).await, (503, "not_yet".into()));
}

#[tokio::test]
async fn non_grant_requests_pass_through_unbuffered() {
    let ts = lan_server().await;
    let aid = ts.any_artifact().await;
    let big = "x".repeat(3 * MIB);
    // The daemon token.
    let r = ts
        .authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": big, "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    // A viewer's cookie and the extension, with bodies their routes refuse
    // unread (small, so the refusal is never a reset mid-send).
    let small = "x".repeat(64 * 1024);
    let v = ts.viewer(Some("Ann")).await;
    let r = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .json(&json!({"body": small}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error(), "{}", r.status());
    let ext = ts.extension().await;
    let _ = ext.post("/api/live/threads", json!({"body": small})).await;
    let g = ts.gate().await;
    assert_eq!(
        (g["read"].as_u64(), g["reserved"].as_u64()),
        (Some(0), Some(0))
    );
}

#[tokio::test]
async fn bad_signatures_consume_no_nonces() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let n = clax_core::sig::new_nonce();
    let call = |corrupt: bool| Call {
        method: "GET",
        path: "/api/sessions",
        authorization: Some(format!("Clax-Grant {}", sam.access())),
        nonce: Some(n.clone()),
        corrupt,
        ..Default::default()
    };
    let r = sam.call(call(true)).await;
    assert_eq!(status_code(r).await, (401, "invalid_signature".into()));
    let r = sam.call(call(false)).await;
    assert_eq!(r.status(), 200, "the bad signature spent nothing");
    let r = sam.call(call(false)).await;
    assert_eq!(status_code(r).await, (401, "replayed_request".into()));
    // Two copies of one valid request at once: exactly one passes.
    for _ in 0..8 {
        let n = clax_core::sig::new_nonce();
        let one = || Call {
            method: "GET",
            path: "/api/sessions",
            authorization: Some(format!("Clax-Grant {}", sam.access())),
            nonce: Some(n.clone()),
            ..Default::default()
        };
        let (a, b) = tokio::join!(sam.call(one()), sam.call(one()));
        let ok = [a.status(), b.status()]
            .iter()
            .filter(|s| s.is_success())
            .count();
        assert_eq!(ok, 1);
        for r in [a, b] {
            if !r.status().is_success() {
                assert_eq!(status_code(r).await, (401, "replayed_request".into()));
            }
        }
    }
}

#[tokio::test]
async fn the_route_limit_caps_the_pre_handler_read() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = ts.any_artifact().await;
    // 3 MiB to a docs route (2 MiB): 413 before anything is read.
    let doc = json!({"data": {"x": "y".repeat(3 * MIB)}});
    let r = sam
        .agent(
            "PUT",
            &format!("/api/artifacts/{aid}/docs/a"),
            Some(&sid),
            Some(doc),
        )
        .await;
    assert_eq!(status_code(r).await, (413, "body_too_large".into()));
    let read_before = ts.agent_gate.state().read;
    assert!(read_before < 1024, "only the registration was read");
    // A 90 MiB publish is within its limit: read, verified, and handed on.
    let big =
        json!({"files": {"index.html": {"content": "x".repeat(90 * MIB), "encoding": "utf8"}}});
    let r = sam
        .agent(
            "POST",
            &format!("/api/artifacts/{aid}/versions"),
            Some(&sid),
            Some(big),
        )
        .await;
    // The handler's checks run after verification: a collaborator may not
    // update an artifact the owner made.
    assert_eq!(status_code(r).await, (403, "not_permitted".into()));
    assert!((ts.agent_gate.state().read - read_before) as usize > 90 * MIB);
    assert_eq!(ts.agent_gate.state().reserved, 0);
}

#[tokio::test]
async fn every_agent_channel_is_an_agent() {
    let ts = lan_server().await;
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let caller = |r: reqwest::RequestBuilder| async move {
        let res = r.send().await.unwrap();
        assert_eq!(res.status(), 200);
        res.json::<Value>().await.unwrap()
    };
    let at = |p: &str| format!("{}{p}", ts.base);
    // The path names a session (the hooks, `clax feedback follow`).
    let v = caller(
        ts.authed(
            ts.client
                .get(at(&format!("/api/_test/sessions/{sid}/caller"))),
        ),
    )
    .await;
    assert_eq!(v, json!({"caller": "owner_agent", "session": sid}));
    // The header names one (the shim, Pi).
    let v = caller(
        ts.authed(ts.client.get(at("/api/_test/caller")))
            .header("x-clax-session", &sid),
    )
    .await;
    assert_eq!(v, json!({"caller": "owner_agent", "session": sid}));
    // The marker with no session.
    let v = caller(
        ts.authed(ts.client.get(at("/api/_test/caller")))
            .header("x-clax-channel", "agent"),
    )
    .await;
    assert_eq!(v, json!({"caller": "owner_agent", "session": null}));
    // The token alone: the owner by hand.
    let v = caller(ts.authed(ts.client.get(at("/api/_test/caller")))).await;
    assert_eq!(v, json!({"caller": "owner", "browser": false}));
    // The owner's browser.
    let v = caller(
        ts.client
            .get(at("/api/_test/caller"))
            .header("cookie", ts.owner_cookie()),
    )
    .await;
    assert_eq!(v, json!({"caller": "owner", "browser": true}));
    // The marker or a session header without the token counts for nothing.
    let viewer = ts.viewer(Some("Ann")).await;
    let v = caller(
        ts.client
            .get(at("/api/_test/caller"))
            .header("cookie", format!("clax_viewer={}", viewer.cookie))
            .header("x-clax-channel", "agent")
            .header("x-clax-session", &sid),
    )
    .await;
    assert_eq!(v, json!({"caller": "viewer", "viewer": viewer.public_id}));
    let v = caller(
        ts.client
            .get(at("/api/_test/caller"))
            .header("x-clax-channel", "agent"),
    )
    .await;
    assert_eq!(v, json!({"caller": "anonymous"}));
    // A connected person's browser.
    let _sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sam_viewer = ts
        .store
        .list_grants()
        .unwrap()
        .into_iter()
        .find_map(|g| g.person)
        .unwrap();
    let row = ts.store.viewer_by_public_id(&sam_viewer).unwrap().unwrap();
    let v = caller(
        ts.client
            .get(at("/api/_test/caller"))
            .header("cookie", format!("clax_viewer={}", row.id)),
    )
    .await;
    assert_eq!(v, json!({"caller": "person", "viewer": sam_viewer}));
}

#[tokio::test]
async fn token_without_signature_is_refused_and_browsers_cannot_mix() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let grant = || Some(format!("Clax-Grant {}", sam.access()));
    // No signature.
    let r = sam
        .call(Call {
            method: "GET",
            path: "/api/sessions",
            authorization: grant(),
            unsigned: true,
            ..Default::default()
        })
        .await;
    assert_eq!(status_code(r).await, (401, "invalid_signature".into()));
    // With a cookie or an Origin, even signed.
    for (k, v) in [
        ("cookie", ts.owner_cookie()),
        (
            "cookie",
            "clax_viewer=01J9Z3K4M5N6P7Q8R9S0T1V2W3".to_string(),
        ),
        ("origin", "http://127.0.0.1".to_string()),
    ] {
        let r = sam
            .call(Call {
                method: "GET",
                path: "/api/sessions",
                authorization: grant(),
                headers: vec![(k, v)],
                ..Default::default()
            })
            .await;
        assert_eq!(
            status_code(r).await,
            (403, "forbidden_origin".into()),
            "{k}"
        );
    }
    // A forged token never falls back to the owner's cookie or token.
    let r = ts
        .client
        .get(format!("{}/api/sessions", ts.base))
        .header("authorization", "Clax-Grant cxg_forged")
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(r).await, (401, "invalid_token".into()));
    let r = ts
        .client
        .get(format!("{}/api/sessions", ts.base))
        .header("authorization", "Clax-Grant cxg_forged")
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(r).await, (403, "forbidden_origin".into()));
    let r = ts
        .client
        .get(format!("{}/api/sessions", ts.base))
        .header("authorization", "Clax-Grant cxg_forged")
        .header("authorization", format!("Bearer {}", ts.token))
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(r).await, (401, "invalid_token".into()));
    // A manage token is not an access token.
    let manage = sam.held().manage.unwrap();
    let r = sam
        .call(Call {
            method: "GET",
            path: "/api/sessions",
            authorization: Some(format!("Clax-Grant {manage}")),
            ..Default::default()
        })
        .await;
    assert_eq!(status_code(r).await, (401, "invalid_token".into()));
    // Another key's signature over the right token.
    let other = clax_core::sig::KeyPair::generate();
    let r = sam
        .call(Call {
            method: "GET",
            path: "/api/sessions",
            authorization: grant(),
            key: Some(&other),
            ..Default::default()
        })
        .await;
    assert_eq!(status_code(r).await, (401, "invalid_signature".into()));
    // A revoked grant's token.
    assert_eq!(sam.revoke_token().await.status(), 204);
    let r = sam.get(None, "/api/sessions").await;
    assert_eq!(status_code(r).await, (401, "grant_revoked".into()));
}

#[tokio::test]
async fn a_grant_is_never_the_owner() {
    let ts = lan_server().await;
    let cmd = ts.invite_with(json!({"me": true})).await;
    let code_ = cmd.rsplit_once("/i/").unwrap().1;
    let mine = TestClient::new(&ts);
    mine.grant_request(
        json!({"access_token": {"access": ["clax:full"]}, "clax": {"invite": code_}}),
    )
    .await;
    for p in [
        "/api/inbox",
        "/api/questions",
        "/api/people/invites",
        "/api/grants",
        "/api/token",
    ] {
        let r = mine.get(None, p).await;
        assert_eq!(status_code(r).await, (403, "forbidden".into()), "{p}");
    }
    let s = mine.register("claude", "h1").await;
    let sid = s["id"].as_str().unwrap();
    let row = ts.store.get_session(sid).unwrap().unwrap();
    assert_eq!(row.person, None);
    assert_eq!(row.grant_id.as_deref(), Some(mine.grant_id().as_str()));
}

#[tokio::test]
async fn session_less_grant_requests_use_person_and_grant_rights() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = sam.get(None, "/api/artifacts").await;
    assert_eq!(status_code(r).await, (503, "not_yet".into()));
    let r = sam.get(None, &format!("/api/sessions/{sid}/watches")).await;
    assert_eq!(status_code(r).await, (400, "no_session".into()));
    // Rights are checked at the artifact the path names.
    let aid = ts.any_artifact().await;
    let r = sam
        .agent(
            "POST",
            &format!("/api/artifacts/{aid}/versions"),
            Some(&sid),
            Some(json!({"files": {"index.html": {"content": "<p>", "encoding": "utf8"}}})),
        )
        .await;
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "not_permitted");
    assert_eq!(
        v["error"]["right"],
        json!({"type": "clax-artifacts", "action": "update"})
    );
    // A reader may watch, and may not write a document.
    let reader = ts.connect_person("Rae", "clax:reader").await;
    let rid = reader.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = reader
        .agent(
            "PUT",
            &format!("/api/sessions/{rid}/watches/{aid}"),
            Some(&rid),
            None,
        )
        .await;
    assert_eq!(r.status(), 200);
    let r = reader
        .agent(
            "PUT",
            &format!("/api/artifacts/{aid}/docs/a"),
            Some(&rid),
            Some(json!({"data": {}})),
        )
        .await;
    assert_eq!(status_code(r).await, (403, "not_permitted".into()));
    // A narrowing of the grant applies to its requests.
    let gid = sam.grant_id();
    ts.store
        .narrow_grant(
            &gid,
            Some(
                clax_core::rights::Rights::parse(
                    &json!([{"type": "clax-artifacts", "actions": ["read"], "locations": ["*"]}]),
                )
                .unwrap(),
            ),
        )
        .unwrap();
    let r = sam
        .agent(
            "PUT",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            None,
        )
        .await;
    assert_eq!(status_code(r).await, (403, "not_permitted".into()));
}

/// The routes of `routes/mod.rs` with the method of each handler, and
/// whether the handler takes `Agent<…>`.
fn every_route() -> Vec<(String, String, bool)> {
    let src = include_str!("../src/routes/mod.rs");
    let src = &src[..src.find("mod l10 {").unwrap()];
    let module = |m: &str| -> Option<&'static str> {
        Some(match m {
            "artifacts" => include_str!("../src/routes/artifacts.rs"),
            "assets" => include_str!("../src/routes/assets.rs"),
            "connect" => include_str!("../src/routes/connect.rs"),
            "content" => include_str!("../src/routes/content.rs"),
            "docs" => include_str!("../src/routes/docs.rs"),
            "events" => include_str!("../src/routes/events.rs"),
            "extension" => include_str!("../src/routes/extension.rs"),
            "feedback" => include_str!("../src/routes/feedback.rs"),
            "grants" => include_str!("../src/routes/grants.rs"),
            "health" => include_str!("../src/routes/health.rs"),
            "inbox" => include_str!("../src/routes/inbox.rs"),
            "live" => include_str!("../src/routes/live.rs"),
            "questions" => include_str!("../src/routes/questions.rs"),
            "room" => include_str!("../src/routes/room.rs"),
            "sample" => include_str!("../src/routes/sample.rs"),
            "sessions" => include_str!("../src/routes/sessions.rs"),
            "shell" => include_str!("../src/routes/shell.rs"),
            "sites" => include_str!("../src/routes/sites.rs"),
            "stream" => include_str!("../src/routes/stream.rs"),
            "threads" => include_str!("../src/routes/threads.rs"),
            "token" => include_str!("../src/routes/token.rs"),
            "viewers" => include_str!("../src/routes/viewers.rs"),
            "watches" => include_str!("../src/routes/watches.rs"),
            "working" => include_str!("../src/routes/working.rs"),
            "agent" => include_str!("../src/agent.rs"),
            "perf" => return None,
            _ => return None,
        })
    };
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find(".route(") {
        let after = &rest[i + ".route(".len()..];
        let mut depth = 1;
        let mut end = 0;
        for (j, c) in after.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = j;
                        break;
                    }
                }
                _ => {}
            }
        }
        let call = &after[..end];
        rest = &after[end..];
        let open = call.find('"').unwrap();
        let close = open + 1 + call[open + 1..].find('"').unwrap();
        let path = call[open + 1..close].to_string();
        let text = &call[close + 1..];
        let mut found = false;
        for m in ["get", "post", "put", "patch", "delete"] {
            let pat = format!("{m}(");
            let mut k = 0;
            while let Some(x) = text[k..].find(&pat) {
                let at = k + x;
                k = at + pat.len();
                if at > 0
                    && (text.as_bytes()[at - 1].is_ascii_alphanumeric()
                        || text.as_bytes()[at - 1] == b'_')
                {
                    continue;
                }
                let h = &text[k..];
                let e = h
                    .find(|c: char| !(c.is_ascii_lowercase() || c == '_' || c == ':'))
                    .unwrap_or(h.len());
                let agent = match h[..e].split_once("::") {
                    Some((md, f)) if !f.contains(':') => module(md).is_some_and(|src| {
                        let s = src.find(&format!("pub async fn {f}(")).unwrap();
                        let sig = &src[s..s + src[s..].find('{').unwrap()];
                        sig.contains("Agent<")
                    }),
                    _ => false,
                };
                out.push((m.to_uppercase(), path.clone(), agent));
                found = true;
            }
        }
        assert!(found, "{path}: no method found");
    }
    out
}

/// Every route not declared for agents refuses an agent credential, signed
/// and valid, before its handler: the owner-only routes included.
#[tokio::test]
async fn a_grant_credential_is_refused_wherever_no_need_is_declared() {
    let ts = TestServer::spawn_on_with_shutdown("0.0.0.0".parse().unwrap()).await;
    let sam = ts.connect_person("Sam", "clax:full").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = ts.any_artifact().await;
    let mut routes = every_route();
    routes.push(("POST".into(), "/mcp".into(), false));
    routes.push(("POST".into(), "/api/admin/shutdown".into(), false));
    let mut checked = 0;
    for (method, path, agent) in routes {
        if agent {
            continue;
        }
        let concrete = path
            .replace("{aid}", &aid)
            .replace("{id}", &sid)
            .replace("{*path}", "a")
            .replace("{*rest}", "a")
            .replace("{n}", "1")
            .replace("{ms}", "1")
            .replace(['{', '}'], "");
        let body = (method != "GET" && method != "DELETE").then(|| json!({}));
        let r = sam.agent(&method, &concrete, Some(&sid), body).await;
        let (status, code) = status_code(r).await;
        // Live routes hide from every caller but the owner first (404).
        if concrete.starts_with("/api/live/") && status == 404 {
            checked += 1;
            continue;
        }
        assert_eq!(
            (status, code.as_str()),
            (403, "forbidden"),
            "{method} {path}"
        );
        checked += 1;
    }
    assert!(checked > 100, "{checked}");
    // The shutdown route is still there for the token.
    let r = ts
        .authed(ts.client.post(format!("{}/api/admin/shutdown", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
}

// The token's own callers are unchanged: every existing `api_*` suite runs
// as before, and the single-owner golden is compared byte for byte.
