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

async fn poll(ts: &TestServer, sid: &str, q: &str) -> Value {
    let r = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?{q}"))
        .await;
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

#[tokio::test]
async fn the_prompt_hooks_poll_records_the_prompt_and_says_where_to_ask() {
    let (_c, ts) = clocked().await;
    let (sid, _aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let v = poll(&ts, &sid, "tier=prompt_hook&prompt=true").await;
    assert_eq!(v["where_to_ask"]["surface"], "terminal");
    assert!(
        v["text"].as_str().unwrap().contains(
            "[clax] If you ask the person anything now, ask in the terminal (chat) (clear): "
        ),
        "{v}"
    );
    assert_eq!(
        where_to_ask_reply(&ts, &sid).await["signals"]["terminal_at"],
        "2026-10-16T10:00:00.000Z"
    );
    // Without the flag (SessionStart's poll) nothing is recorded or said.
    let fresh = ts.register_session("claude", "h2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = poll(&ts, &fresh, "tier=prompt_hook").await;
    assert!(
        v["text"].is_null() && v.get("where_to_ask").is_none(),
        "{v}"
    );
    assert!(where_to_ask_reply(&ts, &fresh).await["signals"]["terminal_at"].is_null());
}

#[tokio::test]
async fn the_prompt_hooks_poll_of_a_session_with_no_page_records_the_prompt_only() {
    let (_c, ts) = clocked().await;
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = poll(&ts, &sid, "tier=prompt_hook&prompt=true").await;
    assert!(
        v["text"].is_null() && v.get("where_to_ask").is_none(),
        "{v}"
    );
    assert_eq!(
        where_to_ask_reply(&ts, &sid).await["signals"]["terminal_at"],
        "2026-10-16T10:00:00.000Z"
    );
}

#[tokio::test]
async fn a_codex_turn_with_no_delivery_counts_as_an_inferred_prompt() {
    let (c, ts) = clocked().await;
    let sid = ts.register_session("codex", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a = ts
        .publish_as(&sid, "Board", "<main><h2>Goals</h2></main>")
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let v = poll(&ts, &sid, "tier=stop_hook").await;
    assert!(
        v["text"].is_null(),
        "a Stop with nothing to hand over still ends the turn: {v}"
    );
    let a = where_to_ask_reply(&ts, &sid).await;
    assert_eq!(a["signals"]["terminal_inferred"], true);
    assert_eq!(says(&a), ("clax", "unsure"));
    // A turn that a thread from Clax started is not a terminal turn.
    c.advance(30);
    ts.thread_as_owner(&aid, "Bigger?").await;
    poll(&ts, &sid, "tier=piggyback").await;
    c.advance(30);
    poll(&ts, &sid, "tier=stop_hook").await;
    assert_eq!(
        where_to_ask_reply(&ts, &sid).await["signals"]["terminal_at"],
        "2026-10-16T10:00:00.000Z"
    );
    // A claude session's Stop infers nothing: its prompts are reported.
    let (claude, _) = paged(&ts, "h2").await;
    poll(&ts, &claude, "tier=stop_hook").await;
    assert!(where_to_ask_reply(&ts, &claude).await["signals"]["terminal_at"].is_null());
}

#[tokio::test]
async fn a_grok_turn_a_stop_handed_a_thread_to_is_not_a_terminal_turn() {
    let (c, ts) = clocked().await;
    let sid = ts.register_session("grok", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a = ts
        .publish_as(&sid, "Board", "<main><h2>Goals</h2></main>")
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let tid = ts.thread_as_owner(&aid, "Bigger?").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The first Stop ends a turn nothing from Clax started, and hands the
    // thread over: the agent goes on with it.
    let v = poll(&ts, &sid, "tier=stop_hook").await;
    assert_eq!(v["feedback"].as_array().unwrap().len(), 1, "{v}");
    assert!(
        v["text"]
            .as_str()
            .unwrap()
            .contains("ask in Clax, in thread"),
        "{v}"
    );
    assert_eq!(v["where_to_ask"]["where"]["thread_id"], tid.as_str());
    assert_eq!(
        where_to_ask_reply(&ts, &sid).await["signals"]["terminal_at"],
        "2026-10-16T10:00:00.000Z"
    );
    // The Stop that ends the thread's turn is no terminal turn.
    c.advance(30);
    let v = poll(&ts, &sid, "tier=stop_hook").await;
    assert!(v["text"].is_null(), "{v}");
    assert_eq!(
        where_to_ask_reply(&ts, &sid).await["signals"]["terminal_at"],
        "2026-10-16T10:00:00.000Z"
    );
    // The turn after that, with nothing from Clax, is the person's again.
    c.advance(30);
    poll(&ts, &sid, "tier=stop_hook").await;
    assert_eq!(
        where_to_ask_reply(&ts, &sid).await["signals"]["terminal_at"],
        "2026-10-16T10:01:00.000Z"
    );
}

#[tokio::test]
async fn a_late_answer_and_a_pull_are_deliveries_from_clax_at_their_thread() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let t = ts.thread_as_owner(&aid, "Which?").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let asked = json!({"source": "ask", "in_terminal": true, "artifact_id": aid, "thread_id": tid,
        "questions": [{"question": "Which layout?", "header": "Layout"}]});
    let qid = ts.ask(&sid, asked).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    typed(&ts, &sid).await;
    c.advance(120);
    let r = ts
        .client
        .post(format!("{}/api/questions/{qid}/pull", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v = poll(&ts, &sid, "tier=piggyback").await;
    let text = v["text"].as_str().unwrap();
    assert!(
        text.contains(&format!("[clax] The person wants your question \"Layout\" ({qid}) asked in Clax instead: ask it now with `ask` with `url_or_id` \"{aid}\" and `thread_id` \"{tid}\"")),
        "{text}"
    );
    assert!(!text.contains("answered your question"), "{text}");
    assert!(
        !text.contains("Esc"),
        "an `ask` question has no terminal dialog: {text}"
    );
    assert!(text.contains("ask in Clax, in thread"), "{text}");
    assert_eq!(v["where_to_ask"]["where"]["thread_id"], tid.as_str());
    assert_eq!(v["answers"][0]["pulled"], true, "{v}");
}

#[tokio::test]
async fn a_pulled_hook_question_says_its_terminal_dialog_stays_until_esc() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    let hook = json!({"source": "hook", "tool_use_id": "tu1", "questions": [{"question": "Which layout?",
        "header": "Layout", "multiSelect": false, "options": [{"label": "Grid", "description": "g"},
        {"label": "List", "description": "l"}]}]});
    let created = ts.ask(&sid, hook).await;
    assert_eq!(created["mode"], "terminal", "{created}");
    let qid = created["question"]["id"].as_str().unwrap().to_string();
    c.advance(120);
    let r = ts
        .client
        .post(format!("{}/api/questions/{qid}/pull", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let v = poll(&ts, &sid, "tier=piggyback").await;
    let text = v["text"].as_str().unwrap();
    assert!(
        text.contains(&format!(
            "[clax] The person wants your question \"Layout\" ({qid}) asked in Clax instead: ask it now with `ask` with `url_or_id` \"{aid}\""
        )),
        "{text}"
    );
    assert!(
        text.contains(
            "The terminal's dialog for it stays open until the person presses Esc there."
        ),
        "{text}"
    );
    // Ask me here is the person's act in Clax, newer than the prompt.
    assert_eq!(
        (
            v["where_to_ask"]["surface"].as_str(),
            v["where_to_ask"]["confidence"].as_str()
        ),
        (Some("clax"), Some("clear")),
        "{v}"
    );
}

#[tokio::test]
async fn a_late_answer_is_the_persons_act_in_clax_at_its_answer() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let asked = json!({"source": "ask", "artifact_id": aid,
        "questions": [{"question": "Which layout?", "header": "Layout",
            "options": [{"label": "Grid"}, {"label": "List"}]}]});
    let qid = ts.ask(&sid, asked).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    typed(&ts, &sid).await;
    c.advance(120);
    ts.answer_question(&qid, json!({"answers": [{"selected": ["Grid"]}]}))
        .await;
    let v = poll(&ts, &sid, "tier=piggyback").await;
    let text = v["text"].as_str().unwrap();
    assert!(
        text.contains("The person answered your question \"Layout\""),
        "{text}"
    );
    assert!(text.ends_with(&clax_line(&v)), "{text}");
    assert_eq!(says(&v), ("clax", "clear"), "{v}");
    assert_eq!(v["where_to_ask"]["where"], json!({"artifact_id": aid}));
    assert_eq!(
        where_to_ask_reply(&ts, &sid).await["signals"]["delivered_at"],
        "2026-10-16T10:02:00.000Z"
    );
}

/// The where-to-ask line a poll's reply `v` ends its text with.
fn clax_line(v: &Value) -> String {
    let w = &v["where_to_ask"];
    let at = match w["where"]["thread_id"].as_str() {
        Some(t) => format!("in Clax, in thread {t}"),
        None if w["surface"] == "clax" => "in Clax".to_string(),
        None => "in the terminal (chat)".to_string(),
    };
    format!(
        "[clax] If you ask the person anything now, ask {at} ({}): {}",
        w["confidence"].as_str().unwrap(),
        w["why"].as_str().unwrap()
    )
}

/// Runs `clax hook --agent claude <event>` with Claude Code's JSON for
/// harness session `hsid` on stdin, against `ts` (found through the
/// `daemon.json` this writes): what it printed on stdout.
async fn claude_hook(ts: &TestServer, event: &str, name: &str, hsid: &str) -> String {
    use tokio::io::AsyncWriteExt;
    let info = clax_core::daemon::DaemonInfo {
        port: ts.addr.port(),
        pid: std::process::id(),
        token: ts.token.clone(),
        started_at: ts.store.now(),
        bind: "127.0.0.1".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        exe: None,
        commit: None,
    };
    clax_core::daemon::write_daemon_info(&ts.home, &info).unwrap();
    let scratch = ts.home.root().join("hook-scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let mut child = tokio::process::Command::new(clax_fake_exe::clax_bin(None))
        .args(["hook", "--agent", "claude", event])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &scratch)
        .env("TMPDIR", &scratch)
        .env("CLAX_HOME", ts.home.root())
        .env("CLAX_NO_OPEN", "1")
        .env("CLAX_CODEX_BIN", "")
        .env("RUST_LOG", "error")
        .current_dir(&scratch)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn clax");
    let input = json!({"session_id": hsid, "hook_event_name": name, "prompt": "Carry on.",
        "cwd": scratch, "stop_hook_active": false});
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.to_string().as_bytes()).await.unwrap();
    drop(stdin);
    // A guard against a hung client, not a budget.
    let out = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
        .await
        .expect("clax hook finished")
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test]
async fn claude_codes_prompt_hook_records_the_prompt_and_lapses_an_in_terminal_question() {
    let (c, ts) = clocked().await;
    let (sid, _aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    let hook = json!({"source": "hook", "tool_use_id": "tu1", "questions": [{"question": "Which layout?",
        "header": "Layout", "multiSelect": false, "options": [{"label": "Grid", "description": "g"},
        {"label": "List", "description": "l"}]}]});
    let created = ts.ask(&sid, hook).await;
    assert_eq!(
        (
            created["mode"].as_str(),
            created["question"]["in_terminal"].as_bool()
        ),
        (Some("terminal"), Some(true)),
        "{created}"
    );
    let qid = created["question"]["id"].as_str().unwrap().to_string();
    c.advance(60);
    let out = claude_hook(&ts, "prompt", "UserPromptSubmit", "h1").await;
    assert_eq!(
        where_to_ask_reply(&ts, &sid).await["signals"]["terminal_at"],
        "2026-10-16T10:01:00.000Z"
    );
    let q = ts.store.question(&qid).unwrap().unwrap();
    assert_eq!(
        format!("{:?}", q.status),
        "Withdrawn",
        "the prompt lapses the open hook in-terminal question"
    );
    assert!(
        out.contains(
            "[clax] If you ask the person anything now, ask in the terminal (chat) (clear): "
        ),
        "the prompt's context says where to ask: {out}"
    );
}
