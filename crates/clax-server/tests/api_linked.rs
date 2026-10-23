//! Linked threads (`clax_server::linked`): a local daemon linked to a
//! hosted test daemon keeps an artifact's comment threads in step with its
//! public twin's, both ways.

use clax_server::linked::{BACKOFF_CAP, BACKOFF_FIRST, BACKSTOP, Linked};
use clax_testkit::ManualSleeper;
use clax_testkit::TestServer;
use clax_testkit::remotes::{Public, connect, connect_app, json_of, local_req, post};
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

    /// Returns once the sync waits out its backstop, however long its
    /// cycle takes: a cycle reading answers of megabytes may.
    async fn settled(&self) {
        until("the sync's cycle ends", || async {
            (self.sleeper.waiting(BACKSTOP) == 1).then_some(())
        })
        .await;
    }

    /// [`Pair::backstop`] after [`Pair::settled`].
    async fn backstop_settled(&self) {
        self.settled().await;
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
async fn a_resent_keyed_write_does_the_sends_its_first_write_did_not_reach() {
    use clax_store::threads::{AUTHOR_VIEWER, NewComment, NewThread};
    let p = Pair::new().await;
    let token = p.local.store.remote_token(&p.remote).unwrap().unwrap();
    let grant = p
        .local
        .store
        .find_remote(&p.remote)
        .unwrap()
        .unwrap()
        .grant_id
        .unwrap();
    let ts = &p.public.ts;
    let st = &ts.store;
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
    // The agent watches the artifact, so what is sent there reaches it.
    let (s, v) = json_of(
        req(
            reqwest::Method::PUT,
            &format!("/api/sessions/{sid}/watches/{}", p.raid),
        )
        .header("x-clax-session", &sid)
        .header(clax_core::remote::SESSION_SECRET_HEADER, &secret)
        .json(&json!({}))
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert!(s < 300, "{s} {v}");
    // Each first write ended after its transaction, before its sends.
    let aid = clax_core::ArtifactId::parse(&p.raid).unwrap();
    let made = st
        .create_thread_keyed_async(
            &clax_core::audit::AuditCtx::DAEMON,
            &aid,
            NewThread {
                version_n: 1,
                anchor: serde_json::from_value(whole()).unwrap(),
                author_name: "Ada".into(),
                author_public_id: None,
                body: "@agent look".into(),
                clip: None,
                via_page: false,
            },
            &[],
            Some((&grant, "clax-linked:thread:k1")),
        )
        .await
        .unwrap();
    let clax_store::linked::Keyed::Made(t) = made else {
        panic!("the thread is made");
    };
    let other = p.hosted_thread("plain", whole(), None).await;
    let otid = other["id"].as_str().unwrap();
    let made = st
        .add_comment_keyed(
            &clax_core::audit::AuditCtx::DAEMON,
            otid,
            NewComment {
                author_kind: AUTHOR_VIEWER,
                author_name: "Ada".into(),
                author_public_id: None,
                via_session_id: None,
                body: "@agent this too".into(),
                via_page: false,
            },
            Some((&grant, "clax-linked:comment:k2")),
        )
        .unwrap();
    let clax_store::linked::Keyed::Made(c) = made else {
        panic!("the comment is made");
    };
    let sent = |tid: &str| st.get_thread(tid).unwrap().unwrap().sent_to_agent;
    assert!(!sent(&t.id) && !sent(otid));
    // Their resends answer what was made, and send it.
    let form = reqwest::multipart::Form::new()
        .text("anchor", whole().to_string())
        .text("body", "@agent look")
        .text("version", "1");
    let (s, v) = json_of(
        req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads", p.raid),
        )
        .header("x-clax-session", &sid)
        .header(clax_core::remote::SESSION_SECRET_HEADER, &secret)
        .header("idempotency-key", "clax-linked:thread:k1")
        .multipart(form)
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        (s, v["thread"]["id"].as_str()),
        (201, Some(t.id.as_str())),
        "{v}"
    );
    assert!(sent(&t.id));
    let (s, v) = json_of(
        req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{otid}/comments", p.raid),
        )
        .header("x-clax-session", &sid)
        .header(clax_core::remote::SESSION_SECRET_HEADER, &secret)
        .header("idempotency-key", "clax-linked:comment:k2")
        .json(&json!({"body": "@agent this too"}))
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        (s, v["comment"]["id"].as_str()),
        (201, Some(c.id.as_str())),
        "{v}"
    );
    assert!(sent(otid));
    assert!(st.comment_has_feedback(&c.id).unwrap());
    assert_eq!(
        bodies(&p.hosted_thread_by(otid).await),
        ["plain", "@agent this too"]
    );
    // An agent reply whose first write ended before it acknowledged what
    // its session was sent is acknowledged by its resend.
    let plain = other["comments"][0]["id"].as_str().unwrap().to_string();
    let unacked = || {
        st.feedback_rows(otid)
            .unwrap()
            .into_iter()
            .filter(|f| f.comment_id == plain && f.target_session_id.as_deref() == Some(&sid))
            .map(|f| f.acknowledged_at.is_none())
            .collect::<Vec<_>>()
    };
    assert_eq!(unacked(), [true]);
    let made = st
        .add_agent_reply_keyed(
            &clax_core::audit::AuditCtx::DAEMON,
            otid,
            NewComment {
                author_kind: clax_store::threads::AUTHOR_AGENT,
                author_name: "claude".into(),
                author_public_id: None,
                via_session_id: Some(sid.clone()),
                body: "on it".into(),
                via_page: false,
            },
            None,
            Some((&grant, "clax-linked:comment:k3")),
        )
        .unwrap()
        .0;
    let clax_store::linked::Keyed::Made(r) = made else {
        panic!("the reply is made");
    };
    let (s, v) = json_of(
        req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/threads/{otid}/comments", p.raid),
        )
        .header("x-clax-session", &sid)
        .header(clax_core::remote::SESSION_SECRET_HEADER, &secret)
        .header("idempotency-key", "clax-linked:comment:k3")
        .json(&json!({"body": "on it", "author_kind": "agent"}))
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        (s, v["comment"]["id"].as_str()),
        (201, Some(r.id.as_str())),
        "{v}"
    );
    assert_eq!(unacked(), [false]);
    assert_eq!(
        bodies(&p.hosted_thread_by(otid).await),
        ["plain", "@agent this too", "on it"]
    );
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

