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
            ("content-type", "application/json".to_string()),
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
    for other in [&owner_sid, &mia_sid] {
        for p in [
            format!("/api/sessions/{other}"),
            format!("/api/sessions/{other}/working"),
            format!("/api/sessions/{other}/watches"),
        ] {
            let r = sam.get(Some(&sid), &p).await;
            assert_eq!(status_code(r).await, (403, "foreign_session".into()), "{p}");
        }
    }
    // A session that does not exist is 400 `unknown_session`, which the
    // shim and Pi take as the sign to register again.
    let unknown = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
    for (h, p) in [
        (Some(unknown), "/api/sessions".to_string()),
        (
            Some(sid.as_str()),
            format!("/api/sessions/{unknown}/watches"),
        ),
    ] {
        let r = sam.get(h, &p).await;
        assert_eq!(status_code(r).await, (400, "unknown_session".into()), "{p}");
    }
    // A header naming a foreign session.
    for other in [&owner_sid, &mia_sid] {
        let r = sam.get(Some(other), "/api/sessions").await;
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
    // A session that does not exist is the handler's to answer, as before.
    let r = ts
        .authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-clax-session", unknown)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(r).await, (400, "unknown_session".into()));
    let r = ts
        .authed(ts.client.get(format!("{}/api/artifacts", ts.base)))
        .header("x-clax-session", unknown)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // The grant's own session passes, and so does another of its own
    // named in the path (whose session then acts).
    let r = sam
        .get(Some(&sid), &format!("/api/sessions/{sid}/watches"))
        .await;
    assert_eq!(r.status(), 200);
    let sid2 = sam.register("claude", "h3").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = sam
        .get(Some(&sid), &format!("/api/sessions/{sid2}/watches"))
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
                agent_headers_sha256: clax_core::sig::agent_headers_sha256(&[
                    Some(sid.as_bytes()),
                    None,
                    None,
                    None,
                    None,
                ]),
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
    let (ts, timer) = TestServer::spawn_agent_timer_with_budget(3 * MIB).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let reg = "/api/sessions";
    // One large unverified body per key: a second waits in the key's
    // queue; another key's is not held up by it, but by the budget.
    let r0 = ts.agent_gate.state().read;
    let mut first = Raw::agent(&ts, &sam, reg, b"", 2 * MIB, false).await;
    first.send(&vec![b'x'; MIB]).await;
    ts.wait_gate("the first body read", |g| g.read == r0 + MIB as u64)
        .await;
    let second = Raw::agent(&ts, &sam, reg, b"", 2 * MIB, false).await;
    ts.wait_gate("the second queued", |g| g.queued == 1 && g.held == 1)
        .await;
    let other = Raw::agent(&ts, &mia, reg, b"", 2 * MIB, false).await;
    ts.wait_gate("Mia's waits for budget", |g| {
        g.held == 2 && g.budget_waiting == 1
    })
    .await;
    // The first body's whole read ends 12 s in (10 s and 1 s per MiB),
    // though it kept above the rate: 408.
    timer.advance(Duration::from_secs(12));
    assert_eq!(first.response().await, (408, "timeout".into()));
    drop((second, other));
    ts.wait_gate("everything back", |g| {
        g.held == 0 && g.reserved == 0 && g.queued == 0 && g.budget_waiting == 0 && g.small == 0
    })
    .await;
    let g = ts.gate().await;
    assert_eq!(
        (g["reserved"].as_u64(), g["held"].as_u64()),
        (Some(0), Some(0))
    );

    // Past the whole budget: 503 busy before any of it is read.
    let (ts, _timer) = TestServer::spawn_agent_timer_with_budget(MIB + MIB / 2).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let r0 = ts.agent_gate.state().read;
    let big = Raw::agent(&ts, &sam, reg, b"", 2 * MIB, false).await;
    assert_eq!(big.response().await, (503, "busy".into()));
    assert_eq!(ts.agent_gate.state().read, r0);
}

