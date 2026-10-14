use crate::common;
use clax_core::working::ManualClock;
use clax_server::testing::TestViewer;
use common::TestServer;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::Arc;
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

    /// The next frame within 5 s; a close is `{"t": "_close", code}`.
    async fn next(&mut self) -> Value {
        loop {
            let m = tokio::time::timeout(Duration::from_secs(5), self.ws.next())
                .await
                .expect("a frame within 5 s")
                .expect("open");
            match m.expect("readable") {
                Message::Text(t) if t.as_str() == "pong" => return json!({"t": "_pong"}),
                Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
                Message::Close(c) => {
                    return json!({"t": "_close", "code": c.map(|c| u16::from(c.code))});
                }
                _ => continue,
            }
        }
    }

    /// Skips frames until a batch whose sender `person` satisfies `pred`.
    async fn sender(&mut self, person: &str, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            let f = self.next().await;
            assert_eq!(f["t"], "live", "{f}");
            if let Some(s) = f["senders"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["person_id"] == person && pred(s))
            {
                return s.clone();
            }
        }
    }
}

fn url(ts: &TestServer, aid: &str) -> String {
    format!(
        "{}/api/artifacts/{aid}/live",
        ts.base.replacen("http://", "ws://", 1)
    )
}

async fn try_connect(
    ts: &TestServer,
    aid: &str,
    headers: &[(&str, String)],
) -> Result<Client, u16> {
    let mut req = url(ts, aid).into_client_request().unwrap();
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

fn cookie(v: &TestViewer) -> (&'static str, String) {
    ("cookie", format!("clax_viewer={}", v.cookie))
}

/// A connected viewer's socket, past its welcome (returned).
async fn join(ts: &TestServer, aid: &str, v: &TestViewer) -> (Client, Value) {
    let mut c = try_connect(ts, aid, &[cookie(v)]).await.expect("upgrade");
    let w = c.next().await;
    assert_eq!(w["t"], "welcome", "{w}");
    (c, w)
}

async fn artifact(ts: &TestServer) -> String {
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Live", "files": {"index.html": {"content": "<main><p>Hello</p></main>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn cursor(x: f64) -> Value {
    json!({"t": "cursor", "version": 1, "file": "index.html",
        "anchor": {"path": [0, 0], "offset": 2, "x": x, "y": 0.5},
        "selection": {"kind": "element", "path": [0]}})
}

#[tokio::test]
async fn a_persons_pointer_typing_and_draft_reach_the_others() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts).await;
    let (ann, ben) = (ts.viewer(Some("Ann")).await, ts.viewer(Some("Ben")).await);
    let (mut a, aw) = join(&ts, &aid, &ann).await;
    let (mut b, bw) = join(&ts, &aid, &ben).await;
    assert_eq!(aw["person_id"], ann.public_id);
    assert_ne!(aw["color"], bw["color"], "two people, two colours");
    a.send(cursor(0.25)).await;
    let s = b
        .sender(&ann.public_id, |s| s.get("cursor").is_some())
        .await;
    assert_eq!(s["color"], aw["color"]);
    assert_eq!(s["cursor"]["anchor"]["offset"], 2);
    assert_eq!(
        s["cursor"]["selection"],
        json!({"kind": "element", "path": [0]})
    );
    a.send(json!({"t": "typing", "thread_id": null})).await;
    a.send(json!({"t": "draft", "thread_id": null, "text": "Looks good"}))
        .await;
    let s = b
        .sender(&ann.public_id, |s| s["draft"]["text"] == "Looks good")
        .await;
    assert_eq!(s["typing"], json!({"thread_id": null}));
    assert!(
        s.get("cursor").is_none(),
        "an unchanged cursor is not sent again: {s}"
    );
    // Posting clears the draft and the typing mark.
    a.send(json!({"t": "draft", "thread_id": null, "text": ""}))
        .await;
    let s = b.sender(&ann.public_id, |s| s.get("draft").is_some()).await;
    assert_eq!(s["draft"], Value::Null);
    assert_eq!(s["typing"], Value::Null);
    // `ping` is answered, and a malformed message is dropped without closing.
    a.ws.send(Message::Text("not json".into())).await.unwrap();
    a.ws.send(Message::Text("ping".into())).await.unwrap();
    // Ann's own batches say Ben is present, and never carry her own state.
    loop {
        let f = a.next().await;
        if f["t"] == "_pong" {
            break;
        }
        assert_eq!(f["t"], "live", "{f}");
        assert_eq!(f["present"], 1, "{f}");
        assert_eq!(f["senders"], json!([]), "{f}");
    }
}

