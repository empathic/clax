mod common;
use artifax_server::testing::TestViewer;
use common::TestServer;
use reqwest::Method;
use serde_json::{Value, json};

enum Who<'a> {
    Token,
    Viewer(&'a TestViewer),
    Nobody,
}

fn req(ts: &TestServer, m: Method, path: &str, who: &Who) -> reqwest::RequestBuilder {
    let r = ts.client.request(m, format!("{}{}", ts.base, path));
    match who {
        Who::Token => r.bearer_auth(&ts.token),
        Who::Viewer(v) => r.header("cookie", format!("artifax_viewer={}", v.cookie)),
        Who::Nobody => r,
    }
}

async fn send(r: reqwest::RequestBuilder) -> (u16, Value) {
    let res = r.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

async fn artifact(ts: &TestServer, caps: Value) -> String {
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Tracker", "capabilities": caps, "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn put_get_patch_delete_round_trip_with_pins() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let url = format!("/api/artifacts/{aid}/docs/tasks/t1");
    let (s, v) =
        send(req(&ts, Method::PUT, &url, &Who::Token).json(&json!({"data": {"title": "Ship"}})))
            .await;
    assert_eq!(
        (s, v["created"].as_bool(), v["doc"]["version"].as_u64()),
        (200, Some(true), Some(1)),
        "{v}"
    );
    let (s, v) =
        send(req(&ts, Method::PUT, &url, &Who::Token).json(&json!({"data": {"title": "x"}}))).await;
    assert_eq!(
        (
            s,
            v["error"]["code"].as_str(),
            v["error"]["current"].as_u64()
        ),
        (400, Some("if_version_required"), Some(1))
    );
    let (s, v) = send(
        req(&ts, Method::PATCH, &url, &Who::Token)
            .json(&json!({"data": {"done": true}, "if_version": 9})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str(), v["error"]["path"].as_str()),
        (409, Some("conflict"), Some("tasks/t1"))
    );
    let (s, v) = send(
        req(&ts, Method::PATCH, &url, &Who::Token)
            .json(&json!({"data": {"done": true}, "if_version": 1})),
    )
    .await;
    assert_eq!(
        (s, v["doc"]["data"].clone()),
        (200, json!({"title": "Ship", "done": true}))
    );
    let (s, v) = send(req(&ts, Method::GET, &url, &Who::Nobody)).await;
    assert_eq!(
        (s, v["doc"]["version"].as_u64(), v["doc"]["id"].as_str()),
        (200, Some(2), Some("t1"))
    );
    let (s, v) = send(req(
        &ts,
        Method::DELETE,
        &format!("{url}?if_version=2"),
        &Who::Token,
    ))
    .await;
    assert_eq!((s, v["deleted"].as_bool()), (200, Some(true)));
    assert_eq!(send(req(&ts, Method::GET, &url, &Who::Token)).await.0, 404);
    let (s, v) = send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/tasks"),
            &Who::Token,
        )
        .json(&json!({"data": {}})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_argument")),
        "odd path"
    );
}

