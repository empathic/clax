//! Connection requests (`clax_server::routes::remote_requests`): an agent
//! of the owner's asks that this home connect to a Clax instance (a hosted
//! test daemon, `Public`); the owner answers in their browser, and a
//! Connect runs the device sign-in the daemon then polls itself, the
//! person approving or denying it on the remote.

use clax_testkit::remotes::{NotYetClax, Public, json_of};
use clax_testkit::{ManualSleeper, TestServer};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// `POST /api/remotes/requests` with `body`, as an agent naming
/// `session` (with the agent channel's marker when `None`).
async fn ask(ts: &TestServer, session: Option<&str>, body: Value) -> (u16, Value) {
    let mut r = ts
        .authed(ts.client.post(format!("{}/api/remotes/requests", ts.base)))
        .json(&body);
    r = match session {
        Some(s) => r.header("x-clax-session", s),
        None => r.header("x-clax-channel", "agent"),
    };
    json_of(r.send().await.unwrap()).await
}

/// `GET /api/remotes/requests/<id>?wait=<wait>` as session `sid`'s agent.
async fn wait(ts: &TestServer, sid: &str, id: &str, wait: u64) -> (u16, Value) {
    let r = ts
        .client
        .get(format!("{}/api/remotes/requests/{id}?wait={wait}", ts.base));
    json_of(waiting_as(r, &ts.token, sid).send().await.unwrap()).await
}

fn waiting_as(r: reqwest::RequestBuilder, token: &str, sid: &str) -> reqwest::RequestBuilder {
    r.bearer_auth(token).header("x-clax-session", sid)
}

/// `POST /api/remotes/requests/<id>/answer {answer}` from the owner's
/// browser (its owner cookie).
async fn answer(ts: &TestServer, id: &str, answer: &str) -> (u16, Value) {
    let r = ts
        .client
        .post(format!("{}/api/remotes/requests/{id}/answer", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"answer": answer}));
    json_of(r.send().await.unwrap()).await
}

