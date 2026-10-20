//! Linked threads (`clax_server::linked`): a local daemon linked to a
//! hosted test daemon keeps an artifact's comment threads in step with its
//! public twin's, both ways.

use clax_server::linked::{BACKOFF_CAP, BACKOFF_FIRST, BACKSTOP, Linked};
use clax_testkit::ManualSleeper;
use clax_testkit::TestServer;
use clax_testkit::remotes::{Public, connect, json_of, local_req, post};
use reqwest::header::{COOKIE, HOST, ORIGIN};
use serde_json::{Value, json};
use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

/// A local daemon linked to a hosted one: the artifact `aid` pushed there
/// as `raid`, the sync's timers on `sleeper`.
struct Pair {
    public: Public,
    local: TestServer,
    sleeper: Arc<ManualSleeper>,
    aid: String,
    raid: String,
    remote: String,
}

const PAGE: &str = "<h1>Plan</h1><p id=\"x\">Hello</p>";

impl Pair {
    async fn new() -> Pair {
        Pair::on("127.0.0.1".parse().unwrap()).await
    }

    async fn on(bind: IpAddr) -> Pair {
        let public = Public::new().await;
        let sleeper = Arc::new(ManualSleeper::default());
        let sl = sleeper.clone();
        let local = TestServer::spawn_on(bind, move |s| {
            s.linked = Arc::new(Linked::new(sl));
        })
        .await;
        let remote = connect(&local, &public).await;
        let v = local.publish("Linked", &[("index.html", PAGE)]).await;
        let aid = v["artifact"]["id"].as_str().unwrap().to_string();
        let (s, v) = post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
        assert_eq!(s, 200, "{v}");
        let raid = v["link"]["remote_artifact_id"]
            .as_str()
            .unwrap()
            .to_string();
        let p = Pair {
            public,
            local,
            sleeper,
            aid,
            raid,
            remote: remote["id"].as_str().unwrap().to_string(),
        };
        // The sync's first cycle is done once it waits for its backstop.
        p.sleeper.wait_for(BACKSTOP, 1).await;
        p
    }

    /// Fires the backstop once the sync waits on it: a full read.
    async fn backstop(&self) {
        self.sleeper.wait_for(BACKSTOP, 1).await;
        self.sleeper.fire(BACKSTOP);
    }

    /// A thread the hosted owner's browser starts on the public copy.
    async fn hosted_thread(&self, body: &str, anchor: Value, clip: Option<&[u8]>) -> Value {
        let mut form = reqwest::multipart::Form::new()
            .text("anchor", anchor.to_string())
            .text("body", body.to_string())
            .text("version", "1");
        if let Some(b) = clip {
            form = form.part(
                "clip",
                reqwest::multipart::Part::bytes(b.to_vec())
                    .file_name("clip.png")
                    .mime_str("image/png")
                    .unwrap(),
            );
        }
        let ts = &self.public.ts;
        let res = ts
            .client
            .post(format!("{}/api/artifacts/{}/threads", ts.base, self.raid))
            .header(HOST, format!("localhost:{}", ts.addr.port()))
            .header(COOKIE, format!("clax_session={}", self.public.cookie))
            .header(ORIGIN, &self.public.origin)
            .multipart(form)
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(res).await;
        assert_eq!(s, 201, "{v}");
        v["thread"].clone()
    }

