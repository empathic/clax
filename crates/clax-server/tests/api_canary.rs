//! What every route and every agent channel hands an agent (spec §7.5,
//! §8.5, §8.6, M7, M8): the routes that serve people refuse the owner's
//! agents, the all-threads listing is the owner's agents' only, and the
//! owner's question list holds the owner's agents' questions only.

use crate::common::TestServer;
use serde_json::{Value, json};

async fn status_code(res: reqwest::Response) -> (u16, String) {
    let status = res.status().as_u16();
    let v: Value = res.json().await.unwrap_or_default();
    (
        status,
        v["error"]["code"].as_str().unwrap_or_default().to_string(),
    )
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The routes of spec §7.5's `not_for_agents` rule that a `GET` reaches.
fn people_reads() -> Vec<String> {
    vec![
        "/api/inbox".into(),
        "/api/inbox/summary".into(),
        "/api/questions".into(),
        format!("/api/live/site?origin={}", enc("http://localhost:5173")),
        "/api/events".into(),
        "/api/stream".into(),
        "/api/people/invites".into(),
        "/api/grants".into(),
    ]
}

#[tokio::test]
async fn denied_routes_with_others_text_refuse_owner_agents() {
    let ts = TestServer::spawn().await;
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for path in people_reads() {
        let url = format!("{}{path}", ts.base);
        // The owner's agent: the token with the marker, or naming a session.
        let r = ts
            .authed(ts.client.get(&url))
            .header("x-clax-channel", "agent")
            .send()
            .await
            .unwrap();
        assert_eq!(
            status_code(r).await,
            (403, "not_for_agents".into()),
            "{path}: marker"
        );
        let r = ts
            .authed(ts.client.get(&url))
            .header("x-clax-session", &sid)
            .send()
            .await
            .unwrap();
        assert_eq!(
            status_code(r).await,
            (403, "not_for_agents".into()),
            "{path}: session"
        );
        // The CLI by hand: the token alone.
        let r = ts.authed(ts.client.get(&url)).send().await.unwrap();
        assert_eq!(r.status(), 200, "{path}: the token alone");
    }
    // The writes of those routes too.
    for (path, body) in [
        ("/api/inbox/read".to_string(), json!({"ids": []})),
        (
            "/api/questions/q/answer".to_string(),
            json!({"answers": {}}),
        ),
    ] {
        let r = ts
            .authed(ts.client.post(format!("{}{path}", ts.base)))
            .header("x-clax-channel", "agent")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            status_code(r).await,
            (403, "not_for_agents".into()),
            "{path}"
        );
    }
    // Denied routes that build no such view serve the owner's agents as
    // before.
    for path in [
        "/api/push".to_string(),
        "/healthz".into(),
        "/api/token".into(),
        format!("/api/live/pages?url={}", enc("http://localhost:5173/")),
    ] {
        let r = ts
            .authed(ts.client.get(format!("{}{path}", ts.base)))
            .header("x-clax-channel", "agent")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{path}");
    }
}

#[tokio::test]
async fn all_threads_listing_refuses_a_persons_agent() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:full").await;
    let r = sam.get(None, "/api/threads").await;
    assert_eq!(status_code(r).await, (403, "forbidden".into()));
    // The owner's agents and the CLI by hand list every thread.
    for marker in [false, true] {
        let mut req = ts.authed(ts.client.get(format!("{}/api/threads", ts.base)));
        if marker {
            req = req.header("x-clax-channel", "agent");
        }
        assert_eq!(req.send().await.unwrap().status(), 200);
    }
}

fn ask_body(artifact: Option<&str>) -> Value {
    let mut b = json!({"source": "ask", "questions": [{"question": "Which?", "header": "Pick",
        "options": [{"label": "A"}, {"label": "B"}]}]});
    if let Some(a) = artifact {
        b["artifact_id"] = json!(a);
    }
    b
}

#[tokio::test]
async fn the_owners_question_list_holds_only_the_owners_agents_questions() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let aid = ts.any_artifact().await;
    let mine = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let owners = ts.ask(&mine, ask_body(Some(&aid))).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = sam
        .agent(
            "POST",
            &format!("/api/sessions/{sid}/questions"),
            Some(&sid),
            Some(ask_body(Some(&aid))),
        )
        .await;
    assert_eq!(r.status(), 201);
    let v: Value = r.json().await.unwrap();
    let theirs = v["question"]["id"].as_str().unwrap().to_string();
    // The asking agent sees its question, never the artifact's title.
    assert_eq!(v["question"]["artifact"]["id"], json!(aid));
    assert_eq!(v["question"]["artifact"]["title"], Value::Null);
    let r = sam
        .get(
            Some(&sid),
            &format!("/api/sessions/{sid}/questions/{theirs}"),
        )
        .await;
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["question"]["id"], json!(theirs));
    assert_eq!(v["question"]["artifact"]["title"], Value::Null);
    // The owner's list and counts hold the owner's agents' questions only.
    let list: Value = ts
        .get_authed("/api/questions?status=all")
        .await
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = list["questions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [owners.as_str()], "{list}");
    assert_eq!(list["open"], 1);
    let r = ts.get_authed(&format!("/api/questions/{theirs}")).await;
    assert_eq!(r.status(), 404);
    let r = ts.get_authed(&format!("/api/questions/{owners}")).await;
    assert_eq!(r.status(), 200);
    // A person's agent may not name a live page or an artifact it may not
    // read; a malformed ID names none (404 for any caller).
    let r = sam
        .agent(
            "POST",
            &format!("/api/sessions/{sid}/questions"),
            Some(&sid),
            Some(ask_body(Some("7q3k9mzx2b4t"))),
        )
        .await;
    assert_eq!(r.status().as_u16(), 404);
}