#[tokio::test]
async fn levels_follow_the_token_the_viewer_name_and_as_level() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {"rules": [{"path": "admin-only", "write": "admin"}, {"path": "owner-only", "write": "owner"}]}})).await;
    let named = ts.viewer(Some("Sam")).await;
    let unnamed = ts.viewer(None).await;
    let put = |path: &str, who: Who<'_>, lww: bool| {
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/{path}"),
            &who,
        )
        .json(&json!({"data": {"n": 1}, "lww": lww}))
    };
    assert_eq!(
        send(put("notes/n1", Who::Viewer(&named), true)).await.0,
        200,
        "named viewer: interact"
    );
    assert_eq!(
        send(put("notes/n2", Who::Viewer(&unnamed), true)).await.0,
        404,
        "unnamed viewer: view"
    );
    assert_eq!(
        send(put("notes/n3", Who::Nobody, true)).await.0,
        404,
        "no cookie: view"
    );
    assert_eq!(
        send(put("admin-only/a", Who::Viewer(&named), true)).await.0,
        404,
        "interact below admin"
    );
    assert_eq!(
        send(put("admin-only/a", Who::Token, true)).await.0,
        200,
        "token without a cookie: owner"
    );
    assert_eq!(
        send(put("owner-only/a", Who::Token, true)).await.0,
        200,
        "an agent meets an owner rule"
    );
    let shell = req(
        &ts,
        Method::PUT,
        &format!("/api/artifacts/{aid}/docs/owner-only/b"),
        &Who::Token,
    )
    .header("cookie", format!("artifax_viewer={}", named.cookie))
    .json(&json!({"data": {}, "lww": true}));
    assert_eq!(
        send(shell).await.0,
        404,
        "token with a cookie (the owner shell): admin, below owner"
    );
    let shell_admin = req(
        &ts,
        Method::PUT,
        &format!("/api/artifacts/{aid}/docs/admin-only/c"),
        &Who::Token,
    )
    .header("cookie", format!("artifax_viewer={}", named.cookie))
    .json(&json!({"data": {}, "lww": true}));
    assert_eq!(
        send(shell_admin).await.0,
        200,
        "the owner shell meets admin"
    );
    let narrowed = req(
        &ts,
        Method::PUT,
        &format!("/api/artifacts/{aid}/docs/admin-only/b?as_level=interact"),
        &Who::Token,
    )
    .json(&json!({"data": {}}));
    assert_eq!(send(narrowed).await.0, 404, "as_level narrows");
    let (s, v) = send(req(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/docs/notes/n1?as_level=owner"),
        &Who::Token,
    ))
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_argument")),
        "as_level never raises"
    );
    assert_eq!(
        send(req(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{aid}/docs/notes/n1"),
            &Who::Viewer(&unnamed)
        ))
        .await
        .0,
        200,
        "view reads"
    );
}

