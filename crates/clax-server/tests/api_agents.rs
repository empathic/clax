//! The agents roster, its stream topic, and what an agent's end leaves
//! behind (spec 2026-10-08-multi-agent-multi-person §13, §14, §6.7).

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use clax_testkit::{EventReader, TestClient};
use serde_json::{Value, json};
use std::sync::Arc;

/// The owner's agent, and Sam's and Mia's, each connected.
struct Roster {
    ts: TestServer,
    clock: Arc<ManualClock>,
    sam: TestClient,
    mia: TestClient,
    sam_cookie: String,
    sam_public: String,
    owner_session: String,
    sam_session: String,
    mia_session: String,
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

async fn roster() -> Roster {
    let clock = Arc::new(ManualClock::at("2026-10-11T09:00:00Z"));
    let ts = TestServer::spawn_on_with_clock("0.0.0.0".parse().unwrap(), clock.clone()).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let v = ts.store.viewer_of_grant(&sam.grant_id()).unwrap();
    let sam_cookie = ts.browser_cookie(&v.id);
    let owner_session = id(&ts.register_session("claude", "owner-h1").await);
    let sam_session = id(&sam.register("claude", "sam-h1").await);
    let mia_session = id(&mia.register("codex", "mia-h1").await);
    Roster {
        ts,
        clock,
        sam,
        mia,
        sam_cookie,
        sam_public: v.public_id,
        owner_session,
        sam_session,
        mia_session,
    }
}

impl Roster {
    /// `GET path` from Sam's browser: (status, body).
    async fn sams_browser(&self, path: &str) -> (u16, Value) {
        let r = self
            .ts
            .client
            .get(format!("{}{path}", self.ts.base))
            .header("cookie", format!("clax_viewer={}", self.sam_cookie))
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    /// Sam's agent asks a question; its creation time.
    async fn sam_asks(&self) -> String {
        let r = self
            .sam
            .agent(
                "POST",
                &format!("/api/sessions/{}/questions", self.sam_session),
                Some(&self.sam_session),
                Some(json!({"source": "ask", "questions": [{"question": "Teal?",
                    "header": "Colour", "options": [{"label": "Teal"}, {"label": "Amber"}]}]})),
            )
            .await;
        assert_eq!(r.status(), 201);
        r.json::<Value>().await.unwrap()["question"]["created_at"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The owner's roster, by hand (the token, no session).
    async fn owners(&self) -> Vec<Value> {
        let r = self.ts.get_authed("/api/agents").await;
        assert_eq!(r.status(), 200);
        r.json::<Value>().await.unwrap()["agents"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn handle(&self, sid: &str) -> String {
        self.ts
            .store
            .get_session(sid)
            .unwrap()
            .unwrap()
            .agent_handle
    }

    /// Sam's browser follows `topics` on `/api/stream`.
    async fn sams_stream(&self, topics: &[&str]) -> EventReader {
        let browser = |r: reqwest::RequestBuilder| {
            r.header("cookie", format!("clax_viewer={}", self.sam_cookie))
        };
        let res = browser(self.ts.client.get(format!("{}/api/stream", self.ts.base)))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let mut ev = EventReader::from_response(res);
        let sid = ev.next_named("ready").await["stream"]
            .as_str()
            .unwrap()
            .to_string();
        let r = browser(
            self.ts
                .client
                .post(format!("{}/api/stream/{sid}", self.ts.base)),
        )
        .json(&json!({"subscribe": topics}))
        .send()
        .await
        .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        ev
    }
}

/// The row of `handle`, or the one named `name` when the handle is hidden.
fn row<'a>(agents: &'a [Value], name: &str) -> &'a Value {
    agents
        .iter()
        .find(|a| a["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {agents:?}"))
}

#[tokio::test]
async fn roster_reports_blocked_since_and_is_person_scoped() {
    let r = roster().await;
    let asked_at = r.sam_asks().await;
    let sam_name =
        r.ts.store
            .get_session(&r.sam_session)
            .unwrap()
            .unwrap()
            .name
            .unwrap();
    let mia_name =
        r.ts.store
            .get_session(&r.mia_session)
            .unwrap()
            .unwrap()
            .name
            .unwrap();
    let owner_name =
        r.ts.store
            .get_session(&r.owner_session)
            .unwrap()
            .unwrap()
            .name
            .unwrap();

    // The owner sees every agent in full.
    let all = r.owners().await;
    assert_eq!(all.len(), 3, "{all:?}");
    for a in &all {
        assert!(a["handle"].as_str().unwrap().starts_with("a_"), "{a}");
        assert_eq!(a["project"], "w");
        assert_eq!(a["live"], true);
    }
    let sams = row(&all, &sam_name);
    assert_eq!(sams["person"], r.sam_public.as_str());
    assert_eq!(sams["state"], "waiting");
    assert_eq!(sams["since"], asked_at.as_str());
    assert_eq!(
        sams["blocked"],
        json!({"since": asked_at, "questions": 1, "approvals": 0})
    );
    assert_eq!(row(&all, &owner_name)["state"], "idle");
    assert_eq!(row(&all, &owner_name)["blocked"], Value::Null);

    // "Blocked since" is the agent strip's.
    let (status, strip) = r.sams_browser("/api/inbox?group=agent").await;
    assert_eq!(status, 200, "{strip}");
    assert_eq!(strip["agents"][0]["blocked"], sams["blocked"]);
    // Its person, named, and Sam's own.
    assert_eq!(strip["agents"][0]["person_name"], "Sam");
    assert_eq!(strip["agents"][0]["own"], true);
    assert!(strip["agents"][0]["person"].is_string(), "{strip}");

    // Sam's browser and Sam's remote CLI: his own agent in full, the
    // others by name and state, never a handle.
    let (status, browser) = r.sams_browser("/api/agents").await;
    assert_eq!(status, 200, "{browser}");
    let cli: Value = r.sam.get(None, "/api/agents").await.json().await.unwrap();
    for view in [&browser, &cli] {
        let agents = view["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 3, "{view}");
        let own = row(agents, &sam_name);
        assert_eq!(own["handle"], r.handle(&r.sam_session).as_str());
        assert_eq!(own["blocked"], sams["blocked"]);
        assert_eq!(own["project"], "w");
        for other in [&mia_name, &owner_name] {
            let o = row(agents, other);
            for k in [
                "handle",
                "project",
                "harness",
                "working",
                "blocked",
                "threads_owned",
                "unread",
            ] {
                assert_eq!(o[k], Value::Null, "{k} of another person's agent: {o}");
            }
            assert_eq!(o["state"], "idle");
            assert!(o["since"].is_string());
        }
        let text = view.to_string();
        for h in [r.handle(&r.mia_session), r.handle(&r.owner_session)] {
            assert!(!text.contains(&h), "another person's handle in {text}");
        }
    }
    assert_eq!(
        row(browser["agents"].as_array().unwrap(), &sam_name)["unread"],
        1
    );
    assert_eq!(
        row(cli["agents"].as_array().unwrap(), &sam_name)["unread"],
        Value::Null
    );

    // The owner's agent works on a page: the owner by hand sees its title,
    // the agent itself does not (titles are withheld per field from agents).
    let aid = r.ts.any_artifact().await;
    let res =
        r.ts.authed(r.ts.client.put(format!(
            "{}/api/sessions/{}/working/{aid}",
            r.ts.base, r.owner_session
        )))
        .header("x-clax-session", &r.owner_session)
        .json(&json!({"message": "on it"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    let by_hand = r.owners().await;
    let w = &row(&by_hand, &owner_name)["working"][0];
    assert_eq!(w["artifact_id"], aid.as_str());
    assert!(w["title"].is_string(), "{w}");
    // The owner's agent sees the owner's agents in full, others' by name.
    let r2 =
        r.ts.authed(r.ts.client.get(format!("{}/api/agents", r.ts.base)))
            .header("x-clax-session", &r.owner_session)
            .send()
            .await
            .unwrap();
    assert_eq!(r2.status(), 200);
    let v: Value = r2.json().await.unwrap();
    let agents = v["agents"].as_array().unwrap();
    assert!(row(agents, &owner_name)["handle"].is_string());
    let w = &row(agents, &owner_name)["working"][0];
    assert_eq!(
        (&w["artifact_id"], &w["title"]),
        (&json!(aid), &Value::Null),
        "{w}"
    );
    assert_eq!(row(agents, &owner_name)["state"], "working");
    assert_eq!(row(agents, &sam_name)["handle"], Value::Null);

    // No credential, no roster.
    assert_eq!(r.ts.get("/api/agents").await.status().as_u16() / 100, 4);

    // An ended agent stays an hour, as `ended`.
    let mut ev = r.sams_stream(&["agents"]).await;
    let res = r
        .mia
        .agent(
            "PATCH",
            &format!("/api/sessions/{}", r.mia_session),
            Some(&r.mia_session),
            Some(json!({"ended": true})),
        )
        .await;
    assert_eq!(res.status(), 200);
    let (name, data) = ev.next().await;
    assert_eq!(name, "agents");
    assert_eq!(data, json!({"topic": "agents"}), "a ping names nothing");
    let ended_at =
        r.ts.store
            .get_session(&r.mia_session)
            .unwrap()
            .unwrap()
            .ended_at
            .unwrap();
    let all = r.owners().await;
    let mias = row(&all, &mia_name);
    assert_eq!(
        (&mias["state"], &mias["live"]),
        (&json!("ended"), &json!(false))
    );
    assert_eq!(mias["since"], ended_at.as_str());
    assert_eq!(
        all.last().unwrap()["name"],
        mia_name.as_str(),
        "live ones first"
    );
    r.clock.advance(3601);
    assert!(
        !r.owners().await.iter().any(|a| a["name"] == mia_name),
        "gone after an hour"
    );
}

#[tokio::test]
async fn reregistering_after_an_idle_end_restores_watches_and_says_what_ended() {
    let r = roster().await;
    let aid = r.ts.any_artifact().await;
    let res = r
        .sam
        .agent(
            "PUT",
            &format!("/api/sessions/{}/watches/{aid}", r.sam_session),
            Some(&r.sam_session),
            None,
        )
        .await;
    assert_eq!(res.status(), 200);
    r.sam_asks().await;
    // 31 minutes without contact: the reaper ends it, never probing it.
    r.clock.advance(31 * 60);
    let (feedback, store) = (r.ts.feedback.clone(), r.ts.store.clone());
    let reaped = tokio::task::spawn_blocking(move || {
        clax_server::daemon::reap_idle(
            &feedback,
            &store,
            std::time::Duration::from_secs(clax_core::Store::SESSION_IDLE_SECS),
            &|_| panic!("a remote session is never probed"),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(reaped.ended.contains(&r.sam_session));
    // Its note in Sam's inbox: the question it withdrew.
    let (_, items) = r.sams_browser("/api/inbox?kind=ended").await;
    let item = &items["items"][0];
    assert_eq!(item["kind"], "ended", "{items}");
    assert_eq!(item["ended"]["reason"], "idle");
    assert_eq!(item["ended"]["questions"], 1);
    let name =
        r.ts.store
            .get_session(&r.sam_session)
            .unwrap()
            .unwrap()
            .name
            .unwrap();
    assert_eq!(
        item["ended"]["text"],
        format!("{name} ended · withdrew 1 question")
    );
    assert_eq!(item["agent"]["handle"], r.handle(&r.sam_session).as_str());
    // The agent's next registration gets its watch back, and its first
    // poll the note, once.
    let back = id(&r.sam.register("claude", "sam-h1").await);
    assert_ne!(back, r.sam_session);
    let w: Value = r
        .sam
        .get(Some(&back), &format!("/api/sessions/{back}/watches"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(w["watches"][0]["artifact_id"], aid.as_str(), "{w}");
    let poll = |sid: String| {
        let sam = &r.sam;
        async move {
            sam.get(
                Some(&sid),
                &format!("/api/sessions/{sid}/feedback?tier=piggyback&wait=0"),
            )
            .await
            .json::<Value>()
            .await
            .unwrap()
        }
    };
    let first = poll(back.clone()).await;
    let note = first["notes"][0].as_str().unwrap();
    assert!(
        note.starts_with("[clax] Your session ended after 30 minutes without contact (")
            && note.contains("no threads were released")
            && note.ends_with("your watches were restored."),
        "{note}"
    );
    assert_eq!(first["text"], note);
    let second = poll(back.clone()).await;
    assert!(second.get("notes").is_none(), "{second}");
    // Mia's machine, registering the same harness session ID, gets nothing.
    let other = id(&r.mia.register("claude", "sam-h1").await);
    let v: Value = r
        .mia
        .get(Some(&other), &format!("/api/sessions/{other}/watches"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["watches"], json!([]));
    assert!(poll_mia(&r, &other).await.get("notes").is_none());
}

async fn poll_mia(r: &Roster, sid: &str) -> Value {
    r.mia
        .get(
            Some(sid),
            &format!("/api/sessions/{sid}/feedback?tier=piggyback&wait=0"),
        )
        .await
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn an_agents_end_tells_the_send_menus_of_what_it_watched() {
    let r = roster().await;
    let aid = r.ts.publish_as(&r.owner_session, "T", "<h2>x</h2>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res =
        r.ts.authed(
            r.ts.client
                .patch(format!("{}/api/sessions/{}", r.ts.base, r.owner_session)),
        )
        .header("x-clax-session", &r.owner_session)
        .json(&json!({"ended": true}));
    let mut ev = r.sams_stream(&[&format!("artifact:{aid}")]).await;
    assert_eq!(res.send().await.unwrap().status(), 200);
    let heard = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let (name, data) = ev.next().await;
            if name == "agents" {
                return data;
            }
        }
    })
    .await
    .expect("no agents event on the artifact's topic within 10 s");
    assert_eq!(
        heard,
        json!({"topic": format!("artifact:{aid}")}),
        "a ping names nothing"
    );
}

/// A person's ended note never names a live page's thread (L10): their
/// browser never sees a live page.
#[tokio::test]
async fn a_persons_ended_note_leaves_out_live_pages() {
    let w = clax_testkit::people::World::two_people().await;
    let cookie =
        w.ts.browser_cookie(&w.ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap().id);
    // Sam's agent owns a thread on Mia's page and one on the live page
    // (written directly: no send reaches a person's agent on a live page).
    let db = rusqlite::Connection::open(w.ts.home.db_path()).unwrap();
    let mut ids = Vec::new();
    for aid in [&w.read.artifact, &w.live] {
        let tid: String = db
            .query_row(
                "SELECT id FROM threads WHERE artifact_id = ?1 AND status = 'open' LIMIT 1",
                [aid],
                |r| r.get(0),
            )
            .unwrap();
        db.execute(
            "UPDATE threads SET target_session_id = ?2 WHERE id = ?1",
            [&tid, &w.sam_session],
        )
        .unwrap();
        ids.push(tid);
    }
    let res = w
        .sam
        .agent(
            "PATCH",
            &format!("/api/sessions/{}", w.sam_session),
            Some(&w.sam_session),
            Some(json!({"ended": true})),
        )
        .await;
    assert_eq!(res.status(), 200);
    let v: Value =
        w.ts.client
            .get(format!("{}/api/inbox?kind=ended", w.ts.base))
            .header("cookie", format!("clax_viewer={cookie}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let item = &v["items"][0];
    assert_eq!(item["kind"], "ended", "{v}");
    let shown: Vec<&str> = item["ended"]["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        [ids[0].as_str()],
        "the live page's thread is left out"
    );
    assert!(!v.to_string().contains(w.live.as_str()), "{v}");
    assert_eq!(
        item["ended"]["threads"][0]["url"],
        format!("/a/{}?thread={}", w.read.artifact, ids[0])
    );
}