/// The live sessions of the by-hand harness the sync speaks in on the public Clax.
fn sync_sessions(p: &Pair) -> usize {
    p.public
        .ts
        .store
        .list_sessions(true)
        .unwrap()
        .into_iter()
        .filter(|s| s.harness == "clax")
        .count()
}

#[tokio::test]
async fn an_outage_keeps_the_syncs_one_session_there() {
    let p = Pair::new().await;
    assert_eq!(sync_sessions(&p), 1);
    // The cycle meets an outage, and the next one the remote back.
    p.local.state.linked.set_offline(true);
    p.backstop().await;
    p.sleeper.wait_for(BACKOFF_FIRST, 1).await;
    p.local.state.linked.set_offline(false);
    p.sleeper.fire(BACKOFF_FIRST);
    p.sleeper.wait_for(BACKSTOP, 1).await;
    assert_eq!(sync_sessions(&p), 1, "the session there was kept");
    // A session the remote ended is replaced, not left behind.
    let sid = p
        .public
        .ts
        .store
        .list_sessions(true)
        .unwrap()
        .into_iter()
        .find(|s| s.name.as_deref() == Some("clax-sync"))
        .unwrap()
        .id;
    p.public
        .ts
        .store
        .end_session(&clax_core::audit::AuditCtx::DAEMON, &sid)
        .unwrap();
    p.local.state.linked.set_offline(true);
    p.backstop().await;
    p.sleeper.wait_for(BACKOFF_FIRST, 1).await;
    p.local.state.linked.set_offline(false);
    p.sleeper.fire(BACKOFF_FIRST);
    p.sleeper.wait_for(BACKSTOP, 1).await;
    assert_eq!(sync_sessions(&p), 1, "a new session, the ended one gone");
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

#[tokio::test]
async fn a_mirrored_thread_deleted_here_stays_gone_and_the_link_keeps_syncing() {
    let p = Pair::new().await;
    let r1 = p.hosted_thread("first", whole(), None).await;
    p.backstop().await;
    let tid = until("the thread comes down", || async {
        p.local_threads()
            .await
            .first()
            .map(|t| t["id"].as_str().unwrap().to_string())
    })
    .await;
    p.sleeper.wait_for(BACKSTOP, 1).await;
    let (s, v) = p
        .local_req(
            reqwest::Method::DELETE,
            &format!("/api/artifacts/{}/threads/{tid}", p.aid),
            json!(null),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // A comment there on the thread deleted here, and a new thread there.
    let (s, v) = p
        .public
        .browser(
            reqwest::Method::POST,
            &format!(
                "/api/artifacts/{}/threads/{}/comments",
                p.raid,
                r1["id"].as_str().unwrap()
            ),
            json!({"body": "still here?"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    p.hosted_thread("second", whole(), None).await;
    p.backstop().await;
    until("the second thread comes down", || async {
        let ts = p.local_threads().await;
        ts.iter().any(|t| bodies(t) == ["second"]).then_some(())
    })
    .await;
    // The cycle ended well: the sync waits for its backstop, not a backoff.
    p.sleeper.wait_for(BACKSTOP, 1).await;
    assert_eq!(p.local.state.linked.status(&p.remote).last_error, None);
    // The deleted thread did not come back.
    let ts = p.local_threads().await;
    assert_eq!(ts.len(), 1, "{ts:?}");
    assert_eq!(bodies(&ts[0]), ["second"]);
}

#[tokio::test]
async fn threads_after_a_run_of_large_ones_all_come_down() {
    use clax_store::threads::{AUTHOR_VIEWER, NewComment, NewThread};
    let p = Pair::new().await;
    let st = &p.public.ts.store;
    let audit = &clax_core::audit::AuditCtx::DAEMON;
    let aid = clax_core::ArtifactId::parse(&p.raid).unwrap();
    // Three threads of about 1.5 MB in a row, each within the 4 MiB a
    // remote's answer may be but not together, then 60 small ones; then
    // the first small one, read early, is the newest.
    let first = p.hosted_thread("large 0", whole(), None).await;
    let owner = first["comments"][0]["author_public_id"]
        .as_str()
        .map(str::to_string);
    let thread = |body: String| NewThread {
        version_n: 1,
        anchor: serde_json::from_value(whole()).unwrap(),
        author_name: "Ada".into(),
        author_public_id: owner.clone(),
        body,
        clip: None,
        via_page: false,
    };
    let comment = |body: String| NewComment {
        author_kind: AUTHOR_VIEWER,
        author_name: "Ada".into(),
        author_public_id: owner.clone(),
        via_session_id: None,
        body,
        via_page: false,
    };
    let made = |k: clax_store::linked::Keyed<clax_core::model::Thread>| match k {
        clax_store::linked::Keyed::Made(t) => t.id,
        clax_store::linked::Keyed::Before { .. } => panic!("an unkeyed thread is made"),
    };
    let mut large = vec![first["id"].as_str().unwrap().to_string()];
    for i in 1..3 {
        let t = st
            .create_thread_keyed_async(audit, &aid, thread(format!("large {i}")), &[], None)
            .await
            .unwrap();
        large.push(made(t));
    }
    let body = "x".repeat(9_000);
    for tid in &large {
        for _ in 0..170 {
            st.add_comment(audit, tid, comment(body.clone())).unwrap();
        }
    }
    let mut small = Vec::new();
    for i in 0..60 {
        let t = st
            .create_thread_keyed_async(audit, &aid, thread(format!("small {i}")), &[], None)
            .await
            .unwrap();
        small.push(made(t));
    }
    st.add_comment(audit, &small[0], comment("newest".into()))
        .unwrap();
    p.backstop_settled().await;
    p.settled().await;
    let v: Value = p
        .local
        .get_authed(&format!(
            "/api/artifacts/{}/threads?include_resolved=true&limit=200",
            p.aid
        ))
        .await
        .json()
        .await
        .unwrap();
    let ts = v["threads"].as_array().cloned().unwrap_or_default();
    assert_eq!(ts.len(), 63, "{} threads came down", ts.len());
    let newest = ts
        .iter()
        .find(|t| t["comments"][0]["body"] == "small 0")
        .unwrap();
    assert_eq!(bodies(newest), ["small 0", "newest"]);
    for t in &ts {
        let n = t["comments"].as_array().unwrap().len();
        if t["comments"][0]["body"]
            .as_str()
            .is_some_and(|b| b.starts_with("large"))
        {
            assert_eq!(n, 171);
        }
    }
    assert_eq!(p.local.state.linked.status(&p.remote).last_error, None);
}

/// Threads `t 1`..=`t {n}` written straight to the public copy's store as
/// the author of thread `first`, so nothing wakes the sync; their IDs.
async fn more_threads(p: &Pair, first: &Value, n: usize) -> Vec<String> {
    let owner = first["comments"][0]["author_public_id"]
        .as_str()
        .map(str::to_string);
    let aid = clax_core::ArtifactId::parse(&p.raid).unwrap();
    let mut ids = Vec::new();
    for i in 1..=n {
        let made = p
            .public
            .ts
            .store
            .create_thread_keyed_async(
                &clax_core::audit::AuditCtx::DAEMON,
                &aid,
                clax_store::threads::NewThread {
                    version_n: 1,
                    anchor: serde_json::from_value(whole()).unwrap(),
                    author_name: "Ada".into(),
                    author_public_id: owner.clone(),
                    body: format!("t {i}"),
                    clip: None,
                    via_page: false,
                },
                &[],
                None,
            )
            .await
            .unwrap();
        let clax_store::linked::Keyed::Made(t) = made else {
            panic!("an unkeyed thread is made");
        };
        ids.push(t.id);
    }
    ids
}

/// The first comments of the local threads, sorted.
async fn local_firsts(p: &Pair) -> Vec<String> {
    let mut b: Vec<String> = p
        .local_threads()
        .await
        .iter()
        .map(|t| t["comments"][0]["body"].as_str().unwrap().to_string())
        .collect();
    b.sort();
    b
}

#[tokio::test]
async fn a_read_past_its_pages_goes_on_in_the_next_cycles() {
    let p = Pair::new().await;
    // Pages of two threads, one page a cycle.
    p.local.state.linked.set_read_limits(2, 1);
    let first = p.hosted_thread("t 0", whole(), None).await;
    until("the first thread comes down", || async {
        (p.local_threads().await.len() == 1).then_some(())
    })
    .await;
    p.settled().await;
    more_threads(&p, &first, 5).await;
    // One backstop: the read goes on, cycle after cycle, to its end.
    p.backstop_settled().await;
    p.settled().await;
    assert_eq!(
        local_firsts(&p).await,
        ["t 0", "t 1", "t 2", "t 3", "t 4", "t 5"]
    );
    assert_eq!(p.local.state.linked.status(&p.remote).last_error, None);
}

#[tokio::test]
async fn a_read_whose_page_cursor_is_deleted_there_starts_over() {
    let p = Pair::new().await;
    // Pages of two threads, one page a cycle; the read waits after its
    // first.
    p.local.state.linked.set_read_limits(2, 1);
    p.local.state.linked.hold_resumed(true);
    let first = p.hosted_thread("t 0", whole(), None).await;
    until("the first thread comes down", || async {
        (p.local_threads().await.len() == 1).then_some(())
    })
    .await;
    p.settled().await;
    let ids = more_threads(&p, &first, 5).await;
    p.backstop_settled().await;
    p.settled().await;
    assert_eq!(local_firsts(&p).await, ["t 0", "t 1"]);
    // The read stopped after `t 1`, which is then deleted there.
    p.public
        .ts
        .store
        .delete_thread(&clax_core::audit::AuditCtx::DAEMON, &ids[0])
        .unwrap();
    p.local.state.linked.hold_resumed(false);
    p.backstop_settled().await;
    // The cycle ends at the backstop, not in a backoff.
    until("the cycle ends", || async {
        (p.sleeper.waiting(BACKSTOP) + p.sleeper.waiting(BACKOFF_FIRST) == 1).then_some(())
    })
    .await;
    assert_eq!(p.local.state.linked.status(&p.remote).last_error, None);
    assert_eq!(
        local_firsts(&p).await,
        ["t 0", "t 1", "t 2", "t 3", "t 4", "t 5"]
    );
    assert_eq!(p.local.state.linked.status(&p.remote).last_error, None);
}

#[tokio::test]
async fn a_thread_past_the_answer_cap_stops_its_links_read_not_the_remotes() {
    use clax_store::threads::{AUTHOR_VIEWER, NewComment};
    let p = Pair::new().await;
    p.hosted_thread("small", whole(), None).await;
    p.backstop_settled().await;
    until("the small thread comes down", || async {
        (p.local_threads().await.len() == 1).then_some(())
    })
    .await;
    p.settled().await;
    // One thread of about 4.5 MB: even a page of it alone passes the 4 MiB
    // a remote's answer may be.
    // Comments by the thread's own author, whom the sync's token sees.
    let big = p.hosted_thread("big", whole(), None).await;
    let owner = big["comments"][0]["author_public_id"]
        .as_str()
        .map(str::to_string);
    let big = big["id"].as_str().unwrap().to_string();
    let body = "x".repeat(9_000);
    for _ in 0..500 {
        p.public
            .ts
            .store
            .add_comment(
                &clax_core::audit::AuditCtx::DAEMON,
                &big,
                NewComment {
                    author_kind: AUTHOR_VIEWER,
                    author_name: "Ada".into(),
                    author_public_id: owner.clone(),
                    via_session_id: None,
                    body: body.clone(),
                    via_page: false,
                },
            )
            .unwrap();
    }
    let links = || async {
        let (s, v) = p
            .local_req(
                reqwest::Method::GET,
                &format!("/api/artifacts/{}/remotes", p.aid),
                json!(null),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["links"][0].clone()
    };
    p.backstop_settled().await;
    // The link says what it could not read; the remote stands reachable.
    let l = until("the link says what it could not read", || async {
        let l = links().await;
        l["read_error"].is_string().then_some(l)
    })
    .await;
    assert!(
        l["read_error"]
            .as_str()
            .is_some_and(|e| e.contains("4 MiB")),
        "{l}"
    );
    assert_eq!(l["sync"]["last_error"], Value::Null, "{l}");
    assert_eq!(l["sync"]["unreachable_since"], Value::Null, "{l}");
    assert_eq!(p.local.state.linked.status(&p.remote).last_error, None);
    // Without the thread there, the link reads to the end again.
    p.public
        .ts
        .store
        .delete_thread(&clax_core::audit::AuditCtx::DAEMON, &big)
        .unwrap();
    p.backstop_settled().await;
    until("the link reads to the end again", || async {
        links().await["read_error"].is_null().then_some(())
    })
    .await;
}

#[tokio::test]
async fn a_page_of_threads_past_the_answer_cap_is_read_in_smaller_pages() {
    use clax_store::threads::{AUTHOR_VIEWER, NewComment};
    let p = Pair::new().await;
    // Five threads of about 900 KB each: one page of them passes the
    // 4 MiB a remote's answer may be.
    let body = "x".repeat(9_000);
    for i in 0..5 {
        let t = p.hosted_thread(&format!("big {i}"), whole(), None).await;
        let tid = t["id"].as_str().unwrap();
        let owner = t["comments"][0]["author_public_id"]
            .as_str()
            .map(str::to_string);
        for _ in 0..100 {
            p.public
                .ts
                .store
                .add_comment(
                    &clax_core::audit::AuditCtx::DAEMON,
                    tid,
                    NewComment {
                        author_kind: AUTHOR_VIEWER,
                        author_name: "Ada".into(),
                        author_public_id: owner.clone(),
                        via_session_id: None,
                        body: body.clone(),
                        via_page: false,
                    },
                )
                .unwrap();
        }
    }
    p.backstop_settled().await;
    until("every thread comes down whole", || async {
        let ts = p.local_threads().await;
        (ts.len() == 5
            && ts
                .iter()
                .all(|t| t["comments"].as_array().is_some_and(|c| c.len() == 101)))
        .then_some(())
    })
    .await;
    p.settled().await;
    assert_eq!(p.local.state.linked.status(&p.remote).last_error, None);
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
    // The answer closes the question here, which ends the wait, and then the
    // link records that it came from there.
    until("the link records the answer as from there", || async {
        let l = p.local_question(&qid).await;
        (l["linked"]["answered_there"] == true).then_some(())
    })
    .await;
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

/// A thread on the whole of version 1 of `raid` there, from the browser
/// whose session cookie is `cookie`.
async fn browser_thread(public: &Public, raid: &str, cookie: &str, body: &str) {
    browser_thread_on(public, raid, cookie, whole(), body).await
}

/// A thread at `anchor` on version 1 of `raid` there, from the browser
/// whose session cookie is `cookie`.
async fn browser_thread_on(public: &Public, raid: &str, cookie: &str, anchor: Value, body: &str) {
    let ts = &public.ts;
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{raid}/threads", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header(COOKIE, format!("clax_session={cookie}"))
        .header(ORIGIN, &public.origin)
        .multipart(
            reqwest::multipart::Form::new()
                .text("anchor", anchor.to_string())
                .text("body", body.to_string())
                .text("version", "1"),
        )
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!(s, 201, "{v}");
}

/// One full read of the sync's, ended.
async fn sync_once(sleeper: &ManualSleeper) {
    sleeper.wait_for(BACKSTOP, 1).await;
    sleeper.fire(BACKSTOP);
    sleeper.wait_for(BACKSTOP, 1).await;
}

/// `aid`'s threads here, as the owner reads them (`sid` `None`) or as
/// session `sid`'s agent does, each with its first comment's body.
async fn threads_here(local: &TestServer, aid: &str, sid: Option<&str>) -> Vec<Value> {
    let (s, v) = local_req(
        local,
        reqwest::Method::GET,
        &format!("/api/artifacts/{aid}/threads?include_resolved=true"),
        json!(null),
        sid,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v["threads"].as_array().cloned().unwrap_or_default()
}

/// The thread among `ts` whose first comment's linked author is `name`.
fn by<'a>(ts: &'a [Value], name: &str) -> &'a Value {
    ts.iter()
        .find(|t| t["comments"][0]["linked"]["author_name"] == name)
        .unwrap_or_else(|| panic!("no thread by {name}: {}", json!(ts)))
}

#[tokio::test]
async fn a_members_device_brings_down_a_strangers_comment_withheld_from_her_agents_until_approved()
{
    let mut public = Public::new().await;
    // Mia and Bob are people there; neither is its owner.
    let owner = public.cookie.clone();
    public.become_person("Mia").await;
    let mia = public.cookie.clone();
    public.cookie = owner;
    let bob_id = public.become_person("Bob").await;
    let bob = public.cookie.clone();
    public.cookie = mia.clone();
    // Mia's phone publishes an app, turns Comments on, shares it with Bob.
    let sleeper = Arc::new(ManualSleeper::default());
    let sl = sleeper.clone();
    let local = TestServer::spawn_on("127.0.0.1".parse().unwrap(), move |s| {
        s.linked = Arc::new(Linked::new(sl));
    })
    .await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = local.publish("Tips", &[("index.html", PAGE)]).await;
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let (s, v) = post(
        &local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "app", "version": 1}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let raid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let copy = format!("/api/remotes/{rid}/artifacts/{raid}");
    for (path, body) in [
        (format!("{copy}/sharing/comments"), json!({"on": true})),
        (
            format!("{copy}/sharing/shares"),
            json!({"person": bob_id, "access": "commenter"}),
        ),
    ] {
        let (s, v) = local_req(&local, reqwest::Method::PUT, &path, body, None).await;
        assert_eq!(s, 200, "{path} {v}");
    }
    // Mia's agent here watches the app.
    let sid = local.register_session("claude", "h-mia").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = local_req(
        &local,
        reqwest::Method::PUT,
        &format!("/api/sessions/{sid}/watches/{aid}"),
        json!({}),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // Bob comments in his browser, and Mia in hers.
    browser_thread(&public, &raid, &bob, "@agent Bob says the button is broken").await;
    browser_thread(&public, &raid, &mia, "Mia's own note").await;
    // Mia's core brings both down: she reads every word.
    sync_once(&sleeper).await;
    let here = threads_here(&local, &aid, None).await;
    assert_eq!(here.len(), 2, "{}", json!(here));
    assert_eq!(
        first_body(by(&here, "Bob")),
        "@agent Bob says the button is broken"
    );
    assert_eq!(first_body(by(&here, "Mia")), "Mia's own note");
    assert_eq!(by(&here, "Bob")["anchor"]["kind"], "artifact");
    // Her agent here is told Bob's comment is withheld, not what it says,
    // and reads her own words and Bob's withheld: she has not approved him
    // here.
    let (s, fb) = local_req(
        &local,
        reqwest::Method::GET,
        &format!("/api/sessions/{sid}/feedback?tier=wait&wait=0"),
        json!(null),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 200, "{fb}");
    let w = &fb["withheld"][0];
    assert!(
        w["author"].as_str().unwrap().starts_with("Bob (on "),
        "{fb}"
    );
    assert!(!fb.to_string().contains("button is broken"), "{fb}");
    let theirs = threads_here(&local, &aid, Some(&sid)).await;
    assert_eq!(first_body(by(&theirs, "Mia")), "Mia's own note");
    let withheld = by(&theirs, "Bob");
    assert_eq!(first_body(withheld), Value::Null, "{withheld}");
    assert_eq!(withheld["comments"][0]["withheld"], true, "{withheld}");
    let author = &withheld["withheld"]["authors"][0];
    let key = author["public_id"].as_str().unwrap().to_string();
    assert!(key.starts_with("remote:"), "{author}");
    assert!(
        author["name"].as_str().unwrap().starts_with("Bob (on "),
        "{author}"
    );
    // Her agent asks her; she approves Bob here from the request's card,
    // in her browser: her agent reads him.
    let (s, v) = local_req(
        &local,
        reqwest::Method::POST,
        &format!("/api/sessions/{sid}/approvals"),
        json!({"thread_id": withheld["id"]}),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["request"]["author"]["public_id"], key.as_str(), "{v}");
    assert!(
        v["request"]["author"]["name"]
            .as_str()
            .unwrap()
            .starts_with("Bob (on "),
        "{v}"
    );
    let rq = v["approval_id"].as_str().unwrap().to_string();
    let (s, v) = json_of(
        local
            .client
            .post(format!("{}/api/approvals/requests/{rq}/decide", local.base))
            .header(COOKIE, local.owner_cookie())
            .json(&json!({"scope": "everywhere"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["approval"]["author"]["public_id"], key.as_str(), "{v}");
    let theirs = threads_here(&local, &aid, Some(&sid)).await;
    assert_eq!(
        first_body(by(&theirs, "Bob")),
        "@agent Bob says the button is broken"
    );
}

#[tokio::test]
async fn a_strangers_mirrored_anchor_is_withheld_from_her_agents_until_approved() {
    let mut public = Public::new().await;
    // Mia and Bob are people there; neither is its owner.
    let owner = public.cookie.clone();
    public.become_person("Mia").await;
    let mia = public.cookie.clone();
    public.cookie = owner;
    let bob_id = public.become_person("Bob").await;
    let bob = public.cookie.clone();
    public.cookie = mia.clone();
    // Mia's phone publishes an app, turns Comments on, shares it with Bob.
    let sleeper = Arc::new(ManualSleeper::default());
    let sl = sleeper.clone();
    let local = TestServer::spawn_on("127.0.0.1".parse().unwrap(), move |s| {
        s.linked = Arc::new(Linked::new(sl));
    })
    .await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = local.publish("Tips", &[("index.html", PAGE)]).await;
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let (s, v) = post(
        &local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "app", "version": 1}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let raid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let copy = format!("/api/remotes/{rid}/artifacts/{raid}");
    for (path, body) in [
        (format!("{copy}/sharing/comments"), json!({"on": true})),
        (
            format!("{copy}/sharing/shares"),
            json!({"person": bob_id, "access": "commenter"}),
        ),
    ] {
        let (s, v) = local_req(&local, reqwest::Method::PUT, &path, body, None).await;
        assert_eq!(s, 200, "{path} {v}");
    }
    // Mia's agent here watches the app.
    let sid = local.register_session("claude", "h-mia").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = local_req(
        &local,
        reqwest::Method::PUT,
        &format!("/api/sessions/{sid}/watches/{aid}"),
        json!({}),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // Bob anchors a comment on words his browser quotes, and Mia on hers.
    let anchor = |quote: &str| {
        json!({"kind": "range", "selector": "#x", "quote": quote,
               "prefix": format!("{quote} before"), "suffix": format!("{quote} after"),
               "label": quote, "name": format!("{quote} by name"),
               "file": "index.html"})
    };
    browser_thread_on(
        &public,
        &raid,
        &bob,
        anchor("Bob quoted this"),
        "@agent see here",
    )
    .await;
    browser_thread_on(
        &public,
        &raid,
        &mia,
        anchor("Mia quoted this"),
        "Mia's note",
    )
    .await;
    sync_once(&sleeper).await;
    // Mia reads both anchors.
    let here = threads_here(&local, &aid, None).await;
    assert_eq!(here.len(), 2, "{}", json!(here));
    assert_eq!(by(&here, "Bob")["anchor"]["quote"], "Bob quoted this");
    assert_eq!(by(&here, "Mia")["anchor"]["quote"], "Mia quoted this");
    // Her agent reads her own anchor, and none of Bob's: not in the list,
    // not since a time, not in the thread, not in its feedback.
    let theirs = threads_here(&local, &aid, Some(&sid)).await;
    let hers = by(&theirs, "Mia");
    assert_eq!(hers["anchor"]["quote"], "Mia quoted this", "{hers}");
    assert_ne!(hers["anchor_withheld"], true, "{hers}");
    let tid = by(&theirs, "Bob")["id"].as_str().unwrap().to_string();
    let (s, since) = local_req(
        &local,
        reqwest::Method::GET,
        &format!("/api/artifacts/{aid}/threads?include_resolved=true&since=1970-01-01T00:00:00Z"),
        json!(null),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 200, "{since}");
    assert_eq!(
        since["threads"].as_array().map(Vec::len),
        Some(2),
        "{since}"
    );
    let (s, single) = local_req(
        &local,
        reqwest::Method::GET,
        &format!("/api/artifacts/{aid}/threads/{tid}"),
        json!(null),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 200, "{single}");
    let (s, fb) = local_req(
        &local,
        reqwest::Method::GET,
        &format!("/api/sessions/{sid}/feedback?tier=wait&wait=0"),
        json!(null),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 200, "{fb}");
    let withheld = by(&theirs, "Bob");
    assert_eq!(withheld["anchor_withheld"], true, "{withheld}");
    assert_eq!(withheld["anchor"]["quote"], Value::Null, "{withheld}");
    for read in [json!(theirs), since, single, fb] {
        assert!(!read.to_string().contains("Bob quoted"), "{read}");
    }
    // She approves Bob here: her agent reads his anchor.
    let (s, v) = local_req(
        &local,
        reqwest::Method::POST,
        &format!("/api/sessions/{sid}/approvals"),
        json!({"thread_id": tid}),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let key = v["request"]["author"]["public_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(key.starts_with("remote:"), "{v}");
    let rq = v["approval_id"].as_str().unwrap().to_string();
    let (s, v) = json_of(
        local
            .client
            .post(format!("{}/api/approvals/requests/{rq}/decide", local.base))
            .header(COOKIE, local.owner_cookie())
            .json(&json!({"scope": "everywhere"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let theirs = threads_here(&local, &aid, Some(&sid)).await;
    let released = by(&theirs, "Bob");
    assert_eq!(released["anchor"]["quote"], "Bob quoted this", "{released}");
    assert_eq!(released["anchor"]["prefix"], "Bob quoted this before");
    assert_eq!(released["anchor"]["suffix"], "Bob quoted this after");
    assert_eq!(first_body(released), "@agent see here");
}

/// The body of local thread `t`'s first comment.
fn first_body(t: &Value) -> Value {
    t["comments"][0]["body"].clone()
}

#[tokio::test]
async fn a_daemons_agent_reads_a_comment_its_person_approved_there_unwithheld() {
    // A daemon signed in with no kind (an agent token) as Ada, the owner
    // there, links its artifact; Bob is a person there whom Ada approved.
    let mut p = Pair::new().await;
    let ada = p.public.cookie.clone();
    let bob_id = p.public.become_person("Bob").await;
    let bob = p.public.cookie.clone();
    p.public.cookie = ada;
    for (method, path, body) in [
        (
            reqwest::Method::PUT,
            format!("/api/hosted/artifacts/{}/shares", p.raid),
            json!({"person": bob_id, "access": "commenter"}),
        ),
        (
            reqwest::Method::POST,
            "/api/approvals".to_string(),
            json!({"author": bob_id, "scope": "everywhere"}),
        ),
    ] {
        let (s, v) = p.public.browser(method, &path, body).await;
        assert!((200..300).contains(&s), "{path} {v}");
    }
    let sid = p.local_agent("fixer").await;
    browser_thread(&p.public, &p.raid, &bob, "@agent Bob says hi").await;
    sync_once(&p.sleeper).await;
    // The remote let Bob's comment through to Ada's token; here her agent
    // reads it as the remote gave it, with no second approval.
    let theirs = threads_here(&p.local, &p.aid, Some(&sid)).await;
    assert_eq!(first_body(by(&theirs, "Bob")), "@agent Bob says hi");
    let rid = p.remote.clone();
    let r = p.local.store.find_remote(&rid).unwrap().unwrap();
    assert_eq!(
        r.client_kind.as_deref(),
        Some(clax_store::remotes::CLIENT_KIND_AGENT)
    );
}

#[tokio::test]
async fn a_device_keys_an_agent_comment_naming_no_person_apart_from_the_owners_agents() {
    let mut public = Public::new().await;
    // Mia is a person there, not its owner; her phone publishes an app
    // with Comments on.
    let owner = public.cookie.clone();
    public.become_person("Mia").await;
    let mia = public.cookie.clone();
    public.cookie = owner;
    let sleeper = Arc::new(ManualSleeper::default());
    let sl = sleeper.clone();
    let local = TestServer::spawn_on("127.0.0.1".parse().unwrap(), move |s| {
        s.linked = Arc::new(Linked::new(sl));
    })
    .await;
    public.cookie = mia.clone();
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = local.publish("Tips", &[("index.html", PAGE)]).await;
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let (s, v) = post(
        &local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "app", "version": 1}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let raid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = local_req(
        &local,
        reqwest::Method::PUT,
        &format!("/api/remotes/{rid}/artifacts/{raid}/sharing/comments"),
        json!({"on": true}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    browser_thread(&public, &raid, &mia, "Mia's note").await;
    // An agent there whose session has no name replies: people's views of
    // its comment carry no `agent`, so name no person.
    let ts = &public.ts;
    let req = |m: reqwest::Method, path: &str| {
        ts.client
            .request(m, format!("{}{path}", ts.base))
            .header(HOST, format!("localhost:{}", ts.addr.port()))
            .bearer_auth(&ts.token)
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
    let sid = v["session"]["id"].as_str().unwrap().to_string();
    let rtid = ts
        .store
        .list_threads(
            &clax_core::ArtifactId::parse(&raid).unwrap(),
            true,
            None,
            10,
        )
        .unwrap()
        .0[0]
        .id
        .clone();
    ts.store
        .add_comment(
            &clax_core::audit::AuditCtx::DAEMON,
            &rtid,
            clax_store::threads::NewComment {
                author_kind: clax_store::threads::AUTHOR_AGENT,
                author_name: "claude".into(),
                author_public_id: None,
                via_session_id: Some(sid.clone()),
                body: "an unnamed agent's reply".into(),
                via_page: false,
            },
        )
        .unwrap();
    rusqlite::Connection::open(ts.state.home.db_path())
        .unwrap()
        .execute("UPDATE sessions SET name = NULL WHERE id = ?1", [&sid])
        .unwrap();
    sync_once(&sleeper).await;
    let here = threads_here(&local, &aid, None).await;
    let t = by(&here, "Mia");
    let tid = t["id"].as_str().unwrap();
    let comments = local.store.get_thread(tid).unwrap().unwrap().comments;
    assert_eq!(comments.len(), 2, "{t}");
    assert_eq!(
        comments[1].remote_author.as_deref(),
        Some(format!("remote:{}#*agent", public.origin).as_str()),
        "{t}"
    );
}

#[tokio::test]
async fn a_remote_signed_in_before_kinds_were_kept_is_asked_its_kind() {
    // A daemon's remote and a device's, each with its kind forgotten, as
    // a remote signed in before migration 49 has none.
    let public = Public::new().await;
    for (app, kind) in [
        (false, clax_store::remotes::CLIENT_KIND_AGENT),
        (true, clax_store::remotes::CLIENT_KIND_APP),
    ] {
        let sleeper = Arc::new(ManualSleeper::default());
        let sl = sleeper.clone();
        let local = TestServer::spawn_on("127.0.0.1".parse().unwrap(), move |s| {
            s.linked = Arc::new(Linked::new(sl));
        })
        .await;
        let r = if app {
            connect_app(&local, &public).await
        } else {
            connect(&local, &public).await
        };
        let rid = r["id"].as_str().unwrap().to_string();
        rusqlite::Connection::open(local.state.home.db_path())
            .unwrap()
            .execute(
                "UPDATE remotes SET client_kind = NULL WHERE id = ?1",
                [&rid],
            )
            .unwrap();
        let v = local.publish("Linked", &[("index.html", PAGE)]).await;
        let aid = v["artifact"]["id"].as_str().unwrap().to_string();
        let body = if app {
            json!({"mode": "app", "version": 1})
        } else {
            json!({})
        };
        let (s, v) = post(&local, &format!("/api/artifacts/{aid}/push"), body).await;
        assert_eq!(s, 200, "{v}");
        sync_once(&sleeper).await;
        let r = local.store.find_remote(&rid).unwrap().unwrap();
        assert_eq!(r.client_kind.as_deref(), Some(kind), "app {app}");
    }
}
