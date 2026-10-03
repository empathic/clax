mod common;
use clax_server::testing::TestViewer;
use common::TestServer;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
}

impl Client {
    async fn send(&mut self, v: Value) {
        self.ws
            .send(Message::Text(v.to_string().into()))
            .await
            .unwrap();
    }

    /// The next frame within `ms`, or `None`; a close is `{"t": "_close", code, reason}`.
    async fn next_within(&mut self, ms: u64) -> Option<Value> {
        loop {
            let m = tokio::time::timeout(Duration::from_millis(ms), self.ws.next())
                .await
                .ok()??;
            match m.expect("readable") {
                Message::Text(t) => return Some(serde_json::from_str(t.as_str()).unwrap()),
                Message::Close(c) => {
                    let c = c.expect("a close frame");
                    return Some(
                        json!({"t": "_close", "code": u16::from(c.code), "reason": c.reason.as_str()}),
                    );
                }
                _ => continue,
            }
        }
    }

    async fn next(&mut self) -> Value {
        self.next_within(5000).await.expect("a frame within 5 s")
    }

    /// Skips frames until one satisfies `pred`.
    async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            let f = self.next().await;
            if pred(&f) {
                return f;
            }
        }
    }
}

const A: &str = "aaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccc";

/// The socket URL; `token` is appended as the owner shell appends it.
fn url(ts: &TestServer, aid: &str, peer: &str, token: Option<&str>) -> String {
    let base = format!(
        "{}/api/artifacts/{aid}/room?peer={peer}",
        ts.base.replacen("http://", "ws://", 1)
    );
    match token {
        Some(t) => format!("{base}&token={t}"),
        None => base,
    }
}

async fn try_connect(
    ts: &TestServer,
    aid: &str,
    peer: &str,
    token: Option<&str>,
    headers: &[(&str, String)],
) -> Result<Client, u16> {
    let mut req = url(ts, aid, peer, token).into_client_request().unwrap();
    for (k, v) in headers {
        req.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(Client { ws }),
        Err(tokio_tungstenite::tungstenite::Error::Http(res)) => Err(res.status().as_u16()),
        Err(e) => panic!("connect: {e}"),
    }
}

async fn connect(
    ts: &TestServer,
    aid: &str,
    peer: &str,
    token: Option<&str>,
    headers: &[(&str, String)],
) -> Client {
    let mut c = try_connect(ts, aid, peer, token, headers)
        .await
        .expect("upgrade");
    assert_eq!(c.next().await, json!({"t": "welcome", "peer": peer}));
    c
}

fn cookie(v: &TestViewer) -> (&'static str, String) {
    ("cookie", format!("clax_viewer={}", v.cookie))
}

// Levels: the owner shell is the token plus its cookie (`admin`); a LAN viewer
// is a cookie without the token (`interact` when named, else `view`).

async fn artifact(ts: &TestServer, caps: Value) -> String {
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Room", "capabilities": caps, "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn caps() -> Value {
    json!({"room": {"topics": {"reaction": "interact"}}, "user": {}})
}

#[tokio::test]
async fn presence_and_messages_reach_every_peer_with_sender_fields() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let (ann, ben) = (ts.viewer(Some("Ann")).await, ts.viewer(Some("Ben")).await);
    let mut a = connect(&ts, &aid, A, Some(ts.token.as_str()), &[cookie(&ann)]).await;
    let first = a.next().await;
    assert_eq!(first["t"], "peers");
    assert_eq!(first["room"], Value::Null);
    assert_eq!(
        first["peers"],
        json!([{"peer": A, "by": ann.public_id, "isMe": true, "sameTab": true, "kind": "viewer", "guest": false, "presence": {}}])
    );
    let mut b = connect(&ts, &aid, B, None, &[cookie(&ben)]).await;
    assert_eq!(b.next().await["peers"].as_array().unwrap().len(), 2);
    let seen = a
        .until(|f| f["t"] == "peer" && f["peer"]["peer"] == B)
        .await;
    assert_eq!(seen["peer"]["isMe"], false);
    assert_eq!(seen["peer"]["by"], ben.public_id.as_str());
    b.send(json!({"t": "presence", "room": null, "state": {"cursor": [1, 2]}}))
        .await;
    let moved = a
        .until(|f| f["t"] == "peer" && f["peer"]["presence"] != json!({}))
        .await;
    assert_eq!(moved["peer"]["presence"], json!({"cursor": [1, 2]}));
    b.send(
        json!({"t": "emit", "id": 1, "room": null, "topic": "reaction", "data": {"kind": "wave"}}),
    )
    .await;
    assert_eq!(
        b.until(|f| f["t"] == "ack").await,
        json!({"t": "ack", "id": 1})
    );
    let msg = a.until(|f| f["t"] == "msg").await;
    assert_eq!(
        msg["msg"],
        json!({"peer": B, "by": ben.public_id, "isMe": false, "sameTab": false, "kind": "viewer", "guest": false, "topic": "reaction", "data": {"kind": "wave"}})
    );
    let echo = b.until(|f| f["t"] == "msg").await;
    assert_eq!(
        (echo["msg"]["isMe"].clone(), echo["msg"]["sameTab"].clone()),
        (json!(true), json!(true))
    );
}