/// A session of `harness`; its ID.
async fn session(ts: &TestServer, harness: &str, hsid: &str) -> String {
    ts.register_session(harness, hsid).await["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The owner's `remote_request` inbox items.
async fn cards(ts: &TestServer) -> Vec<Value> {
    let res = ts
        .get_authed("/api/inbox?kind=remote_request&read=all")
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    v["items"].as_array().cloned().unwrap_or_default()
}

/// An open request for `public` asked by a new session; the session and
/// the request's ID.
async fn opened(ts: &TestServer, public: &Public) -> (String, String) {
    let sid = session(ts, "claude", "h-open").await;
    let (s, v) = ask(
        ts,
        Some(&sid),
        json!({"url": format!("{}/a/7q3k9mzx2b4t", public.origin), "reason": "to read the plan"}),
    )
    .await;
    assert_eq!((s, v["status"].as_str()), (201, Some("open")), "{v}");
    (sid, v["request"]["id"].as_str().unwrap().to_string())
}

/// The user code in a sign-in's `verification_url` (`…/device?code=<c>`).
fn user_code(v: &Value) -> String {
    let url = v["verification_url"].as_str().expect("a verification_url");
    url.rsplit("code=").next().unwrap().to_string()
}

/// A local daemon whose question clock (request waits, sign-in polls) the
/// test fires.
async fn clocked() -> (TestServer, Arc<ManualSleeper>) {
    TestServer::spawn_question_clock().await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_without_a_session_cannot_ask() {
    let ts = TestServer::spawn().await;
    let public = Public::new().await;
    let body = json!({"url": public.origin});
    // The bare token (the CLI by hand, or an agent that named no session).
    let (s, v) = json_of(
        ts.authed(ts.client.post(format!("{}/api/remotes/requests", ts.base)))
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "session_required"), "{v}");
    let (s, v) = ask(&ts, None, body.clone()).await;
    assert_eq!((s, code(&v)), (403, "session_required"), "{v}");
    let (s, v) = ask(&ts, Some("s_not_a_session"), body.clone()).await;
    assert_eq!((s, code(&v)), (403, "session_required"), "{v}");
    // No credential at all.
    let res = ts
        .client
        .post(format!("{}/api/remotes/requests", ts.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert!(cards(&ts).await.is_empty(), "nobody was told");
    let sid = session(&ts, "claude", "h1").await;
    // An origin that is not a Clax is refused before anyone hears of it.
    let site = NotYetClax::start().await;
    let (s, v) = ask(
        &ts,
        Some(&sid),
        json!({"url": format!("{}/x", site.origin)}),
    )
    .await;
    assert_eq!((s, code(&v)), (422, "not_a_clax"), "{v}");
    let (s, v) = ask(&ts, Some(&sid), json!({"url": "http://example.com/a"})).await;
    assert_eq!((s, code(&v)), (422, "not_a_clax"), "{v}");
    // With a session, the request opens.
    let (s, v) = ask(&ts, Some(&sid), body).await;
    assert_eq!((s, v["status"].as_str()), (201, Some("open")), "{v}");
    assert_eq!(v["request"]["origin"], json!(public.origin));
    assert_eq!(cards(&ts).await.len(), 1);
}

/// An ask about a muted origin is answered `muted` before the daemon
/// fetches that origin's discovery: nothing is asked of it.
/// Each ask and the owner's answer are recorded as their actor:
/// `remote_request.ask` (opened, or joined) as each asking session's agent,
/// `remote_request.answer` as the owner.
#[tokio::test(flavor = "multi_thread")]
async fn asks_and_the_owners_answer_are_audited() {
    let ts = TestServer::spawn().await;
    let public = Public::new().await;
    let (first, id) = opened(&ts, &public).await;
    let second = session(&ts, "codex", "h-join").await;
    let (s, v) = ask(&ts, Some(&second), json!({"url": public.origin})).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("open")), "{v}");
    let (s, v) = answer(&ts, &id, "not_now").await;
    assert_eq!(s, 200, "{v}");
    let events: Vec<(String, Value, Value)> = ts
        .store
        .events_after(0, 1000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind.starts_with("remote_request."))
        .map(|e| {
            let body: Value = serde_json::from_str(&e.body).unwrap();
            (e.kind, serde_json::from_str(&e.actor).unwrap(), body)
        })
        .collect();
    let kinds: Vec<&str> = events.iter().map(|(k, _, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "remote_request.ask",
            "remote_request.ask",
            "remote_request.answer"
        ],
        "{events:?}"
    );
    let (_, a1, b1) = &events[0];
    assert_eq!(
        (a1["session_id"].as_str(), b1["opened"].as_bool()),
        (Some(first.as_str()), Some(true))
    );
    assert_eq!(b1["request_id"], json!(id));
    assert_eq!(b1["origin"], json!(public.origin));
    let (_, a2, b2) = &events[1];
    assert_eq!(
        (a2["session_id"].as_str(), b2["opened"].as_bool()),
        (Some(second.as_str()), Some(false))
    );
    let (_, a3, b3) = &events[2];
    assert_eq!(
        (a3["type"].as_str(), b3["answer"].as_str()),
        (Some("owner"), Some("not_now")),
        "{a3}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_muted_origin_is_answered_without_fetching_its_discovery() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "claude", "h-muted").await;
    let site = NotYetClax::start().await;
    ts.store.mute_origin(&site.origin).unwrap();
    let (s, v) = ask(
        &ts,
        Some(&sid),
        json!({"url": format!("{}/x", site.origin)}),
    )
    .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("muted")), "{v}");
    assert_eq!(
        ts.state.discovery.get(&site.origin),
        clax_http::discovery_cache::Cached::Unknown,
        "its discovery was not fetched"
    );
    assert!(cards(&ts).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn two_agents_asking_at_once_make_one_card() {
    let ts = TestServer::spawn().await;
    let public = Public::new().await;
    let (a, b) = (
        session(&ts, "claude", "h1").await,
        session(&ts, "codex", "h2").await,
    );
    let url = |id: &str| json!({"url": format!("{}/a/{id}", public.origin)});
    let (x, y) = tokio::join!(
        ask(&ts, Some(&a), url("7q3k9mzx2b4t")),
        ask(&ts, Some(&b), url("8r4m2nzx3c5v"))
    );
    let mut statuses = vec![x.0, y.0];
    statuses.sort_unstable();
    assert_eq!(statuses, vec![200, 201], "{x:?} {y:?}");
    assert_eq!(x.1["request"]["id"], y.1["request"]["id"]);
    let askers = |v: &Value| v["request"]["askers"].as_u64().unwrap();
    assert_eq!(askers(&x.1).max(askers(&y.1)), 2, "{x:?} {y:?}");
    let items = cards(&ts).await;
    assert_eq!(items.len(), 1, "one card: {items:?}");
    let card = &items[0]["remote_request"];
    assert_eq!(card["origin"], json!(public.origin), "{card}");
    assert_eq!(card["status"], "open");
    assert_eq!(card["askers"], 2);
    assert!(items[0]["agent"]["harness"].is_string(), "{}", items[0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_runs_the_device_flow_and_wakes_the_waiting_agent() {
    let (ts, clock) = clocked().await;
    let public = Public::new().await;
    let (sid, id) = opened(&ts, &public).await;
    let waiting = {
        let r = waiting_as(
            ts.client
                .get(format!("{}/api/remotes/requests/{id}?wait=50", ts.base)),
            &ts.token,
            &sid,
        );
        tokio::spawn(async move { json_of(r.send().await.unwrap()).await })
    };
    clock.wait_for(Duration::from_secs(50), 1).await;
    let (s, v) = answer(&ts, &id, "connect").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["status"], "open");
    let uc = user_code(&v);
    let every = Duration::from_secs(v["interval"].as_u64().unwrap());
    clock.wait_for(every, 1).await;
    // Pressed again while it waits: the same sign-in, and still one driver.
    let (s, again) = answer(&ts, &id, "connect").await;
    assert_eq!((s, user_code(&again)), (200, uc.clone()), "{again}");
    assert_eq!(ts.state.remote_drivers.count(), 1);
    assert_eq!(clock.waiting(every), 1);
    // The agents never see the code or the approval page; the owner does.
    let (s, mine) = json_of(
        waiting_as(
            ts.client.get(format!("{}/api/remotes", ts.base)),
            &ts.token,
            &sid,
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{mine}");
    let pending = &mine["remotes"][0];
    assert_eq!(pending["status"], "pending", "{mine}");
    assert!(
        pending.get("user_code").is_none() && pending.get("verification_url").is_none(),
        "{mine}"
    );
    let (_, owners) = json_of(ts.get_authed("/api/remotes").await).await;
    assert_eq!(owners["remotes"][0]["user_code"], json!(uc), "{owners}");
    // The daemon polls while the person has not approved yet.
    public.clock.advance(5);
    clock.fire(every);
    clock.wait_for(every, 1).await;
    let (s, v) = public
        .browser(
            reqwest::Method::POST,
            &format!("/api/hosted/device/{uc}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    public.clock.advance(5);
    clock.fire(every);
    let (s, v) = waiting.await.unwrap();
    assert_eq!((s, v["status"].as_str()), (200, Some("connected")), "{v}");
    assert!(
        v.get("user_code").is_none() && v.get("verification_url").is_none(),
        "{v}"
    );
    let remote = v["remote"].as_str().unwrap().to_string();
    let (_, rs) = json_of(ts.get_authed("/api/remotes").await).await;
    assert_eq!(rs["remotes"][0]["id"], json!(remote), "{rs}");
    assert_eq!(rs["remotes"][0]["status"], "connected");
    // The card says so, and a later ask answers the remote at once.
    assert_eq!(cards(&ts).await[0]["remote_request"]["status"], "connected");
    assert_eq!(cards(&ts).await[0]["read"], true);
    let (s, v) = ask(&ts, Some(&sid), json!({"url": public.origin})).await;
    assert_eq!(
        (s, &v),
        (200, &json!({"status": "connected", "remote": remote}))
    );
    let (s, v) = answer(&ts, &id, "never").await;
    assert_eq!((s, code(&v)), (409, "request_closed"), "{v}");
}

/// Connects `id` and has the person act on the remote with `act`
/// (`deny`, or nothing for an expiry); the waiting agent's answer.
async fn sign_in_ends(act: Option<&str>) -> (TestServer, Public, String, Value) {
    let (ts, clock) = clocked().await;
    let public = Public::new().await;
    let (sid, id) = opened(&ts, &public).await;
    let (s, v) = answer(&ts, &id, "connect").await;
    assert_eq!(s, 200, "{v}");
    let uc = user_code(&v);
    let every = Duration::from_secs(v["interval"].as_u64().unwrap());
    match act {
        Some(a) => {
            let (s, v) = public
                .browser(
                    reqwest::Method::POST,
                    &format!("/api/hosted/device/{uc}/{a}"),
                    json!({}),
                )
                .await;
            assert!(matches!(s, 200 | 204), "{s}: {v}");
            public.clock.advance(5);
        }
        None => public.clock.advance(3600),
    }
    clock.wait_for(every, 1).await;
    clock.fire(every);
    let (s, v) = wait(&ts, &sid, &id, 50).await;
    assert_eq!(s, 200, "{v}");
    (ts, public, sid, v)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_device_flow_answers_dismissed() {
    let (ts, public, sid, v) = sign_in_ends(Some("deny")).await;
    assert_eq!(v["status"], "dismissed", "{v}");
    let (_, rs) = json_of(ts.get_authed("/api/remotes").await).await;
    assert_eq!(rs["remotes"], json!([]), "the remote is forgotten: {rs}");
    // Denied there holds the origin off as a "Not now".
    let (s, v) = ask(&ts, Some(&sid), json!({"url": public.origin})).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("not_now")), "{v}");
    assert!(v["until"].is_string(), "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_device_flow_answers_dismissed() {
    let (ts, public, sid, v) = sign_in_ends(None).await;
    assert_eq!(v["status"], "dismissed", "{v}");
    let (_, rs) = json_of(ts.get_authed("/api/remotes").await).await;
    assert_eq!(rs["remotes"], json!([]), "{rs}");
    // An expiry holds nothing off: the agent may ask again.
    let (s, v) = ask(&ts, Some(&sid), json!({"url": public.origin})).await;
    assert_eq!((s, v["status"].as_str()), (201, Some("open")), "{v}");
}

/// A remote whose token was revoked there is connected again from an
/// agent's request, and the person denies that sign-in there: the remote
/// is signed out again, its pushed artifact's link kept, not forgotten.
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_reconnect_leaves_the_remote_signed_out_with_its_links() {
    let (ts, clock) = clocked().await;
    let public = Public::new().await;
    let r = ts
        .store
        .add_pending_remote(
            &public.origin,
            "pub",
            &clax_store::remotes::PendingSignIn {
                device_code: clax_core::Secret::new("cxd_old".into()),
                user_code: "ABCD-1234".into(),
                verification_url: format!("{}/device?code=ABCD-1234", public.origin),
                expires_at: "2099-01-01T00:00:00.000Z".into(),
                interval_s: 5,
            },
        )
        .unwrap();
    ts.store
        .connect_remote(
            &r.id,
            &clax_store::remotes::SignedInAs {
                token: clax_core::Secret::new("cxa_old".into()),
                person_public_id: "p1".into(),
                person_name: Some("Ada".into()),
                grant_id: "g_old".into(),
            },
        )
        .unwrap();
    let url = format!("{}/a/7q3k9mzx2b4t", public.origin);
    ts.store
        .link_remote("7q3k9mzx2b4a", &r.id, "7q3k9mzx2b4t", &url, 1, 1)
        .unwrap();
    assert!(ts.store.sign_out_remote(&r.id).unwrap());
    let (sid, id) = opened(&ts, &public).await;
    let (s, v) = answer(&ts, &id, "connect").await;
    assert_eq!(s, 200, "{v}");
    let uc = user_code(&v);
    let every = Duration::from_secs(v["interval"].as_u64().unwrap());
    let (s, v) = public
        .browser(
            reqwest::Method::POST,
            &format!("/api/hosted/device/{uc}/deny"),
            json!({}),
        )
        .await;
    assert!(matches!(s, 200 | 204), "{s}: {v}");
    public.clock.advance(5);
    clock.wait_for(every, 1).await;
    clock.fire(every);
    let (s, v) = wait(&ts, &sid, &id, 50).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("dismissed")), "{v}");
    let back = ts.store.find_remote(&r.id).unwrap().expect("kept");
    assert_eq!(back.status, "signed_out");
    let links = ts.store.remote_links("7q3k9mzx2b4a").unwrap();
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].url, url);
}

#[tokio::test(flavor = "multi_thread")]
async fn answering_is_the_owners_browser_alone() {
    let ts = TestServer::spawn().await;
    let public = Public::new().await;
    let (sid, id) = opened(&ts, &public).await;
    let path = format!("{}/api/remotes/requests/{id}/answer", ts.base);
    let body = json!({"answer": "never"});
    // The token, by hand or as the asking agent: not a browser.
    let (s, v) = json_of(
        ts.authed(ts.client.post(&path))
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "browser_required"), "{v}");
    let (s, v) = json_of(
        ts.authed(ts.client.post(&path))
            .header("x-clax-session", &sid)
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "browser_required"), "{v}");
    // No credential.
    let res = ts.client.post(&path).json(&body).send().await.unwrap();
    assert!(
        matches!(res.status().as_u16(), 401 | 403),
        "{}",
        res.status()
    );
    // A browser of the owner's on another origin.
    let (s, v) = json_of(
        ts.client
            .post(&path)
            .header("cookie", ts.owner_cookie())
            .header("origin", "https://evil.example")
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(matches!(s, 401 | 403), "{s}: {v}");
    let (s, w) = wait(&ts, &sid, &id, 0).await;
    assert_eq!((s, w["status"].as_str()), (200, Some("open")), "{w}");
    // The owner's browser.
    let (s, v) = answer(&ts, &id, "never").await;
    assert_eq!(
        (s, v["request"]["status"].as_str()),
        (200, Some("muted")),
        "{v}"
    );
    let (_, w) = wait(&ts, &sid, &id, 0).await;
    assert_eq!(w["status"], "muted");
    let (s, v) = ask(&ts, Some(&sid), json!({"url": public.origin})).await;
    assert_eq!((s, &v), (200, &json!({"status": "muted"})));
    // Settings, Remotes lists the mute; unmuting is the browser's too.
    let (s, m) = json_of(
        ts.client
            .get(format!("{}/api/remotes/mutes", ts.base))
            .header("cookie", ts.owner_cookie())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (s, &m["mutes"][0]["origin"]),
        (200, &json!(public.origin)),
        "{m}"
    );
    let enc: String = url::form_urlencoded::byte_serialize(public.origin.as_bytes()).collect();
    let unmute = format!("{}/api/remotes/mutes/{enc}", ts.base);
    let (s, v) = json_of(
        ts.authed(ts.client.delete(&unmute))
            .header("x-clax-session", &sid)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "browser_required"), "{v}");
    let (s, v) = json_of(
        ts.client
            .delete(&unmute)
            .header("cookie", ts.owner_cookie())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, &v["unmuted"]), (200, &json!(true)), "{v}");
    let (s, v) = ask(&ts, Some(&sid), json!({"url": public.origin})).await;
    assert_eq!((s, v["status"].as_str()), (201, Some("open")), "{v}");
}

/// Approves sign-in `uc` on `public`, then lets driver clock `clock` poll
/// once (`every`); the request `id`'s answer to session `sid` on `ts`.
async fn approve_and_poll(
    ts: &TestServer,
    public: &Public,
    clock: &ManualSleeper,
    every: Duration,
    (sid, id, uc): (&str, &str, &str),
) -> Value {
    let (s, v) = public
        .browser(
            reqwest::Method::POST,
            &format!("/api/hosted/device/{uc}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    public.clock.advance(5);
    clock.fire(every);
    let (s, v) = wait(ts, sid, id, 50).await;
    assert_eq!(s, 200, "{v}");
    v
}

#[tokio::test(flavor = "multi_thread")]
async fn two_connects_at_once_start_one_sign_in() {
    let (ts, clock) = clocked().await;
    let public = Public::new().await;
    let (sid, id) = opened(&ts, &public).await;
    let (a, b) = tokio::join!(answer(&ts, &id, "connect"), answer(&ts, &id, "connect"));
    assert_eq!((a.0, b.0), (200, 200), "{a:?} {b:?}");
    assert_eq!(user_code(&a.1), user_code(&b.1));
    assert_eq!(a.1["remote"]["id"], b.1["remote"]["id"]);
    let (_, rs) = json_of(ts.get_authed("/api/remotes").await).await;
    assert_eq!(rs["remotes"].as_array().unwrap().len(), 1, "{rs}");
    let every = Duration::from_secs(a.1["interval"].as_u64().unwrap());
    clock.wait_for(every, 1).await;
    assert_eq!(ts.state.remote_drivers.count(), 1);
    let v = approve_and_poll(&ts, &public, &clock, every, (&sid, &id, &user_code(&a.1))).await;
    assert_eq!(v["status"], "connected", "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_again_drives_a_stranded_sign_in() {
    let (ts, clock) = clocked().await;
    let public = Public::new().await;
    let (sid, id) = opened(&ts, &public).await;
    let (_, v) = answer(&ts, &id, "connect").await;
    let uc = user_code(&v);
    let every = Duration::from_secs(v["interval"].as_u64().unwrap());
    clock.wait_for(every, 1).await;
    // As after a restart: no driver of this daemon's holds the request.
    ts.state.remote_drivers.forget(&id);
    let (s, again) = answer(&ts, &id, "connect").await;
    assert_eq!((s, user_code(&again)), (200, uc.clone()), "{again}");
    // A new driver polls it beside the stranded task.
    clock.wait_for(every, 2).await;
    assert!(ts.state.remote_drivers.drives(&id));
    let v = approve_and_poll(&ts, &public, &clock, every, (&sid, &id, &uc)).await;
    assert_eq!(v["status"], "connected", "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_resumes_a_waiting_sign_in() {
    let (ts, clock) = clocked().await;
    let public = Public::new().await;
    let (sid, id) = opened(&ts, &public).await;
    let (_, v) = answer(&ts, &id, "connect").await;
    let uc = user_code(&v);
    let every = Duration::from_secs(v["interval"].as_u64().unwrap());
    clock.wait_for(every, 1).await;
    // The same home, started again: its own clock, nothing in memory.
    let later = Arc::new(ManualSleeper::default());
    let l = later.clone();
    let again = ts.reopened_with(move |s| s.question_sleeper = l).await;
    later.wait_for(every, 1).await;
    assert!(again.state.remote_drivers.drives(&id));
    let v = approve_and_poll(&again, &public, &later, every, (&sid, &id, &uc)).await;
    assert_eq!(v["status"], "connected", "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wait_past_the_polls_one_request_holds_says_to_call_again() {
    let (ts, clock) = clocked().await;
    let public = Public::new().await;
    let (sid, id) = opened(&ts, &public).await;
    let mut held = Vec::new();
    for _ in 0..4 {
        let r = waiting_as(
            ts.client
                .get(format!("{}/api/remotes/requests/{id}?wait=50", ts.base)),
            &ts.token,
            &sid,
        );
        held.push(tokio::spawn(async move {
            json_of(r.send().await.unwrap()).await
        }));
    }
    clock.wait_for(Duration::from_secs(50), 4).await;
    let (s, v) = wait(&ts, &sid, &id, 50).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (&v["status"], &v["call_again"], &v["retry_after_s"]),
        (&json!("open"), &json!(true), &json!(5)),
        "{v}"
    );
    clock.fire(Duration::from_secs(50));
    for h in held {
        assert_eq!(h.await.unwrap().1["status"], "open");
    }
}

/// With a passkey required, answering a connection request is a decision
/// that matters: it waits for a confirmation, which takes the passkey's
/// assertion, and then the Connect proceeds.
#[tokio::test(flavor = "multi_thread")]
async fn with_passkeys_required_answering_connect_takes_an_assertion() {
    let (ts, _clock) = clocked().await;
    let key = clax_testkit::passkey::SoftPasskey::new();
    ts.require_passkey(&key).await;
    let public = Public::new().await;
    let (_sid, id) = opened(&ts, &public).await;
    let (s, v) = answer(&ts, &id, "connect").await;
    assert_eq!(
        (s, v["status"].as_str()),
        (202, Some("confirmation_pending")),
        "{v}"
    );
    let cid = v["confirmation_id"].as_str().unwrap().to_string();
    assert!(
        v["decision"]
            .as_str()
            .unwrap()
            .starts_with("connect this Clax to "),
        "{v}"
    );
    let owner = ts.owner_cookie();
    let confirm = format!("/api/confirmations/{cid}");
    let (s, v) = ts
        .as_browser(
            &owner,
            reqwest::Method::POST,
            &confirm,
            json!({"approve": true}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "passkey_required"), "{v}");
    let (origin, rp) = ts.passkey_rp().await;
    let c = ts
        .passkey_challenge(
            &owner,
            json!({"purpose": "confirm", "confirmation_id": cid}),
        )
        .await;
    let (s, v) = ts
        .as_browser(
            &owner,
            reqwest::Method::POST,
            &confirm,
            json!({"approve": true, "assertion": key.assertion(&c, &origin, &rp, 1)}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let r = ts
        .client
        .post(format!("{}/api/remotes/requests/{id}/answer", ts.base))
        .header("cookie", owner)
        .header(clax_core::confirm::CONFIRMATION_HEADER, &cid)
        .json(&json!({"answer": "connect"}));
    let (s, v) = json_of(r.send().await.unwrap()).await;
    assert_eq!(s, 200, "{v}");
    assert!(!user_code(&v).is_empty());
}
