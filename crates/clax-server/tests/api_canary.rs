//! What every route and every agent channel hands an agent (spec §7.5,
//! §8.5, §8.6, M7, M8): no canary of Mia's, who no one approved, reaches an
//! agent through any route or real client; the routes that serve people
//! refuse the owner's agents; the all-threads listing is the owner's
//! agents' only; and the owner's question list holds the owner's agents'
//! questions only.
//!
//! The filter is live for every route and every agent channel but the Pi
//! extension's, whose sweep is ignored until the task that filters it
//! lifts it.

use crate::common::TestServer;
use crate::common::clients::{HARNESS_ENVS, RealClients};
use crate::common::people::{
    AgentOf, As, CARRIED_BYTES, Call, DOC_PATH, FILE_BYTES, FILE_NAME, TITLE, World,
    without_named_doc,
};
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

/// Every call of the sweep as `agent` that `which` keeps: no response
/// byte carries a canary, unless the route refuses it `not_for_agents`.
/// A docs call that names Mia's document by its path gets that path back
/// in its placeholder's `doc_id` and in an error naming it (spec §8.3,
/// §21.9): those fields alone are left out of the check. A listing is
/// never exempt, and neither is the document no call names.
async fn sweep(w: &World, agent: &AgentOf<'_>, which: impl Fn(&Call) -> bool) {
    let calls: Vec<Call> = w
        .every_agent_call(agent)
        .into_iter()
        .filter(|c| which(c))
        .collect();
    assert!(calls.len() > 40, "{}", calls.len());
    let mut leaks = Vec::new();
    for call in &calls {
        let mut body = call.run(w, agent).await;
        if let Some(path) = call.named_doc() {
            let (head, rest) = body.split_once('\n').unwrap_or((&body, ""));
            if let Ok(v) = serde_json::from_str::<Value>(rest) {
                body = format!("{head}\n{}", without_named_doc(v, &path));
            }
        }
        if body.starts_with("403 ") && body.contains("\"not_for_agents\"") {
            continue;
        }
        for c in w.canaries() {
            if body.contains(c) {
                leaks.push(format!(
                    "{} leaked {c} to {}: {}",
                    call.name,
                    agent.label,
                    &body[..body.len().min(300)]
                ));
            }
        }
        // A person's agent is never named another credential's session.
        if matches!(agent.who, As::Grant(_)) && body.contains(&w.mia_session) {
            leaks.push(format!(
                "{} named Mia's session to {}: {}",
                call.name,
                agent.label,
                &body[..body.len().min(300)]
            ));
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

#[tokio::test]
async fn no_canary_reaches_an_unapproved_agent() {
    let w = World::two_people().await;
    sweep(&w, &w.sam_agent(), |_| true).await;
}

/// A fork keeps the authors of each file and text field it copies (spec
/// 2026-10-08-multi-agent-multi-person §8.2), so an agent reads Mia's
/// words in a fork of her artifact, its own person's, no more than in
/// hers: every read route on the fork, as the agent that forked it and as
/// the owner's agent.
#[tokio::test]
async fn no_canary_reaches_an_unapproved_agent_through_a_fork() {
    let mut w = World::two_people().await;
    let res = w
        .sam
        .agent(
            "POST",
            &format!("/api/artifacts/{}/fork", w.read.artifact),
            Some(&w.sam_session),
            Some(json!({})),
        )
        .await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    let fid = v["artifact"]["id"].as_str().unwrap().to_string();
    // The fork holds her words: the owner by hand reads them there.
    let mut seen = String::new();
    for path in [
        format!("/api/artifacts/{fid}"),
        format!("/api/artifacts/{fid}/versions/1/files/index.html"),
        format!("/api/artifacts/{fid}/versions/1/files/MIACANARYcarried.css"),
    ] {
        let res = w.ts.get_authed(&path).await;
        assert_eq!(res.status(), 200, "{path}");
        seen.push_str(&res.text().await.unwrap());
    }
    for c in [TITLE, FILE_NAME, FILE_BYTES, CARRIED_BYTES] {
        assert!(seen.contains(c), "{c} not in the fork: {seen}");
    }
    w.read.artifact = fid;
    sweep(&w, &w.sam_agent(), |c| c.method == "GET").await;
    sweep(&w, &w.owner_agent(), |c| c.method == "GET" && !c.denied).await;
}

/// The routes a person's agent is refused (Denied) need no filter: as the
/// owner's agent, each answers `not_for_agents` or carries no canary.
#[tokio::test]
async fn no_canary_reaches_the_owners_agent_through_a_denied_route() {
    let w = World::two_people().await;
    sweep(&w, &w.owner_agent(), |c| c.denied).await;
}

#[tokio::test]
async fn no_canary_reaches_an_unapproved_owner_agent() {
    let w = World::two_people().await;
    sweep(&w, &w.owner_agent(), |c| !c.denied).await;
}

/// The world's seed puts each canary where the sweeps look for it: the
/// owner by hand (the token alone) reads every one.
#[tokio::test]
async fn the_owner_by_hand_sees_every_canary() {
    let w = World::two_people().await;
    let ts = &w.ts;
    let aid = &w.read.artifact;
    let mut seen = String::new();
    for path in [
        format!("/api/artifacts/{aid}"),
        format!("/api/artifacts/{aid}/versions/1"),
        format!("/api/artifacts/{aid}/versions/1/files/index.html"),
        format!("/api/artifacts/{aid}/versions/2/files/MIACANARYcarried.css"),
        format!("/_blob/{}", w.read.asset),
        format!("/api/artifacts/{aid}/docs?collection=notes"),
        format!("/api/artifacts/{aid}/threads?include_resolved=true"),
        format!("/api/artifacts/{aid}/threads/{}/clip", w.read.thread),
        format!("/api/artifacts/{aid}/working"),
    ] {
        let r = ts.get_authed(&path).await;
        assert!(r.status().is_success(), "{path}: {}", r.status());
        seen.push_str(&String::from_utf8_lossy(&r.bytes().await.unwrap()));
    }
    // Mia's agent's question is hers: the owner's list leaves it out.
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    let questions: String = db
        .query_row(
            "SELECT questions_json FROM questions WHERE session_id = ?1",
            [&w.mia_session],
            |r| r.get(0),
        )
        .unwrap();
    seen.push_str(&questions);
    for c in w.canaries() {
        assert!(seen.contains(c), "the seed put {c} nowhere the owner reads");
    }
}

/// `outs`' leaks of `w`'s canaries, one per line of output that carries
/// one. `clax db get … <doc ID>` prints the
/// document it was asked for by path and ID (spec §21.9): for a command
/// line that names Mia's document, those fields alone are left out.
fn leaks(w: &World, outs: &[crate::common::clients::Out]) -> Vec<String> {
    let mut leaks = Vec::new();
    for out in outs {
        let named = out.channel.contains(" db get ") && out.channel.contains(DOC_PATH);
        let text = match serde_json::from_str::<Value>(&out.text) {
            Ok(v) if named => without_named_doc(v, &format!("notes/{DOC_PATH}")).to_string(),
            _ => out.text.clone(),
        };
        for c in w.canaries() {
            for line in text.lines().filter(|l| l.contains(c)) {
                let line: String = line.trim().chars().take(240).collect();
                leaks.push(format!("{} leaked {c} in: {line}", out.channel));
            }
        }
    }
    leaks
}

#[tokio::test]
async fn no_canary_reaches_an_agent_through_a_real_client() {
    let w = World::two_people().await;
    let outs = RealClients::new(&w).run_every_channel().await;
    let leaks = leaks(&w, &outs);
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

#[tokio::test]
#[ignore = "lifted by Task 19: the Pi extension is filtered then"]
async fn no_canary_reaches_an_agent_through_pi() {
    let w = World::two_people().await;
    let outs = vec![RealClients::new(&w).pi().await];
    let leaks = leaks(&w, &outs);
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

/// The real clients run, each printing what it hands its agent: the
/// harness the ignored sweep above uses.
#[tokio::test]
async fn every_real_client_runs_against_the_test_daemon() {
    let w = World::two_people().await;
    let clients = RealClients::new(&w);
    let mut outs = clients.run_every_channel().await;
    outs.push(clients.pi().await);
    let channels: Vec<&str> = outs.iter().map(|o| o.channel.as_str()).collect();
    for want in [
        "clax hook --agent claude stop",
        "clax hook --agent claude prompt",
        "clax hook --agent claude session-start",
        "clax feedback follow --once",
        "the MCP shim",
        "/mcp",
        "the Pi extension",
    ] {
        assert!(channels.contains(&want), "{want}: {channels:?}");
    }
    assert!(
        channels.iter().filter(|c| c.contains(".md: ")).count() >= 8,
        "{channels:?}"
    );
    let shim = &outs
        .iter()
        .find(|o| o.channel == "the MCP shim")
        .unwrap()
        .text;
    assert!(shim.contains(&w.read.artifact), "{shim}");
    let pi = &outs
        .iter()
        .find(|o| o.channel == "the Pi extension")
        .unwrap()
        .text;
    assert!(
        pi.contains("clax_list: ") && pi.contains(&w.read.artifact),
        "{pi}"
    );
    let list = &outs
        .iter()
        .find(|o| {
            o.channel.ends_with(
                "list.md: \"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh\" exec list --json",
            )
        })
        .unwrap()
        .text;
    assert!(list.contains(&w.read.artifact), "{list}");
}

#[tokio::test]
async fn cli_under_a_harness_environment_is_an_agent_channel() {
    let w = World::two_people().await;
    let clients = RealClients::new(&w);
    // Under a harness, `clax inbox` reaches a route that serves people:
    // one readable line, exit 2.
    for (k, v) in HARNESS_ENVS {
        let (text, code) = clients.clax(&["inbox"], &[(k, v)]).await;
        assert_eq!(code, 2, "{k}={v}: {text}");
        assert!(
            text.trim()
                .starts_with("The inbox is for people: open http://")
                && text.contains("/inbox in a browser."),
            "{k}={v}: {text}"
        );
    }
    // The owner by hand: no harness variable, or CLAX_CHANNEL=owner over
    // one, reads the inbox and sees Mia's words.
    for env in [
        &[][..],
        &[("CLAX_CHANNEL", "owner"), ("CLAUDECODE", "1")][..],
    ] {
        let (text, code) = clients.clax(&["inbox", "--all"], env).await;
        assert_eq!(code, 0, "{env:?}: {text}");
        let (text, code) = clients.clax(&["--json", "list"], env).await;
        assert_eq!(code, 0, "{env:?}: {text}");
        assert!(
            text.contains(crate::common::people::TITLE),
            "{env:?}: {text}"
        );
        let (text, _) = clients.clax(&["comments", "--all"], env).await;
        assert!(
            text.contains(crate::common::people::REPLY),
            "{env:?}: {text}"
        );
    }
}

#[tokio::test]
async fn cli_under_a_harness_environment_carries_no_canary() {
    let w = World::two_people().await;
    let outs = RealClients::new(&w).cli_under_harnesses().await;
    assert!(outs.len() >= 12 * HARNESS_ENVS.len(), "{}", outs.len());
    let leaks = leaks(&w, &outs);
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

#[tokio::test]
async fn all_threads_listing_is_filtered_for_owner_agents() {
    let w = World::two_people().await;
    let ts = &w.ts;
    let by_hand: Value = ts
        .get_authed("/api/threads?include_resolved=true")
        .await
        .json()
        .await
        .unwrap();
    assert!(by_hand.to_string().contains(crate::common::people::COMMENT));
    let agent: Value = ts
        .authed(
            ts.client
                .get(format!("{}/api/threads?include_resolved=true", ts.base)),
        )
        .header("x-clax-channel", "agent")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = agent.to_string();
    for c in w.canaries() {
        assert!(!text.contains(c), "{c}: {text}");
    }
    let r = w.sam.get(Some(&w.sam_session), "/api/threads").await;
    assert_eq!(status_code(r).await, (403, "forbidden".into()));
}

/// The owner's browser without the token (the owner cookie alone) is no
/// token: no session IDs, and a name to reopen or delete a thread, as for
/// any viewer (`crate::identity`: the owner cookie alone grants no more
/// than a viewer cookie does).
#[tokio::test]
async fn the_owners_cookie_alone_is_not_the_token() {
    let ts = TestServer::spawn().await;
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = ts.publish_as(&sid, "Doc", "<h2>Quarterly goals</h2>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid = ts.thread(&aid, 1, "a thread").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let browser = ts.owner_browser();
    for path in [
        format!("/api/artifacts/{aid}"),
        format!("/api/artifacts/{aid}/versions"),
        format!("/api/artifacts/{aid}/versions/1"),
        format!("/api/artifacts?artifact={aid}"),
    ] {
        let by_cookie = browser.get(&path).await.text().await.unwrap();
        assert!(!by_cookie.contains(&sid), "{path}: {by_cookie}");
        let by_token = ts.get_authed(&path).await.text().await.unwrap();
        assert!(by_token.contains(&sid), "{path}: {by_token}");
    }
    let r = browser
        .post(
            &format!("/api/artifacts/{aid}/threads/{tid}/reopen"),
            json!({}),
        )
        .await;
    assert_eq!(status_code(r).await, (403, "forbidden".into()));
    let r = browser
        .delete(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await;
    assert_eq!(status_code(r).await, (403, "forbidden".into()));
    // The token needs no name.
    let r = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/reopen",
            ts.base
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}
