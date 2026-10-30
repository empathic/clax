//! The decisions that matter, confirmed in the owner's browser while an
//! agent is live (`clax_http::confirm`): a decision made with the daemon
//! token answers 202 `confirmation_pending` and changes nothing until the
//! owner clicks Confirm in a browser an agent cannot have opened; the same
//! call made again with the confirmation's ID then goes ahead, once.

use clax_core::working::ManualClock;
use clax_testkit::remotes::{self, Public};
use clax_testkit::{TestClient, TestServer};
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::Arc;

const HEADER: &str = clax_core::confirm::CONFIRMATION_HEADER;

/// `method path` with the token and `body`, presenting confirmation
/// `confirm` when given: the status and the JSON answer.
async fn token(
    ts: &TestServer,
    method: Method,
    path: &str,
    body: Value,
    confirm: Option<&str>,
) -> (u16, Value) {
    let mut req = ts.authed(ts.client.request(method, format!("{}{path}", ts.base)));
    if let Some(id) = confirm {
        req = req.header(HEADER, id);
    }
    let res = req.json(&body).send().await.unwrap();
    let status = res.status().as_u16();
    let held = res
        .headers()
        .get(HEADER)
        .map(|v| v.to_str().unwrap().to_string());
    let v: Value = res.json().await.unwrap_or(Value::Null);
    if status == 202 && v["status"] == "confirmation_pending" {
        assert_eq!(held.as_deref(), v["confirmation_id"].as_str(), "{v}");
    }
    (status, v)
}

