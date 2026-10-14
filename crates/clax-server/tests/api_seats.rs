//! Live seats and spectators (spec 2026-10-11-clax-dev-hosted-design §7,
//! §16.1, §16.2): a stream's live topics of an artifact hold its person's
//! seat there, a person beyond the seats is refused them and every write,
//! and the spectator snapshot is shared for 5 s per head version.

use crate::common::TestServer;
use clax_core::audit::AuditCtx;
use clax_core::rights::Rights;
use clax_core::seats::SeatLimits;
use clax_core::working::ManualClock;
use clax_server::testing::{EventReader, element_anchor};
use serde_json::{Value, json};
use std::sync::Arc;

const T0: &str = "2026-10-12T10:00:00Z";

fn limits(people: u32, guests: u32) -> SeatLimits {
    SeatLimits {
        people,
        guests,
        agents: 20,
        idle_ms: 180_000,
    }
}

async fn server(people: u32, guests: u32) -> (TestServer, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at(T0));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    ts.seats.set_limits(limits(people, guests));
    (ts, clock)
}

async fn artifact(ts: &TestServer) -> String {
    ts.publish("Seats", &[("index.html", "<h2>Quarterly goals</h2>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn live(aid: &str) -> Vec<String> {
    ["artifact", "presence", "working"]
        .iter()
        .map(|t| format!("{t}:{aid}"))
        .collect()
}

/// One browser's open `/api/stream`: its events and ID, and the `Cookie`
/// header it was opened with.
struct Stream {
    events: EventReader,
    id: String,
    cookie: Option<String>,
}

async fn open(ts: &TestServer, cookie: Option<String>) -> Stream {
    let mut req = ts.client.get(format!("{}/api/stream", ts.base));
    if let Some(c) = &cookie {
        req = req.header("cookie", c);
    }
    let res = req.send().await.unwrap();
    assert_eq!(res.status(), 200);
    let mut events = EventReader::from_response(res);
    let id = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    Stream { events, id, cookie }
}

/// `POST /api/stream/<id>`: status and error code (empty on success).
async fn change(ts: &TestServer, s: &Stream, add: &[String], remove: &[String]) -> (u16, String) {
    let mut req = ts
        .client
        .post(format!("{}/api/stream/{}", ts.base, s.id))
        .json(&json!({"subscribe": add, "unsubscribe": remove}));
    if let Some(c) = &s.cookie {
        req = req.header("cookie", c);
    }
    let res = req.send().await.unwrap();
    let status = res.status().as_u16();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    (
        status,
        v["error"]["code"].as_str().unwrap_or("").to_string(),
    )
}

fn viewer_cookie(c: &str) -> Option<String> {
    Some(format!("clax_viewer={c}"))
}

#[tokio::test]
async fn live_topics_take_seats_first_come_first_served() {
    let (ts, _) = server(1, 1).await;
    let aid = artifact(&ts).await;
    let (p1, p2) = (ts.person("Pat One").await, ts.person("Pat Two").await);
    let a = open(&ts, viewer_cookie(&p1.cookie)).await;
    assert_eq!(
        change(&ts, &a, &live(&aid), &[]).await,
        (200, String::new())
    );
    let b = open(&ts, viewer_cookie(&p2.cookie)).await;
    assert_eq!(
        change(&ts, &b, &live(&aid), &[]).await,
        (429, "no_live_seat".into()),
        "the one seat is taken"
    );
    // Each live topic alone is refused, other topics are not.
    assert_eq!(change(&ts, &b, &live(&aid)[1..2], &[]).await.0, 429);
    assert_eq!(change(&ts, &b, &["gallery".into()], &[]).await.0, 200);
    // One person holds one seat across streams (browsers or tabs).
    let a2 = open(&ts, viewer_cookie(&p1.cookie)).await;
    assert_eq!(change(&ts, &a2, &live(&aid), &[]).await.0, 200);
    // The owner's seat is reserved; a viewer with no name takes none.
    let owner = open(&ts, Some(ts.owner_cookie())).await;
    assert_eq!(change(&ts, &owner, &live(&aid), &[]).await.0, 200);
    let nameless = ts.viewer(None).await;
    let n = open(&ts, viewer_cookie(&nameless.cookie)).await;
    assert_eq!(change(&ts, &n, &live(&aid), &[]).await.0, 200);
    // The seat is held until the person's last stream leaves the live topics.
    assert_eq!(change(&ts, &a, &[], &live(&aid)).await.0, 200);
    assert_eq!(change(&ts, &b, &live(&aid), &[]).await.0, 429);
    assert_eq!(change(&ts, &a2, &[], &live(&aid)[..2]).await.0, 200);
    assert_eq!(
        change(&ts, &b, &live(&aid), &[]).await.0,
        429,
        "working:<id> still holds it"
    );
    assert_eq!(change(&ts, &a2, &[], &live(&aid)[2..]).await.0, 200);
    assert_eq!(change(&ts, &b, &live(&aid), &[]).await.0, 200);
    // Seats are per artifact.
    let other = artifact(&ts).await;
    assert_eq!(change(&ts, &a, &live(&other), &[]).await.0, 200);
}

#[tokio::test]
async fn named_people_and_guests() {
    let (ts, _) = server(2, 1).await;
    let aid = artifact(&ts).await;
    let (g1, g2) = (ts.viewer(Some("Gil")).await, ts.viewer(Some("Gus")).await);
    let s1 = open(&ts, viewer_cookie(&g1.cookie)).await;
    assert_eq!(change(&ts, &s1, &live(&aid), &[]).await.0, 200);
    let s2 = open(&ts, viewer_cookie(&g2.cookie)).await;
    assert_eq!(
        change(&ts, &s2, &live(&aid), &[]).await,
        (429, "no_live_seat".into()),
        "guests share one of the two seats"
    );
    let p = ts.person("Pia").await;
    let sp = open(&ts, viewer_cookie(&p.cookie)).await;
    assert_eq!(
        change(&ts, &sp, &live(&aid), &[]).await.0,
        200,
        "people take the rest"
    );
    // A person whose rights name the artifact holds a reserved seat.
    let named = ts.person("Nia").await;
    let sn = open(&ts, viewer_cookie(&named.cookie)).await;
    assert_eq!(change(&ts, &sn, &live(&aid), &[]).await.0, 429);
    let rights = Rights::parse(&json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": [aid]},
        {"type": "clax-comments", "actions": ["read", "write"], "locations": [aid]},
    ]))
    .unwrap();
    ts.store
        .set_person_rights(&AuditCtx::DAEMON, &named.public_id, Some(rights))
        .unwrap();
    assert_eq!(change(&ts, &sn, &live(&aid), &[]).await.0, 200);
}

#[tokio::test]
async fn an_idle_seat_is_released_and_its_stream_told() {
    let (ts, clock) = server(1, 1).await;
    let aid = artifact(&ts).await;
    let (p1, p2) = (ts.person("Pat One").await, ts.person("Pat Two").await);
    let mut a = open(&ts, viewer_cookie(&p1.cookie)).await;
    assert_eq!(change(&ts, &a, &live(&aid), &[]).await.0, 200);
    // A `here` report is input; an `away` one is not.
    clock.advance(120);
    for state in ["here", "away"] {
        let res = ts
            .client
            .put(format!("{}/api/viewers/me/presence", ts.base))
            .header("cookie", format!("clax_viewer={}", p1.cookie))
            .json(&json!({"artifact_id": aid, "state": state, "tab": "t1"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }
    clock.advance(179);
    ts.seats.sweep();
    let b = open(&ts, viewer_cookie(&p2.cookie)).await;
    assert_eq!(change(&ts, &b, &live(&aid), &[]).await.0, 429, "still held");
    clock.advance(1);
    ts.seats.sweep();
    let mut told = Vec::new();
    for _ in 0..3 {
        let d = a.events.next_named("unseated").await;
        assert_eq!(d["code"], "seat_idle");
        told.push(d["topic"].as_str().unwrap().to_string());
    }
    told.sort();
    assert_eq!(told, live(&aid));
    assert_eq!(
        change(&ts, &b, &live(&aid), &[]).await.0,
        200,
        "the seat went to the next"
    );
    assert_eq!(
        change(&ts, &a, &live(&aid), &[]).await,
        (429, "no_live_seat".into()),
        "the idle person is a spectator now"
    );
}

/// Every write a person makes on threads, as `cookie`: status and code of each.
async fn writes(
    ts: &TestServer,
    aid: &str,
    cookie: &str,
    open_tid: &str,
    resolved_tid: &str,
) -> Vec<(String, u16, String)> {
    let c = format!("clax_viewer={cookie}");
    let base = format!("{}/api/artifacts/{aid}", ts.base);
    let form = reqwest::multipart::Form::new()
        .text("anchor", element_anchor().to_string())
        .text("body", "hello")
        .text("version", "1");
    let reqs = vec![
        (
            "create",
            ts.client.post(format!("{base}/threads")).multipart(form),
        ),
        (
            "comment",
            ts.client
                .post(format!("{base}/threads/{open_tid}/comments"))
                .json(&json!({"body": "a reply"})),
        ),
        (
            "send",
            ts.client.post(format!("{base}/threads/{open_tid}/send")),
        ),
        (
            "send_batch",
            ts.client
                .post(format!("{base}/threads:send"))
                .json(&json!({"thread_ids": [open_tid]})),
        ),
        (
            "resolve",
            ts.client.post(format!("{base}/threads/{open_tid}/resolve")),
        ),
        (
            "reopen",
            ts.client
                .post(format!("{base}/threads/{resolved_tid}/reopen")),
        ),
        (
            "delete",
            ts.client.delete(format!("{base}/threads/{open_tid}")),
        ),
    ];
    let mut out = Vec::new();
    for (what, r) in reqs {
        let res = r.header("cookie", &c).send().await.unwrap();
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        out.push((
            what.to_string(),
            status,
            v["error"]["message"].as_str().unwrap_or("").to_string(),
        ));
    }
    out
}

#[tokio::test]
async fn spectators_are_refused_every_write() {
    let (ts, _) = server(1, 1).await;
    let aid = artifact(&ts).await;
    let open_t = ts.thread_as_owner(&aid, "open").await;
    let done = ts.thread_as_owner(&aid, "done").await;
    let (open_tid, done_tid) = (open_t["id"].as_str().unwrap(), done["id"].as_str().unwrap());
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{done_tid}/resolve",
            ts.base
        ))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // A person without a live stream writes while a seat is free.
    let early = ts.person("Eve").await;
    let res = ts
        .reply_as(&aid, open_tid, &early.cookie, "while free")
        .await;
    assert!(res["id"].is_string());
    let seated = ts.person("Sam").await;
    let s = open(&ts, viewer_cookie(&seated.cookie)).await;
    assert_eq!(change(&ts, &s, &live(&aid), &[]).await.0, 200);
    let message = clax_core::seats::SPECTATOR_MESSAGE;
    for who in [ts.person("Pat").await, ts.viewer(Some("Gil")).await] {
        for (what, status, msg) in writes(&ts, &aid, &who.cookie, open_tid, done_tid).await {
            assert_eq!((status, msg.as_str()), (403, message), "{what}");
        }
    }
    // The seated person writes; the owner is never a spectator.
    ts.reply_as(&aid, open_tid, &seated.cookie, "seated").await;
    ts.thread_as_owner(&aid, "owner").await;
}

#[tokio::test]
async fn the_snapshot_is_shared_per_head_version_for_five_seconds() {
    let (ts, clock) = server(1, 1).await;
    let aid = artifact(&ts).await;
    let p1 = ts.person("Pat One").await;
    let a = open(&ts, viewer_cookie(&p1.cookie)).await;
    assert_eq!(change(&ts, &a, &live(&aid), &[]).await.0, 200);
    let get = |cookie: Option<String>, q: &str| {
        let mut r = ts
            .client
            .get(format!("{}/api/artifacts/{aid}/snapshot{q}", ts.base));
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        r.send()
    };
    let spectator = ts.person("Pat Two").await;
    let res = get(viewer_cookie(&spectator.cookie), "").await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["x-clax-cache"], "miss");
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["artifact"]["id"], aid.as_str());
    assert_eq!(v["seats_free"], 0);
    assert_eq!(v["guest_seats_free"], 0);
    assert_eq!(v["viewing"], 2, "the seated person and the spectator");
    assert!(v["threads"].is_array() && v["versions"].is_array());
    for per_person in ["attention", "looked", "me", "rights"] {
        assert!(v.get(per_person).is_none(), "{per_person}");
    }
    assert!(v["artifact"].get("owner_session_id").is_none());
    // Everyone gets the same bytes while it is shared: the owner, an
    // anonymous viewer (counted by its tab).
    let first = v.to_string();
    for (c, q) in [(Some(ts.owner_cookie()), ""), (None, "?tab=t1")] {
        let res = get(c, q).await.unwrap();
        assert_eq!(res.headers()["x-clax-cache"], "hit");
        assert_eq!(res.json::<Value>().await.unwrap().to_string(), first);
    }
    clock.advance(5);
    let res = get(None, "?tab=t1").await.unwrap();
    assert_eq!(res.headers()["x-clax-cache"], "miss", "5 s later");
    // The owner's and the anonymous tab's polls count them as viewing.
    assert_eq!(res.json::<Value>().await.unwrap()["viewing"], 4);
    // A new version is a new snapshot at once.
    let res = ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 1, "files": {"index.html": {"content": "v2"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let res = get(None, "").await.unwrap();
    assert_eq!(res.headers()["x-clax-cache"], "miss");
    assert_eq!(
        res.json::<Value>().await.unwrap()["artifact"]["current_version"],
        2
    );
    // Agents are never spectators.
    let res = ts
        .client
        .get(format!("{}/api/artifacts/{aid}/snapshot", ts.base))
        .bearer_auth(&ts.token)
        .header("x-clax-channel", "agent")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "not_for_agents"
    );
    let res = get(None, "").await.unwrap();
    assert_eq!(res.status(), 200);
    let res = ts
        .client
        .get(format!("{}/api/artifacts/7q3k9mzx2b4t/snapshot", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

/// The load check of `hosted/scripts/load-seats.ts` on the native daemon:
/// 150 people, 100 seats.
#[tokio::test]
async fn load_150_people_on_100_seats() {
    const PEOPLE: usize = 150;
    const SEATS: u32 = 100;
    let (ts, clock) = server(SEATS, 20).await;
    let aid = artifact(&ts).await;
    let topic = live(&aid);
    let mut people = Vec::new();
    for i in 0..PEOPLE {
        people.push(ts.person(&format!("Person {i:03}")).await);
    }
    let mut streams = Vec::new();
    for p in &people {
        streams.push(open(&ts, viewer_cookie(&p.cookie)).await);
    }
    let outcomes =
        futures::future::join_all(streams.iter().map(|s| change(&ts, s, &topic, &[]))).await;
    let seated: Vec<usize> = (0..PEOPLE).filter(|i| outcomes[*i].0 == 200).collect();
    let refused: Vec<usize> = (0..PEOPLE)
        .filter(|i| outcomes[*i] == (429, "no_live_seat".to_string()))
        .collect();
    assert_eq!(
        (seated.len(), refused.len()),
        (SEATS as usize, PEOPLE - SEATS as usize)
    );
    // Viewers with no name take no seat and are not refused.
    for _ in 0..5 {
        let v = ts.viewer(None).await;
        let s = open(&ts, viewer_cookie(&v.cookie)).await;
        assert_eq!(change(&ts, &s, &topic, &[]).await.0, 200);
    }
    // Every spectator is refused a comment; a seated person comments.
    let t = ts.thread_as_owner(&aid, "load").await;
    let tid = t["id"].as_str().unwrap();
    for i in &refused {
        let res = ts
            .client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/comments",
                ts.base
            ))
            .header("cookie", format!("clax_viewer={}", people[*i].cookie))
            .json(&json!({"body": "let me in"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403);
    }
    ts.reply_as(&aid, tid, &people[seated[0]].cookie, "seated")
        .await;
    // The spectators' polls, all at once and one after another, build the
    // snapshot once per 5 s.
    let poll = |i: usize| {
        ts.client
            .get(format!("{}/api/artifacts/{aid}/snapshot", ts.base))
            .header("cookie", format!("clax_viewer={}", people[i].cookie))
            .send()
    };
    let mut misses = 0;
    for res in futures::future::join_all(refused.iter().map(|i| poll(*i))).await {
        let res = res.unwrap();
        assert_eq!(res.status(), 200);
        misses += usize::from(res.headers()["x-clax-cache"] == "miss");
    }
    for i in &refused {
        misses += usize::from(poll(*i).await.unwrap().headers()["x-clax-cache"] == "miss");
    }
    assert_eq!(misses, 1, "{misses} builds for {} polls", 2 * refused.len());
    clock.advance(5);
    let v: Value = poll(refused[0]).await.unwrap().json().await.unwrap();
    assert_eq!(v["seats_free"], 0);
    assert_eq!(v["viewing"], PEOPLE as u64);
    // Closing seated streams' live topics frees their seats for spectators.
    for i in &seated[..10] {
        assert_eq!(change(&ts, &streams[*i], &[], &topic).await.0, 200);
    }
    for i in &refused[..10] {
        assert_eq!(change(&ts, &streams[*i], &topic, &[]).await.0, 200);
    }
    assert_eq!(change(&ts, &streams[refused[10]], &topic, &[]).await.0, 429);
    // Seated streams without input lose their seats after the idle time.
    clock.advance(180);
    ts.seats.sweep();
    let rest = &refused[10..];
    let outcomes =
        futures::future::join_all(rest.iter().map(|i| change(&ts, &streams[*i], &topic, &[])))
            .await;
    assert!(
        outcomes.iter().all(|o| o.0 == 200),
        "every spectator gets a seat"
    );
    // Guests take at most their share of what is free.
    let mut guests = 0;
    let mut held = Vec::new();
    for i in 0..30 {
        let g = ts.viewer(Some(&format!("Guest {i:02}"))).await;
        let s = open(&ts, viewer_cookie(&g.cookie)).await;
        guests += usize::from(change(&ts, &s, &topic, &[]).await.0 == 200);
        held.push(s);
    }
    assert_eq!(guests, 20);
}

/// An artifact whose page declares `db` and `room` (a `reaction` topic any
/// named viewer may send on).
async fn page_artifact(ts: &TestServer) -> String {
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Board", "capabilities": {"db": {}, "room": {"topics": {"reaction": "interact"}}},
                   "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Every db write a page makes, as `cookie`: what, status and error code.
async fn doc_writes(ts: &TestServer, aid: &str, cookie: &str) -> Vec<(&'static str, u16, String)> {
    let c = format!("clax_viewer={cookie}");
    let base = format!("{}/api/artifacts/{aid}", ts.base);
    let reqs = vec![
        (
            "put",
            ts.client
                .put(format!("{base}/docs/notes/n1"))
                .json(&json!({"data": {"t": "x"}})),
        ),
        (
            "patch",
            ts.client
                .patch(format!("{base}/docs/notes/n1"))
                .json(&json!({"data": {"t": "y"}, "lww": true})),
        ),
        (
            "batch",
            ts.client
                .post(format!("{base}/docs:batch"))
                .json(&json!({"ops": [{"op": "set", "path": "notes/n2", "data": {}}]})),
        ),
        (
            "str_replace",
            ts.client
                .post(format!("{base}/docs:str_replace"))
                .json(&json!({"path": "notes/n1", "field": "t", "old": "x", "new": "z"})),
        ),
        (
            "acquire",
            ts.client
                .post(format!("{base}/docs:acquire"))
                .json(&json!({"path": "notes/n3", "ttl_ms": 5000})),
        ),
        (
            "delete",
            ts.client.delete(format!("{base}/docs/notes/n1?lww=true")),
        ),
    ];
    let mut out = Vec::new();
    for (what, r) in reqs {
        let res = r.header("cookie", &c).send().await.unwrap();
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        out.push((
            what,
            status,
            v["error"]["code"].as_str().unwrap_or("").to_string(),
        ));
    }
    out
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A room socket on `aid` as `cookie`'s viewer, past its `welcome`.
async fn room(ts: &TestServer, aid: &str, peer: &str, cookie: &str) -> Ws {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let url = format!(
        "{}/api/artifacts/{aid}/room?peer={peer}",
        ts.base.replacen("http://", "ws://", 1)
    );
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert("cookie", format!("clax_viewer={cookie}").parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(frame(&mut ws).await["t"], "welcome");
    ws
}

async fn frame(ws: &mut Ws) -> Value {
    use futures::StreamExt;
    loop {
        let m = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("a frame within 5 s")
            .expect("open")
            .expect("readable");
        if let tokio_tungstenite::tungstenite::Message::Text(t) = m {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

async fn send(ws: &mut Ws, v: Value) {
    use futures::SinkExt;
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        v.to_string().into(),
    ))
    .await
    .unwrap();
}

/// The answer to `{t: "emit", id}`, skipping other frames.
async fn answer(ws: &mut Ws, id: u64) -> Value {
    loop {
        let f = frame(ws).await;
        if (f["t"] == "ack" || f["t"] == "nack") && f["id"] == id {
            return f;
        }
    }
}

#[tokio::test]
async fn a_spectators_page_writes_no_documents_and_sends_no_room_messages() {
    let (ts, _) = server(1, 1).await;
    let aid = page_artifact(&ts).await;
    let seated = ts.person("Sam").await;
    let s = open(&ts, viewer_cookie(&seated.cookie)).await;
    assert_eq!(change(&ts, &s, &live(&aid), &[]).await.0, 200);
    for who in [ts.person("Pat").await, ts.viewer(Some("Gil")).await] {
        for (what, status, code) in doc_writes(&ts, &aid, &who.cookie).await {
            assert_eq!((status, code.as_str()), (403, "spectator"), "{what}");
        }
    }
    // The seated person's page writes.
    let res = ts
        .client
        .put(format!("{}/api/artifacts/{aid}/docs/notes/n1", ts.base))
        .header("cookie", format!("clax_viewer={}", seated.cookie))
        .json(&json!({"data": {"t": "x"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // In the room, a spectator's emit is nacked and their presence dropped;
    // the seated person's go through.
    let pat = ts.person("Pat Room").await;
    let mut seat_ws = room(&ts, &aid, "aaaaaaaaaaaaaaaa", &seated.cookie).await;
    let mut spec_ws = room(&ts, &aid, "bbbbbbbbbbbbbbbb", &pat.cookie).await;
    send(
        &mut spec_ws,
        json!({"t": "presence", "room": null, "state": {"cursor": [1, 2]}}),
    )
    .await;
    send(
        &mut spec_ws,
        json!({"t": "emit", "id": 1, "room": null, "topic": "reaction"}),
    )
    .await;
    let nack = answer(&mut spec_ws, 1).await;
    assert_eq!(
        (
            nack["t"].as_str(),
            nack["code"].as_str(),
            nack["message"].as_str()
        ),
        (
            Some("nack"),
            Some("spectator"),
            Some(clax_core::seats::SPECTATOR_MESSAGE)
        )
    );
    send(
        &mut seat_ws,
        json!({"t": "emit", "id": 2, "room": null, "topic": "reaction"}),
    )
    .await;
    // The seated socket hears its own echo and ack, and never the
    // spectator's presence or message.
    loop {
        let f = frame(&mut seat_ws).await;
        assert!(
            !(f["t"] == "peer" && f["peer"]["presence"] != json!({})),
            "a spectator's presence reached a seated peer: {f}"
        );
        if f["t"] == "msg" {
            assert_eq!(f["msg"]["isMe"], true, "{f}");
        }
        if f["t"] == "ack" {
            assert_eq!(f["id"], 2);
            break;
        }
    }
}

#[tokio::test]
async fn spectators_are_counted_as_viewing_and_never_listed_as_here() {
    let (ts, _) = server(1, 1).await;
    let aid = artifact(&ts).await;
    let seated = ts.person("Sam").await;
    let s = open(&ts, viewer_cookie(&seated.cookie)).await;
    assert_eq!(change(&ts, &s, &live(&aid), &[]).await.0, 200);
    let report = |cookie: String| {
        let (ts, aid) = (&ts, aid.clone());
        async move {
            let res = ts
                .client
                .put(format!("{}/api/viewers/me/presence", ts.base))
                .header("cookie", format!("clax_viewer={cookie}"))
                .json(&json!({"artifact_id": aid, "state": "here"}))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200);
            res.json::<Value>().await.unwrap()
        }
    };
    let v = report(seated.cookie.clone()).await;
    assert_eq!(v["people"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["viewing"], 1);
    let (pat, gil) = (ts.person("Pat").await, ts.viewer(Some("Gil")).await);
    // Listed while a seat was free, Pat is dropped once a spectator.
    ts.seats.set_limits(limits(3, 3));
    let v = report(pat.cookie.clone()).await;
    assert_eq!(v["people"].as_array().unwrap().len(), 2, "{v}");
    ts.seats.set_limits(limits(1, 1));
    report(pat.cookie.clone()).await;
    let v = report(gil.cookie.clone()).await;
    let listed: Vec<&str> = v["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["public_id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, vec![seated.public_id.as_str()], "{v}");
    assert_eq!(v["viewing"], 3, "the seated person and two spectators");
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}/presence"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (v["people"].as_array().unwrap().len(), v["viewing"].as_u64()),
        (1, Some(3)),
        "{v}"
    );
}