    /// The public copy's threads, as its owner's CLI reads them.
    async fn hosted_threads(&self) -> Vec<Value> {
        let (s, v) = self
            .public
            .cli(
                reqwest::Method::GET,
                &format!("/api/artifacts/{}/threads?include_resolved=true", self.raid),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["threads"].as_array().cloned().unwrap_or_default()
    }

    async fn hosted_thread_by(&self, rtid: &str) -> Value {
        self.hosted_threads()
            .await
            .into_iter()
            .find(|t| t["id"] == rtid)
            .unwrap_or_else(|| panic!("no public thread {rtid}"))
    }

    /// The local artifact's threads, as the owner reads them.
    async fn local_threads(&self) -> Vec<Value> {
        let v: Value = self
            .local
            .get_authed(&format!(
                "/api/artifacts/{}/threads?include_resolved=true",
                self.aid
            ))
            .await
            .json()
            .await
            .unwrap();
        v["threads"].as_array().cloned().unwrap_or_default()
    }

    async fn local_req(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Value,
        sid: Option<&str>,
    ) -> (u16, Value) {
        local_req(&self.local, method, path, body, sid).await
    }

    /// A thread the owner starts here on version `n`; its view.
    async fn owner_thread(&self, n: u32, body: &str, anchor: Value) -> Value {
        let res = self
            .local
            .authed(self.local.client.post(format!(
                "{}/api/artifacts/{}/threads",
                self.local.base, self.aid
            )))
            .multipart(
                reqwest::multipart::Form::new()
                    .text("anchor", anchor.to_string())
                    .text("body", body.to_string())
                    .text("version", n.to_string()),
            )
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(res).await;
        assert_eq!(s, 201, "{v}");
        v["thread"].clone()
    }

    /// A local agent session watching the artifact.
    async fn local_agent(&self, name: &str) -> String {
        let sid = self
            .local
            .register_session("claude", &format!("h-{name}"))
            .await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (s, v) = self
            .local_req(
                reqwest::Method::PATCH,
                &format!("/api/sessions/{sid}"),
                json!({"name": name}),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        let (s, v) = self
            .local_req(
                reqwest::Method::PUT,
                &format!("/api/sessions/{sid}/watches/{}", self.aid),
                json!({}),
                Some(&sid),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        sid
    }

    /// A local agent's `wait_for_feedback`.
    async fn wait_for_feedback(&self, sid: &str) -> Value {
        let res = self
            .local
            .authed(self.local.client.get(format!(
                "{}/api/sessions/{sid}/feedback?tier=wait&wait=20",
                self.local.base
            )))
            .header("x-clax-session", sid)
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(res).await;
        assert_eq!(s, 200, "{v}");
        v
    }
}

/// Polls `f` until it answers `Some`, for up to 10 s.
/// `f`'s value once it has one, checked again every 50 ms. No wall-clock
/// limit: a load that slows the daemons only delays it, and a condition
/// that never holds is ended by the test runner's timeout, whose output
/// names `what`.
async fn until<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    for tries in 0u32.. {
        if let Some(v) = f().await {
            return v;
        }
        if tries == 200 {
            eprintln!("still waiting: {what}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    unreachable!("the loop returns")
}

fn whole() -> Value {
    json!({"kind": "artifact"})
}

fn element() -> Value {
    json!({"kind": "element", "selector": "#x", "quote": "Hello", "file": "index.html"})
}

fn bodies(t: &Value) -> Vec<String> {
    t["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_hosted_comment_reaches_a_local_agent_and_its_reply_and_resolve_go_back_once() {
    let p = Pair::new().await;
    let sid = p.local_agent("fixer").await;
    // A comment on the public copy, sent to the agent there.
    let rt = p
        .hosted_thread("@agent make the title bigger", element(), None)
        .await;
    let rtid = rt["id"].as_str().unwrap().to_string();
    // The sync wakes on its feedback poll there (no backstop fired): the
    // plugin's channel hears of it (a notice, naming the thread only)...
    let res = p
        .local
        .authed(p.local.client.get(format!(
            "{}/api/sessions/{sid}/notices?wait=20",
            p.local.base
        )))
        .header("x-clax-session", &sid)
        .send()
        .await
        .unwrap();
    let (s, notices) = json_of(res).await;
    assert_eq!(s, 200, "{notices}");
    let noticed = notices["notices"][0]["thread_id"].clone();
    assert!(noticed.is_string(), "{notices}");
    assert!(
        !notices.to_string().contains("make the title bigger"),
        "a notice holds no comment"
    );
    // ... and the local agent's wait_for_feedback hands it over.
    let fb = p.wait_for_feedback(&sid).await;
    assert_eq!(fb["feedback"][0]["thread_id"], noticed, "{fb}");
    let item = &fb["feedback"][0];
    assert_eq!(item["body"], "@agent make the title bigger", "{fb}");
    assert_eq!(
        item["author"],
        format!("Ada (on localhost:{})", p.public.ts.addr.port()),
        "{fb}"
    );
    let tid = item["thread_id"].as_str().unwrap().to_string();
    // Anchored as there, on its twin version, tagged with the remote.
    let local = p.local_threads().await;
    let t = local.iter().find(|t| t["id"] == tid.as_str()).unwrap();
    assert_eq!(t["version_n"], 1);
    assert_eq!(t["anchor"]["selector"], "#x");
    assert_eq!(t["linked"]["origin"], "remote", "{t}");
    assert_eq!(t["linked"]["twin"], true);
    assert_eq!(
        t["linked"]["remote"],
        format!("localhost:{}", p.public.ts.addr.port())
    );
    assert_eq!(t["comments"][0]["linked"]["author_name"], "Ada");
    assert_eq!(
        t["comments"][0]["author_public_id"],
        Value::Null,
        "never a local person"
    );
    // The local agent replies; the remote's first answer is lost.
    p.local.state.linked.lose_next_answers(1);
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{tid}/comments", p.aid),
            json!({"body": "Bigger now.", "author_kind": "agent"}),
            Some(&sid),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // Sent, answer lost: tried again after the backoff, with its key.
    p.sleeper.wait_for(BACKOFF_FIRST, 1).await;
    p.sleeper.fire(BACKOFF_FIRST);
    let there = until("the reply reaches the public thread", || async {
        let t = p.hosted_thread_by(&rtid).await;
        (t["comments"].as_array().unwrap().len() >= 2).then_some(t)
    })
    .await;
    p.backstop().await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    let there = {
        let t = p.hosted_thread_by(&rtid).await;
        assert_eq!(bodies(&t), bodies(&there));
        t
    };
    assert_eq!(
        bodies(&there),
        ["@agent make the title bigger", "Bigger now."],
        "exactly once"
    );
    assert_eq!(there["comments"][1]["author_kind"], "agent", "{there}");
    // Its echo is the same local comment.
    let t = p
        .local_threads()
        .await
        .into_iter()
        .find(|t| t["id"] == tid.as_str())
        .unwrap();
    assert_eq!(bodies(&t), ["@agent make the title bigger", "Bigger now."]);
    // The local agent resolves; it resolves there.
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{tid}/resolve", p.aid),
            json!({"as": "agent"}),
            Some(&sid),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    until("the public thread is resolved", || async {
        (p.hosted_thread_by(&rtid).await["status"] == "resolved").then_some(())
    })
    .await;
    // Reopened there, then resolved there: both come down.
    let (s, v) = p
        .public
        .browser(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{rtid}/reopen", p.raid),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    p.backstop().await;
    until("the local thread is reopened", || async {
        let ts = p.local_threads().await;
        (ts.iter().find(|t| t["id"] == tid.as_str())?["status"] == "open").then_some(())
    })
    .await;
    let (s, v) = p
        .public
        .browser(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{rtid}/resolve", p.raid),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    p.backstop().await;
    until("the local thread is resolved", || async {
        let ts = p.local_threads().await;
        (ts.iter().find(|t| t["id"] == tid.as_str())?["status"] == "resolved").then_some(())
    })
    .await;
    // Nothing went up twice.
    assert_eq!(p.hosted_threads().await.len(), 1);
    assert_eq!(bodies(&p.hosted_thread_by(&rtid).await).len(), 2);
}

#[tokio::test]
async fn a_local_thread_goes_up_and_one_on_an_unpushed_version_waits_for_its_push() {
    let p = Pair::new().await;
    let t = p.owner_thread(1, "a local note", whole()).await;
    let tid = t["id"].as_str().unwrap().to_string();
    let there = until("the local thread reaches the public copy", || async {
        let ts = p.hosted_threads().await;
        (!ts.is_empty()).then_some(ts)
    })
    .await;
    assert_eq!(there.len(), 1);
    assert_eq!(bodies(&there[0]), ["a local note"]);
    assert_eq!(
        there[0]["comments"][0]["author_name"], "Ada",
        "as the token's person"
    );
    // Its echo is the same thread here.
    p.backstop().await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    let local = p.local_threads().await;
    assert_eq!(local.len(), 1);
    assert_eq!(local[0]["id"], tid.as_str());
    assert_eq!(local[0]["linked"]["origin"], "local");
    assert_eq!(local[0]["linked"]["state"], Value::Null, "acknowledged");
    // A thread on a version never pushed is held until it is.
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/versions", p.aid),
            json!({"if_version": 1, "files": {"index.html": {"content": "<h1>Plan 2</h1><p id=\"x\">Hello</p>", "encoding": "utf8"}}}),
            None,
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let v = p.owner_thread(2, "on version two", element()).await;
    let held = v["id"].as_str().unwrap().to_string();
    // Someone else's thread here stays here.
    p.local.thread(&p.aid, 1, "a LAN viewer's").await;
    until("the thread is held", || async {
        let ts = p.local_threads().await;
        (ts.iter().find(|t| t["id"] == held.as_str())?["linked"]["state"] == "held").then_some(())
    })
    .await;
    assert_eq!(p.hosted_threads().await.len(), 1);
    let (s, v) = post(
        &p.local,
        &format!("/api/artifacts/{}/push", p.aid),
        json!({}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let up = until("the held thread goes up on its twin", || async {
        p.hosted_threads()
            .await
            .into_iter()
            .find(|t| t["comments"][0]["body"] == "on version two")
    })
    .await;
    assert_eq!(up["version_n"], 2);
    assert_eq!(up["anchor"]["selector"], "#x");
    // Download only: a new local thread stays local.
    let (s, v) = p
        .local_req(
            reqwest::Method::PATCH,
            &format!("/api/artifacts/{}/remotes/{}", p.aid, p.remote),
            json!({"download_only": true}),
            None,
        )
        .await;
    assert_eq!((s, &v["download_only"]), (200, &json!(true)), "{v}");
    p.owner_thread(2, "kept here", whole()).await;
    p.backstop().await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    assert_eq!(p.hosted_threads().await.len(), 2);
    // ... while a reply on a linked thread still goes up.
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{tid}/comments", p.aid),
            json!({"body": "still linked"}),
            None,
        )
        .await;
    assert_eq!(s, 201, "{v}");
    until("the reply goes up", || async {
        let ts = p.hosted_threads().await;
        ts.iter()
            .any(|t| bodies(t) == ["a local note", "still linked"])
            .then_some(())
    })
    .await;
}

#[tokio::test]
async fn since_is_an_agent_tokens_and_a_keyed_comment_is_written_once() {
    let p = Pair::new().await;
    let rt = p.hosted_thread("first", whole(), None).await;
    let rtid = rt["id"].as_str().unwrap();
    // The owner's CLI there may not narrow by time.
    let (s, v) = p
        .public
        .cli(
            reqwest::Method::GET,
            &format!(
                "/api/artifacts/{}/threads?since=2026-01-01T00:00:00.000Z",
                p.raid
            ),
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_query")),
        "{v}"
    );
    // The local daemon's token: a session, a keyed comment sent twice.
    let token = p.local.store.remote_token(&p.remote).unwrap().unwrap();
    let ts = &p.public.ts;
    let req = |m: reqwest::Method, path: &str| {
        ts.client
            .request(m, format!("{}{path}", ts.base))
            .header(HOST, format!("localhost:{}", ts.addr.port()))
            .bearer_auth(token.expose())
    };
    let (s, v) = json_of(
        req(reqwest::Method::POST, "/api/sessions")
            .json(&json!({"harness": "claude", "cwd": ""}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let (sid, secret) = (
        v["session"]["id"].as_str().unwrap().to_string(),
        v["session_secret"].as_str().unwrap().to_string(),
    );
    let mut ids = Vec::new();
    for _ in 0..2 {
        let (s, v) = json_of(
            req(
                reqwest::Method::POST,
                &format!("/api/artifacts/{}/threads/{rtid}/comments", p.raid),
            )
            .header("x-clax-session", &sid)
            .header(clax_core::remote::SESSION_SECRET_HEADER, &secret)
            .header("idempotency-key", "clax-linked:comment:k1")
            .json(&json!({"body": "once"}))
            .send()
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(s, 201, "{v}");
        ids.push(v["comment"]["id"].clone());
    }
    assert_eq!(ids[0], ids[1]);
    assert_eq!(bodies(&p.hosted_thread_by(rtid).await), ["first", "once"]);
    // An agent token's since lists what changed at or after it.
    let later = "2999-01-01T00:00:00.000Z";
    let (s, v) = json_of(
        req(
            reqwest::Method::GET,
            &format!("/api/artifacts/{}/threads?since={later}", p.raid),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["threads"], json!([]));
}

#[tokio::test]
async fn an_outage_queues_marks_rows_not_sent_at_the_cap_then_drains_in_order() {
    let p = Pair::new().await;
    let t = p.owner_thread(1, "before", whole()).await;
    let tid = t["id"].as_str().unwrap().to_string();
    until("the thread goes up", || async {
        (!p.hosted_threads().await.is_empty()).then_some(())
    })
    .await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    p.local.state.linked.set_offline(true);
    for b in ["one", "two"] {
        let (s, v) = p
            .local_req(
                reqwest::Method::POST,
                &format!("/api/artifacts/{}/threads/{tid}/comments", p.aid),
                json!({"body": b}),
                None,
            )
            .await;
        assert_eq!(s, 201, "{v}");
    }
    // Queued, pending, and the link says since when it is unreachable.
    let mut d = BACKOFF_FIRST;
    p.sleeper.wait_for(d, 1).await;
    until("both are queued", || async {
        let ts = p.local_threads().await;
        (ts[0]["comments"][2]["linked"]["state"] == "pending").then_some(())
    })
    .await;
    let (_, links) = p
        .local_req(
            reqwest::Method::GET,
            &format!("/api/artifacts/{}/remotes", p.aid),
            json!(null),
            None,
        )
        .await;
    assert!(
        links["links"][0]["sync"]["unreachable_since"].is_string(),
        "{links}"
    );
    assert_eq!(links["links"][0]["pending"], 2, "{links}");
    let local = p.local_threads().await;
    assert_eq!(local[0]["comments"][1]["linked"]["state"], "pending");
    // The backoff doubles to its cap; reaching it marks the rows not sent.
    loop {
        p.sleeper.wait_for(d, 1).await;
        if d == BACKOFF_CAP {
            break;
        }
        p.sleeper.fire(d);
        d = (d * 2).min(BACKOFF_CAP);
    }
    let (_, links) = p
        .local_req(
            reqwest::Method::GET,
            &format!("/api/artifacts/{}/remotes", p.aid),
            json!(null),
            None,
        )
        .await;
    assert_eq!(links["links"][0]["failed"], 2, "{links}");
    let local = p.local_threads().await;
    assert_eq!(local[0]["comments"][2]["linked"]["state"], "failed");
    // Back: Retry drains them, in order, and first tells every open page
    // at once that they are sending again.
    p.local.state.linked.set_offline(false);
    let mut events = p.local.state.events.subscribe();
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/remotes/{}/retry", p.aid, p.remote),
            json!({}),
            None,
        )
        .await;
    assert_eq!((s, &v["retried"]), (200, &json!(2)), "{v}");
    let thread = loop {
        let ev = events.recv().await.unwrap();
        if let clax_core::Event::Thread { thread, .. } = ev.event
            && thread["id"] == tid.as_str()
        {
            break thread;
        }
    };
    assert_eq!(
        thread["comments"][2]["linked"]["state"], "pending",
        "{thread}"
    );
    until("the rows drain in order", || async {
        let ts = p.hosted_threads().await;
        (ts.len() == 1 && bodies(&ts[0]) == ["before", "one", "two"]).then_some(())
    })
    .await;
    // The remote shows a row before the sync records its answer here: the
    // link counts it pending until then.
    until("the rows are acknowledged here", || async {
        let (_, links) = p
            .local_req(
                reqwest::Method::GET,
                &format!("/api/artifacts/{}/remotes", p.aid),
                json!(null),
                None,
            )
            .await;
        (links["links"][0]["pending"] == 0 && links["links"][0]["failed"] == 0).then_some(())
    })
    .await;
}

#[tokio::test]
async fn a_reply_whose_answer_was_lost_before_an_outage_comes_back_as_itself() {
    let p = Pair::new().await;
    let rt = p.hosted_thread("from there", whole(), None).await;
    let rtid = rt["id"].as_str().unwrap().to_string();
    p.backstop().await;
    let tid = until("the thread comes down", || async {
        p.local_threads()
            .await
            .first()
            .map(|t| t["id"].as_str().unwrap().to_string())
    })
    .await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    // The reply reaches the remote, its answer is lost, and the remote is
    // then out of reach past the backoff's cap: the row is marked not sent.
    p.local.state.linked.lose_next_answers(1);
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{tid}/comments", p.aid),
            json!({"body": "made there"}),
            None,
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let mut d = BACKOFF_FIRST;
    p.sleeper.wait_for(d, 1).await;
    p.local.state.linked.set_offline(true);
    loop {
        p.sleeper.wait_for(d, 1).await;
        if d == BACKOFF_CAP {
            break;
        }
        p.sleeper.fire(d);
        d = (d * 2).min(BACKOFF_CAP);
    }
    assert_eq!(
        bodies(&p.hosted_thread_by(&rtid).await),
        ["from there", "made there"]
    );
    // Back, with no Retry: the next read finds the reply there. It is the
    // reply made here, not a second copy of it from the remote.
    p.local.state.linked.set_offline(false);
    p.sleeper.fire(BACKOFF_CAP);
    p.sleeper.wait_for(BACKSTOP, 1).await;
    p.backstop().await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    let t = p
        .local_threads()
        .await
        .into_iter()
        .find(|t| t["id"] == tid.as_str())
        .unwrap();
    assert_eq!(bodies(&t), ["from there", "made there"], "{t}");
    assert_eq!(t["comments"][1]["linked"]["origin"], "local", "{t}");
    assert_eq!(t["comments"][1]["linked"]["state"], Value::Null, "{t}");
    assert_eq!(
        bodies(&p.hosted_thread_by(&rtid).await),
        ["from there", "made there"]
    );
}

/// The smallest PNG a clip check takes: its signature and a little more.
fn png() -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(b"\0\0\0\rIHDR clip bytes");
    b
}

#[tokio::test]
async fn a_remote_clip_is_cached_here_and_no_remote_path_reaches_an_agent() {
    let p = Pair::new().await;
    let sid = p.local_agent("viewer").await;
    let rt = p
        .hosted_thread("@agent see the shot", element(), Some(&png()))
        .await;
    assert!(rt["clip_url"].is_string(), "{rt}");
    let fb = p.wait_for_feedback(&sid).await;
    let tid = fb["feedback"][0]["thread_id"].as_str().unwrap().to_string();
    let cache = p
        .local
        .state
        .home
        .root()
        .join("remote-cache")
        .join("clips")
        .join(format!("{tid}.png"));
    // The file is written before the thread is marked as having it: wait
    // for the thread.
    let path = format!("/api/artifacts/{}/threads/{tid}", p.aid);
    let v: Value = until("the clip is cached", || async {
        let v: Value = p.local.get_authed(&path).await.json().await.unwrap();
        (cache.is_file() && v["thread"]["has_clip"] == json!(true)).then_some(v)
    })
    .await;
    assert_eq!(std::fs::read(&cache).unwrap(), png());
    // The thread names only the cached file here, and the clip route serves it.
    assert_eq!(
        v["thread"]["clip_path"],
        cache.to_string_lossy().as_ref(),
        "{v}"
    );
    let hosted_home = p.public.ts.state.home.root().to_string_lossy().to_string();
    assert!(!v.to_string().contains(&hosted_home), "{v}");
    let res = p
        .local
        .get_authed(&format!("/api/artifacts/{}/threads/{tid}/clip", p.aid))
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.bytes().await.unwrap().to_vec(), png());
    // A later handover to an agent names the cached file, never a remote path.
    let (s, v) = p
        .public
        .browser(
            reqwest::Method::POST,
            &format!(
                "/api/artifacts/{}/threads/{}/comments",
                p.raid,
                rt["id"].as_str().unwrap()
            ),
            json!({"body": "and again"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    p.backstop().await;
    let fb = p.wait_for_feedback(&sid).await;
    let text = fb.to_string();
    assert!(!text.contains(&hosted_home), "{text}");
    for i in fb["feedback"].as_array().unwrap() {
        if !i["clip_path"].is_null() {
            assert_eq!(i["clip_path"], cache.to_string_lossy().as_ref());
        }
    }
}

#[tokio::test]
async fn another_local_person_who_cannot_read_the_artifact_sees_no_mirrored_thread() {
    let p = Pair::on("0.0.0.0".parse().unwrap()).await;
    p.hosted_thread("a public remark", whole(), None).await;
    p.backstop().await;
    until("the thread comes down", || async {
        (p.local_threads().await.len() == 1).then_some(())
    })
    .await;
    let sam = p.local.connect_person("Sam", "clax:collaborator").await;
    let other = p
        .local
        .publish("Other", &[("index.html", "<p>other</p>")])
        .await;
    let other = other["artifact"]["id"].as_str().unwrap().to_string();
    let rights = clax_core::rights::Rights::parse(&json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": [other]},
        {"type": "clax-comments", "actions": ["read", "write", "reply", "resolve", "send", "watch"], "locations": [other]},
    ]))
    .unwrap();
    let sam_public = p
        .local
        .store
        .viewer_of_grant(&sam.grant_id())
        .unwrap()
        .public_id;
    p.local
        .store
        .set_person_rights(
            &clax_core::audit::AuditCtx::DAEMON,
            &sam_public,
            Some(rights),
        )
        .unwrap();
    let ssid = sam.register("claude", "sam-h").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = sam
        .agent(
            "GET",
            &format!("/api/artifacts/{}/threads", p.aid),
            Some(&ssid),
            None,
        )
        .await;
    let status = res.status().as_u16();
    let text = res.text().await.unwrap();
    assert!(status == 403 || status == 404, "{status} {text}");
    assert!(!text.contains("a public remark"), "{text}");
    let res = sam.agent("GET", "/api/threads", Some(&ssid), None).await;
    let text = res.text().await.unwrap();
    assert!(!text.contains("a public remark"), "{text}");
    assert!(!text.contains(&p.aid), "{text}");
}

#[tokio::test]
async fn unlinking_keeps_read_only_history_and_unsent_rows_until_dropped() {
    let p = Pair::new().await;
    p.hosted_thread("from there", whole(), None).await;
    p.backstop().await;
    let tid = until("the thread comes down", || async {
        p.local_threads()
            .await
            .first()
            .map(|t| t["id"].as_str().unwrap().to_string())
    })
    .await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    // A reply that cannot go up before the link ends.
    p.local.state.linked.set_offline(true);
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{tid}/comments", p.aid),
            json!({"body": "unsent"}),
            None,
        )
        .await;
    assert_eq!(s, 201, "{v}");
    p.sleeper.wait_for(BACKOFF_FIRST, 1).await;
    let (s, v) = p
        .local_req(
            reqwest::Method::DELETE,
            &format!("/api/artifacts/{}/remotes/{}", p.aid, p.remote),
            json!(null),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["unsent"].as_array().unwrap().len(), 1, "{v}");
    until("the sync stops", || async {
        (!p.local.state.linked.running(&p.remote)).then_some(())
    })
    .await;
    let t = &p.local_threads().await[0];
    assert_eq!(t["linked"]["unlinked"], true, "{t}");
    // Read-only now.
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{tid}/comments", p.aid),
            json!({"body": "more"}),
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("thread_unlinked")),
        "{v}"
    );
    // The unsent row waits for the owner, then goes.
    let (_, links) = p
        .local_req(
            reqwest::Method::GET,
            &format!("/api/artifacts/{}/remotes", p.aid),
            json!(null),
            None,
        )
        .await;
    assert_eq!(links["unsent"][0]["body"], "unsent", "{links}");
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/unsent:drop", p.aid),
            json!({}),
            None,
        )
        .await;
    assert_eq!((s, &v["dropped"]), (200, &json!(1)), "{v}");
    assert_eq!(
        p.hosted_threads().await[0]["comments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// An `ask` of one choice question.
fn ask_body(artifact: Option<&str>, text: &str) -> Value {
    let mut b = json!({"source": "ask", "questions": [{"question": text, "header": "Pick",
        "options": [{"label": "Red"}, {"label": "Blue"}]}]});
    if let Some(a) = artifact {
        b["artifact_id"] = json!(a);
    }
    b
}

impl Pair {
    /// Local agent `sid` asks; the question's view.
    async fn ask(&self, sid: &str, artifact: Option<&str>, text: &str) -> Value {
        let (s, v) = self
            .local_req(
                reqwest::Method::POST,
                &format!("/api/sessions/{sid}/questions"),
                ask_body(artifact, text),
                Some(sid),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["question"].clone()
    }

    /// Local agent `sid`'s poll of question `qid`, waiting up to `wait` s.
    async fn ask_wait(&self, sid: &str, qid: &str, wait: u64) -> Value {
        let (s, v) = self
            .local_req(
                reqwest::Method::GET,
                &format!("/api/sessions/{sid}/questions/{qid}?wait={wait}"),
                json!(null),
                Some(sid),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["question"].clone()
    }

    /// The local question `qid` as the owner sees it.
    async fn local_question(&self, qid: &str) -> Value {
        let v: Value = self
            .local
            .get_authed(&format!("/api/questions/{qid}"))
            .await
            .json()
            .await
            .unwrap();
        v["question"].clone()
    }

    /// The hosted owner's questions, every status.
    async fn hosted_questions(&self) -> Vec<Value> {
        let (s, v) = self
            .public
            .browser(
                reqwest::Method::GET,
                "/api/questions?status=all",
                json!(null),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["questions"].as_array().cloned().unwrap_or_default()
    }

    /// The copy there of the question asked as `text`, once it is there.
    async fn hosted_question(&self, text: &str) -> Value {
        until("the question is asked there", || async {
            self.hosted_questions()
                .await
                .into_iter()
                .find(|q| q["questions"][0]["question"] == text)
        })
        .await
    }
}

#[tokio::test]
async fn a_local_ask_on_a_linked_artifact_is_asked_there_and_answered_from_there() {
    let p = Pair::new().await;
    let sid = p.local_agent("asker").await;
    let q = p.ask(&sid, Some(&p.aid), "Which colour?").await;
    let qid = q["id"].as_str().unwrap().to_string();
    // The local agent waits, as `ask` does.
    let waiting = {
        let req = p
            .local
            .authed(p.local.client.get(format!(
                "{}/api/sessions/{sid}/questions/{qid}?wait=30",
                p.local.base
            )))
            .header("x-clax-session", &sid);
        tokio::spawn(async move {
            let (s, v) = json_of(req.send().await.unwrap()).await;
            assert_eq!(s, 200, "{v}");
            v["question"].clone()
        })
    };
    // Asked there, about the public copy, with the same questions.
    let there = p.hosted_question("Which colour?").await;
    assert_eq!(there["status"], "open", "{there}");
    assert_eq!(there["artifact"]["id"], p.raid.as_str(), "{there}");
    assert_eq!(there["questions"], q["questions"], "{there}");
    assert_eq!(there["source"], "ask");
    // Sent: the local card says where.
    let local = until("the copy is acknowledged here", || async {
        let l = p.local_question(&qid).await;
        (l["linked"]["sent"] == true).then_some(l)
    })
    .await;
    assert_eq!(
        local["linked"]["remote"],
        format!("localhost:{}", p.public.ts.addr.port())
    );
    assert_eq!(local["linked"]["state"], Value::Null, "{local}");
    // The hosted owner answers there; the local `ask` returns that answer.
    let rqid = there["id"].as_str().unwrap();
    let (s, v) = p
        .public
        .browser(
            reqwest::Method::POST,
            &format!("/api/questions/{rqid}/answer"),
            json!({"answers": [{"selected": ["Blue"]}]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let answered = waiting.await.unwrap();
    assert_eq!(answered["status"], "answered", "{answered}");
    assert_eq!(
        answered["answers"],
        json!([{"selected": ["Blue"], "text": null}])
    );
    let local = p.local_question(&qid).await;
    assert_eq!(local["linked"]["answered_there"], true, "{local}");
    // A decline there declines here.
    let q2 = p.ask(&sid, Some(&p.aid), "Ship it?").await;
    let there = p.hosted_question("Ship it?").await;
    let (s, v) = p
        .public
        .browser(
            reqwest::Method::POST,
            &format!("/api/questions/{}/decline", there["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let declined = p.ask_wait(&sid, q2["id"].as_str().unwrap(), 20).await;
    assert_eq!(declined["status"], "declined", "{declined}");
    // Each was asked there once.
    p.backstop().await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    assert_eq!(p.hosted_questions().await.len(), 2);
}

impl Pair {
    /// The live agents of another Clax on the public copy, as its
    /// participants list them.
    async fn relayed(&self) -> Vec<Value> {
        let path = format!("/api/artifacts/{}", self.raid);
        let (s, v) = self
            .public
            .browser(reqwest::Method::GET, &path, json!(null))
            .await;
        assert_eq!(s, 200, "{v}");
        v["artifact"]["participants"]["agents"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a["live"] == true && a["via"].is_string())
            .collect()
    }
}

/// The public copy names the agent the sync relays for as the local agent
/// live on the artifact, with whose Clax it relays from: in its
/// participants (the roster and the Send menu) and on a question the local
/// agent asks. With no local agent live there, the sync's own name shows.
#[tokio::test]
async fn the_public_copy_names_the_local_agent_the_sync_relays_for() {
    let p = Pair::new().await;
    let alone = p.relayed().await;
    assert_eq!(alone.len(), 1, "{alone:?}");
    assert_eq!(alone[0]["name"], "clax-sync", "{alone:?}");
    // The local agent's watch starts a cycle: no backstop is fired.
    let sid = p.local_agent("asker").await;
    let named = until("the public copy names the local agent", || async {
        let a = p.relayed().await;
        (a.len() == 1 && a[0]["name"] == "asker").then_some(a)
    })
    .await;
    let person = named[0]["person"]["name"].as_str().unwrap().to_string();
    assert_eq!(named[0]["via"], format!("{person}'s Clax"), "{named:?}");
    assert_eq!(named[0]["harness"], "claude", "{named:?}");
    // Whether a comment wakes it is for the Clax it runs on, which relays
    // comments to it: this one does not say.
    assert!(named[0].get("wakes").is_none(), "{named:?}");
    // Its question is asked there as that agent, from that Clax.
    p.ask(&sid, Some(&p.aid), "Which colour?").await;
    let there = p.hosted_question("Which colour?").await;
    assert_eq!(there["agent"]["name"], "asker", "{there}");
    assert_eq!(there["agent"]["via"], format!("{person}'s Clax"), "{there}");
    // The inbox's agent strip there names it so too.
    let (s, v) = p
        .public
        .browser(reqwest::Method::GET, "/api/inbox?group=agent", json!(null))
        .await;
    assert_eq!(s, 200, "{v}");
    let strip = v["agents"].as_array().cloned().unwrap_or_default();
    let asker = strip
        .iter()
        .find(|a| a["name"] == "asker")
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(asker["via"], format!("{person}'s Clax"), "{v}");
    assert!(asker["wakes"].is_null(), "{v}");
    // Its session ending here leaves the sync's own name there.
    let (s, v) = p
        .local_req(
            reqwest::Method::PATCH,
            &format!("/api/sessions/{sid}"),
            json!({"ended": true}),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    until("the public copy names the sync again", || async {
        let a = p.relayed().await;
        (a.len() == 1 && a[0]["name"] == "clax-sync").then_some(())
    })
    .await;
}

/// An open copy there is waited on by one poll at a time, however often
/// the sync wakes: the remote holds at most four polls of one question.
#[tokio::test]
async fn an_open_copy_holds_one_poll_there_however_often_the_sync_wakes() {
    let p = Pair::new().await;
    let sid = p.local_agent("asker").await;
    let q = p.ask(&sid, Some(&p.aid), "Which colour?").await;
    let qid = q["id"].as_str().unwrap().to_string();
    let there = p.hosted_question("Which colour?").await;
    let rqid = there["id"].as_str().unwrap().to_string();
    p.public.ts.wait_question_waiters(&rqid, 1).await;
    for _ in 0..5 {
        p.backstop().await;
    }
    p.sleeper.wait_for(BACKSTOP, 1).await;
    // A wake ends the poll there and starts the next, so the count can be
    // 0 for a moment: wait for it to be 1. Polls left holding would keep it
    // above 1, and this wait would fail.
    p.public.ts.wait_question_waiters(&rqid, 1).await;
    // Answered there, the local question closes.
    let (s, v) = p
        .public
        .browser(
            reqwest::Method::POST,
            &format!("/api/questions/{rqid}/answer"),
            json!({"answers": [{"selected": ["Blue"]}]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let answered = p.ask_wait(&sid, &qid, 30).await;
    assert_eq!(answered["status"], "answered", "{answered}");
}

#[tokio::test]
async fn a_question_closed_here_first_is_withdrawn_there() {
    let p = Pair::new().await;
    let sid = p.local_agent("asker").await;
    // Answered here first.
    let q = p.ask(&sid, Some(&p.aid), "Answered here?").await;
    let there = p.hosted_question("Answered here?").await;
    let (s, v) = post(
        &p.local,
        &format!("/api/questions/{}/answer", q["id"].as_str().unwrap()),
        json!({"answers": [{"selected": ["Red"]}]}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let rqid = there["id"].as_str().unwrap().to_string();
    until("the copy there is withdrawn", || async {
        let qs = p.hosted_questions().await;
        (qs.iter().find(|q| q["id"] == rqid.as_str())?["status"] == "withdrawn").then_some(())
    })
    .await;
    // The agent withdraws its own; the copy there is withdrawn too.
    let q = p.ask(&sid, Some(&p.aid), "Withdrawn here?").await;
    let there = p.hosted_question("Withdrawn here?").await;
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!(
                "/api/sessions/{sid}/questions/{}/withdraw",
                q["id"].as_str().unwrap()
            ),
            json!({}),
            Some(&sid),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let rqid = there["id"].as_str().unwrap().to_string();
    until("the second copy there is withdrawn", || async {
        let qs = p.hosted_questions().await;
        (qs.iter().find(|q| q["id"] == rqid.as_str())?["status"] == "withdrawn").then_some(())
    })
    .await;
    // The local answer stands.
    let local = p.local_question(q["id"].as_str().unwrap()).await;
    assert_eq!(local["status"], "withdrawn");
    assert_eq!(local["linked"]["answered_there"], false);
}

#[tokio::test]
async fn a_lost_answer_or_an_outage_asks_there_once() {
    let p = Pair::new().await;
    let sid = p.local_agent("asker").await;
    // The remote's answer to the first try is lost: the resend under its
    // key answers the same copy.
    p.local.state.linked.lose_next_answers(1);
    let q = p.ask(&sid, Some(&p.aid), "Lost answer?").await;
    p.sleeper.wait_for(BACKOFF_FIRST, 1).await;
    p.sleeper.fire(BACKOFF_FIRST);
    let qid = q["id"].as_str().unwrap().to_string();
    until("the copy is acknowledged", || async {
        (p.local_question(&qid).await["linked"]["sent"] == true).then_some(())
    })
    .await;
    p.backstop().await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    let qs = p.hosted_questions().await;
    assert_eq!(qs.len(), 1, "{qs:?}");
    // An outage: asked while the remote is down, pending, then not sent at
    // the cap; Retry asks it there once.
    p.local.state.linked.set_offline(true);
    p.backstop().await;
    let q = p.ask(&sid, Some(&p.aid), "During an outage?").await;
    let qid = q["id"].as_str().unwrap().to_string();
    let mut d = BACKOFF_FIRST;
    p.sleeper.wait_for(d, 1).await;
    until("the copy is pending", || async {
        (p.local_question(&qid).await["linked"]["state"] == "pending").then_some(())
    })
    .await;
    loop {
        p.sleeper.wait_for(d, 1).await;
        if d == BACKOFF_CAP {
            break;
        }
        p.sleeper.fire(d);
        d = (d * 2).min(BACKOFF_CAP);
    }
    until("the copy is not sent", || async {
        (p.local_question(&qid).await["linked"]["state"] == "failed").then_some(())
    })
    .await;
    // The link row counts it, so its Retry shows though no comment failed.
    let (_, links) = p
        .local_req(
            reqwest::Method::GET,
            &format!("/api/artifacts/{}/remotes", p.aid),
            json!(null),
            None,
        )
        .await;
    assert_eq!(
        (
            &links["links"][0]["failed"],
            &links["links"][0]["failed_questions"]
        ),
        (&json!(0), &json!(1)),
        "{links}"
    );
    p.local.state.linked.set_offline(false);
    let (s, v) = post(
        &p.local,
        &format!("/api/artifacts/{}/remotes/{}/retry", p.aid, p.remote),
        json!({}),
    )
    .await;
    assert_eq!((s, &v["retried"]), (200, &json!(1)), "{v}");
    p.hosted_question("During an outage?").await;
    until("the copy is acknowledged", || async {
        (p.local_question(&qid).await["linked"]["sent"] == true).then_some(())
    })
    .await;
    p.backstop().await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    assert_eq!(p.hosted_questions().await.len(), 2);
}

#[tokio::test]
async fn an_unlinked_artifactless_or_download_only_ask_stays_local() {
    let p = Pair::new().await;
    let sid = p.local_agent("asker").await;
    // About no artifact, and about an artifact with no link.
    let none = p.ask(&sid, None, "About nothing?").await;
    let other = p.local.publish("Unlinked", &[("index.html", PAGE)]).await;
    let other = other["artifact"]["id"].as_str().unwrap().to_string();
    let unlinked = p.ask(&sid, Some(&other), "About another?").await;
    // Download only holds a question back, as it holds a new thread.
    let (s, v) = p
        .local_req(
            reqwest::Method::PATCH,
            &format!("/api/artifacts/{}/remotes/{}", p.aid, p.remote),
            json!({"download_only": true}),
            None,
        )
        .await;
    assert_eq!((s, &v["download_only"]), (200, &json!(true)), "{v}");
    let held = p.ask(&sid, Some(&p.aid), "Download only?").await;
    p.backstop().await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    // Made two-way again: the question asked meanwhile stays local, a new
    // one goes up.
    let (s, v) = p
        .local_req(
            reqwest::Method::PATCH,
            &format!("/api/artifacts/{}/remotes/{}", p.aid, p.remote),
            json!({"download_only": false}),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    p.ask(&sid, Some(&p.aid), "Two-way again?").await;
    p.hosted_question("Two-way again?").await;
    p.backstop().await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    let qs = p.hosted_questions().await;
    assert_eq!(qs.len(), 1, "{qs:?}");
    for q in [&none, &unlinked, &held] {
        let l = p.local_question(q["id"].as_str().unwrap()).await;
        assert_eq!(l["status"], "open");
        assert_eq!(l["linked"], Value::Null, "{l}");
    }
}

#[tokio::test]
async fn a_keyed_question_is_asked_once_and_only_its_tokens_sessions_reach_it() {
    let p = Pair::new().await;
    let ts = &p.public.ts;
    let agent = p.local.store.remote_token(&p.remote).unwrap().unwrap();
    let req = |bearer: &str, m: reqwest::Method, path: &str, sess: Option<&(String, String)>| {
        let mut r = ts
            .client
            .request(m, format!("{}{path}", ts.base))
            .header(HOST, format!("localhost:{}", ts.addr.port()))
            .bearer_auth(bearer);
        if let Some((sid, secret)) = sess {
            r = r
                .header("x-clax-session", sid)
                .header(clax_core::remote::SESSION_SECRET_HEADER, secret);
        }
        r
    };
    let session = |bearer: String| async move {
        let (s, v) = json_of(
            req(&bearer, reqwest::Method::POST, "/api/sessions", None)
                .json(&json!({"harness": "claude", "cwd": ""}))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(s, 201, "{v}");
        (
            v["session"]["id"].as_str().unwrap().to_string(),
            v["session_secret"].as_str().unwrap_or_default().to_string(),
        )
    };
    let ask = |bearer: &str, sess: &(String, String), key: Option<&str>| {
        let mut r = req(
            bearer,
            reqwest::Method::POST,
            &format!("/api/sessions/{}/questions", sess.0),
            Some(sess),
        )
        .json(&ask_body(Some(&p.raid), "Keyed?"));
        if let Some(k) = key {
            r = r.header("idempotency-key", k);
        }
        async move { json_of(r.send().await.unwrap()).await }
    };
    let (a, b) = (
        session(agent.expose().to_string()).await,
        session(agent.expose().to_string()).await,
    );
    // Sent twice, then from another of the token's sessions: one question.
    let (s1, v1) = ask(agent.expose(), &a, Some("clax-linked:question:k1")).await;
    let (s2, v2) = ask(agent.expose(), &a, Some("clax-linked:question:k1")).await;
    let (s3, v3) = ask(agent.expose(), &b, Some("clax-linked:question:k1")).await;
    assert_eq!((s1, s2, s3), (201, 200, 200), "{v1} {v2} {v3}");
    let rqid = v1["question"]["id"].as_str().unwrap().to_string();
    assert_eq!(v2["question"]["id"], rqid.as_str());
    assert_eq!(v3["question"]["id"], rqid.as_str());
    // The token's other session reads and withdraws the keyed question...
    let (s, v) = json_of(
        req(
            agent.expose(),
            reqwest::Method::GET,
            &format!("/api/sessions/{}/questions/{rqid}?wait=0", b.0),
            Some(&b),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!((s, &v["question"]["status"]), (200, &json!("open")), "{v}");
    // ... but not one its first session asked unkeyed.
    let (_, v) = ask(agent.expose(), &a, None).await;
    let unkeyed = v["question"]["id"].as_str().unwrap().to_string();
    let (s, v) = json_of(
        req(
            agent.expose(),
            reqwest::Method::GET,
            &format!("/api/sessions/{}/questions/{unkeyed}?wait=0", b.0),
            Some(&b),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(s, 404, "{v}");
    // The daemon token's key is ignored, and its session reaches neither.
    let owner = session(ts.token.clone()).await;
    let (s, v) = ask(&ts.token, &owner, Some("clax-linked:question:k1")).await;
    assert_eq!(s, 201, "{v}");
    assert_ne!(v["question"]["id"], rqid.as_str(), "{v}");
    for path in [
        format!("/api/sessions/{}/questions/{rqid}?wait=0", owner.0),
        format!("/api/sessions/{}/questions/{unkeyed}?wait=0", owner.0),
    ] {
        let (s, v) = json_of(
            req(&ts.token, reqwest::Method::GET, &path, Some(&owner))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(s, 404, "{path}: {v}");
    }
    let (s, v) = json_of(
        req(
            &ts.token,
            reqwest::Method::POST,
            &format!("/api/sessions/{}/questions/{rqid}/withdraw", owner.0),
            Some(&owner),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = json_of(
        req(
            agent.expose(),
            reqwest::Method::POST,
            &format!("/api/sessions/{}/questions/{rqid}/withdraw", b.0),
            Some(&b),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        (s, &v["question"]["status"]),
        (200, &json!("withdrawn")),
        "{v}"
    );
    // A key that is not one is refused.
    let (s, v) = ask(agent.expose(), &a, Some("bad key")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_args")),
        "{v}"
    );
}

#[tokio::test]
async fn a_stopped_sync_ends_its_tasks_and_its_supervisor_starts_none() {
    let p = Pair::new().await;
    let linked = &p.local.state.linked;
    assert!(linked.running(&p.remote));
    let ended = clax_server::linked::stop(&p.local.state);
    assert_eq!(ended.len(), 1, "the remote's task");
    for t in ended {
        assert!(t.await.is_err_and(|e| e.is_cancelled()));
    }
    assert!(!linked.running(&p.remote));
    // The supervisor (a core stops it too; this server does not) wakes
    // with the link still connected: it starts no task once stopped.
    linked.artifacts.lock().unwrap().clear();
    linked.links_changed();
    until("the supervisor reads the links again", || async {
        (!linked.artifacts.lock().unwrap().is_empty()).then_some(())
    })
    .await;
    assert!(
        !linked.running(&p.remote),
        "a stopped sync started a task for the remote"
    );
}