#[tokio::test]
async fn private_documents_stay_private_over_http() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let a = ts.viewer(Some("A")).await;
    let b = ts.viewer(Some("B")).await;
    let mine = format!("/api/artifacts/{aid}/docs/data/users/{}/pick", a.public_id);
    assert_eq!(
        send(
            req(&ts, Method::PUT, &mine, &Who::Viewer(&a))
                .json(&json!({"data": {"n": 3}, "lww": true}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        send(req(&ts, Method::GET, &mine, &Who::Viewer(&a))).await.0,
        200
    );
    assert_eq!(
        send(req(&ts, Method::GET, &mine, &Who::Viewer(&b))).await.0,
        404
    );
    assert_eq!(
        send(req(&ts, Method::GET, &mine, &Who::Token)).await.0,
        404,
        "an agent (owner) too"
    );
    let list = format!(
        "/api/artifacts/{aid}/docs?collection=data/users/{}",
        a.public_id
    );
    assert_eq!(
        send(req(&ts, Method::GET, &list, &Who::Viewer(&b))).await.1["docs"],
        json!([])
    );
    assert_eq!(
        send(req(&ts, Method::GET, &list, &Who::Viewer(&a))).await.1["docs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        send(
            req(&ts, Method::PUT, &mine, &Who::Viewer(&b)).json(&json!({"data": {}, "lww": true}))
        )
        .await
        .0,
        404
    );
}

#[tokio::test]
async fn queries_take_json_where_order_and_cursor() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    for (k, n) in [("a", 3), ("b", 1), ("c", 2)] {
        send(
            req(
                &ts,
                Method::PUT,
                &format!("/api/artifacts/{aid}/docs/tasks/{k}"),
                &Who::Token,
            )
            .json(&json!({"data": {"n": n}})),
        )
        .await;
    }
    let q = |qs: &str| {
        req(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{aid}/docs?collection=tasks&{qs}"),
            &Who::Token,
        )
    };
    let ids = |v: &Value| {
        v["docs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let w = urlencoding(r#"[["n",">",1]]"#);
    let (_, v) = send(q(&format!("where={w}&order_by=n&direction=desc"))).await;
    assert_eq!(
        (ids(&v), v["next_cursor"].clone()),
        (vec!["a".to_string(), "c".to_string()], Value::Null)
    );
    let (_, v) = send(q("limit=2")).await;
    assert_eq!(
        (ids(&v), v["next_cursor"].as_str()),
        (vec!["a".to_string(), "b".to_string()], Some("b"))
    );
    let (_, v) = send(q("limit=2&cursor=b")).await;
    assert_eq!(ids(&v), vec!["c".to_string()]);
    let (s, v) = send(q("where=nope")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_argument"))
    );
    let (s, _) = send(q("direction=sideways")).await;
    assert_eq!(s, 400);
}

fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn batches_are_atomic_over_http() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let url = format!("/api/artifacts/{aid}/docs:batch");
    let (s, v) = send(
        req(&ts, Method::POST, &url, &Who::Token).json(&json!({"writes": [
            {"op": "set", "path": "t/1", "data": {"n": 1}},
            {"op": "update", "path": "t/2", "data": {"n": 2}}
        ]})),
    )
    .await;
    assert_eq!(
        (
            s,
            v["error"]["code"].as_str(),
            v["error"]["op"].as_u64(),
            v["error"]["path"].as_str()
        ),
        (400, Some("invalid_argument"), Some(1), Some("t/2")),
        "update of a missing document fails the batch, naming the failing write's index"
    );
    assert_eq!(
        send(req(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{aid}/docs/t/1"),
            &Who::Token
        ))
        .await
        .0,
        404,
        "nothing landed"
    );
    let (s, v) = send(
        req(&ts, Method::POST, &url, &Who::Token).json(&json!({"writes": [
            {"op": "set", "path": "t/1", "data": {"n": 1}}, {"op": "delete", "path": "t/9"}
        ]})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["results"],
        json!([{"op": "set", "path": "t/1", "version": 1, "deleted": false}, {"op": "delete", "path": "t/9", "version": null, "deleted": false}])
    );
    let (s, _) = send(
        req(&ts, Method::POST, &url, &Who::Token)
            .json(&json!({"writes": [{"op": "move", "path": "t/1"}]})),
    )
    .await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn str_replace_and_acquire_over_http() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/p/1"),
            &Who::Token,
        )
        .json(&json!({"data": {"html": "<h1>Old</h1>"}})),
    )
    .await;
    let (s, v) = send(req(&ts, Method::POST, &format!("/api/artifacts/{aid}/docs:str_replace"), &Who::Token)
        .json(&json!({"path": "p/1", "field": "html", "old_str": "Old", "new_str": "New", "if_version": 1}))).await;
    assert_eq!(
        (s, v["doc"]["data"]["html"].as_str()),
        (200, Some("<h1>New</h1>"))
    );
    let acq = |holder: &str| {
        req(
            &ts,
            Method::POST,
            &format!("/api/artifacts/{aid}/docs:acquire"),
            &Who::Token,
        )
        .json(&json!({"path": "locks/l", "holder": holder, "ttl_ms": 60000}))
    };
    let (_, a) = send(acq("tab-a")).await;
    let (_, b) = send(acq("tab-b")).await;
    assert_eq!(
        (a["acquired"].as_bool(), a["holder"].as_str()),
        (Some(true), Some("tab-a"))
    );
    assert_eq!(
        (b["acquired"].as_bool(), b["holder"].clone()),
        (Some(false), Value::Null)
    );
    assert_eq!(a["expires_at"], b["expires_at"]);
}

#[tokio::test]
async fn doc_events_carry_path_and_version_only() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let mut events = ts.events(&format!("?artifact={aid}")).await;
    let url = format!("/api/artifacts/{aid}/docs/tasks/t1");
    send(req(&ts, Method::PUT, &url, &Who::Token).json(&json!({"data": {"secret": "body"}}))).await;
    assert_eq!(
        events.next_named("doc").await,
        json!({"type": "doc", "artifact_id": aid, "path": "tasks/t1", "version": 1})
    );
    send(req(
        &ts,
        Method::DELETE,
        &format!("{url}?if_version=1"),
        &Who::Token,
    ))
    .await;
    assert_eq!(events.next_named("doc").await["version"], Value::Null);
}