#[tokio::test]
async fn by_is_null_without_a_user_declaration() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"room": {}})).await;
    let ann = ts.viewer(Some("Ann")).await;
    let mut a = connect(&ts, &aid, A, None, &[cookie(&ann)]).await;
    assert_eq!(a.next().await["peers"][0]["by"], Value::Null);
}

#[tokio::test]
async fn admin_topics_refuse_lower_levels() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let (owner, ben, anon) = (
        ts.viewer(Some("Owner")).await,
        ts.viewer(Some("Ben")).await,
        ts.viewer(None).await,
    );
    let mut a = connect(&ts, &aid, A, Some(ts.token.as_str()), &[cookie(&owner)]).await;
    let mut b = connect(&ts, &aid, B, None, &[cookie(&ben)]).await;
    let mut v = connect(&ts, &aid, C, None, &[cookie(&anon)]).await;
    b.send(json!({"t": "emit", "id": 1, "room": null, "topic": "clear"}))
        .await;
    let n = b.until(|f| f["t"] == "nack").await;
    assert_eq!(
        (n["id"].clone(), n["code"].clone()),
        (json!(1), json!("not_permitted"))
    );
    v.send(json!({"t": "emit", "id": 2, "room": null, "topic": "reaction"}))
        .await;
    assert_eq!(v.until(|f| f["t"] == "nack").await["code"], "not_permitted");
    v.send(json!({"t": "presence", "room": null, "state": {"pick": "A"}}))
        .await;
    a.until(|f| f["t"] == "peer" && f["peer"]["peer"] == C && f["peer"]["presence"]["pick"] == "A")
        .await;
    a.send(json!({"t": "emit", "id": 3, "room": null, "topic": "clear"}))
        .await;
    assert_eq!(
        a.until(|f| f["t"] == "ack" || f["t"] == "nack").await,
        json!({"t": "ack", "id": 3})
    );
}

#[tokio::test]
async fn the_token_alone_is_owner_and_a_wrong_token_is_no_token() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let named = ts.viewer(Some("Ben")).await;
    let mut agent = connect(&ts, &aid, A, Some(ts.token.as_str()), &[]).await;
    agent
        .send(json!({"t": "emit", "id": 1, "room": null, "topic": "clear"}))
        .await;
    assert_eq!(
        agent.until(|f| f["t"] == "ack" || f["t"] == "nack").await,
        json!({"t": "ack", "id": 1})
    );
    let mut forged = connect(&ts, &aid, B, Some("not-the-token"), &[cookie(&named)]).await;
    forged
        .send(json!({"t": "emit", "id": 2, "room": null, "topic": "clear"}))
        .await;
    assert_eq!(
        forged.until(|f| f["t"] == "ack" || f["t"] == "nack").await["code"],
        "not_permitted"
    );
    forged
        .send(json!({"t": "emit", "id": 3, "room": null, "topic": "reaction"}))
        .await;
    assert_eq!(
        forged.until(|f| f["t"] == "ack" || f["t"] == "nack").await,
        json!({"t": "ack", "id": 3})
    );
    let mut nobody = connect(&ts, &aid, C, Some("not-the-token"), &[]).await;
    nobody
        .send(json!({"t": "emit", "id": 4, "room": null, "topic": "reaction"}))
        .await;
    assert_eq!(
        nobody.until(|f| f["t"] == "ack" || f["t"] == "nack").await["code"],
        "not_permitted"
    );
}