/// `method path` from the browser holding `cookie` (a `Cookie` header's
/// value), with `body` and the confirmation `confirm` when given.
async fn browser(
    ts: &TestServer,
    cookie: &str,
    method: Method,
    path: &str,
    body: Value,
    confirm: Option<&str>,
) -> (u16, Value) {
    let mut req = ts
        .client
        .request(method, format!("{}{path}", ts.base))
        .header("cookie", cookie);
    if let Some(id) = confirm {
        req = req.header(HEADER, id);
    }
    let res = req.json(&body).send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

/// The confirmation a 202 asked for: its ID, after checking the answer's
/// shape.
fn pending(ts: &TestServer, (status, v): &(u16, Value)) -> String {
    assert_eq!(*status, 202, "{v}");
    assert_eq!(v["status"], "confirmation_pending", "{v}");
    let id = v["confirmation_id"].as_str().unwrap().to_string();
    assert!(id.starts_with("cf_"), "{id}");
    let url = v["confirm_url"].as_str().unwrap();
    assert!(url.starts_with("http://"), "{url}");
    assert!(
        url.ends_with(&format!(":{}/inbox?confirm={id}", ts.addr.port())),
        "{url}"
    );
    id
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// The owner cookie of a browser that logs in now (`name=value`).
async fn login_now(ts: &TestServer) -> String {
    let res = ts.login_with("").await;
    let name = clax_http::identity::owner_cookie_name(ts.base.trim_start_matches("http://"));
    res.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .find(|kv| kv.starts_with(&format!("{name}=")))
        .expect("an owner cookie")
        .to_string()
}

async fn open_world(ts: &TestServer, aid: &str, confirm: Option<&str>) -> (u16, Value) {
    token(
        ts,
        Method::PATCH,
        &format!("/api/artifacts/{aid}"),
        json!({"site_access": "world"}),
        confirm,
    )
    .await
}

async fn site_is_open(ts: &TestServer, aid: &str) -> bool {
    ts.site_get_anon(aid, "/").await.status() == 200
}

async fn page(ts: &TestServer, title: &str) -> String {
    ts.publish(title, &[("index.html", "<h1>hi</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn ask_body() -> Value {
    json!({"source": "ask", "questions": [{"question": "Ship it?", "header": "Ship",
        "options": [{"label": "Yes"}, {"label": "No"}]}]})
}

#[tokio::test]
async fn a_token_answer_waits_for_the_browser_while_an_agent_is_live() {
    let ts = TestServer::spawn().await;
    let sid = ts.register_session("claude", "c-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let qid = ts.ask(&sid, ask_body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/questions/{qid}/answer");
    let answer = json!({"answers": [{"selected": ["Yes"]}]});
    let first = token(&ts, Method::POST, &path, answer.clone(), None).await;
    let id = pending(&ts, &first);
    assert_eq!(first.1["decision"], "answer an agent's question “Ship it?”");
    assert_eq!(ts.question_status(&qid).await, "open", "nothing changed");
    // Asked again, it is the same confirmation, still waiting.
    let again = token(&ts, Method::POST, &path, answer.clone(), None).await;
    assert_eq!(pending(&ts, &again), id);
    // The token reads it, and the owner's browser lists it with the agents live now.
    let (s, v) = token(
        &ts,
        Method::GET,
        &format!("/api/confirmations/{id}"),
        json!(null),
        None,
    )
    .await;
    assert_eq!(
        (s, v["confirmation"]["status"].as_str()),
        (200, Some("open"))
    );
    let (s, v) = browser(
        &ts,
        &ts.owner_cookie(),
        Method::GET,
        "/api/confirmations",
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200);
    let listed = &v["confirmations"][0];
    assert_eq!(listed["id"], id.as_str());
    assert_eq!(listed["words"], "answer an agent's question “Ship it?”");
    assert_eq!(listed["agents"][0]["harness"], "claude");
    // An answer that is not the one confirmed is not let through.
    let other = json!({"answers": [{"selected": ["No"]}]});
    let (s, v) = token(&ts, Method::POST, &path, other, Some(&id)).await;
    assert_eq!((s, code(&v)), (403, "confirmation_mismatch"));
    ts.confirm(&id, true).await;
    let (s, v) = token(
        &ts,
        Method::GET,
        &format!("/api/confirmations/{id}?wait=5"),
        json!(null),
        None,
    )
    .await;
    assert_eq!(
        (s, v["confirmation"]["status"].as_str()),
        (200, Some("approved"))
    );
    let (s, v) = token(&ts, Method::POST, &path, answer, Some(&id)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(ts.question_status(&qid).await, "answered");
    // The owner's agent answering is refused as before: no new power.
    let qid = ts.ask(&sid, ask_body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/questions/{qid}/answer", ts.base)),
        )
        .header("x-clax-channel", "agent")
        .json(&json!({"answers": [{"selected": ["Yes"]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let v: Value = res.json().await.unwrap();
    assert_eq!(code(&v), "not_for_agents");
}

#[tokio::test]
async fn with_no_agent_live_the_token_answers_at_once() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Plain").await;
    let (s, v) = open_world(&ts, &aid, None).await;
    assert_eq!(s, 200, "{v}");
    assert!(site_is_open(&ts, &aid).await);
}

/// Ending every session with the token does not turn confirmations off:
/// an agent ended less than ten minutes ago still counts.
#[tokio::test]
async fn ending_every_agent_keeps_confirmations_on_for_ten_minutes() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let aid = page(&ts, "Held").await;
    let sid = ts.register_session("claude", "c-2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.end_session(&sid).await;
    let id = pending(&ts, &open_world(&ts, &aid, None).await);
    // The card lists the agent that ended, as one that could have asked.
    let (_, v) = browser(
        &ts,
        &ts.owner_cookie(),
        Method::GET,
        "/api/confirmations",
        json!(null),
        None,
    )
    .await;
    assert!(
        v["confirmations"][0]["agents"][0]["ended_at"].is_string(),
        "{v}"
    );
    clock.advance(clax_http::confirm::CONFIRMATION_TTL_SECS - 1);
    assert_eq!(pending(&ts, &open_world(&ts, &aid, None).await), id);
    clock.advance(1);
    let (s, v) = open_world(&ts, &aid, None).await;
    assert_eq!(s, 200, "{v}");
    assert!(site_is_open(&ts, &aid).await);
}

/// Only the owner's own agents on this machine count: an agent of another
/// machine never holds the daemon token.
#[tokio::test]
async fn a_connected_persons_agent_alone_makes_nothing_wait() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    sam.register("claude", "sam-1").await;
    let aid = page(&ts, "Plain").await;
    let (s, v) = open_world(&ts, &aid, None).await;
    assert_eq!(s, 200, "{v}");
    // The owner's own agent does.
    ts.register_session("claude", "owner-1").await;
    let other = page(&ts, "Other").await;
    pending(&ts, &open_world(&ts, &other, None).await);
    let (_, v) = browser(
        &ts,
        &ts.owner_cookie(),
        Method::GET,
        "/api/confirmations",
        json!(null),
        None,
    )
    .await;
    let agents = v["confirmations"][0]["agents"].as_array().unwrap().clone();
    assert_eq!(
        agents.len(),
        1,
        "only the owner's agent is listed: {agents:?}"
    );
}

/// Pairing the extension is itself confirmed while an agent is live; a
/// paired extension then decides as the owner's browser.
#[tokio::test]
async fn pairing_the_extension_while_an_agent_is_live_asks_first() {
    let ts = TestServer::spawn().await;
    let sid = ts.register_session("claude", "c-9").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mint = json!({"extension_id": ts.extension_id()});
    let first = token(
        &ts,
        Method::POST,
        "/api/extension/credentials",
        mint.clone(),
        None,
    )
    .await;
    let id = pending(&ts, &first);
    assert!(
        first.1.get("credential").is_none(),
        "no credential before it is confirmed"
    );
    assert_eq!(
        first.1["decision"],
        "pair the Clax extension, which then acts as your browser"
    );
    ts.confirm(&id, true).await;
    let (s, v) = token(
        &ts,
        Method::POST,
        "/api/extension/credentials",
        mint,
        Some(&id),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["credential"].is_string());
    // The paired extension answers a question without a further confirmation.
    let ext = ts.extension().await;
    let qid = ts.ask(&sid, ask_body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ext
        .post(
            &format!("/api/questions/{qid}/answer"),
            json!({"answers": [{"selected": ["Yes"]}]}),
        )
        .await;
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    assert_eq!(ts.question_status(&qid).await, "answered");
}

#[tokio::test]
async fn a_confirmation_approves_only_its_own_decision_once() {
    let ts = TestServer::spawn().await;
    let (a, b) = (page(&ts, "A").await, page(&ts, "B").await);
    ts.register_session("claude", "c-3").await;
    let id = pending(&ts, &open_world(&ts, &a, None).await);
    ts.confirm(&id, true).await;
    // B's opening is another decision.
    let (s, v) = open_world(&ts, &b, Some(&id)).await;
    assert_eq!((s, code(&v)), (403, "confirmation_mismatch"));
    assert!(!site_is_open(&ts, &b).await);
    let (s, v) = open_world(&ts, &a, Some(&id)).await;
    assert_eq!(s, 200, "{v}");
    assert!(site_is_open(&ts, &a).await);
    // Closed again (closing asks no one), it is not opened by the same confirmation twice.
    let (s, _) = token(
        &ts,
        Method::PATCH,
        &format!("/api/artifacts/{a}"),
        json!({"site_access": "private"}),
        None,
    )
    .await;
    assert_eq!(s, 200);
    let (s, v) = open_world(&ts, &a, Some(&id)).await;
    assert_eq!((s, code(&v)), (409, "confirmation_used"));
    assert!(!site_is_open(&ts, &a).await);
    let (s, v) = open_world(&ts, &a, Some("cf_none")).await;
    assert_eq!((s, code(&v)), (404, "unknown_confirmation"));
}

#[tokio::test]
async fn a_declined_or_expired_confirmation_changes_nothing() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let aid = page(&ts, "Held").await;
    ts.register_session("claude", "c-4").await;
    let id = pending(&ts, &open_world(&ts, &aid, None).await);
    ts.confirm(&id, false).await;
    let (s, v) = open_world(&ts, &aid, Some(&id)).await;
    assert_eq!((s, code(&v)), (403, "declined"));
    assert!(!site_is_open(&ts, &aid).await);
    // A new call asks anew; ten minutes on, it has expired unanswered.
    let id = pending(&ts, &open_world(&ts, &aid, None).await);
    clock.advance(clax_http::confirm::CONFIRMATION_TTL_SECS);
    let (s, v) = open_world(&ts, &aid, Some(&id)).await;
    assert_eq!((s, code(&v)), (410, "confirmation_expired"));
    let res = ts.confirm_raw(&id, true).await;
    assert_eq!(res.status(), 410);
    assert!(!site_is_open(&ts, &aid).await);
    // Approved but left unused past its time, it expires too.
    let id = pending(&ts, &open_world(&ts, &aid, None).await);
    ts.confirm(&id, true).await;
    clock.advance(clax_http::confirm::CONFIRMATION_TTL_SECS);
    let (s, v) = open_world(&ts, &aid, Some(&id)).await;
    assert_eq!((s, code(&v)), (410, "confirmation_expired"));
    assert!(!site_is_open(&ts, &aid).await);
}

#[tokio::test]
async fn a_browser_session_minted_while_an_agent_was_live_cannot_confirm() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Mine").await;
    // Minted before any agent: a browser the owner opened.
    let clean = login_now(&ts).await;
    ts.register_session("claude", "c-5").await;
    // Minted while one is live: an agent could have opened it.
    let minted = login_now(&ts).await;
    let path = format!("/api/artifacts/{aid}");
    let body = json!({"site_access": "world"});
    let key = format!("clax_key_{}={}", ts.addr.port(), ts.token);
    // Neither that session nor the key cookie (the token itself) decides alone.
    for cookie in [&minted, &key] {
        let first = browser(&ts, cookie, Method::PATCH, &path, body.clone(), None).await;
        let id = pending(&ts, &first);
        let (s, v) = browser(
            &ts,
            cookie,
            Method::POST,
            &format!("/api/confirmations/{id}"),
            json!({"approve": true}),
            None,
        )
        .await;
        assert_eq!((s, code(&v)), (403, "browser_required"), "{cookie}");
        let (s, v) = token(
            &ts,
            Method::POST,
            &format!("/api/confirmations/{id}"),
            json!({"approve": true}),
            None,
        )
        .await;
        assert_eq!((s, code(&v)), (403, "browser_required"));
        assert!(!site_is_open(&ts, &aid).await);
    }
    // Nor does the extension, whose credential a token holder can mint.
    let id = pending(&ts, &open_world(&ts, &aid, None).await);
    let ext = ts.extension().await;
    let res = ext
        .post(
            &format!("/api/confirmations/{id}"),
            json!({"approve": true}),
        )
        .await;
    assert_ne!(res.status(), 200);
    assert!(!site_is_open(&ts, &aid).await);
    // The browser opened before decides without asking.
    let (s, v) = browser(&ts, &clean, Method::PATCH, &path, body, None).await;
    assert_eq!(s, 200, "{v}");
    assert!(site_is_open(&ts, &aid).await);
}

/// Decision 5 already needs the owner's browser; a session minted while an
/// agent was live needs a confirmation besides.
#[tokio::test]
async fn people_decisions_from_a_session_minted_while_an_agent_was_live_ask() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let mia = ts.person("Mia").await;
    let pending_grant = {
        let c = TestClient::new(&ts);
        let r = c
            .grant_request(json!({"access_token": {"access": ["clax:reader"]}}))
            .await;
        (
            c.grant_id(),
            r["interact"]["user_code"].as_str().unwrap().to_string(),
        )
    };
    ts.register_session("claude", "c-6").await;
    let minted = login_now(&ts).await;
    let rights = format!("/api/people/{}/rights", mia.public_id);
    let first = browser(
        &ts,
        &minted,
        Method::PUT,
        &rights,
        json!({"rights": "clax:reader"}),
        None,
    )
    .await;
    let id = pending(&ts, &first);
    assert_eq!(first.1["decision"], "change a person's rights");
    ts.confirm(&id, true).await;
    let (s, v) = browser(
        &ts,
        &minted,
        Method::PUT,
        &rights,
        json!({"rights": "clax:reader"}),
        Some(&id),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let revoke = format!("/api/people/{}", mia.public_id);
    pending(
        &ts,
        &browser(&ts, &minted, Method::DELETE, &revoke, json!(null), None).await,
    );
    let (gid, user_code) = pending_grant;
    let approve = format!("/api/grants/{gid}/approve");
    let first = browser(
        &ts,
        &minted,
        Method::POST,
        &approve,
        json!({"user_code": user_code, "name": "Sam"}),
        None,
    )
    .await;
    assert_eq!(pending(&ts, &first).len(), 35);
    // The token alone is refused as before.
    let (s, v) = token(&ts, Method::DELETE, &revoke, json!(null), None).await;
    assert_eq!((s, code(&v)), (403, "browser_required"));
    // From the owner's own browser, none asks.
    let (s, v) = browser(
        &ts,
        &ts.owner_cookie(),
        Method::DELETE,
        &revoke,
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn connecting_a_remote_and_widening_its_public_copy_ask_first() {
    let ts = TestServer::spawn().await;
    ts.register_session("claude", "c-7").await;
    // Asked before any request leaves for the remote.
    let first = token(
        &ts,
        Method::POST,
        "/api/remotes",
        json!({"url": "https://clax.example"}),
        None,
    )
    .await;
    let id = pending(&ts, &first);
    assert_eq!(
        first.1["decision"],
        "connect this Clax to https://clax.example"
    );
    let link = "/api/remotes/r1/artifacts/7q3k9mzx2b4t/sharing/link";
    let first = token(&ts, Method::PUT, link, json!({"access": "commenter"}), None).await;
    assert_ne!(pending(&ts, &first), id);
    // Narrowing asks no one (and fails here for the remote it names).
    let (s, v) = token(&ts, Method::PUT, link, json!({"access": null}), None).await;
    assert_eq!((s, code(&v)), (404, "no_remote"), "{v}");
}

#[tokio::test]
async fn a_token_push_waits_for_the_browser_while_an_agent_is_live() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    remotes::connect(&local, &public).await;
    let aid = page(&local, "To push").await;
    local.register_session("claude", "c-8").await;
    let path = format!("/api/artifacts/{aid}/push");
    let first = token(&local, Method::POST, &path, json!({}), None).await;
    let id = pending(&local, &first);
    assert!(
        first.1["decision"]
            .as_str()
            .unwrap()
            .starts_with("push “To push” to http://localhost:"),
        "{}",
        first.1
    );
    local.confirm(&id, true).await;
    // The approved push, resent asking for standing push power, is another decision.
    let (s, v) = token(
        &local,
        Method::POST,
        &path,
        json!({"auto": true}),
        Some(&id),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "confirmation_mismatch"), "{v}");
    let (s, v) = token(&local, Method::POST, &path, json!({}), Some(&id)).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("pushed")), "{v}");
}
