//! Site-wide threads of live pages (spec 2026-10-05-chrome-overlay-design
//! §7.1, §9.2, §9.5): the site listing, the `site:<origin>` stream topic,
//! moving a thread to another page, and merge rules.
mod common;
use clax_core::extension::extension_origin;
use clax_server::testing::{EventReader, FAKE_PNG};
use common::TestServer;
use reqwest::Method;
use serde_json::{Value, json};

const ORIGIN: &str = "http://localhost:5173";

fn form(url: &str, body: &str) -> reqwest::multipart::Form {
    reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", "Page")
        .text(
            "anchor",
            json!({"kind": "element", "selector": "main", "file": "index.html"}).to_string(),
        )
        .text("body", body.to_string())
        .text("pending", "[]")
        .text("snapshot", format!("<!doctype html><p>{url}"))
        .part(
            "clip",
            reqwest::multipart::Part::bytes(FAKE_PNG.to_vec())
                .mime_str("image/png")
                .unwrap(),
        )
}

/// Posts a comment on `url` as a viewer; answers `(page artifact ID, thread
/// ID, thread view)`.
async fn comment(ts: &TestServer, cookie: &str, url: &str, body: &str) -> (String, String, Value) {
    let res = ts
        .client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form(url, body))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "{url}");
    let v: Value = res.json().await.unwrap();
    (
        v["page"]["artifact_id"].as_str().unwrap().into(),
        v["thread"]["id"].as_str().unwrap().into(),
        v["thread"].clone(),
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

async fn site(ts: &TestServer) -> Value {
    let res = ts
        .get_authed(&format!("/api/live/site?origin={}", enc(ORIGIN)))
        .await;
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

async fn code(res: reqwest::Response) -> String {
    let v: Value = res.json().await.unwrap();
    v["error"]["code"].as_str().unwrap_or_default().to_string()
}

async fn move_to(ts: &TestServer, tid: &str, page_url: &str) -> reqwest::Response {
    ts.post_json(
        &format!("/api/live/threads/{tid}/move"),
        json!({"page_url": page_url}),
    )
    .await
}

async fn add_rule(ts: &TestServer, pattern: &str) -> reqwest::Response {
    ts.post_json(
        "/api/live/rules",
        json!({"origin": ORIGIN, "pattern": pattern}),
    )
    .await
}

async fn threads_of(ts: &TestServer, aid: &str) -> Vec<String> {
    let v: Value = ts
        .get_authed(&format!(
            "/api/artifacts/{aid}/threads?include_resolved=true"
        ))
        .await
        .json()
        .await
        .unwrap();
    v["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect()
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
async fn the_site_lists_its_pages_with_threads_newest_activity_first() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    let (a, ta, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/a"), "First on a").await;
    let (b, tb, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/b?tab=2"), "On b").await;
    comment(&ts, &v.cookie, "http://localhost:3000/a", "Elsewhere").await;
    // A resolve on /a makes it the newest.
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{a}/threads/{ta}/resolve",
            ts.base
        ))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    let s = site(&ts).await;
    assert_eq!(s["origin"], ORIGIN);
    assert_eq!(s["rules"], json!([]));
    let pages = s["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 2, "{s}");
    assert_eq!(pages[0]["page"]["artifact_id"], a.as_str());
    assert_eq!(pages[0]["page"]["path"], "/a");
    assert_eq!(
        pages[0]["summary"],
        json!({"open": 0, "addressed": 0, "resolved": 1,
               "last_activity": pages[0]["threads"][0]["resolved_at"]})
    );
    assert_eq!(pages[1]["page"]["artifact_id"], b.as_str());
    assert_eq!(pages[1]["summary"]["open"], 1);
    let t = &pages[1]["threads"][0];
    assert_eq!(t["id"], tb.as_str());
    assert_eq!(t["page_url"], format!("{ORIGIN}/b?tab=2"));
    assert_eq!(t["page_path"], "/b");
    assert_eq!(t["anchor"]["route"], "?tab=2");
    assert_eq!(t["status"], "open");
    assert!(t["addressed_pending"].is_null());
    assert_eq!(t["moves"], json!([]));
    assert_eq!(t["comments"][0]["body"], "On b");
    assert_eq!(t["comments"][0]["author_name"], "Ana");

    // Any URL of the origin names it; another origin lists only its own.
    let res = ts
        .get_authed(&format!(
            "/api/live/site?origin={}",
            enc(&format!("{ORIGIN}/x?y=1"))
        ))
        .await;
    let other: Value = res.json().await.unwrap();
    assert_eq!(other["pages"].as_array().unwrap().len(), 2);
    let res = ts
        .get_authed(&format!(
            "/api/live/site?origin={}",
            enc("http://localhost:3000")
        ))
        .await;
    let other: Value = res.json().await.unwrap();
    assert_eq!(other["pages"].as_array().unwrap().len(), 1);

    // Only the owner lists a site.
    let res = ts
        .client
        .get(format!("{}/api/live/site?origin={}", ts.base, enc(ORIGIN)))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let res = ts
        .client
        .get(format!("{}/api/live/site?origin={}", ts.base, enc(ORIGIN)))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "the owner's browser lists it");
    // The daemon's own origin is not a site.
    let res = ts
        .get_authed(&format!("/api/live/site?origin={}", enc(&ts.base)))
        .await;
    assert_eq!(code(res).await, "own_origin");
}

#[tokio::test]
async fn a_site_topic_carries_every_page_of_its_origin_and_nothing_else() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    // The topic needs a normalized origin.
    let res = ts
        .authed(ts.client.get(format!("{}/api/stream", ts.base)))
        .send()
        .await
        .unwrap();
    let mut probe = EventReader::from_response(res);
    let sid = probe.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    for bad in [
        "site:http://LOCALHOST:5173",
        "site:http://localhost:5173/",
        "site:localhost",
    ] {
        let res = ts
            .authed(ts.client.post(format!("{}/api/stream/{sid}", ts.base)))
            .json(&json!({"subscribe": [bad]}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400, "{bad}");
        assert_eq!(code(res).await, "invalid_topic", "{bad}");
    }

    let mut events = stream(&ts, &format!("site:{ORIGIN}")).await;
    comment(&ts, &v.cookie, "http://localhost:3000/", "Other origin").await;
    let (a, ta, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/a"), "On a").await;
    let (name, ev) = events.next().await;
    assert_eq!(name, "version", "{ev}");
    assert_eq!(ev["topic"], format!("site:{ORIGIN}"));
    assert_eq!(ev["artifact_id"], a.as_str());
    let (name, ev) = events.next().await;
    assert_eq!(name, "thread", "{ev}");
    assert_eq!(ev["thread"]["id"], ta.as_str());
    assert_eq!(ev["thread"]["page_url"], format!("{ORIGIN}/a"));
    assert_eq!(ev["thread"]["comment_count"], 1);
    let (_, b_t, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/b"), "On b").await;
    let (name, _) = events.next().await;
    assert_eq!(name, "version");
    let (_, ev) = events.next().await;
    assert_eq!(ev["thread"]["id"], b_t.as_str());
}

#[tokio::test]
async fn the_site_routes_and_topic_are_hidden_from_the_lan() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let (lc, lan) = ts.lan();
    let v = ts.viewer(Some("Ana")).await;
    let (_, tid, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/a"), "x").await;
    let site_url = format!("{lan}/api/live/site?origin={}", enc(ORIGIN));
    let rules_url = format!("{lan}/api/live/rules?origin={}", enc(ORIGIN));
    for (m, url, body) in [
        (Method::GET, site_url.clone(), None),
        (Method::GET, rules_url, None),
        (
            Method::POST,
            format!("{lan}/api/live/rules"),
            Some(json!({"origin": ORIGIN, "pattern": "/p/:p"})),
        ),
        (Method::DELETE, format!("{lan}/api/live/rules/x"), None),
        (
            Method::POST,
            format!("{lan}/api/live/threads/{tid}/move"),
            Some(json!({"page_url": format!("{ORIGIN}/b")})),
        ),
    ] {
        let mut req = lc.request(m.clone(), &url);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 404, "{m} {url}");
    }
    assert!(site(&ts).await["pages"][0]["threads"][0]["moves"] == json!([]));
    // The token from the LAN sees it.
    let st = ts.authed(lc.get(&site_url)).send().await.unwrap().status();
    assert_eq!(st, 200);
    // A LAN stream cannot take a site topic.
    let res = lc.get(format!("{lan}/api/stream")).send().await.unwrap();
    let mut events = EventReader::from_response(res);
    let sid = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let res = lc
        .post(format!("{lan}/api/stream/{sid}"))
        .json(&json!({"subscribe": [format!("site:{ORIGIN}")]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn moving_a_thread_refiles_it_and_tells_both_pages() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    let (a, tid, before) = comment(&ts, &v.cookie, &format!("{ORIGIN}/a?x=1"), "Wrong page").await;
    let mut events = stream(&ts, &format!("site:{ORIGIN}")).await;
    let mut old_page = stream(&ts, &format!("artifact:{a}")).await;
    let res = move_to(&ts, &tid, &format!("{ORIGIN}/b?tab=1")).await;
    assert_eq!(res.status(), 200);
    let m: Value = res.json().await.unwrap();
    assert_eq!(m["moved"], true);
    let b = m["page"]["artifact_id"].as_str().unwrap().to_string();
    assert_eq!(m["page"]["path"], "/b");
    let t = &m["thread"];
    assert_eq!(t["artifact_id"], b.as_str());
    assert_eq!(t["page_url"], format!("{ORIGIN}/b?tab=1"));
    assert_eq!(t["anchor"]["route"], "?tab=1");
    assert_eq!(t["comments"], before["comments"]);
    let owner = ts.owner_public_id().await;
    assert_eq!(t["moves"][0]["moved_by"], format!("viewer:{owner}"), "{t}");
    assert_eq!(t["moves"][0]["from_url"], format!("{ORIGIN}/a?x=1"));
    assert_eq!(t["moves"][0]["from_artifact_id"], a.as_str());
    assert!(t["moves"][0]["rule_id"].is_null());
    // The snapshot came along: the thread's version on /b holds /a's page.
    let n = t["version_n"].as_u64().unwrap();
    let html = ts
        .get(&format!("/c/{b}/v/{n}/"))
        .await
        .text()
        .await
        .unwrap();
    assert!(html.contains(&format!("{ORIGIN}/a?x=1")), "{html}");
    // So did the clip.
    let clip = ts.get(t["clip_url"].as_str().unwrap()).await;
    assert_eq!(clip.status(), 200);
    assert_eq!(threads_of(&ts, &a).await, Vec::<String>::new());
    assert_eq!(threads_of(&ts, &b).await, vec![tid.clone()]);

    // The new page's versions, the move on the old page, the thread on the new.
    let mut names = Vec::new();
    loop {
        let (name, ev) = events.next().await;
        names.push(name.clone());
        if name == "thread_moved" {
            assert_eq!(ev["artifact_id"], a.as_str());
            assert_eq!(ev["thread_id"], tid.as_str());
            assert_eq!(ev["to_artifact_id"], b.as_str());
        }
        if name == "thread" {
            assert_eq!(ev["artifact_id"], b.as_str());
            break;
        }
    }
    assert_eq!(names.last().unwrap(), "thread");
    assert!(names.contains(&"thread_moved".to_string()), "{names:?}");
    assert!(
        names
            .iter()
            .all(|n| n == "version" || n == "thread_moved" || n == "thread"),
        "the site gets no thread_deleted: {names:?}"
    );
    // The old page's own topic also gets thread_deleted, for clients that
    // know only that.
    let (name, ev) = old_page.next().await;
    assert_eq!(
        (name.as_str(), &ev["thread_id"]),
        ("thread_moved", &json!(tid))
    );
    let (name, ev) = old_page.next().await;
    assert_eq!(name, "thread_deleted");
    assert_eq!(ev["artifact_id"], a.as_str());
    assert_eq!(ev["thread_id"], tid.as_str());

    // Again: nothing to do.
    let m: Value = move_to(&ts, &tid, &format!("{ORIGIN}/b?tab=1"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(m["moved"], false);
    assert_eq!(m["thread"]["moves"].as_array().unwrap().len(), 1);
    // Only its route changes: still a move, on the same page.
    let m: Value = move_to(&ts, &tid, &format!("{ORIGIN}/b"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(m["moved"], true);
    assert_eq!(m["page"]["artifact_id"], b.as_str());
    assert_eq!(m["thread"]["version_n"].as_u64().unwrap(), n);
    assert!(m["thread"]["anchor"]["route"].is_null());
}

#[tokio::test]
async fn a_move_is_refused_across_origins_and_to_anyone_but_the_owner() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    let (a, tid, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/a"), "x").await;
    let res = move_to(&ts, &tid, "http://localhost:3000/a").await;
    assert_eq!(res.status(), 400);
    assert_eq!(code(res).await, "cross_origin");
    let res = ts
        .get_authed(&format!(
            "/api/live/pages?url={}",
            enc("http://localhost:3000/a")
        ))
        .await;
    let p: Value = res.json().await.unwrap();
    assert!(p["page"].is_null(), "a refused move makes no page");
    let res = move_to(&ts, &tid, &format!("{}/x", ts.base)).await;
    assert_eq!(code(res).await, "own_origin");
    for cookie in [format!("clax_viewer={}", v.cookie), ts.owner_cookie()] {
        let res = ts
            .client
            .post(format!("{}/api/live/threads/{tid}/move", ts.base))
            .header("cookie", cookie.clone())
            .json(&json!({"page_url": format!("{ORIGIN}/b")}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{cookie}");
    }
    let res = move_to(&ts, "01J00000000000000000000000", &format!("{ORIGIN}/b")).await;
    assert_eq!(res.status(), 404);
    // A thread of an HTML artifact is not a live page's.
    let html = ts.publish("T", &[("index.html", "<p>")]).await;
    let hid = html["artifact"]["id"].as_str().unwrap();
    let h = ts.thread(hid, 1, "on html").await;
    let res = move_to(&ts, h["id"].as_str().unwrap(), &format!("{ORIGIN}/b")).await;
    assert_eq!(res.status(), 404);
    let res = ts
        .post_json(
            &format!("/api/live/threads/{tid}/move"),
            json!({"page_url": format!("{ORIGIN}/b"), "x": 1}),
        )
        .await;
    assert_eq!(res.status(), 400);
    assert_eq!(threads_of(&ts, &a).await, vec![tid]);
}

#[tokio::test]
async fn a_rule_merges_pages_and_maps_later_comments() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    let (one, t1, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/users/1"), "one").await;
    let (_, t2, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/users/2#/t"), "two").await;
    let (edit, t3, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/users/2/edit"), "edit").await;
    let res = add_rule(&ts, "/users/:id").await;
    assert_eq!(res.status(), 201);
    let r: Value = res.json().await.unwrap();
    let rule_id = r["rule"]["id"].as_str().unwrap().to_string();
    assert_eq!(r["rule"]["pattern"], "/users/:id");
    assert_eq!(r["rule"]["page_url"], format!("{ORIGIN}/users/:id"));
    assert_eq!(r["page"]["path"], "/users/:id");
    assert_eq!(r["page"]["merged"], true);
    assert_eq!(r["page"]["pattern"], "/users/:id");
    assert_eq!(r["remaining"], 0);
    let canon = r["page"]["artifact_id"].as_str().unwrap().to_string();
    let canon_version = r["page"]["current_version"].clone();
    let mut moved: Vec<String> = serde_json::from_value(r["moved"].clone()).unwrap();
    moved.sort();
    let mut want = vec![t1.clone(), t2.clone()];
    want.sort();
    assert_eq!(moved, want);
    assert_eq!(threads_of(&ts, &one).await, Vec::<String>::new());
    assert_eq!(threads_of(&ts, &edit).await, vec![t3.clone()]);

    // The same rule again: 200, nothing more to move, no copies.
    let res = add_rule(&ts, "/users/:id").await;
    assert_eq!(res.status(), 200);
    let r: Value = res.json().await.unwrap();
    assert_eq!(r["moved"], json!([]));
    assert_eq!(r["remaining"], 0);
    assert_eq!(r["page"]["artifact_id"], canon.as_str());
    assert_eq!(r["page"]["current_version"], canon_version);

    // A later URL the rule maps names the canonical page.
    let p: Value = ts
        .get_authed(&format!(
            "/api/live/pages?url={}",
            enc(&format!("{ORIGIN}/users/3?q=1"))
        ))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(p["page"]["artifact_id"], canon.as_str());
    assert_eq!(p["page"]["merged"], true);
    assert_eq!(p["route"], "?q=1");
    assert_eq!(p["rule"]["id"], rule_id.as_str());
    let (aid, t4, view) = comment(&ts, &v.cookie, &format!("{ORIGIN}/users/3?q=1"), "three").await;
    assert_eq!(aid, canon);
    assert_eq!(view["page_url"], format!("{ORIGIN}/users/3?q=1"));
    assert_eq!(view["page_path"], "/users/3");

    let s = site(&ts).await;
    assert_eq!(s["rules"][0]["id"], rule_id.as_str());
    let group = s["pages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["page"]["artifact_id"] == canon.as_str())
        .unwrap();
    assert_eq!(group["summary"]["open"], 3);
    let urls: Vec<&str> = group["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["page_url"].as_str().unwrap())
        .collect();
    assert_eq!(
        urls,
        vec![
            format!("{ORIGIN}/users/3?q=1"),
            format!("{ORIGIN}/users/2#/t"),
            format!("{ORIGIN}/users/1"),
        ],
        "newest activity first"
    );
    let moved_t2 = group["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == t2.as_str())
        .unwrap();
    assert_eq!(moved_t2["moves"][0]["rule_id"], rule_id.as_str());
    assert!(
        !s["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["page"]["artifact_id"] == one.as_str()),
        "a page left with no threads is not listed"
    );

    assert_eq!(group["page"]["merged"], true);

    // A thread the owner moves onto the merged page stays there.
    let (_, t5, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/other"), "five").await;
    let m: Value = move_to(&ts, &t5, &format!("{ORIGIN}/users/:id"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(m["page"]["artifact_id"], canon.as_str());

    // Deleting the rule un-merges: each thread a rule put there goes back
    // to the page of the path it was made at.
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/live/rules/{rule_id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let d: Value = res.json().await.unwrap();
    assert_eq!(d["remaining"], 0);
    assert_eq!(d["rule"]["deleting"], false);
    let mut back: Vec<String> = serde_json::from_value(d["moved"].clone()).unwrap();
    back.sort();
    let mut want = vec![t1.clone(), t2.clone(), t4.clone()];
    want.sort();
    assert_eq!(back, want);
    assert_eq!(threads_of(&ts, &one).await, vec![t1.clone()]);
    assert_eq!(threads_of(&ts, &canon).await, vec![t5]);
    let s = site(&ts).await;
    let t4_view = s["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["threads"].as_array().unwrap())
        .find(|t| t["id"] == t4.as_str())
        .unwrap()
        .clone();
    assert_eq!(t4_view["page_url"], format!("{ORIGIN}/users/3?q=1"));
    assert_eq!(t4_view["page_path"], "/users/3");
    assert_eq!(t4_view["moves"][0]["kind"], "unmerge");
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/live/rules/{rule_id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let p: Value = ts
        .get_authed(&format!(
            "/api/live/pages?url={}",
            enc(&format!("{ORIGIN}/users/1"))
        ))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(p["page"]["artifact_id"], one.as_str());
    assert_eq!(p["page"]["merged"], false);
    assert!(p["rule"].is_null());
}

#[tokio::test]
async fn the_most_specific_rule_wins_then_the_oldest() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    for p in ["/users/*", "/users/:id", "/:a/x", "/x/:b"] {
        assert_eq!(add_rule(&ts, p).await.status(), 201, "{p}");
    }
    let page_of = |path: &'static str| {
        let ts = &ts;
        let cookie = v.cookie.clone();
        async move {
            let (aid, _, _) = comment(ts, &cookie, &format!("{ORIGIN}{path}"), "x").await;
            let p: Value = ts
                .get_authed(&format!("/api/artifacts/{aid}"))
                .await
                .json()
                .await
                .unwrap();
            p["artifact"]["live"]["path"].as_str().unwrap().to_string()
        }
    };
    assert_eq!(page_of("/users/5").await, "/users/:id");
    assert_eq!(page_of("/users/5/x").await, "/users/*");
    assert_eq!(page_of("/x/x").await, "/:a/x", "a tie goes to the oldest");
    assert_eq!(page_of("/y/x").await, "/:a/x");
    assert_eq!(page_of("/teams").await, "/teams");
    let r: Value = ts
        .get_authed(&format!("/api/live/rules?origin={}", enc(ORIGIN)))
        .await
        .json()
        .await
        .unwrap();
    let patterns: Vec<&str> = r["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["pattern"].as_str().unwrap())
        .collect();
    assert_eq!(patterns, vec!["/users/*", "/users/:id", "/:a/x", "/x/:b"]);
}

#[tokio::test]
async fn rules_are_checked_and_only_the_owner_changes_them() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    for bad in [
        "/users",
        "users/:id",
        "/users/(\\d+)",
        "/a*/:id",
        "/x//:id",
        "/ä/:id",
        "/*",
        "/:p",
        "/:a/:b",
    ] {
        let res = add_rule(&ts, bad).await;
        assert_eq!(res.status(), 400, "{bad}");
        assert_eq!(code(res).await, "invalid_pattern", "{bad}");
    }
    let res = ts
        .post_json(
            "/api/live/rules",
            json!({"origin": ts.base, "pattern": "/a/:id"}),
        )
        .await;
    assert_eq!(res.status(), 400);
    assert_eq!(code(res).await, "own_origin");
    let res = ts
        .post_json(
            "/api/live/rules",
            json!({"origin": "http://127.0.0.1:1", "pattern": "/a/:id", "extra": 1}),
        )
        .await;
    assert_eq!(res.status(), 400);
    for cookie in [format!("clax_viewer={}", v.cookie), ts.owner_cookie()] {
        let res = ts
            .client
            .post(format!("{}/api/live/rules", ts.base))
            .header("cookie", cookie.clone())
            .json(&json!({"origin": ORIGIN, "pattern": "/a/:id"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{cookie}");
    }
    let res = ts
        .client
        .get(format!("{}/api/live/rules?origin={}", ts.base, enc(ORIGIN)))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let res = ts
        .client
        .delete(format!("{}/api/live/rules/x", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

fn ext(ts: &TestServer, m: Method, path: &str, cred: &str) -> reqwest::RequestBuilder {
    ts.client
        .request(m, format!("{}{path}", ts.base))
        .header("origin", extension_origin(&ts.extension_id()))
        .header("sec-fetch-site", "cross-site")
        .header("authorization", format!("Clax-Extension {cred}"))
}

#[tokio::test]
async fn the_extension_lists_moves_and_merges() {
    let ts = TestServer::spawn().await;
    let v: Value = ts
        .post_json(
            "/api/extension/credentials",
            json!({"extension_id": ts.extension_id()}),
        )
        .await
        .json()
        .await
        .unwrap();
    let cred = v["credential"].as_str().unwrap().to_string();
    let viewer = ts.viewer(Some("Ana")).await;
    let (_, tid, _) = comment(&ts, &viewer.cookie, &format!("{ORIGIN}/users/1"), "x").await;
    let res = ext(
        &ts,
        Method::GET,
        &format!("/api/live/site?origin={}", enc(ORIGIN)),
        &cred,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let res = ext(
        &ts,
        Method::POST,
        &format!("/api/live/threads/{tid}/move"),
        &cred,
    )
    .json(&json!({"page_url": format!("{ORIGIN}/users/2")}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let m: Value = res.json().await.unwrap();
    let owner = ts.owner_public_id().await;
    assert_eq!(
        m["thread"]["moves"][0]["moved_by"],
        format!("viewer:{owner}")
    );
    let res = ext(&ts, Method::POST, "/api/live/rules", &cred)
        .json(&json!({"origin": ORIGIN, "pattern": "/users/:id"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let r: Value = res.json().await.unwrap();
    assert_eq!(r["moved"], json!([tid]));
    let id = r["rule"]["id"].as_str().unwrap();
    let res = ext(
        &ts,
        Method::GET,
        &format!("/api/live/rules?origin={}", enc(ORIGIN)),
        &cred,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let res = ext(&ts, Method::DELETE, &format!("/api/live/rules/{id}"), &cred)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // A preflight for a rule's deletion is answered.
    let res = ts
        .client
        .request(Method::OPTIONS, format!("{}/api/live/rules/{id}", ts.base))
        .header("origin", extension_origin(&ts.extension_id()))
        .header("access-control-request-method", "DELETE")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    // A write from the extension without its Origin is refused.
    let res = ts
        .client
        .post(format!("{}/api/live/rules", ts.base))
        .header("authorization", format!("Clax-Extension {cred}"))
        .header("sec-fetch-site", "none")
        .json(&json!({"origin": ORIGIN, "pattern": "/teams/:id"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(code(res).await, "forbidden_origin");
    // The extension's stream takes a site topic.
    let res = ext(&ts, Method::GET, "/api/stream", &cred)
        .send()
        .await
        .unwrap();
    let mut events = EventReader::from_response(res);
    let sid = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ext(&ts, Method::POST, &format!("/api/stream/{sid}"), &cred)
        .json(&json!({"subscribe": [format!("site:{ORIGIN}")]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

/// A session of an agent watching the live page `aid`.
async fn watching_agent(ts: &TestServer, hsid: &str, aid: &str) -> String {
    let s = ts.register_session("claude", hsid).await;
    let sid = s["id"].as_str().unwrap().to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/watches/{aid}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    sid
}

async fn feedback(ts: &TestServer, sid: &str) -> Value {
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=wait&wait=1"))
        .await
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_moved_threads_agent_still_hears_of_it() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    let (a, tid, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/a"), "Fix the header").await;
    let sid = watching_agent(&ts, "move-1", &a).await;
    ts.send_thread(&a, &tid).await;
    let fb = feedback(&ts, &sid).await;
    assert_eq!(fb["feedback"][0]["thread_id"], tid.as_str(), "{fb}");
    let m: Value = move_to(&ts, &tid, &format!("{ORIGIN}/b"))
        .await
        .json()
        .await
        .unwrap();
    let b = m["page"]["artifact_id"].as_str().unwrap().to_string();
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{b}/threads/{tid}/comments",
            ts.base
        ))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .json(&json!({"body": "Also the footer"}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let fb = feedback(&ts, &sid).await;
    let items = fb["feedback"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|f| f["thread_id"] == tid.as_str()
                && f["live"]["page_url"] == format!("{ORIGIN}/b")),
        "{fb}"
    );
}

#[tokio::test]
async fn a_snapshot_settles_only_addresses_of_threads_made_at_its_path() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    assert_eq!(add_rule(&ts, "/users/:id").await.status(), 201);
    let (canon, tid, view) = comment(&ts, &v.cookie, &format!("{ORIGIN}/users/1"), "x").await;
    assert_eq!(view["page_path"], "/users/1");
    let sid = watching_agent(&ts, "pending-1", &canon).await;
    ts.send_thread(&canon, &tid).await;
    let res = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{canon}/threads/{tid}/comments",
            ts.base
        )))
        .header("x-clax-session", &sid)
        .json(&json!({"body": "Fixed", "author_kind": "agent", "addressed": true}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let snapshot = |url: String| {
        let form = reqwest::multipart::Form::new()
            .text("url", url)
            .text("title", "User")
            .text("pending", json!([tid]).to_string())
            .text("snapshot", "<!doctype html><p>after");
        ts.client
            .post(format!("{}/api/live/snapshots", ts.base))
            .header("cookie", format!("clax_viewer={}", v.cookie))
            .multipart(form)
            .send()
    };
    let res = snapshot(format!("{ORIGIN}/users/2")).await.unwrap();
    assert_eq!(res.status(), 409, "another user's page settles nothing");
    let res = snapshot(format!("{ORIGIN}/users/1")).await.unwrap();
    assert_eq!(res.status(), 200);
    let s: Value = res.json().await.unwrap();
    assert_eq!(s["linked"], json!([tid]));
    assert_eq!(s["page"]["merged"], true);
}

#[tokio::test]
async fn a_scope_watch_on_a_merged_path_still_hears_of_it() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    let s = ts.register_session("claude", "scope-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/live-watches", ts.base)),
        )
        .json(&json!({"url": format!("{ORIGIN}/users/1")}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(add_rule(&ts, "/users/:id").await.status(), 201);
    let (canon, tid, _) = comment(&ts, &v.cookie, &format!("{ORIGIN}/users/1"), "x").await;
    ts.send_thread(&canon, &tid).await;
    let fb = feedback(&ts, &sid).await;
    assert_eq!(fb["feedback"][0]["thread_id"], tid.as_str(), "{fb}");
    assert_eq!(
        fb["feedback"][0]["live"]["page_url"],
        format!("{ORIGIN}/users/1")
    );
}

#[tokio::test]
async fn only_the_owner_follows_a_site() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Ana")).await;
    let res = ts
        .client
        .get(format!("{}/api/stream", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .send()
        .await
        .unwrap();
    let mut events = EventReader::from_response(res);
    let sid = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .client
        .post(format!("{}/api/stream/{sid}", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .json(&json!({"subscribe": [format!("site:{ORIGIN}")]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}
