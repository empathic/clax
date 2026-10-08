use crate::common;
use common::TestServer;
use serde_json::{Value, json};

async fn artifact(ts: &TestServer) -> (String, String) {
    let s = ts.register_session("claude", "att-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts
        .publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>")
        .await;
    (sid, a["artifact"]["id"].as_str().unwrap().to_string())
}

async fn att(ts: &TestServer, aid: &str, cookie: &str) -> Value {
    let v: Value = ts
        .client
        .get(format!("{}/api/artifacts/{aid}", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["attention"].clone()
}

async fn look(ts: &TestServer, aid: &str, cookie: &str, ids: &[&str]) -> reqwest::Response {
    ts.client
        .put(format!("{}/api/viewers/me/looked", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .json(&json!({"artifact_id": aid, "thread_ids": ids}))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn comments_name_their_author_and_threads_you_are_in_are_yours() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let mia = ts.viewer(Some("Mia Kovač")).await;
    let t = ts.thread_as(&aid, &alex.cookie, "Two columns").await;
    let tid = t["id"].as_str().unwrap();
    assert_eq!(t["comments"][0]["author_public_id"], alex.public_id);
    assert_eq!(att(&ts, &aid, &alex.cookie).await["open_in"], json!([tid]));
    assert_eq!(att(&ts, &aid, &mia.cookie).await["open_in"], json!([]));
    let other = ts
        .thread_as(&aid, &alex.cookie, "@mia kovač which log?")
        .await;
    let a = att(&ts, &aid, &mia.cookie).await;
    assert_eq!(
        a["open_in"],
        json!([other["id"]]),
        "a full-name mention puts Mia in the thread"
    );
    assert_eq!(
        a["new_replies"],
        json!([other["id"]]),
        "someone else's comment she has not looked at"
    );
}

#[tokio::test]
async fn an_address_after_your_last_look_needs_your_eyes_until_you_look() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let tid = ts.thread_as(&aid, &alex.cookie, "Two columns").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(look(&ts, &aid, &alex.cookie, &[&tid]).await.status(), 200);
    assert_eq!(att(&ts, &aid, &alex.cookie).await["addressed"], json!([]));
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base))).header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "addresses": [tid], "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    let a = att(&ts, &aid, &alex.cookie).await;
    assert_eq!(a["addressed"], json!([tid]));
    assert_eq!(a["addressed_v"], 2);
    look(&ts, &aid, &alex.cookie, &[&tid]).await;
    assert_eq!(
        att(&ts, &aid, &alex.cookie).await["addressed"],
        json!([]),
        "seeing the thread clears it; resolving is not needed"
    );
}

#[tokio::test]
async fn attention_is_the_viewers_own_and_never_anyone_elses() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let tid = ts.thread_as(&aid, &alex.cookie, "Two columns").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    look(&ts, &aid, &alex.cookie, &[&tid]).await;
    let anon: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert!(anon.get("attention").is_none(), "no cookie, no attention");
    let res = ts
        .client
        .get(format!("{}/api/artifacts/{aid}", ts.base))
        .header("cookie", format!("clax_viewer={}", alex.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["vary"], "Cookie");
    assert!(
        res.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("private")
    );
    let threads: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads"))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !threads.to_string().contains("looked"),
        "thread views never carry looked-at marks"
    );
    assert_eq!(look(&ts, &aid, "not-a-cookie", &[&tid]).await.status(), 400);
    let foreign = ts
        .client
        .put(format!("{}/api/viewers/me/looked", ts.base))
        .header("cookie", format!("clax_viewer={}", alex.cookie))
        .header("origin", "http://evil.example")
        .json(&json!({"artifact_id": aid, "thread_ids": [tid]}))
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), 403);
    let all: Value = ts
        .client
        .get(format!("{}/api/viewers/me/attention", ts.base))
        .header("cookie", format!("clax_viewer={}", alex.cookie))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all["artifacts"][&aid]["open_in"], json!([tid]));
    assert!(all["artifacts"][&aid].get("looked").is_none());
}

#[tokio::test]
async fn participants_name_agents_by_handle_only() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    ts.thread_as(&aid, &alex.cookie, "Two columns").await;
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    let p = &v["artifact"]["participants"];
    assert_eq!(
        p["people"],
        json!([{"public_id": alex.public_id, "display_name": "Alex", "seen": null}])
    );
    let agent = &p["agents"][0];
    assert_eq!(agent["harness"], "claude");
    assert_eq!(agent["live"], true);
    let handle = agent["handle"].as_str().unwrap();
    assert!(handle.starts_with("a_") && handle.len() == 24);
    assert!(
        !v.to_string().contains(&sid),
        "no session ID anywhere in the artifact view"
    );
    assert_eq!(v["versions"][0]["agent"], handle);
    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(
        list["artifacts"][0]["participants"]["agents"][0]["handle"],
        handle
    );
    assert!(!list.to_string().contains(&sid));
}

