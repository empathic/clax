//! Joined sites over HTTP (spec 2026-10-05-chrome-overlay-design §7.2,
//! owner decisions 2026-10-06): suggestions, joining, splitting and the
//! owner's answers; a joined site's listing, topics and watches.
use crate::common;
use clax_core::extension::extension_origin;
use clax_server::testing::{EventReader, FAKE_PNG};
use common::TestServer;
use reqwest::Method;
use serde_json::{Value, json};

const A: &str = "http://localhost:7702";
const B: &str = "http://localhost:7703";

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

/// Posts a comment on `url` as a viewer; answers `(page artifact ID,
/// thread ID)`.
async fn comment(ts: &TestServer, cookie: &str, url: &str, title: &str) -> (String, String) {
    let form = reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", title.to_string())
        .text(
            "anchor",
            json!({"kind": "element", "selector": "main", "file": "index.html"}).to_string(),
        )
        .text("body", format!("A note on {url}"))
        .text("pending", "[]")
        .text("snapshot", format!("<!doctype html><p>{url}"))
        .part(
            "clip",
            reqwest::multipart::Part::bytes(FAKE_PNG.to_vec())
                .mime_str("image/png")
                .unwrap(),
        );
    let res = ts
        .client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "{url}");
    let v: Value = res.json().await.unwrap();
    (
        v["page"]["artifact_id"].as_str().unwrap().into(),
        v["thread"]["id"].as_str().unwrap().into(),
    )
}

async fn json_of(res: reqwest::Response) -> (u16, Value) {
    let st = res.status().as_u16();
    (st, res.json().await.unwrap_or(Value::Null))
}

async fn site(ts: &TestServer, origin: &str) -> Value {
    json_of(
        ts.get_authed(&format!("/api/live/site?origin={}", enc(origin)))
            .await,
    )
    .await
    .1
}

