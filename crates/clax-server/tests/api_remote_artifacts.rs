//! A connected remote's artifacts read and commented on through this
//! daemon (`crate::routes::remote_artifacts`, spec 2026-10-29-remote-instances
//! §4), and the MCP tools that call those routes, against a hosted test
//! daemon as the remote.

use clax_testkit::TestServer;
use clax_testkit::mcp::{ADAPTER, mcp_post, rpc_message};
use clax_testkit::remotes::{Public, closed_port, connect, json_of, local_req};
use reqwest::Method;
use reqwest::header::{COOKIE, HOST, ORIGIN};
use serde_json::{Value, json};

const PAGE: &str = "<h1>Their plan</h1>";

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// An artifact the hosted owner's CLI publishes on `public` (private
/// there): its ID.
async fn remote_artifact(public: &Public) -> String {
    let (s, v) = public
        .cli(
            Method::POST,
            "/api/artifacts",
            Some(json!({"title": "Theirs",
                "files": {"index.html": {"content": PAGE, "encoding": "utf8"}}})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    v["artifact"]["id"].as_str().unwrap().to_string()
}

/// A thread on the whole of version 1 of `raid` there, from the browser
/// whose session cookie is `cookie`: the thread.
async fn browser_thread(public: &Public, raid: &str, cookie: &str, body: &str) -> Value {
    let ts = &public.ts;
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{raid}/threads", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header(COOKIE, format!("clax_session={cookie}"))
        .header(ORIGIN, &public.origin)
        .multipart(
            reqwest::multipart::Form::new()
                .text("anchor", json!({"kind": "artifact"}).to_string())
                .text("body", body.to_string())
                .text("version", "1"),
        )
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!(s, 201, "{v}");
    v["thread"].clone()
}

/// The public copy's thread `tid`, as its owner's CLI reads it.
async fn thread_there(public: &Public, raid: &str, tid: &str) -> Value {
    let (s, v) = public
        .cli(
            Method::GET,
            &format!("/api/artifacts/{raid}/threads/{tid}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    v["thread"].clone()
}

/// A request of the owner's agent in session `sid` here (the token and
/// the session), with `headers`.
async fn agent_req(
    local: &TestServer,
    method: Method,
    path: &str,
    sid: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (u16, Value) {
    let mut r = local
        .authed(
            local
                .client
                .request(method, format!("{}{path}", local.base)),
        )
        .header("x-clax-session", sid);
    for (k, v) in headers {
        r = r.header(*k, *v);
    }
    if let Some(b) = body {
        r = r.json(&b);
    }
    json_of(r.send().await.unwrap()).await
}

/// The MCP tool `name`'s result with `args`, through the daemon's
/// `/mcp/json` (the token): its JSON, and whether it is an error.
async fn tool(local: &TestServer, name: &str, args: Value) -> (Value, bool) {
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": args}});
    let res = mcp_post(local, ADAPTER, &call)
        .bearer_auth(&local.token)
        .header("mcp-protocol-version", "2025-06-18")
        .send()
        .await
        .unwrap();
    let v = rpc_message(res).await;
    let text = v["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{v}"))
        .to_string();
    let err = v["result"]["isError"] == true;
    (serde_json::from_str(&text).unwrap(), err)
}

#[tokio::test]
async fn reads_a_private_remote_artifact_with_the_stored_token() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let remote = connect(&local, &public).await;
    let rid = remote["id"].as_str().unwrap().to_string();
    let raid = remote_artifact(&public).await;
    // Private there: no one signed out reads it.
    let anon = public
        .ts
        .client
        .get(format!("{}/api/artifacts/{raid}", public.ts.base))
        .header(HOST, format!("localhost:{}", public.ts.addr.port()))
        .send()
        .await
        .unwrap();
    assert!(
        matches!(anon.status().as_u16(), 401 | 403 | 404),
        "{}",
        anon.status()
    );
    // Here, the owner and the owner's agent read it with the token.
    let sid = local.register_session("claude", "hs-read").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let base = format!("/api/remotes/{rid}/artifacts/{raid}");
    let (s, v) = agent_req(&local, Method::GET, &base, &sid, &[], None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["artifact"]["title"], "Theirs", "{v}");
    let file = local
        .authed(
            local
                .client
                .get(format!("{}{base}/versions/1/files/index.html", local.base)),
        )
        .header("x-clax-session", &sid)
        .send()
        .await
        .unwrap();
    assert_eq!(file.status(), 200);
    assert_eq!(
        file.headers()["content-security-policy"],
        "sandbox; default-src 'none'"
    );
    assert_eq!(file.text().await.unwrap(), PAGE);
    // The remote's own refusals pass through.
    let (s, _) = agent_req(
        &local,
        Method::GET,
        &format!("/api/remotes/{rid}/artifacts/a0000000000z"),
        &sid,
        &[],
        None,
    )
    .await;
    assert_eq!(s, 404);
    let (s, v) = agent_req(
        &local,
        Method::GET,
        &format!("/api/remotes/nope/artifacts/{raid}"),
        &sid,
        &[],
        None,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (404, Some("no_remote")),
        "{v}"
    );
    // The MCP read tool reads it as a local one, naming where it is.
    let url = format!("{}/a/{raid}", public.origin);
    let (r, err) = tool(&local, "read", json!({"url_or_id": url})).await;
    assert!(!err, "{r}");
    assert_eq!(r["content"], PAGE, "{r}");
    assert_eq!(r["artifact_id"], json!(raid), "{r}");
    assert_eq!(r["version"], 1, "{r}");
    assert_eq!(r["url"], json!(url), "{r}");
    assert_eq!(
        r["remote"],
        json!({"origin": public.origin, "name": remote["name"]}),
        "{r}"
    );
    assert!(r.get("withheld_comments").is_none(), "{r}");
    // Tools that change an artifact do not reach one there.
    for (name, args) in [
        ("delete", json!({"url_or_id": url})),
        ("pin", json!({"url_or_id": url})),
        ("tag", json!({"url_or_id": url, "add": ["x"]})),
        ("fork", json!({"url_or_id": url})),
        (
            "db_get",
            json!({"url_or_id": url, "collection": "c", "doc_id": "d"}),
        ),
    ] {
        let (e, err) = tool(&local, name, args).await;
        assert!(err, "{name}: {e}");
        assert_eq!(e["error"]["code"], "not_on_remote", "{name}: {e}");
        assert_eq!(e["error"]["name"], remote["name"], "{name}: {e}");
    }
    // Waiting on one needs a watch of it first (api_remote_watch.rs).
    let (e, err) = tool(&local, "wait_for_feedback", json!({"url_or_id": url})).await;
    assert!(err, "{e}");
    assert_eq!(e["error"]["code"], "not_watching", "{e}");
    assert_eq!(e["error"]["url"], json!(url), "{e}");
}

/// A connected person's agent reaches no remote route: the token there
/// is the owner's.
#[tokio::test]
async fn a_connected_persons_agent_reaches_no_remote_artifact() {
    let public = Public::new().await;
    let local = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let rid = connect(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let raid = remote_artifact(&public).await;
    let sam = local.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "sam-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for path in [
        format!("/api/remotes/{rid}/artifacts/{raid}"),
        format!("/api/remotes/{rid}/artifacts/{raid}/threads"),
        format!("/api/remotes/{rid}/artifacts/{raid}/versions/1/files/index.html"),
    ] {
        let res = sam.get(Some(sid.as_str()), &path).await;
        assert!(
            matches!(res.status().as_u16(), 401 | 403 | 404),
            "{path}: {}",
            res.status()
        );
    }
}

#[tokio::test]
async fn replies_and_resolves_on_a_remote_thread_once_per_key() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let raid = remote_artifact(&public).await;
    let t = browser_thread(&public, &raid, &public.cookie, "@agent tighten the copy").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let sid = local.register_session("claude", "hs-reply").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let base = format!("/api/remotes/{rid}/artifacts/{raid}");
    // Listed and read through this daemon, as the remote shows them.
    let (s, v) = agent_req(
        &local,
        Method::GET,
        &format!("{base}/threads"),
        &sid,
        &[],
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["threads"][0]["id"], json!(tid), "{v}");
    let (s, v) = agent_req(
        &local,
        Method::GET,
        &format!("{base}/threads/{tid}"),
        &sid,
        &[],
        None,
    )
    .await;
    assert_eq!(
        (s, v["thread"]["id"].as_str()),
        (200, Some(tid.as_str())),
        "{v}"
    );
    // The same key twice makes one reply there.
    let reply = json!({"body": "Tightened.", "author_kind": "agent"});
    let mut ids = Vec::new();
    for _ in 0..2 {
        let (s, v) = agent_req(
            &local,
            Method::POST,
            &format!("{base}/threads/{tid}/comments"),
            &sid,
            &[("idempotency-key", "reply-1")],
            Some(reply.clone()),
        )
        .await;
        assert_eq!(s, 201, "{v}");
        ids.push(v["comment"]["id"].clone());
    }
    assert_eq!(ids[0], ids[1]);
    let there = thread_there(&public, &raid, &tid).await;
    let agent: Vec<&Value> = there["comments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["author_kind"] == "agent")
        .collect();
    assert_eq!(agent.len(), 1, "{there}");
    assert_eq!(agent[0]["body"], "Tightened.", "{there}");
    // Resolved there, then reopened.
    let (s, v) = agent_req(
        &local,
        Method::POST,
        &format!("{base}/threads/{tid}/resolve"),
        &sid,
        &[],
        Some(json!({"as": "agent"})),
    )
    .await;
    assert_eq!(
        (s, v["thread"]["status"].as_str()),
        (200, Some("resolved")),
        "{v}"
    );
    assert_eq!(
        thread_there(&public, &raid, &tid).await["status"],
        "resolved"
    );
    let (s, v) = local_req(
        &local,
        Method::POST,
        &format!("{base}/threads/{tid}/reopen"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(
        (s, v["thread"]["status"].as_str()),
        (200, Some("open")),
        "{v}"
    );
    // The owner by hand (the CLI) writes there as the person, in a by-hand
    // session.
    let (s, v) = local_req(
        &local,
        Method::POST,
        &format!("{base}/threads/{tid}/comments"),
        json!({"body": "From the owner"}),
        None,
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["comment"]["author_kind"], "viewer", "{v}");
    let bodies: Vec<Value> = thread_there(&public, &raid, &tid).await["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].clone())
        .collect();
    assert_eq!(
        bodies,
        [
            json!("@agent tighten the copy"),
            json!("Tightened."),
            json!("From the owner")
        ]
    );
}

/// A remote that takes the connection and never answers is unreachable
/// once the proxy's deadline for agent calls passes (here a short one the
/// state sets): the agent is told the remote is unreachable, not that the
/// daemon timed out.
#[tokio::test]
async fn a_remote_that_never_answers_is_unreachable_within_the_deadline() {
    let local = TestServer::spawn_with(|s| {
        s.remote_deadline = std::time::Duration::from_millis(200);
    })
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    // Accepts each connection and holds it, reading and answering nothing.
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            open.push(sock);
        }
    });
    let r = connected_at(&local, &origin, "stalled");
    let raid = "7q3k9mzx2b4t";
    for path in [
        format!("/api/remotes/{}/artifacts/{raid}", r.id),
        format!("/api/remotes/{}/artifacts/{raid}/threads", r.id),
    ] {
        let (s, v) = local_req(&local, Method::GET, &path, json!(null), None).await;
        assert_eq!(s, 502, "{path}: {v}");
        assert_eq!(v["error"]["code"], "remote_unreachable", "{path}: {v}");
        assert_eq!(v["error"]["origin"], json!(origin), "{v}");
    }
    held.abort();
}

#[tokio::test]
async fn an_unreachable_remote_answers_remote_unreachable() {
    let local = TestServer::spawn().await;
    // A remote connected once, at an origin nothing answers now.
    let origin = format!("http://127.0.0.1:{}", closed_port());
    let r = connected_at(&local, &origin, "gone");
    let state = &local.state;
    state.discovery.put_clax(
        &origin,
        clax_http::discovery_cache::Instance {
            kind: "hosted".into(),
            name: "gone".into(),
            site_domain: None,
        },
    );
    let raid = "7q3k9mzx2b4t";
    let (s, v) = local_req(
        &local,
        Method::GET,
        &format!("/api/remotes/{}/artifacts/{raid}", r.id),
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 502, "{v}");
    assert_eq!(v["error"]["code"], "remote_unreachable", "{v}");
    assert_eq!(v["error"]["origin"], json!(origin), "{v}");
    assert_eq!(v["error"]["name"], "gone", "{v}");
    // Its discovery answer is forgotten, to be asked again.
    assert!(matches!(
        state.discovery.get(&origin),
        clax_http::discovery_cache::Cached::Unknown
    ));
    // Still connected: a remote that does not answer has not refused.
    assert_eq!(
        local.store.find_remote(&r.id).unwrap().unwrap().status,
        "connected"
    );
    let (e, err) = tool(
        &local,
        "read",
        json!({"url_or_id": format!("{origin}/a/{raid}")}),
    )
    .await;
    assert!(err, "{e}");
    assert_eq!(e["error"]["code"], "remote_unreachable", "{e}");
}

#[tokio::test]
async fn a_revoked_token_asks_to_connect_again() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let remote = connect(&local, &public).await;
    let rid = remote["id"].as_str().unwrap().to_string();
    let raid = remote_artifact(&public).await;
    // The person revokes this daemon's token there.
    let token = local.store.remote_token(&rid).unwrap().unwrap();
    let res = public
        .ts
        .client
        .delete(format!("{}/api/hosted/agent-token", public.ts.base))
        .header(HOST, format!("localhost:{}", public.ts.addr.port()))
        .bearer_auth(token.expose())
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let base = format!("/api/remotes/{rid}/artifacts/{raid}");
    let url = format!("{}/a/{raid}", public.origin);
    let request = json!({"tool": "connect_instance",
        "command": format!("clax remote request '{}/'", public.origin)});
    let (s, v) = local_req(&local, Method::GET, &base, json!(null), None).await;
    assert_eq!(s, 502, "{v}");
    assert_eq!(v["error"]["code"], "remote_sign_in_required", "{v}");
    assert_eq!(v["error"]["origin"], json!(public.origin), "{v}");
    assert_eq!(v["error"]["name"], remote["name"], "{v}");
    assert_eq!(v["error"]["request"], request, "{v}");
    // Marked signed out, its token forgotten; asked again at once.
    let r = local.store.find_remote(&rid).unwrap().unwrap();
    assert_eq!(r.status, "signed_out");
    assert!(local.store.remote_token(&rid).unwrap().is_none());
    let (s, v) = local_req(
        &local,
        Method::GET,
        &format!("{base}/threads"),
        json!(null),
        None,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (502, Some("remote_sign_in_required")),
        "{v}"
    );
    // Its URLs now name an instance to connect again.
    let (s, v) = json_of(
        local
            .get_authed(&format!("/api/resolve?url={}", enc(&url)))
            .await,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v,
        json!({"kind": "sign_in_required", "origin": public.origin, "name": remote["name"],
               "request": {"tool": "connect_instance",
                           "command": format!("clax remote request '{url}'")}})
    );
    let (e, err) = tool(&local, "read", json!({"url_or_id": url})).await;
    assert!(err, "{e}");
    assert_eq!(e["error"]["code"], "remote_sign_in_required", "{e}");
    assert!(
        e["error"]["next"]
            .as_str()
            .unwrap()
            .contains("connect_instance"),
        "{e}"
    );
}

#[tokio::test]
async fn withheld_comments_are_counted() {
    let mut public = Public::new().await;
    let local = TestServer::spawn().await;
    let remote = connect(&local, &public).await;
    let raid = remote_artifact(&public).await;
    // Bob, a person there whom Ada has not approved for her agents,
    // comments on her artifact twice.
    let ada = public.cookie.clone();
    let bob_id = public.become_person("Bob").await;
    let bob = public.cookie.clone();
    public.cookie = ada;
    let (s, v) = public
        .browser(
            Method::PUT,
            &format!("/api/hosted/artifacts/{raid}/shares"),
            json!({"person": bob_id, "access": "commenter"}),
        )
        .await;
    assert!((200..300).contains(&s), "{v}");
    browser_thread(&public, &raid, &bob, "Bob's first").await;
    browser_thread(&public, &raid, &bob, "Bob's second").await;
    browser_thread(&public, &raid, &public.cookie, "Ada's own").await;
    let url = format!("{}/a/{raid}", public.origin);
    let note = format!(
        "2 comments from people you have not approved for agents on {}",
        remote["name"].as_str().unwrap()
    );
    let (r, err) = tool(&local, "comments_read", json!({"url_or_id": url})).await;
    assert!(!err, "{r}");
    assert_eq!(
        r["withheld_comments"],
        json!({"count": 2, "note": note}),
        "{r}"
    );
    let bodies: Vec<Value> = r["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["comments"][0]["body"].clone())
        .collect();
    assert!(bodies.contains(&json!("Ada's own")), "{r}");
    assert!(!bodies.contains(&json!("Bob's first")), "{r}");
    let (r, err) = tool(&local, "read", json!({"url_or_id": url})).await;
    assert!(!err, "{r}");
    assert_eq!(r["withheld_comments"]["note"], json!(note), "{r}");
}

/// A remote on this daemon's own port, under another scheme or another
/// local name, is the remote's: its URLs resolve to it, not here.
#[tokio::test]
async fn a_remote_on_this_daemons_port_resolves_as_the_remote() {
    let local = TestServer::spawn().await;
    let port = local.addr.port();
    let connected = |origin: &str, name: &str| {
        let r = local
            .store
            .add_pending_remote(
                origin,
                name,
                &clax_store::remotes::PendingSignIn {
                    device_code: clax_core::Secret::new(format!("cxd_{name}")),
                    user_code: "ABCD-1234".into(),
                    verification_url: format!("{origin}/device?code=ABCD-1234"),
                    expires_at: "2099-01-01T00:00:00.000Z".into(),
                    interval_s: 5,
                },
            )
            .unwrap();
        local
            .store
            .connect_remote(
                &r.id,
                &clax_store::remotes::SignedInAs {
                    token: clax_core::Secret::new(format!("cxa_{name}")),
                    person_public_id: "p1".into(),
                    person_name: None,
                    grant_id: format!("g_{name}"),
                },
            )
            .unwrap()
    };
    let tls = connected(&format!("https://localhost:{port}"), "tls");
    let other = connected(&format!("http://127.0.0.2:{port}"), "other");
    let raid = "7q3k9mzx2b4t";
    for r in [&tls, &other] {
        let url = format!("{}/a/{raid}", r.origin);
        let (s, v) = json_of(
            local
                .get_authed(&format!("/api/resolve?url={}", enc(&url)))
                .await,
        )
        .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["kind"], "remote", "{url}: {v}");
        assert_eq!(v["remote"], json!(r.id), "{url}: {v}");
    }
    // This daemon's own URL on its port is still its own.
    let mine = format!("http://localhost:{port}/a/{raid}");
    let (_, v) = json_of(
        local
            .get_authed(&format!("/api/resolve?url={}", enc(&mine)))
            .await,
    )
    .await;
    assert_eq!(v["kind"], "local", "{v}");
}

/// A connected remote row at `origin` here, named `name`, as a finished
/// sign-in leaves it.
fn connected_at(local: &TestServer, origin: &str, name: &str) -> clax_store::remotes::Remote {
    let r = local
        .store
        .add_pending_remote(
            origin,
            name,
            &clax_store::remotes::PendingSignIn {
                device_code: clax_core::Secret::new(format!("cxd_{name}")),
                user_code: "ABCD-1234".into(),
                verification_url: format!("{origin}/device?code=ABCD-1234"),
                expires_at: "2099-01-01T00:00:00.000Z".into(),
                interval_s: 5,
            },
        )
        .unwrap();
    local
        .store
        .connect_remote(
            &r.id,
            &clax_store::remotes::SignedInAs {
                token: clax_core::Secret::new(format!("cxa_{name}")),
                person_public_id: "p1".into(),
                person_name: None,
                grant_id: format!("g_{name}"),
            },
        )
        .unwrap()
}

/// A stand-in remote that answers every request 401 with error `code`:
/// its origin, and the task serving it.
async fn refusing(code: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    use axum::response::IntoResponse;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let app = axum::Router::new().fallback(move || async move {
        (
            reqwest::StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error": {"code": code, "message": "refused"}})),
        )
            .into_response()
    });
    let serving = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, serving)
}

/// Only a 401 saying the token is gone signs a remote out; one refusing a
/// request (a session proof, a replay) keeps the token and is passed on as
/// `remote_refused` with the remote's code.
#[tokio::test]
async fn only_a_refused_token_signs_a_remote_out() {
    let local = TestServer::spawn().await;
    let raid = "7q3k9mzx2b4t";
    for (code, signs_out) in [
        ("invalid_session_proof", false),
        ("replayed_request", false),
        ("token_expired", true),
        ("grant_revoked", true),
    ] {
        let (origin, serving) = refusing(code).await;
        let r = connected_at(&local, &origin, &code.replace('_', "-"));
        let (s, v) = local_req(
            &local,
            Method::GET,
            &format!("/api/remotes/{}/artifacts/{raid}", r.id),
            json!(null),
            None,
        )
        .await;
        let now = local.store.find_remote(&r.id).unwrap().unwrap();
        if signs_out {
            assert_eq!(
                (s, v["error"]["code"].as_str()),
                (502, Some("remote_sign_in_required")),
                "{code}: {v}"
            );
            assert_eq!(now.status, "signed_out", "{code}");
            assert!(local.store.remote_token(&r.id).unwrap().is_none(), "{code}");
        } else {
            assert_eq!(
                (s, v["error"]["code"].as_str()),
                (502, Some("remote_refused")),
                "{code}: {v}"
            );
            assert_eq!(v["error"]["remote_code"], code, "{v}");
            assert_eq!(now.status, "connected", "{code}");
            assert!(local.store.remote_token(&r.id).unwrap().is_some(), "{code}");
        }
        serving.abort();
    }
}

/// An agent with no session here writes nothing there: a write there in
/// the person's by-hand session is the owner's alone.
#[tokio::test]
async fn an_agent_with_no_session_writes_nothing_there() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let raid = remote_artifact(&public).await;
    let t = browser_thread(&public, &raid, &public.cookie, "@agent look").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let base = format!("/api/remotes/{rid}/artifacts/{raid}");
    for (path, body) in [
        (
            format!("{base}/threads/{tid}/comments"),
            json!({"body": "hi", "author_kind": "agent"}),
        ),
        (
            format!("{base}/threads/{tid}/resolve"),
            json!({"as": "agent"}),
        ),
        (format!("{base}/threads/{tid}/reopen"), json!({})),
        (format!("{base}/threads/{tid}/handoff"), json!({})),
    ] {
        let (s, v) = json_of(
            local
                .authed(local.client.post(format!("{}{path}", local.base)))
                .header("x-clax-channel", "agent")
                .json(&body)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (403, Some("no_session")),
            "{path}: {v}"
        );
    }
    // It still reads.
    let (s, _) = json_of(
        local
            .authed(local.client.get(format!("{}{base}/threads", local.base)))
            .header("x-clax-channel", "agent")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200);
    let there = thread_there(&public, &raid, &tid).await;
    assert_eq!(there["comments"].as_array().unwrap().len(), 1, "{there}");
    assert_eq!(there["status"], "open", "{there}");
}