#[tokio::test]
async fn named_rooms_are_isolated_and_leaving_is_seen() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let owner = ts.viewer(Some("Owner")).await;
    let mut a = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    let mut b = connect(&ts, &aid, B, None, &[cookie(&owner)]).await;
    let mut c = connect(&ts, &aid, C, None, &[cookie(&owner)]).await;
    a.send(json!({"t": "join", "id": 1, "room": "table-1"}))
        .await;
    let snap = a
        .until(|f| f["t"] == "peers" && f["room"] == "table-1")
        .await;
    assert_eq!(snap["peers"].as_array().unwrap().len(), 1);
    assert_eq!(
        a.until(|f| f["t"] == "ack").await,
        json!({"t": "ack", "id": 1})
    );
    b.send(json!({"t": "join", "id": 2, "room": "table-1"}))
        .await;
    b.until(|f| f["t"] == "ack").await;
    a.until(|f| f["t"] == "peer" && f["room"] == "table-1" && f["peer"]["peer"] == B)
        .await;
    b.send(json!({"t": "emit", "id": 3, "room": "table-1", "topic": "reaction"}))
        .await;
    let m = a.until(|f| f["t"] == "msg").await;
    assert_eq!(m["room"], "table-1");
    let mut lobby_only = Vec::new();
    while let Some(f) = c.next_within(300).await {
        lobby_only.push(f);
    }
    assert!(
        lobby_only.iter().all(|f| f["room"] != "table-1"),
        "{lobby_only:?}"
    );
    a.send(json!({"t": "leave", "room": "table-1"})).await;
    b.until(|f| f["t"] == "left" && f["room"] == "table-1" && f["peer"] == A)
        .await;
}

#[tokio::test]
async fn grammar_and_bounds_answer_invalid_argument_and_limit_reached() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let owner = ts.viewer(Some("Owner")).await;
    let mut a = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    a.send(json!({"t": "emit", "id": 1, "room": null, "topic": "Bad:topic"}))
        .await;
    assert_eq!(
        a.until(|f| f["t"] == "nack").await["code"],
        "invalid_argument"
    );
    a.send(
        json!({"t": "emit", "id": 2, "room": null, "topic": "reaction", "data": "x".repeat(5000)}),
    )
    .await;
    assert_eq!(
        a.until(|f| f["t"] == "nack").await["code"],
        "invalid_argument"
    );
    a.send(json!({"t": "join", "id": 3, "room": "Bad"})).await;
    assert_eq!(
        a.until(|f| f["t"] == "nack").await["code"],
        "invalid_argument"
    );
    for i in 0..16 {
        a.send(json!({"t": "join", "id": 10 + i, "room": format!("r{i}")}))
            .await;
        a.until(|f| f["t"] == "ack").await;
    }
    a.send(json!({"t": "join", "id": 99, "room": "one-more"}))
        .await;
    assert_eq!(a.until(|f| f["t"] == "nack").await["code"], "limit_reached");
    a.send(json!({"t": "join", "id": 100, "room": "r3"})).await;
    assert_eq!(
        a.until(|f| f["t"] == "ack" || f["t"] == "nack").await,
        json!({"t": "ack", "id": 100})
    );
}

#[tokio::test]
async fn is_me_spans_one_viewers_tabs() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let owner = ts.viewer(Some("Owner")).await;
    let _a = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    let mut a2 = connect(&ts, &aid, B, None, &[cookie(&owner)]).await;
    let snap = a2.next().await;
    let other = snap["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["peer"] == A)
        .unwrap()
        .clone();
    assert_eq!(
        (other["isMe"].clone(), other["sameTab"].clone()),
        (json!(true), json!(false))
    );
}

#[tokio::test]
async fn undeclared_or_missing_artifacts_close_with_not_granted() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    for target in [aid.as_str(), "zzzzzzzzzzzz"] {
        let mut c = try_connect(&ts, target, A, None, &[])
            .await
            .expect("upgrade");
        assert_eq!(
            c.next().await,
            json!({"t": "_close", "code": 4403, "reason": "not_granted"})
        );
    }
}

