//! An artifact whose administrator sets its Content Security Policy to
//! `unrestricted` (`artifacts.content_security_policy`, `PATCH
//! /api/artifacts/<id> {content_security_policy}`,
//! `clax_server::routes::content_security_policy`): its own content keeps
//! only its framing, and its service worker's script is its own file;
//! everything else on its site, and every site at `self`, keeps the
//! lockdown and the kill switch.

use clax_store::site_sessions::ContentSecurityPolicy;
use clax_testkit::{TestServer, error_code as code};
use reqwest::Method;
use serde_json::{Value, json};

fn id(s: &str) -> clax_core::ArtifactId {
    clax_core::ArtifactId::parse(s).unwrap()
}

/// Whether `st` holds artifact `aid` at `unrestricted`.
fn unrestricted(st: &clax_store::Store, aid: &str) -> bool {
    st.content_security_policy(&id(aid)).unwrap() == ContentSecurityPolicy::Unrestricted
}

/// A page, a script, and a service worker of its own.
async fn page(ts: &TestServer, title: &str) -> String {
    ts.publish(
        title,
        &[
            ("index.html", "<h1>hi</h1>"),
            ("app.js", "export {}"),
            ("sw.js", "self.onfetch = () => {};"),
        ],
    )
    .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The owner's `PATCH /api/artifacts/<aid> {content_security_policy: v}` with the
/// token, confirmed in their browser when an agent is live.
async fn set(ts: &TestServer, aid: &str, v: &str) -> (u16, Value) {
    let res = ts
        .confirmed(
            Method::PATCH,
            &format!("/api/artifacts/{aid}"),
            json!({"content_security_policy": v}),
        )
        .await;
    let s = res.status().as_u16();
    (s, res.json().await.unwrap_or_default())
}

/// The `Content-Security-Policy` headers of `res`.
fn policies(res: &reqwest::Response) -> Vec<String> {
    res.headers()
        .get_all("content-security-policy")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

/// Whether `p` is the lockdown after a framing part.
fn lockdown(p: &str) -> bool {
    p.starts_with("frame-ancestors 'self' ")
        && p.contains("connect-src 'self';")
        && p.contains("form-action 'none'")
}

/// Whether `p` is a framing part alone.
fn framing_alone(p: &str) -> bool {
    p.starts_with("frame-ancestors 'self' ") && !p.contains(';')
}

/// A service worker's script request for `path` on `aid`'s site, with the
/// owner's site session.
async fn worker_script(ts: &TestServer, aid: &str, path: &str) -> reqwest::Response {
    ts.client
        .get(format!("{}{path}", ts.base))
        .header("host", ts.site_host(aid))
        .header("cookie", ts.site_cookie(aid).await)
        .header("service-worker", "script")
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn an_artifact_is_self_until_its_owner_serves_it_unrestricted() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Site").await;
    let got = ts.get_authed(&format!("/api/artifacts/{aid}")).await;
    let v: Value = got.json().await.unwrap();
    assert_eq!(v["artifact"]["content_security_policy"], "self", "{v}");
    let (s, v) = set(&ts, &aid, "unrestricted").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["artifact"]["content_security_policy"], "unrestricted",
        "{v}"
    );
    assert_eq!(v["artifact"]["site_access"], "private", "{v}");
    assert_eq!(v["site_url"], format!("http://{}/", ts.site_host(&aid)));
    let got = ts.get_authed(&format!("/api/artifacts/{aid}")).await;
    let v: Value = got.json().await.unwrap();
    assert_eq!(
        v["artifact"]["content_security_policy"], "unrestricted",
        "{v}"
    );
    let (s, v) = set(&ts, &aid, "open").await;
    assert_eq!((s, code(&v)), (400, "invalid_argument"));
    // One setting at a time.
    let (s, v) = ts
        .token_call(
            Method::PATCH,
            &format!("/api/artifacts/{aid}"),
            json!({"content_security_policy": "self", "site_access": "private"}),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"));
    // It is recorded as an update of the artifact.
    let rows: Vec<Value> = ts
        .store
        .events_after(0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == "artifact.update" && r.ids.artifact.as_deref() == Some(aid.as_str()))
        .map(|r| serde_json::from_str::<Value>(&r.body).unwrap()["fields"].clone())
        .collect();
    assert_eq!(rows, [json!({"content_security_policy": "unrestricted"})]);
    let (s, v) = set(&ts, &aid, "self").await;
    assert_eq!(
        (s, &v["artifact"]["content_security_policy"]),
        (200, &json!("self"))
    );
}

#[tokio::test]
async fn an_unrestricted_sites_content_keeps_only_its_framing_and_everything_else_the_lockdown() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Unrestricted").await;
    let content = ["/", "/app.js", "/_clax/v/1/", "/_clax/v/1/app.js"];
    let clax = [
        "/nope",
        "/_clax/v/1/missing.txt",
        "/_clax/healthz",
        "/_clax/bridge.js",
    ];
    for setting in ["self", "unrestricted", "self"] {
        let (s, v) = set(&ts, &aid, setting).await;
        assert_eq!(s, 200, "{v}");
        for path in content {
            let res = ts.site_get(&aid, path).await;
            assert_eq!(res.status(), 200, "{setting} {path}");
            let p = policies(&res);
            assert_eq!(p.len(), 1, "{setting} {path}: {p:?}");
            if setting == "unrestricted" {
                assert!(framing_alone(&p[0]), "{setting} {path}: {p:?}");
            } else {
                assert!(lockdown(&p[0]), "{setting} {path}: {p:?}");
            }
        }
        // Clax's documents on the site keep the lockdown under either.
        for path in clax {
            let res = ts.site_get(&aid, path).await;
            let p = policies(&res);
            assert!(
                p.iter().any(|p| lockdown(p)),
                "{setting} {path} ({}): {p:?}",
                res.status()
            );
            assert!(
                !p.iter().any(|p| framing_alone(p)),
                "{setting} {path}: {p:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_draft_follows_its_artifacts_content_security_policy() {
    let ts = TestServer::spawn().await;
    let sid = ts.register_session("claude", "cp-draft").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = ts.publish_as(&sid, "Drafted", "<h1>v1</h1>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/drafts/{aid}", ts.base)),
        )
        .json(&json!({"files": {"index.html": {"content": "<h1>v2</h1>", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    // The token is handed to the people who list the drafts, never to the
    // agent.
    let listed: Value = ts
        .client
        .get(format!("{}/api/artifacts/{aid}/drafts", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let d = &listed["drafts"][0];
    let url = format!(
        "/_clax/draft/{}/{}/",
        d["agent"].as_str().unwrap(),
        d["token"].as_str().unwrap()
    );
    let res = ts.site_get(&aid, &url).await;
    assert_eq!(res.status(), 200);
    assert!(lockdown(&policies(&res)[0]), "{:?}", policies(&res));
    let (s, v) = set(&ts, &aid, "unrestricted").await;
    assert_eq!(s, 200, "{v}");
    let res = ts.site_get(&aid, &url).await;
    assert_eq!(res.status(), 200);
    let p = policies(&res);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(framing_alone(&p[0]), "{p:?}");
}

#[tokio::test]
async fn an_unrestricted_sites_service_worker_is_its_own_until_it_is_self_again() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Worker").await;
    let kill = clax_http::content_policy::SERVICE_WORKER_KILL_SWITCH;
    let res = worker_script(&ts, &aid, "/sw.js").await;
    assert_eq!(res.text().await.unwrap(), kill, "self");
    set(&ts, &aid, "unrestricted").await;
    for path in ["/sw.js", "/_clax/v/1/sw.js"] {
        let res = worker_script(&ts, &aid, path).await;
        assert_eq!(res.status(), 200, "{path}");
        let p = policies(&res);
        assert!(framing_alone(&p[0]), "{path}: {p:?}");
        assert_eq!(
            res.text().await.unwrap(),
            "self.onfetch = () => {};",
            "{path}"
        );
    }
    // A path it has none at is a 404 of Clax's, never the kill switch.
    let res = worker_script(&ts, &aid, "/elsewhere.js").await;
    assert_eq!(res.status(), 404);
    assert!(policies(&res).iter().any(|p| lockdown(p)));
    // At `self` again, its worker's next update check gets the kill switch.
    set(&ts, &aid, "self").await;
    for path in ["/sw.js", "/_clax/v/1/sw.js", "/elsewhere.js"] {
        let res = worker_script(&ts, &aid, path).await;
        assert_eq!(res.status(), 200, "{path}");
        assert_eq!(res.text().await.unwrap(), kill, "{path}");
    }
}

#[tokio::test]
async fn only_who_administers_it_and_never_a_page_sets_it() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Mine").await;
    let path = format!("/api/artifacts/{aid}");
    // A connected person who did not make it.
    let sam = ts.person("Sam").await;
    let (s, v) = ts
        .as_browser(
            &format!("clax_viewer={}", sam.cookie),
            Method::PATCH,
            &path,
            json!({"content_security_policy": "unrestricted"}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "admin_only"), "{v}");
    // The owner's browser sends it alone, without the token.
    let (s, v) = ts
        .as_browser(
            &ts.owner_cookie(),
            Method::PATCH,
            &path,
            json!({"content_security_policy": "unrestricted"}),
        )
        .await;
    assert_eq!(
        (s, &v["artifact"]["content_security_policy"]),
        (200, &json!("unrestricted")),
        "{v}"
    );
    // A page never, to `unrestricted` or back.
    for v in ["unrestricted", "self"] {
        let res = ts
            .authed(ts.client.patch(format!("{}{path}", ts.base)))
            .header("x-clax-via", "page")
            .json(&json!({"content_security_policy": v}))
            .send()
            .await
            .unwrap();
        let s = res.status().as_u16();
        let body: Value = res.json().await.unwrap();
        assert_eq!((s, code(&body)), (403, "not_for_pages"), "{v}");
    }
    assert!(unrestricted(&ts.store, &aid));
}

#[tokio::test]
async fn the_owners_unrestricted_waits_for_their_browser_while_an_agent_is_live_and_self_never_asks()
 {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Held").await;
    ts.register_session("claude", "cp-confirm").await;
    let path = format!("/api/artifacts/{aid}");
    let (s, v) = ts
        .token_call(
            Method::PATCH,
            &path,
            json!({"content_security_policy": "unrestricted"}),
            None,
        )
        .await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(v["status"], "confirmation_pending", "{v}");
    assert_eq!(
        v["decision"],
        "serve “Held” without Clax's Content Security Policy, loading code from and sending what it reads to any site"
    );
    assert!(!unrestricted(&ts.store, &aid));
    let id = v["confirmation_id"].as_str().unwrap().to_string();
    ts.confirm(&id, true).await;
    let (s, v) = ts
        .token_call(
            Method::PATCH,
            &path,
            json!({"content_security_policy": "unrestricted"}),
            Some(&id),
        )
        .await;
    assert_eq!(
        (s, &v["artifact"]["content_security_policy"]),
        (200, &json!("unrestricted")),
        "{v}"
    );
    // Returning to `self` asks no one.
    let (s, v) = ts
        .token_call(
            Method::PATCH,
            &path,
            json!({"content_security_policy": "self"}),
            None,
        )
        .await;
    assert_eq!(
        (s, &v["artifact"]["content_security_policy"]),
        (200, &json!("self")),
        "{v}"
    );
}

/// `method path` as the owner's agent in session `sid`.
async fn as_agent(ts: &TestServer, sid: &str, path: &str, body: Value) -> (u16, Value) {
    let res = ts
        .authed(ts.client.patch(format!("{}{path}", ts.base)))
        .header("x-clax-session", sid)
        .json(&body)
        .send()
        .await
        .unwrap();
    let s = res.status().as_u16();
    (s, res.json().await.unwrap_or_default())
}

#[tokio::test]
async fn an_agents_unrestricted_waits_on_its_person_and_an_approval_is_used_once() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Asked").await;
    let sid = ts.register_session("claude", "cp-agent").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/artifacts/{aid}");
    let now_unrestricted = || unrestricted(&ts.store, &aid);
    let (s, v) = as_agent(
        &ts,
        &sid,
        &path,
        json!({"content_security_policy": "unrestricted"}),
    )
    .await;
    assert_eq!(
        (s, v["status"].as_str()),
        (202, Some("approval_pending")),
        "{v}"
    );
    let qid = v["question_id"].as_str().unwrap().to_string();
    assert!(!now_unrestricted(), "nothing changes before the answer");
    let q = ts.store.question(&qid).unwrap().unwrap();
    assert!(
        q.questions[0].question.contains("\"Asked\""),
        "{:?}",
        q.questions
    );
    let labels: Vec<&str> = q.questions[0]
        .options
        .iter()
        .map(|o| o.label.as_str())
        .collect();
    assert_eq!(labels, ["Serve it unrestricted", "Keep it to its own site"]);
    // Asking again waits on the same question; with it, still waiting.
    let (_, again) = as_agent(
        &ts,
        &sid,
        &path,
        json!({"content_security_policy": "unrestricted"}),
    )
    .await;
    assert_eq!(again["question_id"], qid.as_str());
    let body = json!({"content_security_policy": "unrestricted", "question_id": qid});
    let (s, v) = as_agent(&ts, &sid, &path, body.clone()).await;
    assert_eq!(
        (s, v["status"].as_str()),
        (202, Some("approval_pending")),
        "{v}"
    );
    // The question opening a site to the world is not this one.
    let (s, v) = as_agent(
        &ts,
        &sid,
        &path,
        json!({"site_access": "world", "question_id": qid}),
    )
    .await;
    assert_eq!((s, code(&v)), (404, "unknown_question"), "{v}");
    ts.answer_question(
        &qid,
        json!({"answers": [{"selected": ["Serve it unrestricted"]}]}),
    )
    .await;
    let (s, v) = as_agent(&ts, &sid, &path, body.clone()).await;
    assert_eq!(
        (s, &v["artifact"]["content_security_policy"]),
        (200, &json!("unrestricted")),
        "{v}"
    );
    assert!(now_unrestricted());
    // Returning it to `self` asks no one; the approval is spent.
    let (s, _) = as_agent(&ts, &sid, &path, json!({"content_security_policy": "self"})).await;
    assert_eq!(s, 200);
    assert!(!now_unrestricted());
    let (s, v) = as_agent(&ts, &sid, &path, body).await;
    assert_eq!((s, code(&v)), (409, "approval_used"), "{v}");
    // Keeping it to its own site refuses.
    let (_, v) = as_agent(
        &ts,
        &sid,
        &path,
        json!({"content_security_policy": "unrestricted"}),
    )
    .await;
    let qid = v["question_id"].as_str().unwrap().to_string();
    ts.answer_question(
        &qid,
        json!({"answers": [{"selected": ["Keep it to its own site"]}]}),
    )
    .await;
    let (s, v) = as_agent(
        &ts,
        &sid,
        &path,
        json!({"content_security_policy": "unrestricted", "question_id": qid}),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_approved"), "{v}");
    assert!(!now_unrestricted());
}

#[tokio::test]
async fn a_fork_starts_self() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Source").await;
    set(&ts, &aid, "unrestricted").await;
    let res = ts
        .post_json(&format!("/api/artifacts/{aid}/fork"), json!({}))
        .await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    let fid = v["artifact"]["id"].as_str().unwrap().to_string();
    assert!(!unrestricted(&ts.store, &fid));
    let res = ts.site_get(&fid, "/").await;
    assert!(lockdown(&policies(&res)[0]), "{:?}", policies(&res));
}

#[tokio::test]
async fn a_push_to_a_public_clax_starts_self() {
    use clax_testkit::remotes::{Public, connect, owner_post};
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    connect(&local, &public).await;
    let aid = page(&local, "Pushed").await;
    set(&local, &aid, "unrestricted").await;
    let (s, v) = owner_post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    let rid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!unrestricted(&public.ts.store, &rid));
    assert!(unrestricted(&local.store, &aid));
}

/// `GET path` on `aid`'s site with the owner's site session `cookie`,
/// naming `etag` in `If-None-Match` when given.
async fn site_get_if(
    ts: &TestServer,
    aid: &str,
    cookie: &str,
    path: &str,
    etag: Option<&str>,
) -> reqwest::Response {
    let mut r = ts
        .client
        .get(format!("{}{path}", ts.base))
        .header("host", ts.site_host(aid))
        .header("cookie", cookie);
    if let Some(e) = etag {
        r = r.header("if-none-match", e);
    }
    r.send().await.unwrap()
}

/// A response's `Cache-Control` and `ETag`.
fn caching(res: &reqwest::Response) -> (String, Option<String>) {
    let h = |n: &str| {
        res.headers()
            .get(n)
            .map(|v| v.to_str().unwrap().to_string())
    };
    (h("cache-control").unwrap_or_default(), h("etag"))
}

#[tokio::test]
async fn an_unrestricted_sites_files_revalidate_and_a_copy_kept_from_then_is_never_not_modified_once_self()
 {
    let ts = TestServer::spawn().await;
    let aid = ts
        .publish(
            "Cached",
            &[
                ("index.html", "<h1>hi</h1>"),
                ("x.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            ],
        )
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let cookie = ts.site_cookie(&aid).await;
    // A version's page and its SVG (a document served raw), and the
    // current version's at the root.
    let paths = ["/_clax/v/1/", "/_clax/v/1/x.svg", "/", "/x.svg"];
    let mut at_self = Vec::new();
    for path in paths {
        let res = site_get_if(&ts, &aid, &cookie, path, None).await;
        assert_eq!(res.status(), 200, "{path}");
        assert!(lockdown(&policies(&res)[0]), "{path}");
        at_self.push(caching(&res));
    }
    // At `self`, a version's raw file is immutable and has no tag; the rest
    // revalidate.
    assert!(at_self[1].0.contains("immutable"), "{at_self:?}");
    assert_eq!(at_self[1].1, None, "{at_self:?}");
    for i in [0, 2, 3] {
        assert!(at_self[i].0.contains("no-cache"), "{at_self:?}");
    }

    let (s, v) = set(&ts, &aid, "unrestricted").await;
    assert_eq!(s, 200, "{v}");
    let mut unrestricted_tags = Vec::new();
    for (path, before) in paths.iter().zip(&at_self) {
        let res = site_get_if(&ts, &aid, &cookie, path, None).await;
        assert_eq!(res.status(), 200, "{path}");
        assert!(framing_alone(&policies(&res)[0]), "{path}");
        let (cc, tag) = caching(&res);
        // Revalidated on every use, never kept on its own.
        assert!(cc.contains("no-cache"), "{path}: {cc}");
        assert!(
            !cc.contains("immutable") && !cc.contains("max-age"),
            "{path}: {cc}"
        );
        // Its tag names the policy, so it is never the `self` copy's.
        let tag = tag.unwrap_or_else(|| panic!("{path}: no ETag"));
        assert_ne!(Some(&tag), before.1.as_ref(), "{path}");
        // While `unrestricted`, the copy is current.
        let res = site_get_if(&ts, &aid, &cookie, path, Some(&tag)).await;
        assert_eq!(res.status(), 304, "{path}");
        unrestricted_tags.push(tag);
    }

    // At `self` again: a copy kept from while `unrestricted` is answered
    // whole, with the lockdown, and caching is as it was before.
    let (s, v) = set(&ts, &aid, "self").await;
    assert_eq!(s, 200, "{v}");
    for ((path, tag), before) in paths.iter().zip(&unrestricted_tags).zip(&at_self) {
        let res = site_get_if(&ts, &aid, &cookie, path, Some(tag)).await;
        assert_eq!(res.status(), 200, "{path}");
        assert!(lockdown(&policies(&res)[0]), "{path}");
        assert_eq!(&caching(&res), before, "{path}");
    }
}

#[tokio::test]
async fn an_agent_with_no_session_serves_no_site_unrestricted() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Sessionless").await;
    let res = ts
        .authed(ts.client.patch(format!("{}/api/artifacts/{aid}", ts.base)))
        .header("x-clax-channel", "agent")
        .json(&json!({"content_security_policy": "unrestricted"}))
        .send()
        .await
        .unwrap();
    let s = res.status().as_u16();
    let v: Value = res.json().await.unwrap();
    assert_eq!((s, code(&v)), (400, "no_session"), "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("publish_public's content_security_policy"),
        "{v}"
    );
    assert!(!unrestricted(&ts.store, &aid));
}

#[tokio::test]
async fn a_persons_agent_serves_only_its_persons_clax_unrestricted_once_they_approve() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    // Sam may update every artifact, so only who administers it decides.
    let sam = ts.connect_person("Sam", "clax:full").await;
    let sid = sam.register("claude", "cp-sam").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let call = |path: String, body: Value| {
        let (sam, sid) = (&sam, sid.clone());
        async move {
            let res = sam.agent("PATCH", &path, Some(&sid), Some(body)).await;
            let s = res.status().as_u16();
            (s, res.json::<Value>().await.unwrap_or_default())
        }
    };
    // The owner's clax is not Sam's to change.
    let owners = page(&ts, "The owner's").await;
    let (s, v) = call(
        format!("/api/artifacts/{owners}"),
        json!({"content_security_policy": "unrestricted"}),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "admin_only"), "{v}");
    // Sam's own clax waits on Sam.
    let res = sam
        .agent(
            "POST",
            "/api/artifacts",
            Some(&sid),
            Some(json!({"title": "Sam's", "files": {"index.html": {"content": "<p>s</p>", "encoding": "utf8"}}})),
        )
        .await;
    assert_eq!(res.status(), 201);
    let mine = res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/artifacts/{mine}");
    let (s, v) = call(
        path.clone(),
        json!({"content_security_policy": "unrestricted"}),
    )
    .await;
    assert_eq!(
        (s, v["status"].as_str()),
        (202, Some("approval_pending")),
        "{v}"
    );
    let qid = v["question_id"].as_str().unwrap().to_string();
    assert!(!unrestricted(&ts.store, &mine));
    // Sam answers in his browser.
    let viewer = ts.store.viewer_of_grant(&sam.grant_id()).unwrap();
    let (lan, base) = ts.lan();
    let res = lan
        .post(format!("{base}/api/questions/{qid}/answer"))
        .header(
            "cookie",
            format!("clax_viewer={}", ts.browser_cookie(&viewer.id)),
        )
        .json(&json!({"answers": [{"selected": ["Serve it unrestricted"]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    let (s, v) = call(
        path,
        json!({"content_security_policy": "unrestricted", "question_id": qid}),
    )
    .await;
    assert_eq!(
        (s, &v["artifact"]["content_security_policy"]),
        (200, &json!("unrestricted")),
        "{v}"
    );
}

#[tokio::test]
async fn an_approval_to_open_a_site_is_not_one_to_serve_it_unrestricted() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Opened").await;
    let sid = ts.register_session("claude", "cp-world").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/artifacts/{aid}");
    let (s, v) = as_agent(&ts, &sid, &path, json!({"site_access": "world"})).await;
    assert_eq!(s, 202, "{v}");
    let qid = v["question_id"].as_str().unwrap().to_string();
    ts.answer_question(
        &qid,
        json!({"answers": [{"selected": ["Open to the world"]}]}),
    )
    .await;
    let (s, v) = as_agent(
        &ts,
        &sid,
        &path,
        json!({"content_security_policy": "unrestricted", "question_id": qid}),
    )
    .await;
    assert_eq!((s, code(&v)), (404, "unknown_question"), "{v}");
    assert!(!unrestricted(&ts.store, &aid));
    // It still opens the site, once.
    let (s, v) = as_agent(
        &ts,
        &sid,
        &path,
        json!({"site_access": "world", "question_id": qid}),
    )
    .await;
    assert_eq!(
        (s, &v["artifact"]["site_access"]),
        (200, &json!("world")),
        "{v}"
    );
}

#[tokio::test]
async fn a_page_never_opens_or_closes_its_site() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Page").await;
    let path = format!("/api/artifacts/{aid}");
    for v in ["world", "private"] {
        let res = ts
            .authed(ts.client.patch(format!("{}{path}", ts.base)))
            .header("x-clax-via", "page")
            .json(&json!({"site_access": v}))
            .send()
            .await
            .unwrap();
        let s = res.status().as_u16();
        let body: Value = res.json().await.unwrap();
        assert_eq!((s, code(&body)), (403, "not_for_pages"), "{v}: {body}");
    }
    assert!(!ts.store.site_open_to_world(&id(&aid)).unwrap());
}

#[tokio::test]
async fn a_live_pages_snapshot_keeps_its_snapshot_policy_when_unrestricted() {
    let ts = TestServer::spawn().await;
    let aid = page(&ts, "Live").await;
    ts.state.live_ids.insert(&aid, "http://localhost:5173");
    let host = ts.site_host(&aid);
    let cookie = ts.site_cookie(&aid).await;
    let mut was = Vec::new();
    for setting in ["self", "unrestricted"] {
        let (s, v) = set(&ts, &aid, setting).await;
        assert_eq!(s, 200, "{v}");
        for path in ["/", "/_clax/v/1/"] {
            let res = site_get_if(&ts, &aid, &cookie, path, None).await;
            assert_eq!(res.status(), 200, "{setting} {path}");
            let p = policies(&res);
            assert_eq!(p.len(), 2, "{setting} {path}: {p:?}");
            assert!(framing_alone(&p[0]), "{setting} {path}: {p:?}");
            assert!(
                p[1].starts_with(&format!("script-src http://{host}/_clax/;"))
                    && p[1].contains("connect-src 'none'"),
                "{setting} {path}: {p:?}"
            );
            was.push((path, caching(&res)));
        }
    }
    // Its caching does not change with the setting either.
    assert_eq!(was[..2], was[2..]);
}

/// `POST /api/drafts` with the token: a draft artifact of `of` (or of a
/// new app), offline unless `inherit_net`; its ID.
async fn draft_artifact(ts: &TestServer, of: Option<&str>, inherit_net: bool) -> String {
    let res = ts
        .authed(ts.client.post(format!("{}/api/drafts", ts.base)))
        .json(
            &json!({"of": of, "title": "Draft", "inherit_net": inherit_net,
                      "files": {"index.html": "<p>draft</p>"}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["draft"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_draft_of_an_app_follows_its_live_app_and_any_other_draft_is_self() {
    let ts = TestServer::spawn().await;
    let app = page(&ts, "App").await;
    set(&ts, &app, "unrestricted").await;
    let follows = draft_artifact(&ts, Some(&app), true).await;
    // A new app's draft has no live app; a bundle update's check draft is
    // made offline.
    let new_app = draft_artifact(&ts, None, true).await;
    let offline = draft_artifact(&ts, Some(&app), false).await;
    let served = |d: String| {
        let ts = &ts;
        async move { policies(&ts.site_get(&d, "/").await) }
    };
    assert!(unrestricted(&ts.store, &follows));
    assert!(framing_alone(&served(follows.clone()).await[0]));
    for d in [&new_app, &offline] {
        assert!(!unrestricted(&ts.store, d), "{d}");
        assert!(lockdown(&served(d.clone()).await[0]), "{d}");
    }
    // At `self` again, the app's draft is too.
    set(&ts, &app, "self").await;
    assert!(!unrestricted(&ts.store, &follows));
    assert!(lockdown(&served(follows).await[0]));
}

/// A draft follows its app's setting, so it takes none of its own: an
/// agent's or the owner's `content_security_policy` on a draft, either way, is
/// refused 403 `draft`, and its own column stays `self` for when its app
/// is gone.
#[tokio::test]
async fn a_draft_takes_no_content_security_policy_of_its_own() {
    let ts = TestServer::spawn().await;
    let app = page(&ts, "App").await;
    set(&ts, &app, "unrestricted").await;
    let draft = draft_artifact(&ts, Some(&app), true).await;
    let sid = ts.register_session("claude", "cp-draft-own").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/artifacts/{draft}");
    for v in ["unrestricted", "self"] {
        let (s, body) = as_agent(&ts, &sid, &path, json!({"content_security_policy": v})).await;
        assert_eq!(
            (s, body["error"]["code"].as_str()),
            (403, Some("draft")),
            "agent {v}: {body}"
        );
        let (s, body) = set(&ts, &draft, v).await;
        assert_eq!(
            (s, body["error"]["code"].as_str()),
            (403, Some("draft")),
            "owner {v}: {body}"
        );
    }
    assert!(unrestricted(&ts.store, &draft), "it follows its app");
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{app}", ts.base)))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    assert!(
        !unrestricted(&ts.store, &draft),
        "with its app gone, its own setting is self"
    );
}

#[tokio::test]
async fn an_apps_push_with_its_bundle_starts_self() {
    use clax_testkit::remotes::{Public, connect_app, post};
    let public = Public::new().await;
    let ada = TestServer::spawn().await;
    let rid = connect_app(&ada, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = page(&ada, "Tips").await;
    set(&ada, &aid, "unrestricted").await;
    let (s, v) = post(
        &ada,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "app", "version": 1}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let raid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let content = ada
        .store
        .get_version(&id(&aid), 1)
        .unwrap()
        .unwrap()
        .content_sha256
        .unwrap();
    let res = ada
        .authed(ada.client.put(format!(
            "{}/api/artifacts/{aid}/remotes/{rid}/bundle?remote_n=1",
            ada.base
        )))
        .header("content-type", "application/vnd.empathic.clax+zip")
        .header("clax-bundle-lineage", "l_abcdefghijklmnopqrstuvwxyz")
        .header("clax-bundle-version", "1")
        .header("clax-bundle-content-sha256", content)
        .body(b"bundle".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
    assert!(!unrestricted(&public.ts.store, &raid));
    assert!(unrestricted(&ada.store, &aid));
}
