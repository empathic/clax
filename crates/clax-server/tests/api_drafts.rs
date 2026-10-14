//! Agents' drafts (hosted spec 2026-10-11-clax-dev-hosted-design §16.4):
//! the draft routes' rights and limits, the draft URL, and the `drafts`
//! events people's streams carry.

use crate::common;
use crate::common::people::World;
use clax_core::working::ManualClock;
use clax_server::testing::EventReader;
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

const BASE: &str = "<!doctype html><html><head><title>Plan</title><link rel=stylesheet href=style.css></head><body><h1>Quarterly plan</h1><p>Ship the beta</p><p>Wording only the base has</p></body></html>";
const DRAFT: &str = "<!doctype html><html><head><title>Plan</title><link rel=stylesheet href=style.css></head><body><h1>Quarterly plan</h1><p>Ship the beta in May</p></body></html>";
const CSS: &str = "h1 { color: teal }";

fn utf8(s: &str) -> Value {
    json!({"content": s, "encoding": "utf8"})
}

async fn server() -> (TestServer, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    (TestServer::spawn_with_clock(clock.clone()).await, clock)
}

/// A claude session and an artifact it made, with a page and a stylesheet.
async fn setup(ts: &TestServer) -> (String, String) {
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .authed(ts.client.post(format!("{}/api/artifacts", ts.base)))
        .header("x-clax-session", &sid)
        .json(
            &json!({"title": "Plan", "files": {"index.html": utf8(BASE), "style.css": utf8(CSS)}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let aid = r.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    (sid, aid)
}

async fn put(ts: &TestServer, sid: &str, aid: &str, body: Value) -> reqwest::Response {
    ts.authed(
        ts.client
            .put(format!("{}/api/sessions/{sid}/drafts/{aid}", ts.base)),
    )
    .json(&body)
    .send()
    .await
    .unwrap()
}

async fn put_ok(ts: &TestServer, sid: &str, aid: &str, html: &str) -> Value {
    let r = put(ts, sid, aid, json!({"files": {"index.html": utf8(html)}})).await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    r.json().await.unwrap()
}

/// The live drafts as the owner's browser lists them.
async fn listed(ts: &TestServer, aid: &str) -> Vec<Value> {
    let r = ts
        .client
        .get(format!("{}/api/artifacts/{aid}/drafts", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    r.json::<Value>().await.unwrap()["drafts"]
        .as_array()
        .unwrap()
        .clone()
}

fn draft_url(aid: &str, d: &Value, path: &str) -> String {
    format!(
        "/c/{aid}/draft/{}/{}/{path}",
        d["agent"].as_str().unwrap(),
        d["token"].as_str().unwrap()
    )
}

async fn code(r: reqwest::Response) -> (u16, Value) {
    let s = r.status().as_u16();
    (s, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn people_list_a_draft_and_its_url_serves_marked_pages_over_the_base() {
    let (ts, _) = server().await;
    let (sid, aid) = setup(&ts).await;
    let r = put(
        &ts,
        &sid,
        &aid,
        json!({"files": {"index.html": utf8(DRAFT)}, "note": " Rewording\n the plan ", "if_version": 1}),
    )
    .await;
    assert_eq!(r.status(), 200);
    let answered: Value = r.json().await.unwrap();
    assert_eq!(answered["draft"]["base_version"], 1);
    assert_eq!(answered["draft"]["paths"], json!(["index.html"]));
    assert_eq!(answered["draft"]["note"], "Rewording the plan");
    let drafts = listed(&ts, &aid).await;
    assert_eq!(drafts.len(), 1);
    let d = &drafts[0];
    assert_eq!(d["harness"], "claude");
    assert!(d["agent"].as_str().unwrap().starts_with("a_"), "{d}");
    assert_eq!(d["base_version"], 1);
    assert_eq!(d["note"], "Rewording the plan");
    let token = d["token"].as_str().unwrap();
    assert_eq!(token.len(), 22);
    // The agent is never handed the capability; the listing never names its session.
    assert!(!answered.to_string().contains(token), "{answered}");
    assert!(!d.to_string().contains(&sid), "{d}");

    let r = ts.get(&draft_url(&aid, d, "")).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["cache-control"], "no-store");
    assert_eq!(r.headers()["x-content-type-options"], "nosniff");
    // The page's requests never carry the token in Referer.
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    assert!(
        r.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .starts_with("sandbox allow-scripts"),
        "{:?}",
        r.headers()
    );
    assert!(
        r.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let page = r.text().await.unwrap();
    assert!(
        page.contains("<p data-clax-draft-changed>Ship the beta in May</p>"),
        "{page}"
    );
    assert!(page.contains("<h1>Quarterly plan</h1>"), "{page}");
    assert!(page.contains("clax:draft-diff"), "{page}");
    assert!(!page.contains("Wording only the base has"), "{page}");
    // Not wrapped: a draft page never greets the shell.
    assert!(!page.contains("/_clax/bridge"), "{page}");
    assert_eq!(
        ts.get(&draft_url(&aid, d, "index.html"))
            .await
            .text()
            .await
            .unwrap(),
        page
    );
    // A path the draft does not write comes from the base version.
    let css = ts.get(&draft_url(&aid, d, "style.css")).await;
    assert_eq!(css.status(), 200);
    assert_eq!(css.headers()["cache-control"], "no-store");
    assert_eq!(css.headers()["referrer-policy"], "no-referrer");
    assert_eq!(css.text().await.unwrap(), CSS);
    assert_eq!(
        ts.get(&draft_url(&aid, d, "missing.css")).await.status(),
        404
    );
    // The root without its slash goes to it.
    let bare = draft_url(&aid, d, "");
    let r = ts.get(bare.trim_end_matches('/')).await;
    assert_eq!(r.status(), 200);
    // A wrong token, a malformed one, and another agent's handle answer 404.
    let wrong = format!(
        "/c/{aid}/draft/{}/{}/",
        d["agent"].as_str().unwrap(),
        "A".repeat(22)
    );
    assert_eq!(ts.get(&wrong).await.status(), 404);
    let short = format!("/c/{aid}/draft/{}/abc/", d["agent"].as_str().unwrap());
    assert_eq!(ts.get(&short).await.status(), 404);
    let other = format!("/c/{aid}/draft/a_someoneelse/{token}/");
    assert_eq!(ts.get(&other).await.status(), 404);
    // On the artifact's own origin it is framed by the shell alone.
    let r = ts
        .client
        .get(format!(
            "{}/draft/{}/{token}/",
            ts.base,
            d["agent"].as_str().unwrap()
        ))
        .header("host", format!("{aid}.localhost:4321"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers()["content-security-policy"],
        "frame-ancestors 'self' http://localhost:4321 http://127.0.0.1:4321"
    );
}

#[tokio::test]
async fn a_revision_gets_a_new_token_and_the_old_url_stops_answering() {
    let (ts, clock) = server().await;
    let (sid, aid) = setup(&ts).await;
    put_ok(&ts, &sid, &aid, DRAFT).await;
    let first = listed(&ts, &aid).await[0].clone();
    clock.advance(1);
    put_ok(&ts, &sid, &aid, "<p>Second take</p>").await;
    let second = listed(&ts, &aid).await[0].clone();
    assert_ne!(first["token"], second["token"]);
    assert_eq!(ts.get(&draft_url(&aid, &first, "")).await.status(), 404);
    let page = ts
        .get(&draft_url(&aid, &second, ""))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        page.contains("<p data-clax-draft-changed>Second take</p>"),
        "{page}"
    );
    // A path the draft removes is gone from it.
    clock.advance(1);
    let r = put(
        &ts,
        &sid,
        &aid,
        json!({"files": {"index.html": utf8(DRAFT), "style.css": null}}),
    )
    .await;
    assert_eq!(r.status(), 200);
    let third = listed(&ts, &aid).await[0].clone();
    assert_eq!(third["paths"], json!(["index.html", "style.css"]));
    assert_eq!(
        ts.get(&draft_url(&aid, &third, "style.css")).await.status(),
        404
    );
}

#[tokio::test]
async fn people_hear_each_change_and_publishing_ends_the_draft() {
    let (ts, clock) = server().await;
    let (sid, aid) = setup(&ts).await;
    let mut stream = ts
        .stream_as_owner(&[&format!("working:{aid}"), &format!("artifact:{aid}")])
        .await;
    let mut old = ts.events(&format!("?artifact={aid}")).await;
    put_ok(&ts, &sid, &aid, DRAFT).await;
    let e = stream.next_named("drafts").await;
    assert_eq!(e["artifact_id"], aid.as_str());
    assert_eq!(e["topic"], format!("working:{aid}"));
    let d = &e["drafts"][0];
    assert_eq!(d["token"], listed(&ts, &aid).await[0]["token"]);
    assert!(!e.to_string().contains(&sid), "{e}");
    // Discarding ends it.
    let r = ts
        .authed(
            ts.client
                .delete(format!("{}/api/sessions/{sid}/drafts/{aid}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.json::<Value>().await.unwrap()["ended"], true);
    assert_eq!(stream.next_named("drafts").await["drafts"], json!([]));
    clock.advance(1);
    put_ok(&ts, &sid, &aid, DRAFT).await;
    let token = stream.next_named("drafts").await["drafts"][0]["token"].clone();
    // The session's publish lands the version and ends the draft.
    let r = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "files": {"index.html": utf8(DRAFT)}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    assert_eq!(stream.next_named("drafts").await["drafts"], json!([]));
    assert_eq!(stream.next_named("version").await["n"], 2);
    assert!(listed(&ts, &aid).await.is_empty());
    let gone = format!(
        "/c/{aid}/draft/{}/{}/",
        d["agent"].as_str().unwrap(),
        token.as_str().unwrap()
    );
    assert_eq!(ts.get(&gone).await.status(), 404);
    // `/api/events` never carries a draft: it reached the version without one.
    loop {
        let (name, _) = old.next().await;
        assert_ne!(name, "drafts");
        if name == "version" {
            break;
        }
    }
}

#[tokio::test]
async fn a_draft_ends_after_two_idle_minutes_and_with_its_session() {
    let (ts, clock) = server().await;
    let (sid, aid) = setup(&ts).await;
    let mut stream = ts.stream_as_owner(&[&format!("working:{aid}")]).await;
    put_ok(&ts, &sid, &aid, DRAFT).await;
    stream.next_named("drafts").await;
    clock.advance(clax_core::drafts::DRAFT_IDLE_SECS - 1);
    assert_eq!(listed(&ts, &aid).await.len(), 1);
    clock.advance(1);
    assert!(listed(&ts, &aid).await.is_empty());
    clax_server::drafts::sweep_and_announce(&ts.drafts, &ts.events);
    assert_eq!(stream.next_named("drafts").await["drafts"], json!([]));
    put_ok(&ts, &sid, &aid, DRAFT).await;
    assert_eq!(listed(&ts, &aid).await.len(), 1);
    ts.end_session(&sid).await;
    assert!(listed(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn drafts_are_capped_in_size_rate_and_base_version() {
    let (ts, clock) = server().await;
    let (sid, aid) = setup(&ts).await;
    let big = "x".repeat(clax_core::drafts::MAX_DRAFT_BYTES + 1);
    let (s, v) = code(
        put(
            &ts,
            &sid,
            &aid,
            json!({"files": {"index.html": utf8(&big)}}),
        )
        .await,
    )
    .await;
    assert_eq!((s, v["error"]["code"].as_str()), (413, Some("too_large")));
    let (s, v) = code(put(&ts, &sid, &aid, json!({"files": {}})).await).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_args"))
    );
    let (s, v) = code(put(&ts, &sid, &aid, json!({"files": {"../x.html": utf8("x")}})).await).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_path"))
    );
    let (s, v) = code(
        put(
            &ts,
            &sid,
            &aid,
            json!({"if_version": 2, "files": {"index.html": utf8(DRAFT)}}),
        )
        .await,
    )
    .await;
    assert_eq!((s, v["error"]["current"].clone()), (409, json!(1)));
    // None of those counted: two drafts a second pass, the third waits.
    put_ok(&ts, &sid, &aid, DRAFT).await;
    put_ok(&ts, &sid, &aid, DRAFT).await;
    let (s, v) = code(
        put(
            &ts,
            &sid,
            &aid,
            json!({"files": {"index.html": utf8(DRAFT)}}),
        )
        .await,
    )
    .await;
    assert_eq!(s, 429);
    assert_eq!(v["error"]["code"], "rate_limited");
    assert_eq!(v["error"]["retry_after_ms"], 1000);
    clock.advance(1);
    put_ok(&ts, &sid, &aid, DRAFT).await;
    // An ended session drafts no more.
    ts.end_session(&sid).await;
    clock.advance(1);
    let (s, v) = code(
        put(
            &ts,
            &sid,
            &aid,
            json!({"files": {"index.html": utf8(DRAFT)}}),
        )
        .await,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("unknown_session"))
    );
}

#[tokio::test]
async fn drafting_needs_the_publish_right_and_drafts_reach_people_only() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_on_with_clock("0.0.0.0".parse().unwrap(), clock).await;
    let (sid, aid) = setup(&ts).await;
    // No credential.
    let r = ts
        .client
        .put(format!("{}/api/sessions/{sid}/drafts/{aid}", ts.base))
        .json(&json!({"files": {"index.html": utf8(DRAFT)}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    // A reader's agent may not publish, so may not draft.
    let rae = ts.connect_person("Rae", "clax:reader").await;
    let rid = rae.register("claude", "rae-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let body = json!({"files": {"index.html": utf8(DRAFT)}});
    let r = rae
        .agent(
            "PUT",
            &format!("/api/sessions/{rid}/drafts/{aid}"),
            Some(&rid),
            Some(body.clone()),
        )
        .await;
    let (s, v) = code(r).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("not_permitted"))
    );
    assert_eq!(
        v["error"]["right"],
        json!({"type": "clax-artifacts", "action": "update"})
    );
    // A collaborator's agent updates only its person's artifacts.
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let ssid = sam.register("claude", "sam-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = sam
        .agent(
            "PUT",
            &format!("/api/sessions/{ssid}/drafts/{aid}"),
            Some(&ssid),
            Some(body.clone()),
        )
        .await;
    assert_eq!(code(r).await.0, 403);
    let r = sam
        .agent(
            "POST",
            "/api/artifacts",
            Some(&ssid),
            Some(json!({"title": "Sam's", "files": {"index.html": utf8(BASE)}})),
        )
        .await;
    assert_eq!(r.status(), 201);
    let own = r.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = sam
        .agent(
            "PUT",
            &format!("/api/sessions/{ssid}/drafts/{own}"),
            Some(&ssid),
            Some(body.clone()),
        )
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let d = listed(&ts, &own).await[0].clone();
    let r = sam
        .agent(
            "DELETE",
            &format!("/api/sessions/{ssid}/drafts/{own}"),
            Some(&ssid),
            None,
        )
        .await;
    assert_eq!(r.json::<Value>().await.unwrap()["ended"], true);
    // No agent lists drafts or opens a draft URL: the owner's with the
    // token naming its session, nor a person's.
    put_ok(&ts, &sid, &aid, DRAFT).await;
    let d2 = listed(&ts, &aid).await[0].clone();
    for path in [
        format!("/api/artifacts/{aid}/drafts"),
        draft_url(&aid, &d2, ""),
    ] {
        let r = ts
            .authed(ts.client.get(format!("{}{path}", ts.base)))
            .header("x-clax-session", &sid)
            .send()
            .await
            .unwrap();
        assert_eq!(code(r).await.1["error"]["code"], "not_for_agents", "{path}");
        let r = sam.agent("GET", &path, Some(&ssid), None).await;
        assert_eq!(r.status(), 403, "{path}");
    }
    assert_eq!(ts.get(&draft_url(&own, &d, "")).await.status(), 404);
}

#[tokio::test]
async fn a_draft_is_compared_only_with_base_files_its_agent_may_see() {
    let w = World::two_people().await;
    let (ts, aid, sid) = (&w.ts, &w.read.artifact, &w.owner_session);
    // Mia's page is withheld from the owner's agent: its draft of the page
    // is compared with nothing, so every element with text is marked.
    let r = put(
        ts,
        sid,
        aid,
        json!({"files": {"index.html": utf8("<h2>Quarterly goals</h2><p>Mine now</p>")}}),
    )
    .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let d = listed(ts, aid).await[0].clone();
    let page = ts.get(&draft_url(aid, &d, "")).await.text().await.unwrap();
    assert!(
        page.contains("<h2 data-clax-draft-changed>Quarterly goals</h2>"),
        "{page}"
    );
    assert!(
        page.contains("<p data-clax-draft-changed>Mine now</p>"),
        "{page}"
    );
    // Mia's files never come through the draft; the owner's agent's own do.
    for p in [common::people::FILE_NAME, common::people::CARRIED_NAME] {
        assert_eq!(ts.get(&draft_url(aid, &d, p)).await.status(), 404, "{p}");
    }
    let mine = ts.get(&draft_url(aid, &d, "owner.html")).await;
    assert_eq!(mine.status(), 200);
    assert_eq!(mine.text().await.unwrap(), "<p>mine</p>");
    for c in w.canaries() {
        assert!(!page.contains(c), "{c} in {page}");
    }
}

/// `/api/stream` opened by `client` (with `cookie` when given) and
/// subscribed to `topics`.
async fn stream_with(
    client: &reqwest::Client,
    base: &str,
    cookie: Option<&str>,
    topics: &[&str],
) -> EventReader {
    let with = |r: reqwest::RequestBuilder| match cookie {
        Some(c) => r.header("cookie", format!("clax_viewer={c}")),
        None => r,
    };
    let res = with(client.get(format!("{base}/api/stream")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut events = EventReader::from_response(res);
    let id = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let res = with(client.post(format!("{base}/api/stream/{id}")))
        .json(&json!({ "subscribe": topics }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    events
}

/// The events up to the next `working` one, by name.
async fn names_until_working(events: &mut EventReader) -> Vec<String> {
    let mut names = Vec::new();
    loop {
        let (name, _) = events.next().await;
        let done = name == "working";
        names.push(name);
        if done {
            return names;
        }
    }
}

#[tokio::test]
async fn drafts_reach_the_owner_and_connected_people_only_and_keep_the_agent_working() {
    let w = World::two_people().await;
    let (ts, aid, sid) = (&w.ts, &w.read.artifact, &w.owner_session);
    let topic = format!("working:{aid}");
    let (lan, base) = ts.lan();
    let lee = lan
        .get(format!("{base}/api/viewers/me"))
        .send()
        .await
        .unwrap();
    let lee_cookie = lee.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .and_then(|kv| kv.strip_prefix("clax_viewer="))
        .unwrap()
        .to_string();
    let mut mia = stream_with(&lan, &base, Some(&w.mia_cookie), &[&topic]).await;
    let mut lee = stream_with(&lan, &base, Some(&lee_cookie), &[&topic]).await;
    let mut nobody = stream_with(&lan, &base, None, &[&topic]).await;
    let mut owner = ts.stream_as_owner(&[&topic]).await;
    let r = put(ts, sid, aid, json!({"files": {"index.html": utf8(DRAFT)}})).await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    // A draft is announced before the working record it starts, so a
    // stream that heard `working` without `drafts` was never sent it.
    assert_eq!(names_until_working(&mut owner).await, ["drafts", "working"]);
    assert_eq!(names_until_working(&mut mia).await, ["drafts", "working"]);
    assert_eq!(names_until_working(&mut lee).await, ["working"]);
    assert_eq!(names_until_working(&mut nobody).await, ["working"]);
    // The listing answers the owner and Mia, and no one unconnected.
    assert_eq!(listed(ts, aid).await.len(), 1);
    let r = w
        .mia_browser(lan.get(format!("{base}/api/artifacts/{aid}/drafts")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<Value>().await.unwrap()["drafts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for cookie in [Some(lee_cookie.as_str()), None] {
        let mut req = lan.get(format!("{base}/api/artifacts/{aid}/drafts"));
        if let Some(c) = cookie {
            req = req.header("cookie", format!("clax_viewer={c}"));
        }
        let (s, v) = code(req.send().await.unwrap()).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (403, Some("forbidden")),
            "{cookie:?}"
        );
    }
    // The drafting agent is working on the artifact: the top bar does not call it idle.
    let handle = listed(ts, aid).await[0]["agent"].clone();
    assert!(
        ts.working
            .for_artifact(aid)
            .iter()
            .any(|r| json!(r.agent) == handle),
        "{:?}",
        ts.working.for_artifact(aid)
    );
}

#[tokio::test]
async fn a_base_page_past_the_comparison_budget_is_compared_with_nothing() {
    let (ts, clock) = server().await;
    let (sid, aid) = setup(&ts).await;
    let pad = "x".repeat(clax_server::routes::drafts::MAX_BASE_COMPARED as usize);
    let big = format!("<h1>Kept heading</h1><!-- {pad} -->");
    let r = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "files": {"index.html": utf8(&big), "small.html": utf8("<h1>Kept heading</h1>")}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    clock.advance(1);
    let r = put(
        &ts,
        &sid,
        &aid,
        json!({"files": {"index.html": utf8("<h1>Kept heading</h1>"), "small.html": utf8("<h1>Kept heading</h1>")}}),
    )
    .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let d = listed(&ts, &aid).await[0].clone();
    let page = ts.get(&draft_url(&aid, &d, "")).await.text().await.unwrap();
    assert!(
        page.contains("<h1 data-clax-draft-changed>Kept heading</h1>"),
        "{page}"
    );
    // A small page is still compared.
    let small = ts
        .get(&draft_url(&aid, &d, "small.html"))
        .await
        .text()
        .await
        .unwrap();
    assert!(small.starts_with("<h1>Kept heading</h1>"), "{small}");
}
