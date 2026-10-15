//! Linked threads (`clax_server::linked`): a local daemon linked to a
//! hosted test daemon keeps an artifact's comment threads in step with its
//! public twin's, both ways.

use crate::api_remotes::{Public, connect, json_of, local_req, post};
use crate::common::TestServer;
use clax_server::linked::{BACKOFF_CAP, BACKOFF_FIRST, BACKSTOP, Linked};
use clax_testkit::ManualSleeper;
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
async fn until<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    for _ in 0..200 {
        if let Some(v) = f().await {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never: {what}");
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
    // Back: Retry drains them, in order.
    p.local.state.linked.set_offline(false);
    let (s, v) = p
        .local_req(
            reqwest::Method::POST,
            &format!("/api/artifacts/{}/remotes/{}/retry", p.aid, p.remote),
            json!({}),
            None,
        )
        .await;
    assert_eq!((s, &v["retried"]), (200, &json!(2)), "{v}");
    until("the rows drain in order", || async {
        let ts = p.hosted_threads().await;
        (ts.len() == 1 && bodies(&ts[0]) == ["before", "one", "two"]).then_some(())
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
    assert_eq!(
        (
            links["links"][0]["pending"].clone(),
            links["links"][0]["failed"].clone()
        ),
        (json!(0), json!(0))
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
    until("the clip is cached", || async {
        cache.is_file().then_some(())
    })
    .await;
    assert_eq!(std::fs::read(&cache).unwrap(), png());
    // The thread names only the cached file here, and the clip route serves it.
    let v: Value = p
        .local
        .get_authed(&format!("/api/artifacts/{}/threads/{tid}", p.aid))
        .await
        .json()
        .await
        .unwrap();
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