#[tokio::test]
async fn doc_events_for_private_paths_reach_only_their_owner() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let a = ts.viewer(Some("A")).await;
    let b = ts.viewer(Some("B")).await;
    let mut as_a = ts
        .events_as(&format!("?artifact={aid}"), Some(&a.cookie))
        .await;
    let mut as_b = ts
        .events_as(&format!("?artifact={aid}"), Some(&b.cookie))
        .await;
    let mut anon = ts.events(&format!("?artifact={aid}")).await;
    let private = format!("data/users/{}/pick", a.public_id);
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/{private}"),
            &Who::Viewer(&a),
        )
        .json(&json!({"data": {"n": 1}, "lww": true})),
    )
    .await;
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/shared/s"),
            &Who::Viewer(&a),
        )
        .json(&json!({"data": {"n": 1}, "lww": true})),
    )
    .await;
    assert_eq!(as_a.next_named("doc").await["path"], private.as_str());
    assert_eq!(as_a.next_named("doc").await["path"], "shared/s");
    assert_eq!(
        as_b.next_named("doc").await["path"],
        "shared/s",
        "B never sees A's private path"
    );
    assert_eq!(anon.next_named("doc").await["path"], "shared/s");
}

#[tokio::test]
async fn sse_never_carries_a_viewer_cookie() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let a = ts.viewer(Some("A")).await;
    let mut as_a = ts
        .events_as(&format!("?artifact={aid}"), Some(&a.cookie))
        .await;
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/data/users/{}/p", a.public_id),
            &Who::Viewer(&a),
        )
        .json(&json!({"data": {}, "lww": true})),
    )
    .await;
    let ev = as_a.next_named("doc").await;
    assert!(ev["path"].as_str().unwrap().contains(&a.public_id));
    assert!(!ev.to_string().contains(&a.cookie));
}

#[tokio::test]
async fn docs_routes_refuse_foreign_origins_and_unknown_artifacts() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let port = ts.addr.port();
    let (s, v) = send(
        req(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{aid}/docs/t/1"),
            &Who::Nobody,
        )
        .header("origin", format!("http://{aid}.localhost:{port}")),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden_origin"))
    );
    assert_eq!(
        send(req(
            &ts,
            Method::GET,
            "/api/artifacts/zzzzzzzzzzzz/docs/t/1",
            &Who::Token
        ))
        .await
        .0,
        404
    );
}

#[tokio::test]
async fn doc_events_follow_the_subscribers_level() {
    let ts = TestServer::spawn().await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [
            {"path": "secret", "read": "owner", "write": "owner"},
            {"path": "staff", "read": "admin", "write": "admin"},
            {"path": "team", "read": "interact"}
        ]}}),
    )
    .await;
    let named = ts.viewer(Some("Sam")).await;
    let shell_viewer = ts.viewer(Some("Owner")).await;
    let q = format!("?artifact={aid}");
    let shell_q = format!("{q}&token={}", ts.token);
    let mut view = ts.events(&q).await;
    let mut interact = ts.events_as(&q, Some(&named.cookie)).await;
    let mut shell = ts.events_as(&shell_q, Some(&shell_viewer.cookie)).await;
    let mut agent = ts.events_with(&q, |r| r.bearer_auth(&ts.token)).await;
    for path in ["secret/s", "staff/s", "team/t", "open/o"] {
        send(
            req(
                &ts,
                Method::PUT,
                &format!("/api/artifacts/{aid}/docs/{path}"),
                &Who::Token,
            )
            .json(&json!({"data": {}})),
        )
        .await;
    }
    assert_eq!(view.next_named("doc").await["path"], "open/o");
    assert_eq!(interact.next_named("doc").await["path"], "team/t");
    assert_eq!(interact.next_named("doc").await["path"], "open/o");
    for want in ["staff/s", "team/t", "open/o"] {
        assert_eq!(
            shell.next_named("doc").await["path"],
            want,
            "the owner shell is admin"
        );
    }
    for want in ["secret/s", "staff/s", "team/t", "open/o"] {
        assert_eq!(
            agent.next_named("doc").await["path"],
            want,
            "the token is owner"
        );
    }
}

