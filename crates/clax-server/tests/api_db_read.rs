//! Cross-app reads over HTTP (spec 2026-10-12-flutter-app-design §3.8): the
//! `X-Clax-Reader` header on B's document reads, `?reader=` on the event
//! stream and the `docs:<B>?reader=<A>` topic, the owner's grant routes, and
//! `docs:collections`.

use crate::common;
use clax_server::testing::EventReader;
use common::TestServer;
use reqwest::Method;
use serde_json::{Value, json};

const READER: &str = "x-clax-reader";

async fn app(ts: &TestServer, title: &str, caps: Value) -> String {
    let res = ts
        .authed(ts.client.post(format!("{}/api/artifacts", ts.base)))
        .json(&json!({"title": title, "capabilities": caps, "files": {"index.html": {"content": "<p>x</p>"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn patch_caps(ts: &TestServer, aid: &str, caps: Value) {
    let res = ts
        .authed(ts.client.patch(format!("{}/api/artifacts/{aid}", ts.base)))
        .json(&json!({"capabilities": caps}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

async fn send(r: reqwest::RequestBuilder) -> (u16, Value) {
    let res = r.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

/// A request with the token, as the owner's shell or an agent makes it.
fn authed(ts: &TestServer, m: Method, path: &str) -> reqwest::RequestBuilder {
    ts.authed(ts.client.request(m, format!("{}{path}", ts.base)))
}

/// Writes `data` at `path` of `aid` with the token.
async fn seed(ts: &TestServer, aid: &str, path: &str, data: Value) {
    let (status, body) = send(
        authed(
            ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/{path}"),
        )
        .json(&json!({"data": data, "lww": true})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

/// `GET /api/artifacts/<source>/docs/<path>` with the token and
/// `X-Clax-Reader: <reader>`.
async fn read_as(ts: &TestServer, reader: &str, source: &str, path: &str) -> (u16, Value) {
    send(
        authed(
            ts,
            Method::GET,
            &format!("/api/artifacts/{source}/docs/{path}"),
        )
        .header(READER, reader),
    )
    .await
}

/// `GET /api/artifacts/<source>/docs?collection=<c>` with the token and
/// `X-Clax-Reader: <reader>`.
async fn list_as(ts: &TestServer, reader: &str, source: &str, c: &str) -> (u16, Value) {
    send(
        authed(
            ts,
            Method::GET,
            &format!("/api/artifacts/{source}/docs?collection={c}"),
        )
        .header(READER, reader),
    )
    .await
}

fn assert_not_granted(got: &(u16, Value), reader: &str, source: &str) {
    assert_eq!(got.0, 403, "{}", got.1);
    assert_eq!(got.1["error"]["code"], "not_granted", "{}", got.1);
    assert_eq!(
        got.1["error"]["message"],
        format!("{reader} may not read {source}'s documents")
    );
}

async fn grant(ts: &TestServer, reader: &str, source: &str) -> (u16, Value) {
    send(authed(
        ts,
        Method::PUT,
        &format!("/api/artifacts/{reader}/db-grants/{source}"),
    ))
    .await
}

async fn revoke(ts: &TestServer, reader: &str, source: &str) -> (u16, Value) {
    send(authed(
        ts,
        Method::DELETE,
        &format!("/api/artifacts/{reader}/db-grants/{source}"),
    ))
    .await
}

/// A source with one document, `runs/r1`, and a reader declaring nothing.
async fn pair(ts: &TestServer) -> (String, String) {
    let b = app(ts, "Runs", json!({"db": {}})).await;
    seed(ts, &b, "runs/r1", json!({"km": 5})).await;
    let a = app(ts, "Coach", json!({"db": {}})).await;
    (a, b)
}

#[tokio::test]
async fn reader_without_declaration_is_not_granted() {
    let ts = TestServer::spawn().await;
    let (a, b) = pair(&ts).await;
    assert_not_granted(&read_as(&ts, &a, &b, "runs/r1").await, &a, &b);
    assert_not_granted(&list_as(&ts, &a, &b, "runs").await, &a, &b);
    // A grant without the declaration reads nothing either.
    assert_eq!(grant(&ts, &a, &b).await.0, 200);
    assert_not_granted(&read_as(&ts, &a, &b, "runs/r1").await, &a, &b);
}

#[tokio::test]
async fn declared_reader_without_share_or_grant_is_not_granted() {
    let ts = TestServer::spawn().await;
    let (a, b) = pair(&ts).await;
    patch_caps(&ts, &a, json!({"db": {"read": [b]}})).await;
    assert_not_granted(&read_as(&ts, &a, &b, "runs/r1").await, &a, &b);
    assert_not_granted(&list_as(&ts, &a, &b, "runs").await, &a, &b);
    // A share with someone else is no share with A.
    let c = app(&ts, "Other", json!({"db": {}})).await;
    patch_caps(&ts, &b, json!({"db": {"share": {"read": [c]}}})).await;
    assert_not_granted(&read_as(&ts, &a, &b, "runs/r1").await, &a, &b);
}

#[tokio::test]
async fn reader_header_must_name_an_artifact() {
    let ts = TestServer::spawn().await;
    let (_a, b) = pair(&ts).await;
    let got = read_as(&ts, "not an id", &b, "runs/r1").await;
    assert_eq!(got.0, 400, "{}", got.1);
    assert_eq!(got.1["error"]["code"], "invalid_args");
    let got = read_as(&ts, "zzzzzzzzzzzz", &b, "runs/r1").await;
    assert_eq!(got.0, 404, "an unknown reader: {}", got.1);
}

#[tokio::test]
async fn share_lets_the_declared_reader_read() {
    let ts = TestServer::spawn().await;
    let (a, b) = pair(&ts).await;
    patch_caps(&ts, &a, json!({"db": {"read": [b]}})).await;
    patch_caps(&ts, &b, json!({"db": {"share": {"read": [a]}}})).await;
    let got = read_as(&ts, &a, &b, "runs/r1").await;
    assert_eq!(got.0, 200, "{}", got.1);
    assert_eq!(got.1["doc"]["data"], json!({"km": 5}));
    let got = list_as(&ts, &a, &b, "runs").await;
    assert_eq!(got.0, 200, "{}", got.1);
    assert_eq!(got.1["docs"].as_array().unwrap().len(), 1);
    // Without the token, a reader is still at most `view`.
    let got = send(
        ts.client
            .get(format!("{}/api/artifacts/{b}/docs/runs/r1", ts.base))
            .header(READER, &a),
    )
    .await;
    assert_eq!(got.0, 200, "{}", got.1);

    // B stops sharing: the next read is refused.
    patch_caps(&ts, &b, json!({"db": {}})).await;
    assert_not_granted(&read_as(&ts, &a, &b, "runs/r1").await, &a, &b);
}

#[tokio::test]
async fn grant_lets_the_declared_reader_read_and_revoke_ends_it() {
    let ts = TestServer::spawn().await;
    let (a, b) = pair(&ts).await;
    patch_caps(&ts, &a, json!({"db": {"read": [b]}})).await;

    let (status, body) = grant(&ts, &a, &b).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["grant"]["source_id"], b.as_str());
    let at = body["grant"]["granted_at"].as_str().unwrap().to_string();
    let (status, again) = grant(&ts, &a, &b).await;
    assert_eq!(status, 200);
    assert_eq!(
        again["grant"]["granted_at"], at,
        "granting again keeps the grant"
    );
    let (status, list) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{a}/db-grants"),
    ))
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        list,
        json!({"grants": [{"source_id": b, "granted_at": at}]})
    );

    let got = read_as(&ts, &a, &b, "runs/r1").await;
    assert_eq!(got.0, 200, "{}", got.1);
    assert_eq!(got.1["doc"]["data"], json!({"km": 5}));

    assert_eq!(revoke(&ts, &a, &b).await, (200, json!({"revoked": true})));
    assert_eq!(revoke(&ts, &a, &b).await, (200, json!({"revoked": false})));
    assert_not_granted(&read_as(&ts, &a, &b, "runs/r1").await, &a, &b);
    let (_, list) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{a}/db-grants"),
    ))
    .await;
    assert_eq!(list, json!({"grants": []}));
}

#[tokio::test]
async fn reader_header_never_writes() {
    let ts = TestServer::spawn().await;
    let (a, b) = pair(&ts).await;
    patch_caps(&ts, &a, json!({"db": {"read": [b]}})).await;
    assert_eq!(grant(&ts, &a, &b).await.0, 200);
    let doc = format!("/api/artifacts/{b}/docs/runs/r1");
    let writes = [
        authed(&ts, Method::PUT, &doc).json(&json!({"data": {"km": 0}, "lww": true})),
        authed(&ts, Method::PATCH, &doc).json(&json!({"data": {"km": 0}, "lww": true})),
        authed(&ts, Method::DELETE, &format!("{doc}?lww=true")),
        authed(&ts, Method::POST, &format!("/api/artifacts/{b}/docs:batch"))
            .json(&json!({"writes": [{"op": "set", "path": "runs/r1", "data": {}}], "lww": true})),
        authed(
            &ts,
            Method::POST,
            &format!("/api/artifacts/{b}/docs:str_replace"),
        )
        .json(
            &json!({"path": "runs/r1", "field": "km", "old_str": "5", "new_str": "0", "lww": true}),
        ),
        authed(
            &ts,
            Method::POST,
            &format!("/api/artifacts/{b}/docs:acquire"),
        )
        .json(&json!({"path": "locks/l", "holder": "h"})),
        // Nor may a reader change its own grants or the source's declaration.
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{a}/db-grants/{b}"),
        ),
        authed(
            &ts,
            Method::DELETE,
            &format!("/api/artifacts/{a}/db-grants/{b}"),
        ),
        authed(&ts, Method::PATCH, &format!("/api/artifacts/{b}"))
            .json(&json!({"capabilities": {"db": {"share": {"read": [a]}}}})),
    ];
    for w in writes {
        let (status, body) = send(w.header(READER, &a)).await;
        assert_eq!(status, 403, "{body}");
        assert_eq!(body["error"]["code"], "read_only", "{body}");
    }
    let got = send(authed(&ts, Method::GET, &doc)).await;
    assert_eq!(got.1["doc"]["data"], json!({"km": 5}), "unchanged");
    assert_eq!(got.1["doc"]["version"], 1);
    assert!(
        send(authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{b}/docs/locks/l")
        ))
        .await
        .0 == 404,
        "no lock taken"
    );
    // The grant is still there.
    assert_eq!(read_as(&ts, &a, &b, "runs/r1").await.0, 200);
}

#[tokio::test]
async fn reader_never_sees_private_documents() {
    let ts = TestServer::spawn().await;
    let (a, b) = pair(&ts).await;
    patch_caps(&ts, &a, json!({"db": {"read": [b]}})).await;
    patch_caps(&ts, &b, json!({"db": {"share": {"read": [a]}}})).await;
    let me = ts.owner_public_id().await;
    let private = format!("data/users/{me}/x");
    // The owner's browser (the token and the owner cookie) writes its own
    // private document.
    let (status, body) = send(
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{b}/docs/{private}"),
        )
        .header("cookie", ts.owner_cookie())
        .json(&json!({"data": {"secret": 1}, "lww": true})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let own = send(
        authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{b}/docs/{private}"),
        )
        .header("cookie", ts.owner_cookie()),
    )
    .await;
    assert_eq!(own.0, 200, "the owner's browser reads it: {}", own.1);

    // The same browser reading as A does not.
    let got = send(
        authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{b}/docs/{private}"),
        )
        .header("cookie", ts.owner_cookie())
        .header(READER, &a),
    )
    .await;
    assert_eq!(got.0, 404, "{}", got.1);
    let got = send(
        authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{b}/docs?collection=data/users/{me}"),
        )
        .header("cookie", ts.owner_cookie())
        .header(READER, &a),
    )
    .await;
    assert_eq!(got.0, 200, "{}", got.1);
    assert_eq!(got.1["docs"], json!([]));
}

#[tokio::test]
async fn reader_level_is_view() {
    let ts = TestServer::spawn().await;
    let b = app(
        &ts,
        "Runs",
        json!({"db": {"rules": [{"path": "secret", "read": "admin", "write": "admin"}]}}),
    )
    .await;
    seed(&ts, &b, "secret/s1", json!({"pin": 1234})).await;
    seed(&ts, &b, "open/o1", json!({"hi": 1})).await;
    let a = app(&ts, "Coach", json!({"db": {"read": [b]}})).await;
    assert_eq!(grant(&ts, &a, &b).await.0, 200);
    assert_eq!(
        send(authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{b}/docs/secret/s1")
        ))
        .await
        .0,
        200,
        "the token alone reads it"
    );
    assert_eq!(read_as(&ts, &a, &b, "secret/s1").await.0, 404);
    assert_eq!(read_as(&ts, &a, &b, "open/o1").await.0, 200);
    let got = list_as(&ts, &a, &b, "secret").await;
    assert_eq!(got.0, 200);
    assert_eq!(got.1["docs"], json!([]));
    // `?as_level` never raises a reader.
    let got = send(
        authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{b}/docs/secret/s1?as_level=admin"),
        )
        .header(READER, &a),
    )
    .await;
    assert_eq!(got.0, 404);
}

#[tokio::test]
async fn stream_delivers_b_doc_events_only_while_granted() {
    let ts = TestServer::spawn().await;
    let b = app(
        &ts,
        "Runs",
        json!({"db": {"rules": [{"path": "secret", "read": "admin", "write": "admin"}]}}),
    )
    .await;
    let a = app(&ts, "Coach", json!({"db": {"read": [b]}})).await;
    assert_eq!(grant(&ts, &a, &b).await.0, 200);
    let me = ts.owner_public_id().await;
    let mut ev = ts
        .events_with(&format!("?artifact={b}&types=doc,thread&reader={a}"), |r| {
            r.bearer_auth(&ts.token)
        })
        .await;
    // Unreadable at `view` with no viewer: never delivered (document
    // versions count per artifact, so the readable one is version 3).
    seed(&ts, &b, "secret/s1", json!({})).await;
    send(
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{b}/docs/data/users/{me}/x"),
        )
        .header("cookie", ts.owner_cookie())
        .json(&json!({"data": {}, "lww": true})),
    )
    .await;
    seed(&ts, &b, "runs/r1", json!({})).await;
    assert_eq!(
        ev.next().await,
        (
            "doc".to_string(),
            json!({"type": "doc", "artifact_id": b, "path": "runs/r1", "version": 3})
        )
    );

    assert_eq!(revoke(&ts, &a, &b).await.0, 200);
    seed(&ts, &b, "runs/r2", json!({})).await;
    ts.thread(&b, 1, "marker one").await;
    assert_eq!(ev.next().await.0, "thread", "no doc event once revoked");

    // Granted again by a share: delivered again.
    patch_caps(&ts, &b, json!({"db": {"share": {"read": [a]}, "rules": [{"path": "secret", "read": "admin", "write": "admin"}]}})).await;
    seed(&ts, &b, "runs/r3", json!({})).await;
    assert_eq!(ev.next().await.1["path"], "runs/r3");

    // A drops its declaration: not delivered.
    patch_caps(&ts, &a, json!({"db": {}})).await;
    seed(&ts, &b, "runs/r4", json!({})).await;
    ts.thread(&b, 1, "marker two").await;
    assert_eq!(ev.next().await.0, "thread");
}

