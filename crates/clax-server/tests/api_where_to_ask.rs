//! Where an agent should ask its person (spec
//! 2026-10-16-ask-where-attention-is-design §1): the session's signals,
//! the where-to-ask route, terminal activity, and deliveries.

use clax_testkit::TestServer;
use clax_testkit::where_to_ask::{
    browser_of, clocked, clocked_with_sam, owner_tab, paged, person_paged, stream_as, tab_as,
    typed, typed_as, where_to_ask_as, where_to_ask_reply,
};
use serde_json::{Value, json};

fn says(v: &Value) -> (&str, &str) {
    (
        v["where_to_ask"]["surface"].as_str().unwrap(),
        v["where_to_ask"]["confidence"].as_str().unwrap(),
    )
}

#[tokio::test]
async fn with_no_clax_open_the_terminal_is_clear() {
    let (_c, ts) = clocked().await;
    let (sid, _aid) = paged(&ts, "h1").await;
    let v = where_to_ask_reply(&ts, &sid).await;
    assert_eq!(says(&v), ("terminal", "clear"));
    assert!(
        v["where_to_ask"]["why"]
            .as_str()
            .unwrap()
            .contains("No Clax tab or panel")
    );
}

#[tokio::test]
async fn a_hidden_tab_is_not_engagement() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    // A tab holding `questions` while hidden: it can show a question, and
    // nothing more.
    let _hidden = ts.stream_as_owner(&["questions"]).await;
    let t = typed(&ts, &sid).await;
    assert_eq!(t["terminal_at"], "2026-10-16T10:00:00.000Z");
    let v = where_to_ask_reply(&ts, &sid).await;
    assert_eq!(says(&v), ("terminal", "clear"));
    assert_eq!(v["where_to_ask"]["where"]["artifact_id"], aid.as_str());
    assert_eq!(v["signals"]["terminal_at"], "2026-10-16T10:00:00.000Z");
    c.advance(clax_core::surface::QUIET_SECS + 1);
    assert_eq!(
        says(&where_to_ask_reply(&ts, &sid).await),
        ("clax", "unsure"),
        "quiet leans to Clax"
    );
}

#[tokio::test]
async fn focused_input_newer_than_the_prompt_suggests_clax_at_the_sessions_page() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    c.advance(120);
    owner_tab(&ts, &aid, "t1", true, 0).await;
    let v = where_to_ask_reply(&ts, &sid).await;
    assert_eq!(says(&v), ("clax", "clear"));
    assert_eq!(v["where_to_ask"]["where"], json!({"artifact_id": aid}));
    // Within a minute of the prompt is too close to tell.
    c.advance(10);
    typed(&ts, &sid).await;
    assert_eq!(
        says(&where_to_ask_reply(&ts, &sid).await),
        ("clax", "unsure")
    );
}

#[tokio::test]
async fn focus_on_another_artifact_is_not_engagement_on_the_sessions_page() {
    let (c, ts) = clocked().await;
    let (sid, _aid) = paged(&ts, "h1").await;
    let (_other_sid, other) = paged(&ts, "h2").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    c.advance(120);
    owner_tab(&ts, &other, "t2", true, 0).await;
    assert_eq!(
        says(&where_to_ask_reply(&ts, &sid).await),
        ("terminal", "clear")
    );
}

#[tokio::test]
async fn a_sent_thread_places_the_suggestion_in_that_thread() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    c.advance(120);
    let t = ts.thread_as_owner(&aid, "Bigger title?").await;
    let tid = t["id"].as_str().unwrap();
    let r = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback"))
        .await;
    assert_eq!(r.status(), 200);
    let v = where_to_ask_reply(&ts, &sid).await;
    assert_eq!(says(&v), ("clax", "clear"));
    assert_eq!(
        v["where_to_ask"]["where"],
        json!({"thread_id": tid, "artifact_id": aid})
    );
    assert_eq!(v["signals"]["delivered_at"], "2026-10-16T10:02:00.000Z");
}

