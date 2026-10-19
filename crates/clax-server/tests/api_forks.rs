//! `POST /api/artifacts/<aid>/fork` on a local daemon: the owner, the
//! owner's agents, a connected person's browser within their rights, a
//! person's agent (whose view of the copy and its source is filtered), and
//! a guest, who may not.

use clax_core::rights::Rights;
use clax_testkit::TestServer;
use clax_testkit::people::World;
use serde_json::{Value, json};

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let s = r.status().as_u16();
    (s, r.json().await.unwrap_or(Value::Null))
}

/// `POST <path>` from the browser holding viewer cookie `cookie`.
async fn post_as(ts: &TestServer, cookie: &str, path: &str) -> (u16, Value) {
    json_of(
        ts.client
            .post(format!("{}{path}", ts.base))
            .header("cookie", format!("clax_viewer={cookie}"))
            .json(&json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await
}

/// `GET <path>` from the browser holding viewer cookie `cookie`.
async fn get_as(ts: &TestServer, cookie: &str, path: &str) -> (u16, Value) {
    json_of(
        ts.client
            .get(format!("{}{path}", ts.base))
            .header("cookie", format!("clax_viewer={cookie}"))
            .send()
            .await
            .unwrap(),
    )
    .await
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

async fn publish(ts: &TestServer, aid: &str, if_version: u32, html: &str) {
    let res = ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": if_version,
                   "files": {"index.html": {"content": html, "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn the_owner_forks_a_chosen_version_and_the_fork_names_its_source() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Plan", &[("index.html", "<p>one")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    publish(&ts, &aid, 1, "<p>two").await;
    ts.thread(&aid, 1, "a thought").await;
    let (s, v) = json_of(
        ts.post_json(&format!("/api/artifacts/{aid}/fork"), json!({"version": 1}))
            .await,
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let fid = v["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["url"], json!(format!("/a/{fid}")));
    assert_eq!(v["version"]["note"], json!(format!("Forked from {aid}@v1")));
    let lineage = json!({"artifact_id": aid, "version": 1, "title": "Plan"});
    assert_eq!(v["artifact"]["forked_from"], lineage);
    // The fork reads as an artifact of its own, with its source named.
    let (_, got) = json_of(ts.get(&format!("/api/artifacts/{fid}")).await).await;
    assert_eq!(got["artifact"]["forked_from"], lineage);
    assert_eq!(got["artifact"]["title"], "Plan");
    assert_eq!(got["artifact"]["may_fork"], false, "no credential, no fork");
    let (_, got) = json_of(ts.get_authed(&format!("/api/artifacts/{fid}")).await).await;
    assert_eq!(got["artifact"]["may_fork"], true);
    let body = ts
        .get(&format!("/api/artifacts/{fid}/versions/1/files/index.html"))
        .await
        .text()
        .await
        .unwrap();
    assert_eq!(body, "<p>one");
    let (_, threads) = json_of(ts.get(&format!("/api/artifacts/{fid}/threads")).await).await;
    assert_eq!(threads["threads"], json!([]));
    // The source is no fork.
    let (_, src) = json_of(ts.get(&format!("/api/artifacts/{aid}")).await).await;
    assert!(src["artifact"].get("forked_from").is_none(), "{src}");
    // The page's bootstrap names the source too.
    let page = ts.get(&format!("/a/{fid}")).await.text().await.unwrap();
    assert!(
        page.contains(&format!("\"forked_from\":{{\"artifact_id\":\"{aid}\"")),
        "{page}"
    );
}

#[tokio::test]
async fn an_agent_session_forks_and_watches_its_fork() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Plan", &[("index.html", "<p>one")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let sid = ts.register_session("claude", "fork-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/fork", ts.base))
                .header("x-clax-session", &sid)
                .json(&json!({})),
        )
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!(s, 201, "{v}");
    let fid = v["artifact"]["id"].as_str().unwrap();
    assert_eq!(v["artifact"]["owner_session_id"], json!(sid));
    let (_, w) = json_of(ts.get_authed(&format!("/api/sessions/{sid}/watches")).await).await;
    assert!(w.to_string().contains(fid), "{w}");
}

#[tokio::test]
async fn a_persons_browser_cannot_fork_as_an_owner_agent_session() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Plan", &[("index.html", "<p>one")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let sid = ts.register_session("claude", "fork-2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let rae = ts.person("Rae").await;
    let (s, v) = json_of(
        ts.client
            .post(format!("{}/api/artifacts/{aid}/fork", ts.base))
            .header("cookie", format!("clax_viewer={}", rae.cookie))
            .header("x-clax-session", &sid)
            .json(&json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let fid = v["artifact"]["id"].as_str().unwrap();
    let (_, got) = json_of(ts.get_authed(&format!("/api/artifacts/{fid}")).await).await;
    assert!(got["artifact"]["owner_session_id"].is_null(), "{got}");
    let (_, w) = json_of(ts.get_authed(&format!("/api/sessions/{sid}/watches")).await).await;
    assert!(!w.to_string().contains(fid), "{w}");
}

#[tokio::test]
async fn a_connected_persons_browser_forks_within_their_rights_and_a_guest_may_not() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Plan", &[("index.html", "<p>one")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/artifacts/{aid}/fork");
    let rae = ts.person("Rae").await;
    let (s, v) = post_as(&ts, &rae.cookie, &path).await;
    assert_eq!(s, 201, "{v}");
    let fid = v["artifact"]["id"].as_str().unwrap().to_string();
    let st = ts.store.clone();
    let (f, pid) = (fid.clone(), rae.public_id.clone());
    assert!(
        tokio::task::spawn_blocking(move || st.artifact_created_by(&f, Some(&pid)))
            .await
            .unwrap()
            .unwrap(),
        "the fork is Rae's"
    );
    // A reader creates nothing; rights that do not read the source find
    // no such artifact.
    let st = ts.store.clone();
    let pid = rae.public_id.clone();
    let narrow = json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": [fid]},
        {"type": "clax-artifacts", "actions": ["create"], "locations": ["*"]},
    ]);
    tokio::task::spawn_blocking(move || {
        let audit = clax_core::audit::AuditCtx::DAEMON;
        st.set_person_rights(&audit, &pid, Some(Rights::preset("clax:reader").unwrap()))
            .unwrap();
    })
    .await
    .unwrap();
    let (s, v) = post_as(&ts, &rae.cookie, &path).await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    let (_, v) = get_as(&ts, &rae.cookie, &format!("/api/artifacts/{aid}")).await;
    assert_eq!(
        v["artifact"]["may_fork"], false,
        "a reader's view offers no fork"
    );
    let st = ts.store.clone();
    let pid = rae.public_id.clone();
    tokio::task::spawn_blocking(move || {
        let audit = clax_core::audit::AuditCtx::DAEMON;
        st.set_person_rights(&audit, &pid, Some(Rights::parse(&narrow).unwrap()))
            .unwrap();
    })
    .await
    .unwrap();
    let (s, v) = post_as(&ts, &rae.cookie, &path).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    // Nor is the source named to her on her fork.
    let (s, v) = get_as(&ts, &rae.cookie, &format!("/api/artifacts/{fid}")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["artifact"]["forked_from"],
        json!({"artifact_id": aid, "version": 1, "title": null})
    );
    // A guest: a named viewer who is no connected person.
    let gil = ts.viewer(Some("Gil")).await;
    let (s, v) = post_as(&ts, &gil.cookie, &path).await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    let (_, v) = get_as(&ts, &gil.cookie, &format!("/api/artifacts/{aid}")).await;
    assert_eq!(
        v["artifact"]["may_fork"], false,
        "a guest's view offers no fork"
    );
    // No credential at all.
    let (s, _) = json_of(
        ts.client
            .post(format!("{}{path}", ts.base))
            .json(&json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 401);
}

#[tokio::test]
async fn a_persons_agent_forks_as_its_person_and_sees_what_it_may() {
    let w = World::two_people().await;
    let aid = &w.read.artifact;
    let (s, v) = json_of(
        w.sam
            .agent(
                "POST",
                &format!("/api/artifacts/{aid}/fork"),
                Some(&w.sam_session),
                Some(json!({})),
            )
            .await,
    )
    .await;
    assert_eq!(s, 201, "{v}");
    // Sam approved no one: Mia's words, copied into Sam's fork, stay hers
    // and withheld, the source's title too.
    let text = v.to_string();
    for c in w.canaries() {
        assert!(!text.contains(c), "{c} in {text}");
    }
    assert_eq!(v["artifact"]["forked_from"]["artifact_id"], json!(aid));
    let fid = v["artifact"]["id"].as_str().unwrap().to_string();
    let st = w.ts.store.clone();
    let sam =
        w.ts.store
            .viewer_of_grant(&w.sam.grant_id())
            .unwrap()
            .public_id;
    assert!(
        tokio::task::spawn_blocking(move || st.artifact_created_by(&fid, Some(&sam)))
            .await
            .unwrap()
            .unwrap(),
        "the fork is Sam's"
    );
    // Without a session, a person's agent writes nothing.
    let (s, v) = json_of(
        w.sam
            .agent(
                "POST",
                &format!("/api/artifacts/{aid}/fork"),
                None,
                Some(json!({})),
            )
            .await,
    )
    .await;
    assert_eq!((s, code(&v)), (400, "no_session"), "{v}");
}