#[tokio::test]
async fn concurrent_large_uploads_from_one_machine_succeed() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    // Two agents of one machine send bodies over 1 MiB at once: both are
    // verified (one waits for the key's permit) and reach the handler.
    let body = |h: &str| {
        json!({"harness": "claude", "harness_session_id": h,
                               "cwd": "x".repeat(3 * MIB / 2)})
    };
    let (a, b) = tokio::join!(
        sam.agent("POST", "/api/sessions", None, Some(body("h1"))),
        sam.agent("POST", "/api/sessions", None, Some(body("h2")))
    );
    for r in [a, b] {
        assert_eq!(r.status(), 201);
    }

    // The key's queue holds 4: a sixth is refused at once.
    let (ts, _timer) = TestServer::spawn_agent_timer().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let path = "/api/sessions";
    let holder = Raw::agent(&ts, &sam, path, b"", 2 * MIB, false).await;
    ts.wait_gate("the holder", |g| g.held == 1).await;
    let mut queued = Vec::new();
    for i in 1..=4 {
        queued.push(Raw::agent(&ts, &sam, path, b"", 2 * MIB, false).await);
        ts.wait_gate("queued", |g| g.queued == i).await;
    }
    let sixth = Raw::agent(&ts, &sam, path, b"", 2 * MIB, false).await;
    assert_eq!(sixth.response().await, (429, "limit_reached".into()));
    drop((holder, queued));
    ts.wait_gate("all released", |g| {
        g.held == 0 && g.queued == 0 && g.reserved == 0
    })
    .await;
}

#[tokio::test]
async fn a_trickled_unverified_body_is_dropped() {
    let (ts, timer) = TestServer::spawn_agent_timer().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let path = "/api/sessions";
    let body =
        serde_json::to_vec(&json!({"harness": "claude", "cwd": "x".repeat(2 * MIB - 64)})).unwrap();
    // Below 64 KiB/s once the first 10 s are over.
    let r0 = ts.agent_gate.state().read;
    let mut raw = Raw::agent(&ts, &sam, path, &body, body.len(), true).await;
    raw.send(&body[..1024]).await;
    ts.wait_gate("a KiB read", |g| g.read == r0 + 1024).await;
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
    // Fast enough for the rate, but not whole within 10 s and 1 s per MiB.
    let r0 = ts.agent_gate.state().read;
    let mut raw = Raw::agent(&ts, &sam, path, &body, body.len(), true).await;
    raw.send(&body[..3 * MIB / 2]).await;
    ts.wait_gate("most of it read", |g| g.read == r0 + (3 * MIB / 2) as u64)
        .await;
    timer.advance(Duration::from_secs(11));
    raw.send(&body[3 * MIB / 2..3 * MIB / 2 + 1024]).await;
    ts.wait_gate("a little more", |g| {
        g.read == r0 + (3 * MIB / 2 + 1024) as u64
    })
    .await;
    timer.advance(Duration::from_secs(1));
    assert_eq!(raw.response().await, (408, "timeout".into()));
    ts.wait_gate("released", |g| g.held == 0 && g.reserved == 0)
        .await;
    // A real body then proceeds: verified, it reaches the handler.
    let mut raw = Raw::agent(&ts, &sam, path, &body, body.len(), true).await;
    raw.send(&body).await;
    assert_eq!(raw.response().await.0, 201);
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

/// A signed request held back and resent within its window with one agent
/// header rewritten, emptied or dropped: each is 401 `invalid_signature`
/// and spends no nonce, so the request as signed still passes once.
#[tokio::test]
async fn rewritten_agent_headers_fail_the_signature_and_spend_no_nonce() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = |v: Value| v["id"].as_str().unwrap().to_string();
    let (s1, s2) = (
        sid(sam.register("claude", "h1").await),
        sid(sam.register("claude", "h2").await),
    );
    let signed: Vec<(&'static str, String)> = vec![
        ("x-clax-session", s1.clone()),
        ("x-clax-channel", "agent".into()),
        ("x-clax-via", "mcp".into()),
        ("x-clax-git", "e30".into()),
        ("x-clax-call", "e30".into()),
    ];
    let others = [s2.as_str(), "owner", "cli", "e31", "e31"];
    let n = clax_core::sig::new_nonce();
    let call = |nonce: String, headers, sent| Call {
        method: "GET",
        path: "/api/sessions",
        authorization: Some(format!("Clax-Grant {}", sam.access())),
        nonce: Some(nonce),
        headers,
        sent_headers: sent,
        ..Default::default()
    };
    for (i, (name, _)) in signed.iter().enumerate() {
        let rewrite = |v: Option<&str>| {
            let mut h = signed.clone();
            match v {
                Some(v) => h[i].1 = v.to_string(),
                None => {
                    h.remove(i);
                }
            }
            h
        };
        for v in [Some(others[i]), Some(""), None] {
            let r = sam
                .call(call(n.clone(), signed.clone(), Some(rewrite(v))))
                .await;
            assert_eq!(
                status_code(r).await,
                (401, "invalid_signature".into()),
                "{name} as {v:?}"
            );
        }
        // The rewritten value is one the daemon takes when it is signed.
        let r = sam
            .call(call(
                clax_core::sig::new_nonce(),
                rewrite(Some(others[i])),
                None,
            ))
            .await;
        assert_eq!(r.status(), 200, "{name} signed as {}", others[i]);
    }
    let r = sam.call(call(n.clone(), signed.clone(), None)).await;
    assert_eq!(r.status(), 200, "no failed attempt spent the nonce");
    let r = sam.call(call(n, signed, None)).await;
    assert_eq!(status_code(r).await, (401, "replayed_request".into()));
}