#[tokio::test]
async fn a_subscribers_level_is_fixed_when_its_stream_opens() {
    // The shell opens its stream only after the viewer lookup has set the
    // cookie (Task 5). A stream opened with the token but before the cookie
    // is `owner` with no viewer, so it never hears its own viewer's private
    // documents; the stream opened after the cookie does.
    let ts = TestServer::spawn().await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "staff", "read": "admin", "write": "admin"}]}}),
    )
    .await;
    let shell_q = format!("?artifact={aid}&token={}", ts.token);
    let mut before_cookie = ts.events(&shell_q).await;
    let owner = ts.viewer(Some("Owner")).await;
    let mut after_cookie = ts.events_as(&shell_q, Some(&owner.cookie)).await;
    let private = format!("data/users/{}/p", owner.public_id);
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/{private}"),
            &Who::Viewer(&owner),
        )
        .json(&json!({"data": {}, "lww": true})),
    )
    .await;
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/staff/s"),
            &Who::Token,
        )
        .json(&json!({"data": {}})),
    )
    .await;
    assert_eq!(before_cookie.next_named("doc").await["path"], "staff/s");
    assert_eq!(
        after_cookie.next_named("doc").await["path"],
        private.as_str()
    );
    assert_eq!(after_cookie.next_named("doc").await["path"], "staff/s");
}

#[tokio::test]
async fn private_doc_events_skip_the_owner_shell_and_agents() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let a = ts.viewer(Some("A")).await;
    let owner = ts.viewer(Some("Owner")).await;
    let q = format!("?artifact={aid}");
    let mut shell = ts
        .events_as(&format!("{q}&token={}", ts.token), Some(&owner.cookie))
        .await;
    let mut agent = ts.events(&format!("{q}&token={}", ts.token)).await;
    let mut mine = ts.events_as(&q, Some(&a.cookie)).await;
    let private = format!("data/users/{}/p", a.public_id);
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/{private}"),
            &Who::Viewer(&a),
        )
        .json(&json!({"data": {}, "lww": true})),
    )
    .await;
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/shared/s"),
            &Who::Viewer(&a),
        )
        .json(&json!({"data": {}, "lww": true})),
    )
    .await;
    assert_eq!(mine.next_named("doc").await["path"], private.as_str());
    assert_eq!(shell.next_named("doc").await["path"], "shared/s");
    assert_eq!(agent.next_named("doc").await["path"], "shared/s");
}

#[tokio::test]
async fn a_wrong_stream_token_counts_as_none() {
    let ts = TestServer::spawn().await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "team", "read": "interact"}]}}),
    )
    .await;
    let a = ts.viewer(Some("A")).await;
    let mut wrong = ts
        .events(&format!("?artifact={aid}&token=not-the-token"))
        .await;
    let mut garbled = ts
        .events(&format!("?artifact={aid}&token=%zz{}", ts.token))
        .await;
    let private = format!("data/users/{}/p", a.public_id);
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/{private}"),
            &Who::Viewer(&a),
        )
        .json(&json!({"data": {}, "lww": true})),
    )
    .await;
    for path in ["team/t", "open/o"] {
        send(
            req(
                &ts,
                Method::PUT,
                &format!("/api/artifacts/{aid}/docs/{path}"),
                &Who::Token,
            )
            .json(&json!({"data": {}})),
        )
        .await;
    }
    assert_eq!(
        wrong.next_named("doc").await["path"],
        "open/o",
        "view, no viewer: neither the private nor the interact document"
    );
    assert_eq!(
        garbled.next_named("doc").await["path"],
        "open/o",
        "an invalid encoding is no token"
    );
}

#[tokio::test]
async fn query_values_are_percent_decoded() {
    let ts = TestServer::spawn().await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "secret", "read": "owner", "write": "owner"}]}}),
    )
    .await;
    let encoded: String = ts.token.bytes().map(|b| format!("%{b:02X}")).collect();
    let mut agent = ts.events(&format!("?artifact={aid}&token={encoded}")).await;
    send(
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/secret/s"),
            &Who::Token,
        )
        .json(&json!({"data": {}})),
    )
    .await;
    assert_eq!(
        agent.next_named("doc").await["path"],
        "secret/s",
        "an encoded token is the token"
    );
    let narrowed = req(
        &ts,
        Method::PUT,
        &format!("/api/artifacts/{aid}/docs/notes/n?as_level=%69nteract"),
        &Who::Token,
    )
    .json(&json!({"data": {}}));
    let (s, v) = send(narrowed).await;
    assert_eq!(s, 200, "interact may write an undeclared path: {v}");
    let narrowed = req(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/docs/secret/s?as_level=%61dmin"),
        &Who::Token,
    );
    assert_eq!(send(narrowed).await.0, 404, "an encoded as_level narrows");
    let (s, v) = send(req(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/docs/secret/s?as_level=%zz"),
        &Who::Token,
    ))
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_argument")),
        "an invalid encoding is refused"
    );
}

