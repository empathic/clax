//! Publishing to a public Clax (`clax_http::public`,
//! `clax_server::routes::remotes`): a local daemon signs in to a hosted
//! one by device code, pushes an artifact's versions with their authorship,
//! pushes new ones, refuses when the public copy moved, and asks the owner
//! before an agent's push.

use clax_core::ArtifactId;
use clax_testkit::TestServer;
use clax_testkit::remotes::{Public, connect, connect_with, json_of, local_req, owner_post, post};
use reqwest::header::HOST;
use serde_json::{Value, json};

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

fn files(pairs: &[(&str, &str)]) -> Value {
    let mut m = serde_json::Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), json!({"content": v, "encoding": "utf8"}));
    }
    Value::Object(m)
}

/// Publishes version `n + 1` of `aid` locally, as `session` when given.
async fn version(ts: &TestServer, aid: &str, n: u32, body: Value, session: Option<&str>) -> Value {
    let mut b = body;
    b["if_version"] = json!(n);
    let (s, v) = local_req(
        ts,
        reqwest::Method::POST,
        &format!("/api/artifacts/{aid}/versions"),
        b,
        session,
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v
}

/// A local artifact of three versions: the owner's, then two of a Claude
/// session named `planner` (the second drops `notes.txt`).
async fn three_versions(local: &TestServer) -> String {
    let v = local
        .publish(
            "Plan",
            &[("index.html", "<h1>v1</h1>"), ("notes.txt", "first")],
        )
        .await;
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let sid = local.register_session("claude", "h-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = local_req(
        local,
        reqwest::Method::PATCH,
        &format!("/api/sessions/{sid}"),
        json!({"name": "planner"}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    version(
        local,
        &aid,
        1,
        json!({"files": files(&[("index.html", "<h1>v2</h1>")]), "label": "Second", "note": "Tightened"}),
        Some(&sid),
    )
    .await;
    version(
        local,
        &aid,
        2,
        json!({"files": {"index.html": {"content": "<h1>v3</h1>", "encoding": "utf8"}, "notes.txt": null}, "label": "Third"}),
        Some(&sid),
    )
    .await;
    aid
}

#[tokio::test]
async fn a_push_publishes_every_version_with_its_authorship_and_later_ones_follow() {
    let mut public = Public::new().await;
    let sam = public.become_person("Sam").await;
    let local = TestServer::spawn().await;
    let remote = connect(&local, &public).await;
    assert_eq!(remote["person_public_id"], sam.as_str());
    // The token is listed on the person's People page, named for this machine.
    let (_, toks) = public
        .browser(
            reqwest::Method::GET,
            "/api/hosted/agent-tokens",
            json!(null),
        )
        .await;
    assert_eq!(toks["tokens"][0]["id"], remote["grant_id"], "{toks}");
    let aid = three_versions(&local).await;
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "pushed");
    assert_eq!(v["pushed"].as_array().unwrap().len(), 3, "{v}");
    let rid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(v["url"], format!("{}/a/{rid}", public.origin));
    assert_eq!(
        (
            v["link"]["pushed_n"].as_u64(),
            v["link"]["remote_n"].as_u64()
        ),
        (Some(3), Some(3))
    );
    // On the public instance: three versions, their labels, notes and files,
    // as Sam reads them (the artifact is Sam's: out of the owner's reach).
    let (s, pv) = public
        .browser(
            reqwest::Method::GET,
            &format!("/api/artifacts/{rid}/versions"),
            json!(null),
        )
        .await;
    assert_eq!(s, 200, "{pv}");
    let vs = pv["versions"].as_array().unwrap();
    assert_eq!(vs.len(), 3);
    assert_eq!(vs[1]["label"], "Second");
    assert_eq!(vs[1]["note"], "Tightened");
    assert!(vs[0]["files"].get("notes.txt").is_some());
    assert!(vs[2]["files"].get("notes.txt").is_none(), "{}", vs[2]);
    // Each version's publisher is the person; its agent the local agent's
    // name. The owner's own version is the person's by hand there: no
    // session, so no agent and no harness on its by-line.
    let rid_id = ArtifactId::parse(&rid).unwrap();
    let versions = public.ts.store.list_versions(&rid_id).unwrap();
    assert_eq!(versions[0].session_id, None, "{:?}", versions[0]);
    let names: Vec<Option<String>> = versions[1..]
        .iter()
        .map(|v| {
            public
                .ts
                .store
                .get_session(v.session_id.as_deref().unwrap())
                .unwrap()
                .unwrap()
                .name
        })
        .collect();
    assert_eq!(names[0].as_deref(), Some("planner"));
    assert_eq!(names[1].as_deref(), Some("planner"));
    let (_, pa) = public
        .browser(
            reqwest::Method::GET,
            &format!("/api/artifacts/{rid}"),
            json!(null),
        )
        .await;
    assert_eq!(pa["artifact"]["owner_harness"], Value::Null, "{pa}");
    assert_eq!(pa["versions"][0]["agent_harness"], Value::Null, "{pa}");
    assert_eq!(pa["versions"][1]["agent_harness"], "claude", "{pa}");
    for v in &versions {
        assert_eq!(v.authors.person.as_deref(), Some(sam.as_str()));
    }
    let a = public.ts.store.get_artifact(&rid_id).unwrap().unwrap();
    assert_eq!(a.title, "Plan");
    assert_eq!(a.authors.creator.as_deref(), Some(sam.as_str()));
    // Nothing new: nothing pushed.
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(
        (s, v["pushed"].as_array().map(Vec::len)),
        (200, Some(0)),
        "{v}"
    );
    // A new local version, pushed again.
    version(
        &local,
        &aid,
        3,
        json!({"files": files(&[("index.html", "<h1>v4</h1>")]), "label": "Fourth"}),
        None,
    )
    .await;
    let (s, v) = local_req(
        &local,
        reqwest::Method::GET,
        &format!("/api/artifacts/{aid}/remotes"),
        json!(null),
        None,
    )
    .await;
    assert_eq!((s, v["links"][0]["behind"].as_u64()), (200, Some(1)), "{v}");
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 4, "remote_n": 4}]));
    let (_, pv) = public
        .browser(
            reqwest::Method::GET,
            &format!("/api/artifacts/{rid}"),
            json!(null),
        )
        .await;
    assert_eq!(pv["artifact"]["current_version"], 4);
}