/// Every thread ID of the site listing of `origin`.
async fn site_threads(ts: &TestServer, origin: &str) -> Vec<String> {
    let v = site(ts, origin).await;
    let mut out: Vec<String> = v["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["threads"].as_array().unwrap().clone())
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

async fn join(ts: &TestServer, origin: &str, with: &str) -> (u16, Value) {
    json_of(
        ts.post_json(
            "/api/live/sites/join",
            json!({"origin": origin, "with": with}),
        )
        .await,
    )
    .await
}

async fn suggest(ts: &TestServer, url: &str, title: &str) -> Value {
    json_of(
        ts.get_authed(&format!(
            "/api/live/sites/suggest?url={}&title={}",
            enc(url),
            enc(title)
        ))
        .await,
    )
    .await
    .1
}

#[tokio::test]
async fn a_suggested_join_lists_both_origins_threads_together_and_is_idempotent() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    let (a_home, t1) = comment(&ts, &v.cookie, &format!("{A}/"), "My App").await;
    let (_, t2) = comment(&ts, &v.cookie, &format!("{A}/settings"), "Settings").await;
    let (b_home, t3) = comment(&ts, &v.cookie, &format!("{B}/"), "My App").await;
    let (_, t4) = comment(&ts, &v.cookie, &format!("{B}/about"), "About").await;
    // Clax turned on at B: A has a page of its path, so it is suggested.
    let s = suggest(&ts, &format!("{B}/settings"), "").await;
    assert_eq!(s["origin"], B);
    assert_eq!(s["site"]["joined"], false);
    assert_eq!(s["suggestions"][0]["origin"], A, "{s}");
    assert_eq!(s["suggestions"][0]["reason"], "path");
    // Not another host family.
    let other = suggest(&ts, "http://example.com/settings", "").await;
    assert_eq!(other["suggestions"], json!([]));
    // Before: separate sites.
    assert_eq!(site_threads(&ts, B).await.len(), 2);

    let (st, j) = join(&ts, B, A).await;
    assert_eq!(st, 200, "{j}");
    assert_eq!(j["joined"], true);
    assert_eq!(j["remaining"], 0);
    assert_eq!(j["site"]["key"], A);
    assert_eq!(j["site"]["name"], B, "named after its newest origin");
    assert_eq!(j["moved"], json!([t3.as_str()]));
    let origins: Vec<&str> = j["site"]["origins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["origin"].as_str().unwrap())
        .collect();
    assert_eq!(origins, vec![B, A]);

    // Threads from both origins list together, from either.
    let mut all = vec![t1.clone(), t2.clone(), t3.clone(), t4.clone()];
    all.sort();
    assert_eq!(site_threads(&ts, A).await, all);
    assert_eq!(site_threads(&ts, B).await, all);
    let listing = site(&ts, B).await;
    assert_eq!(listing["site"]["key"], A);
    assert_eq!(listing["origin"], B);
    // B's / page was merged into the site's once empty, and kept whole: its
    // link and its snapshot (which no thread names now) are still served,
    // and it is out of the gallery's listing.
    let (st, b) = json_of(ts.get_authed(&format!("/api/artifacts/{b_home}")).await).await;
    assert_eq!(st, 200);
    assert_eq!(b["artifact"]["live"]["merged_into"], a_home.as_str());
    let snap = ts
        .get_authed(&format!(
            "/api/artifacts/{b_home}/versions/1/files/index.html"
        ))
        .await;
    assert_eq!(snap.status(), 200);
    assert!(snap.text().await.unwrap().contains(&format!("{B}/")));
    let list = json_of(ts.get_authed("/api/artifacts").await).await.1;
    assert!(
        list["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["id"] != b_home.as_str())
    );
    // A lookup from B finds the site's page of the path; a comment from B lands there.
    let p = json_of(
        ts.get_authed(&format!("/api/live/pages?url={}", enc(&format!("{B}/"))))
            .await,
    )
    .await
    .1;
    assert_eq!(p["page"]["artifact_id"], a_home.as_str());
    let (page, _) = comment(&ts, &v.cookie, &format!("{B}/settings"), "Settings").await;
    assert_eq!(
        page,
        site(&ts, A).await["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["page"]["path"] == "/settings")
            .unwrap()["page"]["artifact_id"]
            .as_str()
            .unwrap()
    );
    // A move from a page made on B to one made on A is within the site.
    let res = ts
        .post_json(
            &format!("/api/live/threads/{t4}/move"),
            json!({"page_url": format!("{A}/settings")}),
        )
        .await;
    assert_eq!(res.status(), 200);

    // Again: nothing changes.
    let (st, again) = join(&ts, B, A).await;
    assert_eq!(st, 200);
    assert_eq!(
        (again["joined"].clone(), again["remaining"].clone()),
        (json!(false), json!(0))
    );
    assert_eq!(again["moved"], json!([]));
    // A joined origin is suggested nothing.
    assert_eq!(
        suggest(&ts, &format!("{B}/"), "").await["suggestions"],
        json!([])
    );
    // The gallery's listing has one entry for the site.
    let sites = json_of(ts.get_authed("/api/live/sites").await).await.1;
    let list = sites["sites"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{sites}");
    assert_eq!(list[0]["site"]["name"], B);
    assert_eq!(list[0]["threads"], 5);
    // A live page's view names its site's origins and opens on the newest.
    let art = json_of(ts.get_authed(&format!("/api/artifacts/{a_home}")).await)
        .await
        .1;
    assert_eq!(art["artifact"]["live"]["origins"], json!([B, A]));
    assert_eq!(art["artifact"]["live"]["page_url"], format!("{B}/"));
}

#[tokio::test]
async fn a_watch_on_one_origin_hears_a_comment_on_another_of_its_site() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    comment(&ts, &v.cookie, &format!("{A}/"), "App").await;
    let s = ts.register_session("claude", "sites-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/live-watches", ts.base)),
        )
        .json(&json!({"url": format!("{B}/")}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let w: Value = res.json().await.unwrap();
    assert_eq!(w["site"]["joined"], false);
    let (st, _) = join(&ts, B, A).await;
    assert_eq!(st, 200);
    // The watch made on B now names the site and both origins.
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/live-watches", ts.base)),
        )
        .json(&json!({"url": format!("{B}/")}))
        .send()
        .await
        .unwrap();
    let w: Value = res.json().await.unwrap();
    assert_eq!(w["site"]["joined"], true);
    assert_eq!(w["site"]["origins"].as_array().unwrap().len(), 2);
    // A comment on A, sent to the agent, reaches the session watching B.
    let (aid, tid) = comment(&ts, &v.cookie, &format!("{A}/later"), "Later").await;
    ts.send_thread(&aid, &tid).await;
    let fb: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=wait&wait=1"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(fb["feedback"][0]["thread_id"], tid.as_str(), "{fb}");
}

/// A stream opened with the token, subscribed to `topic`.
async fn stream(ts: &TestServer, topic: &str) -> EventReader {
    let res = ts
        .authed(ts.client.get(format!("{}/api/stream", ts.base)))
        .send()
        .await
        .unwrap();
    let mut events = EventReader::from_response(res);
    let sid = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .authed(ts.client.post(format!("{}/api/stream/{sid}", ts.base)))
        .json(&json!({"subscribe": [topic]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{topic}");
    events
}

#[tokio::test]
async fn each_origins_site_topic_hears_the_whole_site_and_its_changes() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    comment(&ts, &v.cookie, &format!("{A}/"), "App").await;
    let mut on_b = stream(&ts, &format!("site:{B}")).await;
    join(&ts, B, A).await;
    let ev = on_b.next_named("site").await;
    assert_eq!(ev["site"], A);
    assert_eq!(ev["origins"], json!([B, A]));
    let (_, tid) = comment(&ts, &v.cookie, &format!("{A}/x"), "X").await;
    let ev = on_b.next_named("thread").await;
    assert_eq!(ev["topic"], format!("site:{B}"));
    assert_eq!(ev["thread"]["id"], tid.as_str());
    let res = ts
        .post_json("/api/live/sites/split", json!({"origin": B}))
        .await;
    let (st, sp) = json_of(res).await;
    assert_eq!(st, 200);
    assert_eq!(sp["split"], true);
    assert_eq!(sp["site"]["key"], A);
    assert_eq!(sp["site"]["joined"], false);
    let ev = on_b.next_named("site").await;
    assert_eq!(ev["left"], json!([B]));
}

#[tokio::test]
async fn a_split_origin_keys_its_own_pages_again_and_the_site_keeps_its_history() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    let (_, t1) = comment(&ts, &v.cookie, &format!("{A}/"), "App").await;
    let (_, t2) = comment(&ts, &v.cookie, &format!("{B}/b"), "B").await;
    join(&ts, B, A).await;
    let res = ts
        .post_json("/api/live/sites/split", json!({"origin": B}))
        .await;
    assert_eq!(res.status(), 200);
    // Split again: nothing to do.
    let (st, sp) = json_of(
        ts.post_json("/api/live/sites/split", json!({"origin": B}))
            .await,
    )
    .await;
    assert_eq!((st, sp["split"].clone()), (200, json!(false)));
    // The site keeps what it holds, B's page included.
    let mut both = vec![t1, t2];
    both.sort();
    assert_eq!(site_threads(&ts, A).await, both);
    // B's new comments are its own.
    let (_, t3) = comment(&ts, &v.cookie, &format!("{B}/"), "App").await;
    assert_eq!(site_threads(&ts, B).await, vec![t3]);
    // A split pair is not suggested again.
    assert_eq!(
        suggest(&ts, &format!("{B}/"), "App").await["suggestions"],
        json!([])
    );
}

#[tokio::test]
async fn answers_keep_a_pair_from_being_suggested() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    comment(&ts, &v.cookie, &format!("{A}/"), "App").await;
    assert_eq!(
        suggest(&ts, &format!("{B}/"), "").await["suggestions"][0]["origin"],
        A
    );
    let res = ts
        .post_json(
            "/api/live/sites/answer",
            json!({"origin": B, "with": A, "answer": "later"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(
        suggest(&ts, &format!("{B}/"), "").await["suggestions"],
        json!([])
    );
    let res = ts
        .post_json(
            "/api/live/sites/answer",
            json!({"origin": B, "with": A, "answer": "always"}),
        )
        .await;
    assert_eq!(res.status(), 400);
    // A manual join still works.
    let (st, _) = join(&ts, B, A).await;
    assert_eq!(st, 200);
}

#[tokio::test]
async fn joins_are_refused_for_the_daemons_own_origin_unknown_sites_and_one_origin() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    comment(&ts, &v.cookie, &format!("{A}/"), "App").await;
    let own = ts.base.clone();
    let code = |v: &Value| v["error"]["code"].as_str().unwrap_or("").to_string();
    let (st, e) = join(&ts, &own, A).await;
    assert_eq!((st, code(&e)), (400, "own_origin".into()));
    let (st, e) = join(&ts, A, &own).await;
    assert_eq!((st, code(&e)), (400, "own_origin".into()));
    let (st, e) = join(&ts, B, "http://localhost:9").await;
    assert_eq!((st, code(&e)), (400, "unknown_site".into()));
    let (st, e) = join(&ts, A, A).await;
    assert_eq!((st, code(&e)), (400, "same_origin".into()));
    let res = ts
        .client
        .post(format!("{}/api/live/sites/join", ts.base))
        .json(&json!({"origin": B, "with": A}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "only the owner joins");
}

#[tokio::test]
async fn the_sites_routes_are_hidden_from_the_lan_and_admitted_for_the_extension() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let (lc, lan) = ts.lan();
    let v = ts.viewer(Some("Ana")).await;
    comment(&ts, &v.cookie, &format!("{A}/"), "App").await;
    for (m, path, body) in [
        (Method::GET, "/api/live/sites".to_string(), None),
        (
            Method::GET,
            format!("/api/live/sites/suggest?url={}", enc(&format!("{B}/"))),
            None,
        ),
        (
            Method::POST,
            "/api/live/sites/join".into(),
            Some(json!({"origin": B, "with": A})),
        ),
        (
            Method::POST,
            "/api/live/sites/split".into(),
            Some(json!({"origin": B})),
        ),
        (
            Method::POST,
            "/api/live/sites/answer".into(),
            Some(json!({"origin": B, "with": A, "answer": "never"})),
        ),
    ] {
        let mut req = lc.request(m.clone(), format!("{lan}{path}"));
        if let Some(b) = &body {
            req = req.json(b);
        }
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 404, "{m} {path}");
    }
    // Nothing was written from the LAN.
    assert_eq!(
        suggest(&ts, &format!("{B}/"), "").await["suggestions"][0]["origin"],
        A
    );
    // The extension's credential is admitted.
    let c: Value = ts
        .post_json(
            "/api/extension/credentials",
            json!({"extension_id": ts.extension_id()}),
        )
        .await
        .json()
        .await
        .unwrap();
    let cred = c["credential"].as_str().unwrap().to_string();
    let ext = |m: Method, path: &str| {
        ts.client
            .request(m, format!("{}{path}", ts.base))
            .header("origin", extension_origin(&ts.extension_id()))
            .header("sec-fetch-site", "cross-site")
            .header("authorization", format!("Clax-Extension {cred}"))
    };
    let res = ext(
        Method::GET,
        &format!("/api/live/sites/suggest?url={}", enc(&format!("{B}/"))),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let res = ext(Method::POST, "/api/live/sites/join")
        .json(&json!({"origin": B, "with": A}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = ext(Method::GET, "/api/live/sites").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let res = ext(Method::POST, "/api/live/sites/split")
        .json(&json!({"origin": B}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

/// A server whose store the test can reach, to leave a join or an un-merge
/// half done as an interrupted client would.
async fn with_store() -> (TestServer, std::sync::Arc<clax_core::Store>) {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let s2 = slot.clone();
    let ts = TestServer::spawn_with(move |st| {
        *s2.lock().unwrap() = Some(st.store.clone());
    })
    .await;
    let store = slot.lock().unwrap().take().unwrap();
    (ts, store)
}

#[tokio::test]
async fn an_unfinished_join_is_listed_and_holds_off_other_joins_and_splits_until_finished() {
    let (ts, store) = with_store().await;
    let v = ts.viewer(Some("Ana")).await;
    comment(&ts, &v.cookie, &format!("{A}/x"), "X").await;
    comment(&ts, &v.cookie, &format!("{B}/x"), "X").await;
    comment(&ts, &v.cookie, &format!("{B}/x"), "X").await;
    comment(&ts, &v.cookie, "http://localhost:7704/y", "Y").await;
    // The join's first batch only, as a client that stopped would leave it.
    store.join_origins(B, A).unwrap();
    let (moves, _) = store.join_candidates(A, 1).unwrap();
    let how = clax_core::store::site::MoveBy {
        by: "viewer:x".into(),
        kind: clax_core::store::site::KIND_JOIN,
        rule_id: None,
    };
    store
        .refile_threads(&clax_core::audit::AuditCtx::DAEMON, &moves, &how, &[])
        .unwrap();
    // Its pending page is listed, marked, with what is left.
    let listing = site(&ts, A).await;
    assert_eq!(listing["site"]["joining"], 1, "{listing}");
    assert!(
        listing["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["page"]["pending"] == true)
    );
    let code = |v: &Value| v["error"]["code"].as_str().unwrap_or("").to_string();
    let (st, e) = join(&ts, "http://localhost:7704", A).await;
    assert_eq!((st, code(&e)), (409, "joining".into()));
    let (st, e) = json_of(
        ts.post_json("/api/live/sites/split", json!({"origin": B}))
            .await,
    )
    .await;
    assert_eq!((st, code(&e)), (409, "joining".into()));
    // Joining the same pair again finishes it; then the others go through.
    let (st, j) = join(&ts, B, A).await;
    assert_eq!((st, j["remaining"].clone()), (200, json!(0)));
    assert_eq!(site(&ts, A).await["site"]["joining"], 0);
    let (st, _) = join(&ts, "http://localhost:7704", A).await;
    assert_eq!(st, 200);
}

#[tokio::test]
async fn a_join_waits_for_an_unmerge_under_way() {
    let (ts, store) = with_store().await;
    let v = ts.viewer(Some("Ana")).await;
    comment(&ts, &v.cookie, &format!("{A}/users/1"), "U").await;
    comment(&ts, &v.cookie, &format!("{B}/"), "B").await;
    let res = ts
        .post_json(
            "/api/live/rules",
            json!({"origin": A, "pattern": "/users/:id"}),
        )
        .await;
    let rule: Value = res.json().await.unwrap();
    store
        .mark_rule_deleted(
            &clax_core::audit::AuditCtx::DAEMON,
            rule["rule"]["id"].as_str().unwrap(),
        )
        .unwrap();
    let (st, e) = join(&ts, B, A).await;
    assert_eq!((st, e["error"]["code"].clone()), (409, json!("unmerging")));
}

/// A comment posted on artifact `aid` the way the shell posts one.
async fn shell_thread(ts: &TestServer, cookie: &str, aid: &str) -> (u16, Value) {
    let form = reqwest::multipart::Form::new()
        .text("anchor", clax_server::testing::element_anchor().to_string())
        .text("body", "On the old page")
        .text("version", "1");
    json_of(
        ts.client
            .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
            .header("cookie", format!("clax_viewer={cookie}"))
            .multipart(form)
            .send()
            .await
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn a_merged_away_page_takes_no_thread_hides_from_the_lan_and_is_released_when_its_page_goes()
{
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let (lc, lan) = ts.lan();
    let v = ts.viewer(Some("Ana")).await;
    let (a_home, _) = comment(&ts, &v.cookie, &format!("{A}/"), "App").await;
    let (b_home, _) = comment(&ts, &v.cookie, &format!("{B}/"), "App").await;
    let (st, j) = join(&ts, B, A).await;
    assert_eq!((st, j["remaining"].clone()), (200, json!(0)));
    // A new thread goes to the page it was merged into, not here.
    let (st, e) = shell_thread(&ts, &v.cookie, &b_home).await;
    assert_eq!(st, 409, "{e}");
    assert_eq!(e["error"]["code"], "merged_away");
    assert_eq!(e["error"]["merged_into"], a_home.as_str());
    assert_eq!(e["error"]["page_url"], format!("{A}/"));
    // Hidden from LAN viewers, as any live page.
    for path in [format!("/api/artifacts/{b_home}"), format!("/a/{b_home}")] {
        let res = lc.get(format!("{lan}{path}")).send().await.unwrap();
        assert_eq!(res.status(), 404, "{path}");
    }
    // Deleting the page it was merged into releases it, in that delete:
    // listed, viewable and deletable again, still hidden from the LAN.
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{a_home}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let list = json_of(ts.get_authed("/api/artifacts").await).await.1;
    let back = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == b_home.as_str())
        .cloned()
        .expect("listed again");
    assert_eq!(back["live"]["page_url"], format!("{B}/"));
    assert!(back["live"].get("merged_into").is_none());
    let res = lc
        .get(format!("{lan}/api/artifacts/{b_home}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{b_home}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
}