#[tokio::test]
async fn the_last_version_a_person_viewed_is_public_and_their_looks_are_not() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let tid = ts.thread_as(&aid, &alex.cookie, "Two columns").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .client
        .put(format!("{}/api/viewers/me/seen", ts.base))
        .header("cookie", format!("clax_viewer={}", alex.cookie))
        .json(&json!({"artifact_id": aid, "version": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    look(&ts, &aid, &alex.cookie, &[&tid]).await;
    let anon: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        anon["artifact"]["participants"]["people"][0]["seen"], 1,
        "another viewer, or no viewer, reads Alex's last seen version"
    );
    assert!(
        !anon.to_string().contains("looked"),
        "Alex's looked-at marks stay Alex's"
    );
    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"][0]["participants"]["people"][0]["seen"], 1);
}

#[tokio::test]
async fn agents_are_live_only_when_a_send_can_reach_them_and_the_most_recently_active_comes_first()
{
    let ts = TestServer::spawn().await;
    let (owner, aid) = artifact(&ts).await;
    let w = ts.register_session("codex", "att-watch").await;
    let watcher = w["id"].as_str().unwrap().to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{watcher}/watches/{aid}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let agents = |v: &Value| {
        v["artifact"]["participants"]["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                (
                    a["harness"].as_str().unwrap().to_string(),
                    a["live"].as_bool().unwrap(),
                )
            })
            .collect::<Vec<_>>()
    };
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        agents(&v),
        [("codex".to_string(), true), ("claude".to_string(), true)],
        "the newest watch is the most recent activity"
    );
    let res = ts
        .authed(ts.client.patch(format!("{}/api/sessions/{owner}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        agents(&v),
        [("codex".to_string(), true), ("claude".to_string(), false)],
        "an ended publisher stays listed, not live"
    );
}

#[tokio::test]
async fn no_session_id_reaches_a_tokenless_caller() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = artifact(&ts).await;
    for path in [
        format!("/api/artifacts/{aid}"),
        "/api/artifacts".to_string(),
        format!("/api/artifacts?artifact={aid}"),
    ] {
        let anon: Value = ts.get(&path).await.json().await.unwrap();
        assert!(
            !anon.to_string().contains(&sid),
            "{path} without the token: {anon}"
        );
        let authed: Value = ts.get_authed(&path).await.json().await.unwrap();
        assert!(
            authed.to_string().contains(&sid),
            "{path} with the token keeps the owner session"
        );
    }
    let one: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert!(one["artifact"].get("owner_session_id").is_none());
    assert!(one["versions"][0].get("session_id").is_none());
    assert_eq!(
        one["artifact"]["owner_harness"], "claude",
        "the harness stays: the by-line needs it"
    );
}

#[tokio::test]
async fn no_version_route_names_a_session_without_the_token() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = artifact(&ts).await;
    for path in [
        format!("/api/artifacts/{aid}/versions"),
        format!("/api/artifacts/{aid}/versions/1"),
    ] {
        let anon: Value = ts.get(&path).await.json().await.unwrap();
        assert!(
            !anon.to_string().contains(&sid),
            "{path} without the token: {anon}"
        );
        assert!(anon.to_string().contains("agent_harness"), "{path}: {anon}");
        let authed: Value = ts.get_authed(&path).await.json().await.unwrap();
        assert!(
            authed.to_string().contains(&sid),
            "{path} with the token keeps the session"
        );
    }
}

async fn get_with(ts: &TestServer, path: &str, cookie: &str) -> reqwest::Response {
    ts.client
        .get(format!("{}{path}", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn one_artifacts_attention_and_list_entry_match_the_whole_answers() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = artifact(&ts).await;
    let (_sid2, other) = {
        let s = ts.register_session("claude", "att-2").await;
        let sid = s["id"].as_str().unwrap().to_string();
        let a = ts
            .publish_as(&sid, "U", "<main><h2>Other</h2></main>")
            .await;
        (sid, a["artifact"]["id"].as_str().unwrap().to_string())
    };
    let alex = ts.viewer(Some("Alex")).await;
    let tid = ts.thread_as(&aid, &alex.cookie, "Two columns").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.thread_as(&other, &alex.cookie, "Elsewhere").await;

    let all: Value = get_with(&ts, "/api/viewers/me/attention", &alex.cookie)
        .await
        .json()
        .await
        .unwrap();
    let res = get_with(
        &ts,
        &format!("/api/viewers/me/attention?artifact={aid}"),
        &alex.cookie,
    )
    .await;
    assert_eq!(res.headers()["vary"], "Cookie");
    let one: Value = res.json().await.unwrap();
    assert_eq!(one["artifacts"].as_object().unwrap().len(), 1, "{one}");
    assert_eq!(one["artifacts"][&aid], all["artifacts"][&aid]);
    assert_eq!(one["artifacts"][&aid]["open_in"], json!([tid]));
    assert!(one["artifacts"][&aid].get("looked").is_none());

    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    let entry = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == aid)
        .unwrap()
        .clone();
    let single: Value = ts
        .get(&format!("/api/artifacts?artifact={aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(single["artifacts"], json!([entry]));
    assert_eq!(
        single["artifacts"][0]["participants"]["people"][0]["public_id"],
        alex.public_id
    );

    let no_cookie: Value = ts
        .get(&format!("/api/viewers/me/attention?artifact={aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(no_cookie, json!({"artifacts": {}}));
    let bad = get_with(
        &ts,
        "/api/viewers/me/attention?artifact=nope!",
        &alex.cookie,
    )
    .await;
    assert_eq!(bad.status(), 400);
    assert_eq!(ts.get("/api/artifacts?artifact=nope!").await.status(), 400);

    let del = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{other}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204);
    let gone: Value = get_with(
        &ts,
        &format!("/api/viewers/me/attention?artifact={other}"),
        &alex.cookie,
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(
        gone,
        json!({"artifacts": {}}),
        "a deleted artifact has no attention"
    );
    let gone: Value = ts
        .get(&format!("/api/artifacts?artifact={other}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(gone, json!({"artifacts": []}));
}