#[tokio::test]
async fn an_oversized_batch_names_the_docs_batch_limit() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let big = "x".repeat(15 * 1024 * 1024);
    let (s, v) = send(
        req(
            &ts,
            Method::POST,
            &format!("/api/artifacts/{aid}/docs:batch"),
            &Who::Token,
        )
        .json(&json!({"writes": [{"op": "set", "path": "t/1", "data": {"s": big}}]})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (413, Some("body_too_large"))
    );
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("the docs batch limit"),
        "{v}"
    );
}

#[tokio::test]
async fn document_404s_name_the_path_and_artifact_404s_do_not() {
    let ts = TestServer::spawn().await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "locked", "read": "admin", "write": "admin"}]}}),
    )
    .await;
    let t = |m: Method, p: &str| req(&ts, m, p, &Who::Token);
    t(Method::PUT, &format!("/api/artifacts/{aid}/docs/locked/x"))
        .json(&json!({"data": {"n": 1}}))
        .send()
        .await
        .unwrap();
    let doc_404 =
        |path: &str| json!({"error": {"code": "not_found", "message": "not found", "path": path}});

    let (s, missing) = send(t(
        Method::GET,
        &format!("/api/artifacts/{aid}/docs/tasks/nope"),
    ))
    .await;
    assert_eq!(
        (s, missing),
        (404, doc_404("tasks/nope")),
        "a missing document"
    );
    let (s, hidden) = send(t(
        Method::GET,
        &format!("/api/artifacts/{aid}/docs/locked/x?as_level=interact"),
    ))
    .await;
    assert_eq!(
        (s, hidden),
        (404, doc_404("locked/x")),
        "a hidden document reads as missing"
    );
    let (s, refused) = send(
        t(
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/locked/y?as_level=interact"),
        )
        .json(&json!({"data": {}})),
    )
    .await;
    assert_eq!((s, refused), (404, doc_404("locked/y")), "a refused write");
    let (s, v) = send(
        t(
            Method::POST,
            &format!("/api/artifacts/{aid}/docs:str_replace"),
        )
        .json(&json!({"path": "tasks/nope", "field": "f", "old_str": "a", "new_str": "b"})),
    )
    .await;
    assert_eq!(
        (s, v),
        (404, doc_404("tasks/nope")),
        "str_replace on a missing document"
    );

    // A missing artifact keeps the phase 1 body: no path.
    let artifact_404 = json!({"error": {"code": "not_found", "message": "not found"}});
    let gone = "7q3k9mzx2b4t";
    for r in [
        t(Method::GET, &format!("/api/artifacts/{gone}/docs/tasks/t1")),
        t(Method::PUT, &format!("/api/artifacts/{gone}/docs/tasks/t1")).json(&json!({"data": {}})),
        t(
            Method::GET,
            &format!("/api/artifacts/{gone}/docs?collection=tasks"),
        ),
        t(Method::POST, &format!("/api/artifacts/{gone}/docs:batch"))
            .json(&json!({"writes": [{"op": "delete", "path": "tasks/t1"}]})),
    ] {
        assert_eq!(send(r).await, (404, artifact_404.clone()));
    }
}

#[tokio::test]
async fn rules_changed_by_patch_apply_to_the_next_call() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let named = ts.viewer(Some("Sam")).await;
    let put = || {
        req(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/notes/n1"),
            &Who::Viewer(&named),
        )
        .json(&json!({"data": {"n": 1}, "lww": true}))
    };
    assert_eq!(send(put()).await.0, 200);
    let res = ts
        .authed(ts.client.patch(format!("{}/api/artifacts/{aid}", ts.base)))
        .json(&json!({"capabilities": {"db": {"rules": [{"path": "", "write": "admin"}]}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(send(put()).await.0, 404);
}