#[tokio::test]
async fn a_late_joiner_gets_what_others_show_and_a_leaver_is_dropped() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts).await;
    let (ann, ben) = (ts.viewer(Some("Ann")).await, ts.viewer(Some("Ben")).await);
    let (mut a, _) = join(&ts, &aid, &ann).await;
    a.send(cursor(0.5)).await;
    let (mut b, _) = join(&ts, &aid, &ben).await;
    b.sender(&ann.public_id, |s| s["cursor"]["anchor"]["x"] == 0.5)
        .await;
    drop(a);
    loop {
        let f = b.next().await;
        if f["drop"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| *d == ann.public_id)
        {
            break;
        }
    }
}

#[tokio::test]
async fn agents_and_callers_without_a_viewer_are_refused() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts).await;
    let auth = ("authorization", format!("Bearer {}", ts.token));
    // The token alone (the CLI, a script) has no viewer.
    assert_eq!(
        try_connect(&ts, &aid, std::slice::from_ref(&auth))
            .await
            .err(),
        Some(400)
    );
    // The owner's agent: the token naming a session, or the agent channel.
    assert_eq!(
        try_connect(
            &ts,
            &aid,
            &[auth.clone(), ("x-clax-channel", "agent".into())]
        )
        .await
        .err(),
        Some(403)
    );
    // A person's agent: a grant credential never reaches the upgrade (an
    // unverified one is 401 before routing; a verified one is refused by
    // `DeniedToGrants`).
    assert_eq!(
        try_connect(&ts, &aid, &[("authorization", "Clax-Grant abc".into())])
            .await
            .err(),
        Some(401)
    );
    // A page's origin.
    let ann = ts.viewer(Some("Ann")).await;
    let port = ts.addr.port();
    assert_eq!(
        try_connect(
            &ts,
            &aid,
            &[
                cookie(&ann),
                ("origin", format!("http://{aid}.localhost:{port}"))
            ]
        )
        .await
        .err(),
        Some(403)
    );
    assert_eq!(
        try_connect(&ts, "nonexistent00", &[cookie(&ann)])
            .await
            .err(),
        Some(400)
    );
    assert_eq!(
        try_connect(&ts, "aaaaaaaaaaaa", &[cookie(&ann)])
            .await
            .err(),
        Some(404)
    );
}

#[tokio::test]
async fn a_deleted_artifact_closes_its_sockets() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts).await;
    let ann = ts.viewer(Some("Ann")).await;
    let (mut a, _) = join(&ts, &aid, &ann).await;
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base)))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    assert_eq!(a.next().await, json!({"t": "_close", "code": 4410}));
}

/// The rate is judged by the daemon's clock, stopped here: each second's
/// messages are sent, then a `ping` waits for the daemon to have read them,
/// then the clock moves on.
#[tokio::test]
async fn a_socket_over_the_rate_for_five_seconds_in_a_row_is_closed() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let aid = artifact(&ts).await;
    let ann = ts.viewer(Some("Ann")).await;
    let (mut a, _) = join(&ts, &aid, &ann).await;
    // The burst (60), and one over it.
    for _ in 0..61 {
        a.send(cursor(0.5)).await;
    }
    for second in 1..=4 {
        a.ws.send(Message::Text("ping".into())).await.unwrap();
        loop {
            if a.next().await["t"] == "_pong" {
                break;
            }
        }
        clock.advance(1);
        // 40 in a second, 10 over the rate of 30.
        for _ in 0..40 {
            a.send(cursor(0.5)).await;
        }
        if second < 4 {
            continue;
        }
        loop {
            let f = a.next().await;
            if f["t"] == "_close" {
                assert_eq!(f["code"], 1008);
                return;
            }
        }
    }
}