#[tokio::test]
async fn a_push_is_refused_when_the_public_copy_moved_and_goes_new_or_onto_a_fork() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    connect(&local, &public).await;
    let aid = three_versions(&local).await;
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    let rid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    // Someone publishes on the public copy.
    let (s, v) = public
        .cli(
            reqwest::Method::POST,
            &format!("/api/artifacts/{rid}/versions"),
            Some(json!({"if_version": 3, "files": files(&[("index.html", "<h1>theirs</h1>")])})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    version(
        &local,
        &aid,
        3,
        json!({"files": files(&[("index.html", "<h1>mine</h1>")])}),
        None,
    )
    .await;
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!((s, code(&v)), (409, "remote_moved"), "{v}");
    assert_eq!(
        (
            v["error"]["remote_version"].as_u64(),
            v["error"]["pushed_version"].as_u64()
        ),
        (Some(4), Some(3)),
        "{v}"
    );
    // Nothing was overwritten.
    let (_, pv) = public
        .cli(reqwest::Method::GET, &format!("/api/artifacts/{rid}"), None)
        .await;
    assert_eq!(pv["artifact"]["current_version"], 4);
    // Onto a fork of the public copy at the version pushed last.
    let (s, v) = owner_post(
        &local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "fork"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let fork = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(fork, rid);
    assert_eq!(v["pushed"], json!([{"n": 4, "remote_n": 2}]));
    let (_, fv) = public
        .cli(
            reqwest::Method::GET,
            &format!("/api/artifacts/{fork}"),
            None,
        )
        .await;
    assert_eq!(
        fv["artifact"]["forked_from"]["artifact_id"],
        rid.as_str(),
        "{fv}"
    );
    // As a new public artifact: every version again.
    let (s, v) = owner_post(
        &local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "new"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"].as_array().unwrap().len(), 4);
    assert_ne!(v["link"]["remote_artifact_id"].as_str().unwrap(), fork);
}

#[tokio::test]
async fn an_agents_push_waits_for_the_owners_approval_in_the_browser() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    connect(&local, &public).await;
    let aid = three_versions(&local).await;
    let sid = local.register_session("claude", "h-2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let push = |body: Value| {
        let (local, aid, sid) = (&local, aid.clone(), sid.clone());
        async move {
            local_req(
                local,
                reqwest::Method::POST,
                &format!("/api/artifacts/{aid}/push"),
                body,
                Some(&sid),
            )
            .await
        }
    };
    // An agent chooses nothing for the owner.
    let (s, v) = push(json!({"auto": true})).await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (s, v) = push(json!({})).await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(v["status"], "approval_pending");
    let qid = v["question_id"].as_str().unwrap().to_string();
    // Asking again waits on the same question.
    let (_, again) = push(json!({})).await;
    assert_eq!(again["question_id"], qid.as_str());
    let (s, v) = push(json!({"question_id": qid})).await;
    assert_eq!(
        (s, v["status"].as_str()),
        (202, Some("approval_pending")),
        "{v}"
    );
    // An answer through the daemon token (what an agent holds too) waits
    // for the owner's browser while the agent is live, and answers nothing.
    let (s, v) = post(
        &local,
        &format!("/api/questions/{qid}/answer"),
        json!({"answers": [{"selected": ["Publish and push new versions"]}]}),
    )
    .await;
    assert_eq!(
        (s, v["status"].as_str()),
        (202, Some("confirmation_pending")),
        "{v}"
    );
    let (s, v) = push(json!({"question_id": qid})).await;
    assert_eq!(
        (s, v["status"].as_str()),
        (202, Some("approval_pending")),
        "{v}"
    );
    // Asked anew, answered in the owner's browser: pushed, and new versions follow.
    let (s, v) = push(json!({})).await;
    assert_eq!(s, 202, "{v}");
    let qid = v["question_id"].as_str().unwrap().to_string();
    local
        .answer_question(
            &qid,
            json!({"answers": [{"selected": ["Publish and push new versions"]}]}),
        )
        .await;
    let (s, v) = push(json!({"question_id": qid})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["link"]["auto"], true);
    let rid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    // A new version is pushed without asking, by the agent and on its own.
    version(
        &local,
        &aid,
        3,
        json!({"files": files(&[("index.html", "<h1>v4</h1>")])}),
        Some(&sid),
    )
    .await;
    let mut current = 0;
    for _ in 0..3000 {
        let (_, pv) = public
            .cli(reqwest::Method::GET, &format!("/api/artifacts/{rid}"), None)
            .await;
        current = pv["artifact"]["current_version"].as_u64().unwrap_or(0);
        if current == 4 {
            break;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(current, 4);
    // Declined: refused.
    let other = local.publish("Other", &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = local_req(
        &local,
        reqwest::Method::POST,
        &format!("/api/artifacts/{other}/push"),
        json!({}),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 202, "{v}");
    let qid = v["question_id"].as_str().unwrap().to_string();
    local
        .answer_question(&qid, json!({"answers": [{"selected": ["Don't publish"]}]}))
        .await;
    let (s, v) = local_req(
        &local,
        reqwest::Method::POST,
        &format!("/api/artifacts/{other}/push"),
        json!({"question_id": qid}),
        Some(&sid),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_approved"), "{v}");
    let (s, v) = local_req(
        &local,
        reqwest::Method::POST,
        &format!("/api/artifacts/{other}/push"),
        json!({"question_id": qid}),
        Some(&sid),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "approval_used"), "{v}");
}

#[tokio::test]
async fn remotes_are_the_owners_and_a_hosted_daemon_has_none() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let sid = local.register_session("claude", "h-3").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = local_req(
        &local,
        reqwest::Method::POST,
        "/api/remotes",
        json!({"url": public.origin}),
        Some(&sid),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
    let (s, v) = owner_post(&local, "/api/remotes", json!({"url": "http://example.com"})).await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let aid = local.publish("P", &[("index.html", "x")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!((s, code(&v)), (404, "no_remote"), "{v}");
    // A remote denied there is forgotten here.
    let (s, v) = owner_post(
        &local,
        "/api/remotes",
        json!({"url": public.origin, "name": "pub"}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let uc = v["user_code"].as_str().unwrap().to_string();
    let (s, _) = public
        .browser(
            reqwest::Method::POST,
            &format!("/api/hosted/device/{uc}/deny"),
            json!({}),
        )
        .await;
    assert_eq!(s, 204);
    let (s, v) = owner_post(&local, "/api/remotes/pub/poll", json!({})).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("denied")), "{v}");
    let (_, v) = local_req(
        &local,
        reqwest::Method::GET,
        "/api/remotes",
        json!(null),
        None,
    )
    .await;
    assert_eq!(v["remotes"], json!([]));
    // A remote's token never shows.
    connect(&local, &public).await;
    let (_, v) = local_req(
        &local,
        reqwest::Method::GET,
        "/api/remotes",
        json!(null),
        None,
    )
    .await;
    assert!(!v.to_string().contains("cxa_"), "{v}");
    let (s, v) = local_req(
        &local,
        reqwest::Method::DELETE,
        &format!("/api/remotes/{}", v["remotes"][0]["id"].as_str().unwrap()),
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // On the hosted daemon, none of it.
    let (s, _) = public.cli(reqwest::Method::GET, "/api/remotes", None).await;
    assert_eq!(s, 404);
}

/// The agent token `local` holds on its only remote.
fn token_of(local: &TestServer) -> String {
    let r = local.store.remotes().unwrap().remove(0);
    local
        .store
        .remote_token(&r.id)
        .unwrap()
        .unwrap()
        .expose()
        .to_string()
}

/// A request to `public` as the agent token `token`, in a by-hand session.
async fn as_token(
    public: &Public,
    token: &str,
    path: &str,
    body: Value,
    key: Option<&str>,
) -> (u16, Value) {
    let base = public.ts.base.to_string();
    let host = format!("localhost:{}", public.ts.addr.port());
    let (s, sess) = json_of(
        public
            .ts
            .client
            .post(format!("{base}/api/sessions"))
            .header(HOST, &host)
            .bearer_auth(token)
            .json(&json!({"harness": "clax", "cwd": ""}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 201, "{sess}");
    let mut r = public
        .ts
        .client
        .post(format!("{base}{path}"))
        .header(HOST, &host)
        .bearer_auth(token)
        .header("x-clax-session", sess["session"]["id"].as_str().unwrap())
        .header(
            "x-clax-session-secret",
            sess["session_secret"].as_str().unwrap(),
        )
        .json(&body);
    if let Some(k) = key {
        r = r.header("idempotency-key", k);
    }
    json_of(r.send().await.unwrap()).await
}

#[tokio::test]
async fn a_push_whose_answer_was_lost_is_answered_the_version_it_made() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    connect(&local, &public).await;
    let token = token_of(&local);
    let aid = local
        .publish("Plan", &[("index.html", "<h1>v1</h1>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The public Clax took the first push, but its answer never came back.
    let key = clax_server::public::version_key(&aid, 1, "new:-");
    let body = json!({"title": "Plan", "files": files(&[("index.html", "<h1>v1</h1>")])});
    let (s, first) = as_token(&public, &token, "/api/artifacts", body.clone(), Some(&key)).await;
    assert_eq!(s, 201, "{first}");
    let rid = first["artifact"]["id"].as_str().unwrap().to_string();
    // Sent again by hand: the same artifact, not a second one.
    let (s, again) = as_token(&public, &token, "/api/artifacts", body, Some(&key)).await;
    assert_eq!(
        (s, again["artifact"]["id"].as_str()),
        (201, Some(rid.as_str())),
        "{again}"
    );
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["link"]["remote_artifact_id"], rid.as_str(), "{v}");
    assert_eq!(v["pushed"], json!([{"n": 1, "remote_n": 1}]));
    // A later version whose answer was lost: no `remote_moved` on retry.
    version(
        &local,
        &aid,
        1,
        json!({"files": files(&[("index.html", "<h1>v2</h1>")])}),
        None,
    )
    .await;
    let key = clax_server::public::version_key(&aid, 2, &rid);
    let (s, v) = as_token(
        &public,
        &token,
        &format!("/api/artifacts/{rid}/versions"),
        json!({"if_version": 1, "files": files(&[("index.html", "<h1>v2</h1>")])}),
        Some(&key),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 2, "remote_n": 2}]));
    let (_, pv) = public
        .cli(reqwest::Method::GET, &format!("/api/artifacts/{rid}"), None)
        .await;
    assert_eq!(pv["artifact"]["current_version"], 2);
    // A key names one artifact's publishes only.
    let (s, v) = as_token(
        &public,
        &token,
        "/api/artifacts",
        json!({"title": "X", "files": files(&[("index.html", "x")])}),
        Some(&key),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["artifact"]["id"], rid.as_str(), "the key's own artifact");
}

#[tokio::test]
async fn a_link_says_when_its_remote_signed_this_clax_out() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = local
        .publish("Plan", &[("index.html", "<h1>v1</h1>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    let links = |v: &Value| v["links"][0]["remote_status"].as_str().map(str::to_string);
    let path = format!("/api/artifacts/{aid}/remotes");
    let (_, v) = local_req(&local, reqwest::Method::GET, &path, json!(null), None).await;
    assert_eq!(links(&v).as_deref(), Some("connected"), "{v}");
    // The person revokes the token there: the next push meets the refusal
    // and signs the remote out; the link stays, saying why it waits.
    public.revoke(&local, &rid).await;
    let push = format!("/api/artifacts/{aid}/push");
    let (s, v) = owner_post(&local, &push, json!({})).await;
    assert_eq!((s, code(&v)), (502, "remote_signed_out"), "{v}");
    assert!(local.store.remote_token(&rid).unwrap().is_none());
    let (_, v) = local_req(&local, reqwest::Method::GET, &path, json!(null), None).await;
    assert_eq!(links(&v).as_deref(), Some("signed_out"), "{v}");
    assert_eq!(v["remotes"], json!([]), "{v}");
    // A push names it signed out at once, as the Clax app's routes do.
    let (s, v) = owner_post(&local, &push, json!({"remote": rid})).await;
    assert_eq!((s, code(&v)), (502, "remote_signed_out"), "{v}");
    let (s, v) = local_req(
        &local,
        reqwest::Method::GET,
        &format!("/api/remotes/{rid}/apps"),
        json!(null),
        None,
    )
    .await;
    assert_eq!((s, code(&v)), (502, "remote_signed_out"), "{v}");
}

#[tokio::test]
async fn the_clax_apps_route_meeting_a_revoked_token_signs_the_remote_out() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    public.revoke(&local, &rid).await;
    let (s, v) = local_req(
        &local,
        reqwest::Method::GET,
        &format!("/api/remotes/{rid}/apps"),
        json!(null),
        None,
    )
    .await;
    assert_eq!((s, code(&v)), (502, "remote_signed_out"), "{v}");
    assert_eq!(
        local.store.find_remote(&rid).unwrap().unwrap().status,
        "signed_out"
    );
}

#[tokio::test]
async fn a_push_after_an_unlink_makes_a_new_public_copy() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = local
        .publish("Plan", &[("index.html", "<h1>v1</h1>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut made: Vec<String> = Vec::new();
    for _ in 0..3 {
        let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["pushed"], json!([{"n": 1, "remote_n": 1}]), "{v}");
        let raid = v["link"]["remote_artifact_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!made.contains(&raid), "{raid} made again: {made:?}");
        made.push(raid);
        let (s, v) = local_req(
            &local,
            reqwest::Method::DELETE,
            &format!("/api/artifacts/{aid}/remotes/{rid}"),
            json!(null),
            None,
        )
        .await;
        assert!((200..300).contains(&s), "{s} {v}");
    }
}

#[tokio::test]
async fn a_pages_assets_are_uploaded_and_its_references_rewritten() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    connect(&local, &public).await;
    let aid = local
        .publish("Logo", &[("index.html", "<p>soon</p>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let png: &[u8] = b"\x89PNG\r\n\x1a\nfake";
    let part = reqwest::multipart::Part::bytes(png.to_vec())
        .file_name("logo.png")
        .mime_str("image/png")
        .unwrap();
    let res = local
        .authed(
            local
                .client
                .post(format!("{}/api/artifacts/{aid}/assets", local.base))
                .multipart(reqwest::multipart::Form::new().part("file", part)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let asset: Value = res.json().await.unwrap();
    let asset_id = asset["asset"]["id"].as_str().unwrap().to_string();
    // An asset no version references, which stays here.
    let part = reqwest::multipart::Part::bytes(b"unused".to_vec())
        .file_name("x.txt")
        .mime_str("text/plain")
        .unwrap();
    local
        .authed(
            local
                .client
                .post(format!("{}/api/artifacts/{aid}/assets", local.base))
                .multipart(reqwest::multipart::Form::new().part("file", part)),
        )
        .send()
        .await
        .unwrap();
    let page = format!(
        "<img src=\"http://localhost:7480/_blob/{asset_id}\"><img src=\"/_blob/{asset_id}\">"
    );
    version(
        &local,
        &aid,
        1,
        json!({"files": files(&[("index.html", &page)])}),
        None,
    )
    .await;
    // A new public artifact whose first version loads an asset.
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["pushed"],
        json!([{"n": 1, "remote_n": 1}, {"n": 2, "remote_n": 2}])
    );
    assert_eq!(v["assets_not_pushed"], 1, "{v}");
    let rid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let rid_id = ArtifactId::parse(&rid).unwrap();
    let assets = public.ts.store.list_assets(&rid_id).unwrap();
    assert_eq!(assets.len(), 1, "uploaded once");
    let (bytes, _) = public
        .ts
        .store
        .read_file(&rid_id, 2, "index.html")
        .unwrap()
        .unwrap();
    let html = String::from_utf8(bytes).unwrap();
    assert!(!html.contains(&asset_id), "{html}");
    let url = clax_http::hosted::asset_url(&assets[0].id, false);
    assert_eq!(html.matches(&url).count(), 2, "{html} {url}");
    // The page's asset loads from its site on the public Clax.
    let res = public.ts.site_get(&rid, &url).await;
    assert_eq!(res.status(), 200);
    assert_eq!(&res.bytes().await.unwrap()[..], png);
    // A later version referencing it again uploads nothing new.
    version(
        &local,
        &aid,
        2,
        json!({"files": files(&[("index.html", &format!("{page}<p>2</p>"))])}),
        None,
    )
    .await;
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 3, "remote_n": 3}]));
    assert_eq!(public.ts.store.list_assets(&rid_id).unwrap().len(), 1);
    // The by-hand harness is another Clax's alone: not the daemon token's,
    // nor an agent token minted on People for an agent.
    let (s, v) = owner_post(
        &local,
        "/api/sessions",
        json!({"harness": "clax", "cwd": ""}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (s, minted) = public
        .browser(
            reqwest::Method::POST,
            "/api/hosted/agent-tokens",
            json!({"label": "agent"}),
        )
        .await;
    assert_eq!(s, 201, "{minted}");
    let (s, v) = json_of(
        public
            .ts
            .client
            .post(format!("{}/api/sessions", public.ts.base))
            .header(HOST, format!("localhost:{}", public.ts.addr.port()))
            .bearer_auth(minted["token"].as_str().unwrap())
            .json(&json!({"harness": "clax", "cwd": ""}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
}

#[tokio::test]
async fn a_refused_automatic_push_reaches_the_owners_inbox() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    connect(&local, &public).await;
    let aid = three_versions(&local).await;
    let (s, v) = owner_post(
        &local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"auto": true}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let rid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, _) = public
        .cli(
            reqwest::Method::POST,
            &format!("/api/artifacts/{rid}/versions"),
            Some(json!({"if_version": 3, "files": files(&[("index.html", "<h1>theirs</h1>")])})),
        )
        .await;
    assert_eq!(s, 201);
    version(
        &local,
        &aid,
        3,
        json!({"files": files(&[("index.html", "<h1>mine</h1>")])}),
        None,
    )
    .await;
    let mut item = Value::Null;
    for _ in 0..3000 {
        let (_, v) = local_req(
            &local,
            reqwest::Method::GET,
            "/api/inbox?kind=push",
            json!(null),
            None,
        )
        .await;
        if let Some(i) = v["items"].as_array().and_then(|a| a.first()) {
            item = i.clone();
            break;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(item["kind"], "push", "{item}");
    assert_eq!(item["push"]["n"], 4, "{item}");
    assert_eq!(item["push"]["code"], "remote_moved", "{item}");
    assert_eq!(item["push"]["origin"], public.origin.as_str(), "{item}");
    assert_eq!(item["artifact"]["id"], aid.as_str(), "{item}");
}

#[tokio::test]
async fn an_approval_pushes_the_versions_the_owner_was_asked_about() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    connect(&local, &public).await;
    let aid = three_versions(&local).await;
    let sid = local.register_session("claude", "h-9").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/artifacts/{aid}/push");
    let (s, v) = local_req(&local, reqwest::Method::POST, &path, json!({}), Some(&sid)).await;
    assert_eq!(s, 202, "{v}");
    let qid = v["question_id"].as_str().unwrap().to_string();
    local
        .answer_question(&qid, json!({"answers": [{"selected": ["Publish"]}]}))
        .await;
    // A version published after the owner answered is not theirs to push.
    version(
        &local,
        &aid,
        3,
        json!({"files": files(&[("index.html", "<h1>v4</h1>")])}),
        Some(&sid),
    )
    .await;
    let (s, v) = local_req(
        &local,
        reqwest::Method::POST,
        &path,
        json!({"question_id": qid}),
        Some(&sid),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"].as_array().unwrap().len(), 3, "{v}");
    assert_eq!(v["link"]["pushed_n"], 3, "{v}");
}

#[tokio::test]
async fn removing_a_remote_revokes_its_token_there_or_says_it_could_not() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let remote = connect(&local, &public).await;
    let gid = remote["grant_id"].as_str().unwrap().to_string();
    let id = remote["id"].as_str().unwrap().to_string();
    let (s, v) = local_req(
        &local,
        reqwest::Method::DELETE,
        &format!("/api/remotes/{id}"),
        json!(null),
        None,
    )
    .await;
    assert_eq!((s, v["revoked"].as_bool()), (200, Some(true)), "{v}");
    assert_ne!(public.ts.store.grant(&gid).unwrap().status, "approved");
    // Revoked there already: forgotten here all the same.
    let remote = connect(&local, &public).await;
    let gid = remote["grant_id"].as_str().unwrap().to_string();
    let (s, _) = public
        .browser(
            reqwest::Method::DELETE,
            &format!("/api/hosted/agent-tokens/{gid}"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200);
    let id = remote["id"].as_str().unwrap().to_string();
    let (s, v) = local_req(
        &local,
        reqwest::Method::DELETE,
        &format!("/api/remotes/{id}"),
        json!(null),
        None,
    )
    .await;
    assert_eq!((s, v["revoked"].as_bool()), (200, Some(false)), "{v}");
    assert!(local.store.remotes().unwrap().is_empty());
}

#[tokio::test]
async fn a_sign_in_approved_after_its_remote_was_removed_revokes_its_token_there() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let (s, v) = post(&local, "/api/remotes", json!({"url": public.origin})).await;
    assert_eq!(s, 201, "{v}");
    let id = v["remote"]["id"].as_str().unwrap().to_string();
    let uc = v["user_code"].as_str().unwrap().to_string();
    let (s, v) = public
        .browser(
            reqwest::Method::POST,
            &format!("/api/hosted/device/{uc}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    public.clock.advance(5);
    // The poll's token request is answered...
    let r = local.store.find_remote(&id).unwrap().unwrap();
    let code = local.store.remote_device_code(&id).unwrap().unwrap();
    let (s, v) = public
        .cli(
            reqwest::Method::POST,
            "/api/hosted/device/token",
            Some(json!({"device_code": code.expose()})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let gid = v["grant_id"].as_str().unwrap().to_string();
    assert_eq!(public.ts.store.grant(&gid).unwrap().status, "approved");
    // ...and the remote is removed before the poll connects it.
    local.store.remove_remote(&id).unwrap();
    let signed = clax_store::remotes::SignedInAs {
        token: clax_core::Secret::new(v["token"].as_str().unwrap().to_string()),
        person_public_id: v["person"]["public_id"].as_str().unwrap().to_string(),
        person_name: None,
        grant_id: gid.clone(),
    };
    let out = clax_server::public::finish_sign_in(&local.state, &r, signed).await;
    assert_eq!(out.err().map(|e| e.code), Some("not_found"));
    assert_ne!(
        public.ts.store.grant(&gid).unwrap().status,
        "approved",
        "the token no one holds is revoked there"
    );
}

#[tokio::test]
async fn an_agents_cli_push_is_refused_and_pointed_at_publish_public() {
    let local = TestServer::spawn().await;
    let aid = local.publish("P", &[("index.html", "x")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = json_of(
        local
            .authed(
                local
                    .client
                    .post(format!("{}/api/artifacts/{aid}/push", local.base)),
            )
            .header("x-clax-channel", "agent")
            .json(&json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "no_session"), "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("publish_public")
    );
}

#[tokio::test]
async fn a_sign_in_names_the_page_to_return_to() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let port = local.addr.port();
    // A path on this daemon's own origin alone.
    for bad in ["https://evil.example/", "//evil.example/a", "a/x", "/a\\x"] {
        let (s, v) = owner_post(
            &local,
            "/api/remotes",
            json!({"url": public.origin, "return_to": bad}),
        )
        .await;
        assert_eq!((s, code(&v)), (400, "invalid_args"), "{bad}: {v}");
    }
    // Asked at a host the API answers that is not one of the shell's
    // origins, the sign-in starts without one.
    let res = local
        .authed(local.client.post(format!("{}/api/remotes", local.base)))
        .header(HOST, format!("[::1]:{port}"))
        .json(&json!({"url": public.origin, "return_to": "/a/x?publish=1"}))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!((s, &v["return_to_kept"]), (201, &json!(false)), "{v}");
    let uc = v["user_code"].as_str().unwrap();
    let (_, peek) = public
        .browser(
            reqwest::Method::GET,
            &format!("/api/hosted/device/{uc}"),
            json!(null),
        )
        .await;
    assert!(peek["return_to"].is_null(), "{peek}");
    let id = v["remote"]["id"].as_str().unwrap();
    let (s, _) = local_req(
        &local,
        reqwest::Method::DELETE,
        &format!("/api/remotes/{id}"),
        json!(null),
        None,
    )
    .await;
    assert!(s < 300, "{s}");
    // At its own origin, the approval page sends the person back there.
    connect_with(
        &local,
        &public,
        json!({"url": public.origin, "return_to": "/a/x?publish=1"}),
        Some(&format!("http://localhost:{port}/a/x?publish=1")),
    )
    .await;
}

#[tokio::test]
async fn a_clax_app_sign_in_names_its_kind_to_the_remote() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    // The Clax app's kind alone.
    let (s, v) = owner_post(
        &local,
        "/api/remotes",
        json!({"url": public.origin, "kind": "clax-cli"}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (s, v) = owner_post(
        &local,
        "/api/remotes",
        json!({"url": public.origin, "kind": "clax-app", "machine": "Clax on Pixel"}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let uc = v["user_code"].as_str().unwrap().to_string();
    let id = v["remote"]["id"].as_str().unwrap().to_string();
    // The approval page learns it is the Clax app's.
    let (s, peek) = public
        .browser(
            reqwest::Method::GET,
            &format!("/api/hosted/device/{uc}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 200, "{peek}");
    assert_eq!(
        (&peek["client_name"], &peek["client_kind"]),
        (&json!("Clax on Pixel"), &json!("clax-app")),
        "{peek}"
    );
    let (s, v) = public
        .browser(
            reqwest::Method::POST,
            &format!("/api/hosted/device/{uc}/approve"),
            json!({}),
        )
        .await;
    assert_eq!((s, &v["token"]["kind"]), (200, &json!("device")), "{v}");
    public.clock.advance(5);
    let (s, v) = owner_post(&local, &format!("/api/remotes/{id}/poll"), json!({})).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("connected")), "{v}");
    // Its token is the person's device's: it reaches their apps library.
    let token = token_of(&local);
    let gid = public.ts.store.bearer_grant(&token).unwrap().unwrap();
    assert!(public.ts.store.is_person_device_grant(&gid).unwrap());
    let (s, v) = json_of(
        public
            .ts
            .client
            .get(format!("{}/api/hosted/apps", public.ts.base))
            .header(HOST, format!("localhost:{}", public.ts.addr.port()))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, v), (200, json!([])));
    // A daemon's sign-in names no kind, and its token is an agent's.
    let other = TestServer::spawn().await;
    connect(&other, &public).await;
    let gid = public
        .ts
        .store
        .bearer_grant(&token_of(&other))
        .unwrap()
        .unwrap();
    assert!(!public.ts.store.is_person_device_grant(&gid).unwrap());
}
