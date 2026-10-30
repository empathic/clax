//! Watching an artifact on a connected Clax (spec
//! 2026-10-29-remote-instances §4, "watch"): the daemon follows it, keeping
//! its threads in a local mirror (`clax_server::linked`), so a comment there
//! sent to a local agent reaches it as a local one does; each watching
//! local session is an agent of its own there.

use clax_core::working::ManualClock;
use clax_server::linked::{BACKSTOP, Linked, MIRROR_SWEEP};
use clax_testkit::remotes::{Public, connect, json_of, local_req, post};
use clax_testkit::{ManualSleeper, TestServer};
use reqwest::Method;
use reqwest::header::{COOKIE, HOST, ORIGIN};
use serde_json::{Value, json};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

const PAGE: &str = "<h1>Their plan</h1>";

/// A local daemon connected to a hosted one, on whose owner's artifact
/// `raid` its agents comment; the sync's timers on `sleeper`, the local
/// store's clock `clock`.
struct There {
    public: Public,
    local: TestServer,
    sleeper: Arc<ManualSleeper>,
    clock: Arc<ManualClock>,
    rid: String,
    raid: String,
    url: String,
}

impl There {
    async fn new() -> There {
        let public = Public::new().await;
        let sleeper = Arc::new(ManualSleeper::default());
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let sl = sleeper.clone();
        let local = TestServer::spawn_with_clock_and(clock.clone(), move |s| {
            s.linked = Arc::new(Linked::new(sl));
        })
        .await;
        let remote = connect(&local, &public).await;
        let (s, v) = public
            .cli(
                Method::POST,
                "/api/artifacts",
                Some(json!({"title": "Theirs",
                    "files": {"index.html": {"content": PAGE, "encoding": "utf8"}}})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let raid = v["artifact"]["id"].as_str().unwrap().to_string();
        let url = format!("{}/a/{raid}", public.origin);
        There {
            public,
            local,
            sleeper,
            clock,
            rid: remote["id"].as_str().unwrap().to_string(),
            raid,
            url,
        }
    }

    /// A local agent session named `name`.
    async fn agent(&self, name: &str) -> String {
        let sid = self
            .local
            .register_session("claude", &format!("h-{name}"))
            .await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (s, v) = local_req(
            &self.local,
            Method::PATCH,
            &format!("/api/sessions/{sid}"),
            json!({"name": name}),
            None,
        )
        .await;
        assert_eq!(s, 200, "{v}");
        sid
    }

    /// Session `sid`'s `PUT` or `DELETE` of its remote watch of `raid`.
    async fn remote_watch(&self, method: Method, sid: &str, raid: &str) -> (u16, Value) {
        local_req(
            &self.local,
            method,
            &format!("/api/sessions/{sid}/remote-watches"),
            json!({"remote": self.rid, "artifact_id": raid, "url": self.url}),
            Some(sid),
        )
        .await
    }

    /// Session `sid` watches the artifact there; the answer.
    async fn watch(&self, sid: &str) -> Value {
        let (s, v) = self.remote_watch(Method::PUT, sid, &self.raid).await;
        assert_eq!(s, 200, "{v}");
        v
    }

    /// A thread the hosted owner's browser starts on the artifact there:
    /// its ID there.
    async fn thread_there(&self, body: &str) -> String {
        let ts = &self.public.ts;
        let res = ts
            .client
            .post(format!("{}/api/artifacts/{}/threads", ts.base, self.raid))
            .header(HOST, format!("localhost:{}", ts.addr.port()))
            .header(COOKIE, format!("clax_session={}", self.public.cookie))
            .header(ORIGIN, &self.public.origin)
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
        v["thread"]["id"].as_str().unwrap().to_string()
    }

    /// The hosted owner sends thread `rtid` there to the agent `to` names
    /// (every agent watching it for `None`).
    async fn send_there(&self, rtid: &str, to: Option<&str>) -> (u16, Value) {
        let body = match to {
            Some(t) => json!({"to": t}),
            None => json!({}),
        };
        self.public
            .browser(
                Method::POST,
                &format!("/api/artifacts/{}/threads/{rtid}/send", self.raid),
                body,
            )
            .await
    }

    /// Session `sid`'s notices, waiting up to `wait` seconds.
    async fn notices(&self, sid: &str, wait: u64) -> Value {
        let res = self
            .local
            .authed(self.local.client.get(format!(
                "{}/api/sessions/{sid}/notices?wait={wait}",
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

    /// Session `sid`'s `wait_for_feedback`, waiting up to `wait` seconds.
    async fn wait_for_feedback(&self, sid: &str, wait: u64) -> Value {
        let res = self
            .local
            .authed(self.local.client.get(format!(
                "{}/api/sessions/{sid}/feedback?tier=wait&wait={wait}",
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

    /// The names of the live sessions there.
    fn sessions_there(&self) -> Vec<String> {
        self.public
            .ts
            .store
            .list_sessions(true)
            .unwrap()
            .into_iter()
            .filter_map(|s| s.name)
            .collect()
    }

    /// Returns once the agent named `name` has its session there.
    async fn agent_there(&self, name: &str) {
        until("the agent's session there", || async {
            self.sessions_there()
                .iter()
                .any(|n| n == name)
                .then_some(())
        })
        .await;
    }

    /// Returns once the sync waits out its backstop: its cycle ended.
    async fn settled(&self) {
        until("the sync's cycle ends", || async {
            (self.sleeper.waiting(BACKSTOP) == 1).then_some(())
        })
        .await;
    }

    /// The mirror of the artifact there, while this home follows it.
    fn mirror(&self) -> Option<String> {
        self.local
            .state
            .store
            .remote_mirror(&self.rid, &self.raid)
            .unwrap()
    }
}

/// `f`'s value once it has one, checked again every 50 ms. No wall-clock
/// limit: a condition that never holds is ended by the test runner's
/// timeout, whose output names `what`.
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

#[tokio::test]
async fn a_comment_on_a_watched_remote_artifact_reaches_feedback() {
    let t = There::new().await;
    let sid = t.agent("fixer").await;
    let w = t.watch(&sid).await;
    assert_eq!(w["url"], json!(t.url), "{w}");
    assert_eq!(w["artifact_id"], json!(t.raid), "{w}");
    assert_eq!(w["remote"]["origin"], json!(t.public.origin), "{w}");
    assert_eq!(w["watch"]["replies_armed"], true, "{w}");
    // The agent is one there, under its own name, as soon as it watches.
    until("the agent's session there", || async {
        t.sessions_there()
            .iter()
            .any(|n| n == "fixer")
            .then_some(())
    })
    .await;
    // The resolver names the remote artifact and its mirror, never the
    // mirror as a local artifact.
    let (s, r) = local_req(
        &t.local,
        Method::GET,
        &format!("/api/resolve?url={}", enc(&t.url)),
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["kind"], "remote", "{r}");
    assert_eq!(r["mirror"], json!(t.mirror().unwrap()), "{r}");
    // A person there comments and sends it to the agent by its name.
    let rtid = t.thread_there("Make the heading bigger").await;
    let (s, v) = t.send_there(&rtid, Some("fixer")).await;
    assert_eq!(s, 200, "{v}");
    // The notice names it as there: the remote's address and thread.
    let n = t.notices(&sid, 20).await;
    let notice = &n["notices"][0];
    assert_eq!(notice["thread_id"], json!(rtid), "{n}");
    assert_eq!(notice["url"], json!(t.url), "{n}");
    assert_eq!(notice["artifact_id"], json!(t.raid), "{n}");
    assert_eq!(notice["remote"]["origin"], json!(t.public.origin), "{n}");
    let line = n["lines"][0].as_str().unwrap();
    assert!(line.contains(&format!("url_or_id \"{}\"", t.url)), "{line}");
    assert!(!n.to_string().contains("heading bigger"), "{n}");
    // Read through the remote proxy, as comments_read reads it, the notice
    // is acknowledged: wait_for_feedback hands nothing over again.
    let res = t
        .local
        .authed(t.local.client.get(format!(
            "{}/api/remotes/{}/artifacts/{}/threads/{rtid}",
            t.local.base, t.rid, t.raid
        )))
        .header("x-clax-session", &sid)
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!(s, 200, "{v}");
    let fb = t.wait_for_feedback(&sid, 0).await;
    assert_eq!(fb["feedback"], json!([]), "{fb}");
    // A second comment there reaches wait_for_feedback, with its body.
    let rtid2 = t.thread_there("And the footer").await;
    let (s, v) = t.send_there(&rtid2, Some("fixer")).await;
    assert_eq!(s, 200, "{v}");
    let fb = t.wait_for_feedback(&sid, 20).await;
    let item = &fb["feedback"][0];
    assert_eq!(item["thread_id"], json!(rtid2), "{fb}");
    assert_eq!(item["url"], json!(t.url), "{fb}");
    assert_eq!(item["body"], "And the footer", "{fb}");
    assert_eq!(item["version"], 1, "{fb}");
    assert_eq!(item["remote"]["origin"], json!(t.public.origin), "{fb}");
    assert!(fb["text"].as_str().unwrap().contains(&t.url), "{fb}");
    // The agent's reply goes up as itself, in its own session there.
    let res = t
        .local
        .authed(t.local.client.post(format!(
            "{}/api/remotes/{}/artifacts/{}/threads/{rtid2}/comments",
            t.local.base, t.rid, t.raid
        )))
        .header("x-clax-session", &sid)
        .header("idempotency-key", "reply-1")
        .json(&json!({"body": "Done", "author_kind": "agent"}))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["comment"]["author_kind"], "agent", "{v}");
    assert_eq!(
        t.sessions_there().iter().filter(|n| *n == "fixer").count(),
        1,
        "no session of its own for the write"
    );
}

#[tokio::test]
async fn each_watching_session_is_its_own_agent_there() {
    let t = There::new().await;
    let alpha = t.agent("alpha").await;
    let beta = t.agent("beta").await;
    t.watch(&alpha).await;
    t.watch(&beta).await;
    until("both agents there", || async {
        let names = t.sessions_there();
        (names.iter().any(|n| n == "alpha") && names.iter().any(|n| n == "beta")).then_some(())
    })
    .await;
    let rtid = t.thread_there("Only for alpha").await;
    let (s, v) = t.send_there(&rtid, Some("alpha")).await;
    assert_eq!(s, 200, "{v}");
    let n = t.notices(&alpha, 20).await;
    assert_eq!(n["notices"][0]["thread_id"], json!(rtid), "{n}");
    // The cycle that sent it to alpha is done: beta got nothing.
    t.settled().await;
    let none = t.notices(&beta, 0).await;
    assert_eq!(none["notices"], json!([]), "{none}");
    let fb = t.wait_for_feedback(&beta, 0).await;
    assert_eq!(fb["feedback"], json!([]), "{fb}");
    // Alpha's reply is alpha's there; beta's is refused, the thread not its.
    let reply = |sid: &str, key: &str| {
        let req = t
            .local
            .authed(t.local.client.post(format!(
                "{}/api/remotes/{}/artifacts/{}/threads/{rtid}/comments",
                t.local.base, t.rid, t.raid
            )))
            .header("x-clax-session", sid)
            .header("idempotency-key", key)
            .json(&json!({"body": format!("from {sid}"), "author_kind": "agent"}));
        async move { json_of(req.send().await.unwrap()).await }
    };
    let (s, v) = reply(&alpha, "a-1").await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = reply(&beta, "b-1").await;
    assert_eq!(s, 409, "{v}");
}

#[tokio::test]
async fn unwatching_ends_the_remote_session_and_drops_late_notices() {
    let t = There::new().await;
    let sid = t.agent("leaver").await;
    t.watch(&sid).await;
    until("the agent's session there", || async {
        t.sessions_there()
            .iter()
            .any(|n| n == "leaver")
            .then_some(())
    })
    .await;
    let (s, v) = t.remote_watch(Method::DELETE, &sid, &t.raid).await;
    assert_eq!(s, 204, "{v}");
    until("the agent's session there ends", || async {
        (!t.sessions_there().iter().any(|n| n == "leaver")).then_some(())
    })
    .await;
    // It is no longer an agent there to send to, and a send to every
    // agent there reaches nothing here.
    let rtid = t.thread_there("Too late").await;
    let (s, v) = t.send_there(&rtid, Some("leaver")).await;
    assert_eq!(s, 400, "{v}");
    let (s, v) = t.send_there(&rtid, None).await;
    assert_eq!(s, 200, "{v}");
    // Its only link no longer watched, the remote's sync has ended.
    until("the remote's sync ends", || async {
        (!t.local.state.linked.running(&t.rid)).then_some(())
    })
    .await;
    assert_eq!(t.notices(&sid, 0).await["notices"], json!([]));
    assert_eq!(t.wait_for_feedback(&sid, 0).await["feedback"], json!([]));
    // The link is left alone: watched again, the same mirror comes back.
    let m = t.mirror().unwrap();
    let links = t.local.state.store.all_remote_links().unwrap();
    assert!(links[0].unwatched_at.is_some(), "{links:?}");
    let w = t.watch(&sid).await;
    assert_eq!(w["mirror"], json!(m), "{w}");
}

#[tokio::test]
async fn mirrors_are_hidden_from_lists_and_dropped_after_a_week() {
    let t = There::new().await;
    let sid = t.agent("lister").await;
    t.watch(&sid).await;
    t.agent_there("lister").await;
    let m = t.mirror().unwrap();
    let rtid = t.thread_there("A comment there").await;
    let (s, v) = t.send_there(&rtid, Some("lister")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        t.notices(&sid, 20).await["notices"][0]["thread_id"],
        json!(rtid)
    );
    // In no listing: the gallery, the inbox, its search.
    for path in ["/api/artifacts", "/api/inbox", "/api/inbox?q=comment"] {
        let (s, v) = local_req(&t.local, Method::GET, path, json!(null), None).await;
        assert_eq!(s, 200, "{path}: {v}");
        assert!(!v.to_string().contains(&m), "{path}: {v}");
    }
    // Unwatched by every session, it is kept for a week, then dropped.
    let (s, v) = t.remote_watch(Method::DELETE, &sid, &t.raid).await;
    assert_eq!(s, 204, "{v}");
    t.clock.advance(6 * 24 * 3600);
    t.sleeper.wait_for(MIRROR_SWEEP, 1).await;
    t.sleeper.fire(MIRROR_SWEEP);
    t.sleeper.wait_for(MIRROR_SWEEP, 1).await;
    assert_eq!(t.mirror(), Some(m.clone()), "kept within the week");
    t.clock.advance(24 * 3600 + 1);
    t.sleeper.fire(MIRROR_SWEEP);
    until("the mirror is dropped", || async {
        t.mirror().is_none().then_some(())
    })
    .await;
    assert!(t.local.state.store.all_remote_links().unwrap().is_empty());
    // Watched again, a new mirror is made.
    let w = t.watch(&sid).await;
    assert_ne!(w["mirror"], json!(m), "{w}");
}

#[tokio::test]
async fn a_pushed_artifacts_public_url_is_never_followed() {
    let t = There::new().await;
    let v = t.local.publish("Mine", &[("index.html", PAGE)]).await;
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let (s, v) = post(&t.local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    let pushed = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let public_url = v["link"]["url"].as_str().unwrap().to_string();
    // Its public URL resolves to the local artifact...
    let (s, r) = local_req(
        &t.local,
        Method::GET,
        &format!("/api/resolve?url={}", enc(&public_url)),
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(
        (r["kind"].as_str(), r["artifact_id"].as_str()),
        (Some("local"), Some(aid.as_str())),
        "{r}"
    );
    // ...and is never followed: the local artifact is the one to watch.
    let sid = t.agent("pusher").await;
    let (s, e) = t.remote_watch(Method::PUT, &sid, &pushed).await;
    assert_eq!(s, 400, "{e}");
    assert_eq!(e["error"]["code"], "pushed_here", "{e}");
    assert_eq!(e["error"]["artifact_id"], json!(aid), "{e}");
    assert_eq!(
        t.local.state.store.remote_mirror(&t.rid, &pushed).unwrap(),
        None
    );
    assert!(
        t.local
            .state
            .store
            .all_remote_links()
            .unwrap()
            .iter()
            .all(|l| !l.followed)
    );
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