#[tokio::test]
async fn events_reader_must_name_an_artifact() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .get(format!("{}/api/events?reader=nope", ts.base))
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_args");
}

/// Opens `/api/stream` as the owner's browser; the reader and the stream ID.
async fn owner_stream(ts: &TestServer) -> (EventReader, String) {
    let res = ts
        .client
        .get(format!("{}/api/stream", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut events = EventReader::from_response(res);
    let id = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    (events, id)
}

async fn subscribe(ts: &TestServer, stream: &str, topics: &[&str]) -> (u16, Value) {
    send(
        ts.client
            .post(format!("{}/api/stream/{stream}", ts.base))
            .header("cookie", ts.owner_cookie())
            .json(&json!({ "subscribe": topics })),
    )
    .await
}

#[tokio::test]
async fn reader_topic_of_the_shared_stream_delivers_only_while_granted() {
    let ts = TestServer::spawn().await;
    let b = app(
        &ts,
        "Runs",
        json!({"db": {"rules": [{"path": "secret", "read": "admin", "write": "admin"}]}}),
    )
    .await;
    let a = app(&ts, "Coach", json!({"db": {"read": [b]}})).await;
    let topic = format!("docs:{b}?reader={a}");
    let (mut ev, id) = owner_stream(&ts).await;

    let (status, body) = subscribe(&ts, &id, &[&topic]).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"]["code"], "not_granted");
    for bad in [
        format!("docs:{b}?reader=nope"),
        format!("docs:{b}?viewer={a}"),
    ] {
        let (status, body) = subscribe(&ts, &id, &[&bad]).await;
        assert_eq!(status, 400, "{bad}: {body}");
        assert_eq!(body["error"]["code"], "invalid_topic");
    }

    assert_eq!(grant(&ts, &a, &b).await.0, 200);
    let artifact_topic = format!("artifact:{b}");
    let (status, body) = subscribe(&ts, &id, &[&topic, &artifact_topic]).await;
    assert_eq!(status, 200, "{body}");
    let me = ts.owner_public_id().await;
    // The stream's caller is the owner's browser (admin, the owner's
    // viewer), but the reader topic carries what `view` with no viewer reads.
    seed(&ts, &b, "secret/s1", json!({})).await;
    send(
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{b}/docs/data/users/{me}/x"),
        )
        .header("cookie", ts.owner_cookie())
        .json(&json!({"data": {}, "lww": true})),
    )
    .await;
    seed(&ts, &b, "runs/r1", json!({})).await;
    let (name, data) = ev.next().await;
    assert_eq!(name, "doc");
    assert_eq!(
        data,
        json!({"topic": topic, "artifact_id": b, "path": "runs/r1", "version": 3})
    );

    assert_eq!(revoke(&ts, &a, &b).await.0, 200);
    seed(&ts, &b, "runs/r2", json!({})).await;
    ts.thread(&b, 1, "marker").await;
    let (name, data) = ev.next().await;
    assert_eq!(
        (name.as_str(), &data["topic"]),
        ("thread", &json!(artifact_topic))
    );
}

#[tokio::test]
async fn grant_routes_need_the_token() {
    let ts = TestServer::spawn().await;
    let (a, b) = pair(&ts).await;
    let one = format!("{}/api/artifacts/{a}/db-grants/{b}", ts.base);
    let all = format!("{}/api/artifacts/{a}/db-grants", ts.base);
    for r in [
        ts.client.put(&one),
        ts.client.delete(&one),
        ts.client.get(&all),
        // The owner's browser cookie is not the token.
        ts.client.put(&one).header("cookie", ts.owner_cookie()),
    ] {
        assert_eq!(send(r).await.0, 401);
    }
    let ghost = "zzzzzzzzzzzz";
    for (r, s) in [(a.as_str(), ghost), (ghost, b.as_str())] {
        assert_eq!(grant(&ts, r, s).await.0, 404);
        assert_eq!(revoke(&ts, r, s).await.0, 404);
    }
    assert_eq!(
        send(authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{ghost}/db-grants")
        ))
        .await
        .0,
        404
    );
    let (status, body) = grant(&ts, &a, &a).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_args");
    assert_eq!(revoke(&ts, &a, &a).await.1["error"]["code"], "invalid_args");
}