/// Pings count against the rate too: a socket that only pings, over the
/// rate for five seconds in a row, is closed (each ping is still answered).
#[tokio::test]
async fn a_socket_flooding_pings_is_closed_too() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let aid = artifact(&ts).await;
    let ann = ts.viewer(Some("Ann")).await;
    let (mut a, _) = join(&ts, &aid, &ann).await;
    let ping = || Message::Text("ping".into());
    for second in 0..5 {
        if second > 0 {
            clock.advance(1);
        }
        // The burst (60) and one over it, then 40 a second, 10 over the rate.
        let n = if second == 0 { 61 } else { 40 };
        for _ in 0..n {
            a.ws.send(ping()).await.unwrap();
        }
        // Each is answered, until the close.
        for _ in 0..n {
            let f = a.next().await;
            if f["t"] == "_close" {
                assert_eq!(second, 4, "closed in the fifth second over the rate");
                assert_eq!(f["code"], 1008);
                return;
            }
            assert_eq!(f["t"], "_pong", "{f}");
        }
    }
    panic!("never closed");
}

fn seat_limits(people: u32) -> clax_core::seats::SeatLimits {
    clax_core::seats::SeatLimits {
        people,
        guests: people,
        agents: 20,
        idle_ms: 180_000,
    }
}

/// The close a socket gets as it opens, past no welcome.
async fn refused(ts: &TestServer, aid: &str, v: &TestViewer) -> Value {
    let mut c = try_connect(ts, aid, &[cookie(v)]).await.expect("upgrade");
    c.next().await
}

/// A person beyond the live seats (spec §16.1) is a spectator: their
/// socket closes 4429 before it joins, so nothing of theirs reaches the
/// seated, who do not count them present. A seat freed is taken.
#[tokio::test]
async fn a_spectators_socket_is_closed_and_relays_nothing() {
    let ts = TestServer::spawn().await;
    ts.seats.set_limits(seat_limits(1));
    let aid = artifact(&ts).await;
    let (ann, ben) = (ts.viewer(Some("Ann")).await, ts.viewer(Some("Ben")).await);
    let (mut a, _) = join(&ts, &aid, &ann).await;
    let f = refused(&ts, &aid, &ben).await;
    assert_eq!(f, json!({"t": "_close", "code": 4429}), "{f}");
    // Ann's batches never carry Ben, nor count him present.
    a.ws.send(Message::Text("ping".into())).await.unwrap();
    loop {
        let f = a.next().await;
        if f["t"] == "_pong" {
            break;
        }
        assert_eq!(f["present"], 0, "{f}");
        assert_eq!(f["senders"], json!([]), "{f}");
    }
    assert_eq!(ts.live_layers.sockets(&aid), 1);
    // Ann leaves; her seat frees, and Ben's next socket is seated.
    drop(a);
    let mut tries = 0;
    loop {
        let mut c = try_connect(&ts, &aid, &[cookie(&ben)])
            .await
            .expect("upgrade");
        let f = c.next().await;
        if f["t"] == "welcome" {
            break;
        }
        assert_eq!(f["code"], 4429, "{f}");
        tries += 1;
        assert!(tries < 200, "Ann's seat never freed");
        tokio::task::yield_now().await;
    }
}

/// A socket's seat released for want of input closes it 4429 with reason
/// `seat_idle`, and the person's layers leave the others' view.
#[tokio::test]
async fn an_idle_seat_closes_its_socket() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    ts.seats.set_limits(seat_limits(5));
    let aid = artifact(&ts).await;
    let (ann, ben) = (ts.viewer(Some("Ann")).await, ts.viewer(Some("Ben")).await);
    let (mut a, _) = join(&ts, &aid, &ann).await;
    let (mut b, _) = join(&ts, &aid, &ben).await;
    a.send(cursor(0.5)).await;
    b.sender(&ann.public_id, |s| s.get("cursor").is_some())
        .await;
    clock.advance(180);
    ts.seats.sweep();
    let close = loop {
        match a.ws.next().await.expect("open").expect("readable") {
            Message::Close(c) => break c.expect("a close frame"),
            _ => continue,
        }
    };
    assert_eq!(u16::from(close.code), 4429);
    assert_eq!(close.reason.as_str(), "seat_idle");
}