#[tokio::test]
async fn a_deleted_delivered_thread_places_the_suggestion_on_its_artifact() {
    let (_c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let t = ts.thread_as_owner(&aid, "Gone soon").await;
    let tid = t["id"].as_str().unwrap().to_string();
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback"))
        .await;
    ts.delete_thread_as_owner(&aid, &tid).await;
    let v = where_to_ask_reply(&ts, &sid).await;
    assert_eq!(v["where_to_ask"]["where"], json!({"artifact_id": aid}));
}

#[tokio::test]
async fn an_ended_or_unknown_session_has_no_where_to_ask() {
    let (_c, ts) = clocked().await;
    let (sid, _aid) = paged(&ts, "h1").await;
    ts.end_session(&sid).await;
    let r = ts
        .get_authed(&format!("/api/sessions/{sid}/where-to-ask"))
        .await;
    assert_eq!(r.status(), 400);
    let r = ts
        .post_json(&format!("/api/sessions/{sid}/terminal"), json!({}))
        .await;
    assert_eq!(r.status(), 400);
    let r = ts.get_authed("/api/sessions/s_nope/where-to-ask").await;
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn a_persons_agent_needs_its_persons_own_clax_open() {
    let ts = TestServer::spawn_on_with_shutdown("0.0.0.0".parse().unwrap()).await;
    let sam = ts.connect_person("Sam", "clax:full").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The owner has Clax open; Sam does not.
    let _owners = ts.stream_as_owner(&["questions"]).await;
    let r = sam
        .agent(
            "POST",
            &format!("/api/sessions/{sid}/terminal"),
            Some(&sid),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        r.status(),
        200,
        "a signed request from another machine is stamped here"
    );
    let r = sam
        .agent(
            "GET",
            &format!("/api/sessions/{sid}/where-to-ask"),
            Some(&sid),
            None,
        )
        .await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(says(&v), ("terminal", "clear"));
    assert!(
        v["where_to_ask"]["why"]
            .as_str()
            .unwrap()
            .contains("No Clax tab or panel"),
        "{v}"
    );
}

#[tokio::test]
async fn a_persons_agent_is_never_shown_a_page_it_may_not_read() {
    let ts = TestServer::spawn_on_with_shutdown("0.0.0.0".parse().unwrap()).await;
    let a = ts.publish("Owner's", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let sam = ts.connect_person("Sam", "clax:full").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = sam
        .agent(
            "PUT",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            None,
        )
        .await;
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    // The owner narrows Sam to his own space: the watched page is no longer his to read.
    let sam_public = ts.store.viewer_of_grant(&sam.grant_id()).unwrap().public_id;
    let r = ts
        .client
        .put(format!("{}/api/people/{sam_public}/rights", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"rights": [
            {"type": "clax-artifacts", "actions": ["read"], "locations": ["own"]},
            {"type": "clax-ask", "actions": ["ask"]}
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let r = sam
        .agent(
            "GET",
            &format!("/api/sessions/{sid}/where-to-ask"),
            Some(&sid),
            None,
        )
        .await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert!(
        v["where_to_ask"]["where"].get("artifact_id").is_none(),
        "{v}"
    );
}

#[tokio::test]
async fn a_persons_agent_reads_its_persons_engagement_not_the_owners() {
    let (c, ts, sam) = clocked_with_sam().await;
    let (sid, aid) = person_paged(&sam, "h1").await;
    let sams = browser_of(&ts, &sam);
    let _stream = stream_as(&ts, &sams, &["questions"]).await;
    typed_as(&sam, &sid).await;
    c.advance(120);
    // The owner, focused on Sam's page, is not Sam.
    owner_tab(&ts, &aid, "o1", true, 0).await;
    let v = where_to_ask_as(&sam, &sid).await;
    assert_eq!(says(&v), ("terminal", "clear"), "{v}");
    tab_as(&ts, &sams, &aid, "s1", true, 0).await;
    let v = where_to_ask_as(&sam, &sid).await;
    assert_eq!(says(&v), ("clax", "clear"), "{v}");
    assert_eq!(v["where_to_ask"]["where"], json!({"artifact_id": aid}));
}

#[tokio::test]
async fn the_daemon_token_never_reads_a_persons_focus_or_input() {
    let (c, ts, sam) = clocked_with_sam().await;
    let (sid, aid) = person_paged(&sam, "h1").await;
    let sams = browser_of(&ts, &sam);
    let _stream = stream_as(&ts, &sams, &["questions", "inbox"]).await;
    typed_as(&sam, &sid).await;
    c.advance(120);
    tab_as(&ts, &sams, &aid, "s1", true, 0).await;
    assert_eq!(says(&where_to_ask_as(&sam, &sid).await), ("clax", "clear"));
    // Only the person's own agent reads where they are: the owner's tools
    // are refused a session of another credential's.
    let r = ts
        .get_authed(&format!("/api/sessions/{sid}/where-to-ask"))
        .await;
    assert_eq!(r.status(), 403);
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"]["code"],
        "foreign_session"
    );
    let r = ts
        .post_json(&format!("/api/sessions/{sid}/terminal"), json!({}))
        .await;
    assert_eq!(r.status(), 403);
}

#[tokio::test]
async fn a_thread_taken_late_counts_when_it_was_written_not_when_it_was_taken() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let t = ts.thread_as_owner(&aid, "Bigger title?").await;
    let tid = t["id"].as_str().unwrap().to_string();
    c.advance(19 * 60);
    typed(&ts, &sid).await;
    c.advance(60);
    let r = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback"))
        .await;
    assert_eq!(r.status(), 200);
    let v = where_to_ask_reply(&ts, &sid).await;
    assert_eq!(v["signals"]["delivered_at"], "2026-10-16T10:20:00.000Z");
    // The prompt at 10:19 is newer than the comment of 10:00; the thread,
    // taken at 10:20, still instigates the work.
    assert_eq!(says(&v), ("terminal", "clear"), "{v}");
    assert_eq!(
        v["where_to_ask"]["where"],
        json!({"thread_id": tid, "artifact_id": aid})
    );
}

#[tokio::test]
async fn a_thread_another_author_wrote_places_the_question_but_is_not_the_persons_activity() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    c.advance(120);
    // A LAN viewer, not the owner, writes on the page; the owner sends it on.
    let lucy = ts.viewer(Some("Lucy")).await;
    let tid = ts.thread_as(&aid, &lucy.cookie, "The chart is wrong").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/send",
            ts.base
        ))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let r = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback"))
        .await;
    assert_eq!(r.status(), 200);
    let v = where_to_ask_reply(&ts, &sid).await;
    assert_eq!(
        v["signals"]["delivered_at"], "2026-10-16T10:02:00.000Z",
        "{v}"
    );
    assert_eq!(says(&v), ("terminal", "clear"), "{v}");
    assert_eq!(
        v["where_to_ask"]["where"],
        json!({"thread_id": tid, "artifact_id": aid})
    );
}