#[tokio::test]
async fn collections_lists_paths_and_counts() {
    let ts = TestServer::spawn().await;
    let b = app(&ts, "Runs", json!({"db": {}})).await;
    for p in ["runs/r1", "runs/r2", "notes/n1"] {
        seed(&ts, &b, p, json!({})).await;
    }
    let me = ts.owner_public_id().await;
    send(
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{b}/docs/data/users/{me}/x"),
        )
        .header("cookie", ts.owner_cookie())
        .json(&json!({"data": {}, "lww": true})),
    )
    .await;
    let path = format!("/api/artifacts/{b}/docs:collections");
    let got = send(authed(&ts, Method::GET, &path)).await;
    assert_eq!(
        got,
        (
            200,
            json!({"collections": [{"path": "notes", "count": 1}, {"path": "runs", "count": 2}]})
        )
    );
    assert_eq!(
        send(ts.client.get(format!("{}{path}", ts.base))).await.0,
        401
    );
    assert_eq!(
        send(
            ts.client
                .get(format!("{}{path}", ts.base))
                .header("cookie", ts.owner_cookie())
        )
        .await
        .0,
        401,
        "the owner's browser cookie is not the token"
    );
    // A reader never lists another app's collections this way.
    let a = app(&ts, "Coach", json!({"db": {"read": [b]}})).await;
    assert_eq!(grant(&ts, &a, &b).await.0, 200);
    let got = send(authed(&ts, Method::GET, &path).header(READER, &a)).await;
    assert_eq!(got.0, 403, "{}", got.1);
    assert_eq!(got.1["error"]["code"], "not_granted");
    assert_eq!(
        send(authed(
            &ts,
            Method::GET,
            "/api/artifacts/zzzzzzzzzzzz/docs:collections"
        ))
        .await
        .0,
        404
    );
}