#[tokio::test]
async fn deleting_the_artifact_closes_its_sockets_with_revoked() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let mut a = connect(&ts, &aid, A, None, &[]).await;
    a.next().await;
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base)))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    assert_eq!(
        a.until(|f| f["t"] == "_close").await,
        json!({"t": "_close", "code": 4403, "reason": "revoked"})
    );
}

#[tokio::test]
async fn a_version_that_drops_room_closes_its_sockets_with_revoked() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let mut a = connect(&ts, &aid, A, None, &[]).await;
    a.next().await;
    let res = ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 1, "capabilities": {"db": {}}, "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await;
    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(
        a.until(|f| f["t"] == "_close").await,
        json!({"t": "_close", "code": 4403, "reason": "revoked"})
    );
}

#[tokio::test]
async fn a_second_socket_with_the_same_label_replaces_the_first() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let owner = ts.viewer(Some("Owner")).await;
    let mut old = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    let _new = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    assert_eq!(
        old.until(|f| f["t"] == "_close").await,
        json!({"t": "_close", "code": 4409, "reason": "replaced"})
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut later = connect(&ts, &aid, B, None, &[cookie(&owner)]).await;
    let snap = later.next().await;
    let labels: Vec<&str> = snap["peers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["peer"].as_str().unwrap())
        .collect();
    assert_eq!(labels, [A, B]);
}

#[tokio::test]
async fn closing_a_socket_is_left_for_everyone() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let mut a = connect(&ts, &aid, A, None, &[]).await;
    let b = connect(&ts, &aid, B, None, &[]).await;
    a.until(|f| f["t"] == "peer" && f["peer"]["peer"] == B)
        .await;
    drop(b);
    assert_eq!(
        a.until(|f| f["t"] == "left").await,
        json!({"t": "left", "room": null, "peer": B})
    );
}

#[tokio::test]
async fn foreign_origins_and_bad_labels_are_refused_before_the_upgrade() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let port = ts.addr.port();
    let origin = ("origin", format!("http://{aid}.localhost:{port}"));
    assert_eq!(
        try_connect(&ts, &aid, A, None, &[origin]).await.err(),
        Some(403)
    );
    assert_eq!(
        try_connect(&ts, &aid, A, None, &[("origin", "null".to_string())])
            .await
            .err(),
        Some(403)
    );
    assert_eq!(
        try_connect(&ts, &aid, "short", None, &[]).await.err(),
        Some(400)
    );
    assert_eq!(
        try_connect(
            &ts,
            &aid,
            A,
            None,
            &[("host", format!("rebind.example:{port}"))]
        )
        .await
        .err(),
        Some(403)
    );
}

/// The daemon reads at most `MAX_ROOM_MESSAGE_BYTES` of one message: a
/// larger one closes the socket (1009) rather than being buffered whole.
#[tokio::test]
async fn a_message_over_the_size_cap_closes_the_socket() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let mut a = connect(&ts, &aid, A, None, &[]).await;
    a.next().await;
    let big = "x".repeat(clax_server::routes::room::MAX_ROOM_MESSAGE_BYTES + 1);
    let _ =
        a.ws.send(Message::Text(
            json!({"t": "presence", "room": null, "state": {"k": big}})
                .to_string()
                .into(),
        ))
        .await;
    let mut closed = false;
    for _ in 0..4 {
        match tokio::time::timeout(Duration::from_secs(5), a.ws.next()).await {
            Ok(None | Some(Err(_))) => {
                closed = true;
                break;
            }
            Ok(Some(Ok(Message::Close(c)))) => {
                assert_eq!(c.map(|c| u16::from(c.code)), Some(1009));
                closed = true;
                break;
            }
            Ok(Some(Ok(_))) => continue,
            Err(_) => break,
        }
    }
    assert!(closed, "the socket closes");
    // A message within the cap is read (and refused by its own bounds, quietly).
    let mut b = connect(&ts, &aid, B, None, &[]).await;
    b.next().await;
    b.send(json!({"t": "presence", "room": null, "state": {"k": "x".repeat(5000)}}))
        .await;
    b.send(json!({"t": "join", "id": 1, "room": "r"})).await;
    let ack = b.until(|f| f["t"] == "ack").await;
    assert_eq!(ack["id"], 1);
}