/// An agent header given twice is refused, even with the value signed, and
/// spends no nonce.
#[tokio::test]
async fn a_repeated_agent_header_is_refused() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let n = clax_core::sig::new_nonce();
    let call = |sent: Option<Vec<(&'static str, String)>>| Call {
        method: "GET",
        path: "/api/sessions",
        authorization: Some(format!("Clax-Grant {}", sam.access())),
        nonce: Some(n.clone()),
        headers: vec![("x-clax-session", sid.clone())],
        sent_headers: sent,
        ..Default::default()
    };
    for twice in [
        vec![
            ("x-clax-session", sid.clone()),
            ("x-clax-session", sid.clone()),
        ],
        vec![
            ("x-clax-session", sid.clone()),
            ("X-Clax-Session", sid.clone()),
        ],
        vec![
            ("x-clax-session", sid.clone()),
            ("x-clax-via", "mcp".into()),
            ("x-clax-via", "mcp".into()),
        ],
    ] {
        let r = sam.call(call(Some(twice.clone()))).await;
        assert_eq!(
            status_code(r).await,
            (401, "invalid_signature".into()),
            "{twice:?}"
        );
    }
    assert_eq!(sam.call(call(None)).await.status(), 200);
}

#[tokio::test]
async fn the_route_limit_caps_the_pre_handler_read() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:full").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = ts.any_artifact().await;
    let read_before = ts.agent_gate.state().read;
    // Over a route's limit (registration: 2 MiB): 413 before anything is
    // read.
    let big = Raw::agent(&ts, &sam, "/api/sessions", b"", 3 * MIB, false).await;
    assert_eq!(big.response().await, (413, "body_too_large".into()));
    // A body over a GET route's limit: 413 before anything is read.
    let poll = format!("/api/sessions/{sid}/feedback?artifact={aid}");
    let headers = [
        ("authorization", format!("Clax-Grant {}", sam.access())),
        ("clax-signature", sam.signature("GET", &poll, b"", false)),
        ("x-clax-session", sid.clone()),
        ("content-length", (12 * MIB).to_string()),
    ];
    let raw = Raw::open(&ts, &sam, "GET", &poll, &headers).await;
    assert_eq!(raw.response().await, (413, "body_too_large".into()));
    assert_eq!(ts.agent_gate.state().read, read_before);
    // Within the limit: read, verified and handed on.
    let r = sam
        .agent(
            "POST",
            "/api/sessions",
            None,
            Some(json!({"harness": "claude", "cwd": "x".repeat(MIB + MIB / 2)})),
        )
        .await;
    assert_eq!(r.status(), 201);
    assert!((ts.agent_gate.state().read - read_before) as usize > MIB);
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
    assert_eq!(r.status(), 200);
    let r = sam.get(None, &format!("/api/sessions/{sid}/watches")).await;
    assert_eq!(status_code(r).await, (400, "no_session".into()));
    // A reader may watch.
    let aid = ts.any_artifact().await;
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
    // The grant's narrowing applies, checked at the path's artifact.
    let gid = sam.grant_id();
    ts.store
        .narrow_grant(
            &clax_core::audit::AuditCtx::DAEMON,
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
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "not_permitted");
    assert_eq!(
        v["error"]["right"],
        json!({"type": "clax-comments", "action": "watch"})
    );
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
            "approvals" => include_str!("../src/routes/approvals.rs"),
            "questions" => include_str!("../src/routes/questions.rs"),
            "room" => include_str!("../src/routes/room.rs"),
            "sample" => include_str!("../src/routes/sample.rs"),
            "sessions" => include_str!("../src/routes/sessions.rs"),
            "shell" => include_str!("../src/routes/shell.rs"),
            "sites" => include_str!("../src/routes/sites.rs"),
            "stream" => include_str!("../src/routes/stream.rs"),
            "threads" => include_str!("../src/routes/threads.rs"),
            "token" => include_str!("../src/routes/token.rs"),
            "toolpath" => include_str!("../src/routes/toolpath.rs"),
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
    assert!(checked > 60, "{checked}");
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

#[tokio::test]
async fn small_bodies_are_bounded_per_key() {
    let (ts, _timer) = TestServer::spawn_agent_timer().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let path = "/api/sessions";
    let mut open = Vec::new();
    for i in 1..=8 {
        open.push(Raw::agent(&ts, &sam, path, b"", MIB, false).await);
        ts.wait_gate("small bodies in flight", |g| g.small == i * MIB)
            .await;
    }
    let ninth = Raw::agent(&ts, &sam, path, b"", MIB, false).await;
    assert_eq!(ninth.response().await, (429, "limit_reached".into()));
    // Another key has its own allowance.
    let theirs = Raw::agent(&ts, &mia, path, b"", MIB, false).await;
    ts.wait_gate("Mia's too", |g| g.small == 9 * MIB).await;
    drop((open, theirs));
    ts.wait_gate("every allowance back", |g| g.small == 0 && g.reserved == 0)
        .await;
}

/// Makes a live page as the owner's browser does: a comment on a page the
/// extension snapshots. Its artifact ID.
async fn live_page(ts: &TestServer) -> String {
    let v = ts.viewer(Some("Alex")).await;
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("title", "Settings")
        .text(
            "anchor",
            json!({"kind": "element", "selector": "main", "quote": "Save", "file": "index.html"})
                .to_string(),
        )
        .text("body", "The button overflows")
        .text("pending", "[]")
        .text("snapshot", "<p>private</p>")
        .part(
            "clip",
            reqwest::multipart::Part::bytes(clax_server::testing::FAKE_PNG.to_vec())
                .mime_str("image/png")
                .unwrap(),
        );
    let r = ts
        .client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .multipart(form)
        .send()
        .await
        .unwrap();
    let v: Value = r.json().await.unwrap();
    v["page"]["artifact_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn live_pages_are_hidden_from_agents_of_other_machines() {
    let ts = lan_server().await;
    let live = live_page(&ts).await;
    let lan = ts.connect_person("Sam", "clax:full").await;
    // A machine reaching the daemon over loopback (a tunnel, a local
    // proxy) is no closer to the owner's browsing.
    let local = ts.connect_person("Lou", "clax:full").await.on_loopback();
    for c in [&lan, &local] {
        let sid = c.register("claude", "h1").await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let watch = format!("/api/sessions/{sid}/watches/{live}");
        for (m, p, body) in [
            ("PUT", watch.clone(), None),
            ("DELETE", watch.clone(), None),
            ("GET", format!("/api/artifacts/{live}"), None),
            (
                "POST",
                format!("/api/artifacts/{live}/versions"),
                Some(json!({"files": {}})),
            ),
            (
                "POST",
                format!("/api/artifacts/{live}/assets"),
                Some(json!({})),
            ),
            (
                "PUT",
                format!("/api/artifacts/{live}/docs/a"),
                Some(json!({"data": {}})),
            ),
        ] {
            let r = c.agent(m, &p, Some(&sid), body).await;
            assert_eq!(r.status(), 404, "{m} {p}");
        }
        let list: Value = c
            .get(Some(&sid), &format!("/api/sessions/{sid}/watches"))
            .await
            .json()
            .await
            .unwrap();
        assert!(!list.to_string().contains(&live), "{list}");
    }
}

#[tokio::test]
async fn a_remote_session_is_ended_when_idle_whatever_pid_it_reports() {
    let clock = std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-01T09:00:00Z"));
    let ts = TestServer::spawn_on_with_clock("0.0.0.0".parse().unwrap(), clock.clone()).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let me = std::process::id();
    let remote = sam
        .register_with(json!({"harness": "claude", "cwd": "/w", "pid": me, "parent_pid": 77}))
        .await;
    let rid = remote["id"].as_str().unwrap();
    assert_eq!(ts.store.get_session(rid).unwrap().unwrap().pid, None);
    let local: Value = ts
        .post_json(
            "/api/sessions",
            json!({"harness": "claude", "cwd": "/w", "pid": me}),
        )
        .await
        .json()
        .await
        .unwrap();
    let lid = local["session"]["id"].as_str().unwrap();
    clock.advance(clax_core::Store::SESSION_IDLE_SECS as i64 + 1);
    let alive = |p: u32| p == me;
    let reaped = ts
        .store
        .reap_sessions(
            Duration::from_secs(clax_core::Store::SESSION_IDLE_SECS),
            &alive,
        )
        .unwrap();
    assert!(reaped.ended.iter().any(|s| s == rid), "{:?}", reaped.ended);
    assert!(
        !reaped.ended.iter().any(|s| s == lid),
        "a live local process keeps its session"
    );
}

/// The daemon token never names another credential's session (M4): every
/// route under `/api/sessions/{id}`, whatever its class, is 403
/// `foreign_session` for the token naming a grant's session in its path.
#[tokio::test]
async fn the_token_never_names_a_grants_session_on_any_session_route() {
    let ts = lan_server().await;
    let aid = ts.any_artifact().await;
    let sam = ts.connect_person("Sam", "clax:full").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut checked = 0;
    for (method, path, _) in every_route() {
        if !path.starts_with("/api/sessions/{id}") && !path.starts_with("/api/_test/sessions/{id}")
        {
            continue;
        }
        let concrete = path
            .replace("{id}", &sid)
            .replace("{aid}", &aid)
            .replace("{qid}", "q1");
        let req = ts.client.request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{concrete}", ts.base),
        );
        let req = if method == "GET" || method == "DELETE" {
            req
        } else {
            req.json(&json!({}))
        };
        let r = ts.authed(req).send().await.unwrap();
        assert_eq!(
            status_code(r).await,
            (403, "foreign_session".into()),
            "{method} {path}"
        );
        checked += 1;
    }
    assert!(checked >= 20, "{checked}");
}

/// `Agent<N>` itself keeps a person's agent off a route that serves the
/// owner's agents only (`need!(…, owner_agents_only)`), whatever reached
/// it: `verify_layer` refuses such a request first over HTTP, so the
/// extractor is driven directly with a verified grant.
#[tokio::test]
async fn the_extractor_refuses_a_persons_agent_on_an_owner_agents_route() {
    use axum::extract::FromRequestParts;
    use clax_server::agent::{Agent, FeedbackPoll, ThreadsAll, VerifiedGrant};
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let s2 = slot.clone();
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), move |st| {
        *s2.lock().unwrap() = Some(st.clone());
    })
    .await;
    let state = slot.lock().unwrap().take().unwrap();
    let sam = ts.connect_person("Sam", "clax:full").await;
    let parts = || {
        let (mut parts, ()) = axum::http::Request::get("/api/threads")
            .header("authorization", format!("Clax-Grant {}", sam.access()))
            .body(())
            .unwrap()
            .into_parts();
        parts.extensions.insert(VerifiedGrant {
            grant_id: sam.grant_id(),
        });
        parts
    };
    let e = Agent::<ThreadsAll>::from_request_parts(&mut parts(), &state)
        .await
        .err()
        .expect("refused");
    assert_eq!((e.status.as_u16(), e.code), (403, "forbidden"));
    // A route open to grants goes on to its other checks.
    let r = Agent::<FeedbackPoll>::from_request_parts(&mut parts(), &state).await;
    assert!(
        r.as_ref().err().is_none_or(|e| e.code != "forbidden"),
        "{:?}",
        r.err().map(|e| e.code)
    );
    // Every Filtered route's filter is live.
    for (_, _, need) in clax_server::agent::ROUTES {
        if need.class == clax_server::agent::Class::Filtered {
            assert!(
                clax_server::agent::FILTERED_LIVE.contains(&need.name),
                "{}",
                need.name
            );
        }
    }
}
