//! The owner identity: every browser of the owner's, the token (the CLI) and
//! any other owner credential act as one viewer; LAN viewers stay apart.

mod common;
use common::TestServer;
use serde_json::{Value, json};

/// A browser of the owner's: the owner cookie, plus the stray `clax_viewer`
/// cookie it may still hold from before.
fn owner_browser(ts: &TestServer, legacy: Option<&str>) -> String {
    match legacy {
        Some(c) => format!("{}; clax_viewer={c}", ts.owner_cookie()),
        None => ts.owner_cookie(),
    }
}

async fn me(ts: &TestServer, cookie: &str) -> Value {
    ts.client
        .get(format!("{}/api/viewers/me", ts.base))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["viewer"]
        .clone()
}

async fn put_name(ts: &TestServer, cookie: Option<&str>, name: &str) -> reqwest::Response {
    let r = ts.client.put(format!("{}/api/viewers/me", ts.base));
    let r = match cookie {
        Some(c) => r.header("cookie", c),
        None => ts.authed(r),
    };
    r.json(&json!({"display_name": name})).send().await.unwrap()
}

async fn reply(req: reqwest::RequestBuilder) -> Value {
    let res = req.json(&json!({"body": "ok"})).send().await.unwrap();
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["comment"].clone()
}

fn comments_url(ts: &TestServer, aid: &str, tid: &str) -> String {
    format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base)
}

#[tokio::test]
async fn two_browsers_and_the_cli_are_one_owner_and_a_lan_viewer_is_another() {
    let ts = TestServer::spawn().await;
    let a = ts
        .publish("T", &[("index.html", "<main><h2>x</h2></main>")])
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let chrome = owner_browser(&ts, None);
    let safari = owner_browser(&ts, Some("01J9Z3K4M5N6P7Q8R9S0T1V2W3"));
    assert_eq!(put_name(&ts, Some(&chrome), "Alex").await.status(), 200);
    let (c, s) = (me(&ts, &chrome).await, me(&ts, &safari).await);
    assert_eq!(c["public_id"], s["public_id"], "one public ID");
    assert_eq!(s["display_name"], "Alex", "one name");
    let owner = c["public_id"].as_str().unwrap().to_string();
    assert_eq!(
        ts.owner_public_id().await,
        owner,
        "the token is the owner too"
    );
    // No viewer cookie is minted for an owner's browser.
    let res = ts
        .client
        .get(format!("{}/api/viewers/me", ts.base))
        .header("cookie", &chrome)
        .send()
        .await
        .unwrap();
    assert!(res.headers().get("set-cookie").is_none());

    let t = ts
        .thread_as(&aid, "01J9Z3K4M5N6P7Q8R9S0T1V2W4", "first")
        .await;
    let tid = t["id"].as_str().unwrap().to_string();
    let from_chrome = reply(
        ts.client
            .post(comments_url(&ts, &aid, &tid))
            .header("cookie", &chrome),
    )
    .await;
    let from_cli = reply(ts.authed(ts.client.post(comments_url(&ts, &aid, &tid)))).await;
    for c in [&from_chrome, &from_cli] {
        assert_eq!(
            (c["author_public_id"].as_str(), c["author_name"].as_str()),
            (Some(owner.as_str()), Some("Alex"))
        );
    }
    // The CLI resolves as the owner.
    let res = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/resolve",
            ts.base
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["thread"]["resolved_by"], format!("viewer:{owner}"));
    assert_eq!(v["thread"]["resolved_by_name"], "Alex");

    // A LAN viewer (its cookie alone) is someone else.
    let lan = ts.viewer(Some("Sam")).await;
    assert_ne!(lan.public_id, owner);
    let from_lan = ts.reply_as(&aid, &tid, &lan.cookie, "me too").await;
    let last = from_lan["comments"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["author_public_id"], lan.public_id.as_str());
    assert_eq!(last["author_name"], "Sam");
}

#[tokio::test]
async fn seen_and_looked_marks_are_the_owners_in_every_browser() {
    let ts = TestServer::spawn().await;
    let a = ts
        .publish("T", &[("index.html", "<main><h2>x</h2></main>")])
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let chrome = owner_browser(&ts, None);
    let safari = owner_browser(&ts, None);
    let res = ts
        .client
        .put(format!("{}/api/viewers/me/seen", ts.base))
        .header("cookie", &chrome)
        .json(&json!({"artifact_id": aid, "version": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let seen: Value = ts
        .client
        .get(format!("{}/api/viewers/me/seen?artifact={aid}", ts.base))
        .header("cookie", &safari)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(seen["seen"], 1);
    let t = ts.thread(&aid, 1, "hello").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let res = ts
        .client
        .put(format!("{}/api/viewers/me/looked", ts.base))
        .header("cookie", &chrome)
        .json(&json!({"artifact_id": aid, "thread_ids": [tid]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let att: Value = ts
        .client
        .get(format!("{}/api/artifacts/{aid}", ts.base))
        .header("cookie", &safari)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(att["attention"]["looked"][&tid].is_string(), "{att}");
}

#[tokio::test]
async fn the_owner_is_listed_once_and_here_while_any_browser_is() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let report = |cookie: String, state: &'static str, tab: &'static str| {
        let (ts, aid) = (&ts, aid.clone());
        async move {
            let res = ts
                .client
                .put(format!("{}/api/viewers/me/presence", ts.base))
                .header("cookie", cookie)
                .json(&json!({"artifact_id": aid, "state": state, "tab": tab}))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200);
            res.json::<Value>().await.unwrap()["people"].clone()
        }
    };
    report(owner_browser(&ts, None), "here", "chrome-1").await;
    report(owner_browser(&ts, None), "away", "chrome-2").await;
    let people = report(owner_browser(&ts, None), "away", "safari-1").await;
    assert_eq!(people.as_array().unwrap().len(), 1, "{people}");
    assert_eq!(people[0]["state"], "here");
    let lan = ts.viewer(Some("Sam")).await;
    let people = report(format!("clax_viewer={}", lan.cookie), "here", "t").await;
    assert_eq!(people.as_array().unwrap().len(), 2, "{people}");
}

#[tokio::test]
async fn a_name_set_anywhere_reaches_every_browser_at_once() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let mut ev = ts.events(&format!("?artifact={aid}&types=presence")).await;
    let res = ts
        .client
        .put(format!("{}/api/viewers/me/presence", ts.base))
        .header("cookie", owner_browser(&ts, None))
        .json(&json!({"artifact_id": aid, "state": "here"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        ev.next_named("presence").await["people"][0]["display_name"],
        Value::Null
    );
    // `clax comments name` (the token) renames the owner in every browser.
    assert_eq!(put_name(&ts, None, "Alex").await.status(), 200);
    let e = ev.next_named("presence").await;
    assert_eq!(e["people"][0]["display_name"], "Alex");
    assert_eq!(
        me(&ts, &owner_browser(&ts, None)).await["display_name"],
        "Alex"
    );
}

#[tokio::test]
async fn the_shells_token_request_claims_its_browsers_old_viewer() {
    let ts = TestServer::spawn().await;
    let a = ts
        .publish("T", &[("index.html", "<main><h2>x</h2></main>")])
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let chrome = ts.viewer(Some("Alex")).await;
    let safari = ts.viewer(Some("Alex on Safari")).await;
    let t = ts.thread_as(&aid, &safari.cookie, "from safari").await;
    let token = |cookie: String| {
        let ts = &ts;
        async move {
            ts.client
                .get(format!("{}/api/token", ts.base))
                .header("sec-fetch-site", "same-origin")
                .header("cookie", format!("clax_viewer={cookie}"))
                .send()
                .await
                .unwrap()
        }
    };
    let res = token(chrome.cookie.clone()).await;
    let sets: Vec<String> = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(
        sets.iter()
            .any(|s| s.starts_with("clax_viewer=; ") && s.contains("Max-Age=0")),
        "the old viewer cookie is removed: {sets:?}"
    );
    // The first browser's viewer becomes the owner, keeping its ID and name.
    let owner = me(&ts, &owner_browser(&ts, None)).await;
    assert_eq!(owner["public_id"], chrome.public_id.as_str());
    assert_eq!(owner["display_name"], "Alex");
    // The second browser's viewer is folded in: its comment is the owner's.
    token(safari.cookie.clone()).await;
    let v: Value = ts
        .get(&format!(
            "/api/artifacts/{aid}/threads/{}",
            t["id"].as_str().unwrap()
        ))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["thread"]["comments"][0]["author_public_id"],
        chrome.public_id.as_str()
    );
    let gone: Value = ts
        .get(&format!("/api/viewers?ids={}", safari.public_id))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(gone["viewers"], json!([]));
    // A request that is not the shell's claims nothing.
    let lan = ts.viewer(Some("Sam")).await;
    let res = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("cookie", format!("clax_viewer={}", lan.cookie))
        .send()
        .await
        .unwrap();
    assert!(res.headers().get("set-cookie").is_none());
    assert_eq!(
        me(&ts, &format!("clax_viewer={}", lan.cookie)).await["public_id"],
        lan.public_id.as_str()
    );
}

#[tokio::test]
async fn the_owner_cookie_is_identity_not_the_token() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .post(format!("{}/api/artifacts", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(
            &json!({"title": "x", "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401, "publishing still needs the token");
    // A document only `admin` may write: the owner cookie alone is the
    // owner's identity at a viewer's level, not the shell's `admin`.
    let a = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "D", "capabilities": {"db": {"rules": [{"path": "staff", "write": "admin"}]}},
                   "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}}),
        )
        .await;
    let aid = a.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    put_name(&ts, None, "Alex").await;
    let put = |r: reqwest::RequestBuilder| async move {
        r.json(&json!({"data": {}}))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    };
    let url = format!("{}/api/artifacts/{aid}/docs/staff/s", ts.base);
    assert_ne!(
        put(ts.client.put(&url).header("cookie", ts.owner_cookie())).await,
        200,
        "refused"
    );
    assert_eq!(
        put(ts
            .authed(ts.client.put(&url))
            .header("cookie", ts.owner_cookie()))
        .await,
        200,
        "the token from the owner's browser is admin"
    );
}

/// A non-loopback IPv4 address of this machine (connecting a UDP socket sends nothing).
fn non_loopback_ipv4() -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("10.255.255.255:1").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// A daemon on every interface, and a LAN viewer of it: its cookie, minted
/// for a request from `ip`, and its public ID.
async fn lan_daemon() -> Option<(TestServer, std::net::IpAddr)> {
    let Some(ip) = non_loopback_ipv4() else {
        eprintln!("skipping: this machine has no non-loopback IPv4 address");
        return None;
    };
    let ts = TestServer::spawn_on(
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        |_| {},
    )
    .await;
    Some((ts, ip))
}

async fn lan_viewer(ts: &TestServer, ip: std::net::IpAddr) -> (String, String) {
    let res = ts
        .client
        .get(format!("http://{ip}:{}/api/viewers/me", ts.addr.port()))
        .send()
        .await
        .unwrap();
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("clax_viewer=")
        .to_string();
    let v: Value = res.json().await.unwrap();
    (
        cookie,
        v["viewer"]["public_id"].as_str().unwrap().to_string(),
    )
}

/// The shell's token request in a browser holding `cookies`.
async fn shell_token(ts: &TestServer, cookies: &str) -> reqwest::Response {
    ts.client
        .get(format!("{}/api/token", ts.base))
        .header("sec-fetch-site", "same-origin")
        .header("cookie", cookies)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_planted_lan_cookie_is_never_adopted_as_the_owner() {
    let Some((ts, ip)) = lan_daemon().await else {
        return;
    };
    let (planted, lan_id) = lan_viewer(&ts, ip).await;
    // Another local page set `clax_viewer=<the LAN viewer's cookie>`; the
    // owner's shell then fetches the token.
    shell_token(&ts, &format!("clax_viewer={planted}")).await;
    let owner = me(&ts, &owner_browser(&ts, None)).await;
    assert_ne!(
        owner["public_id"],
        lan_id.as_str(),
        "the LAN viewer did not become the owner"
    );
    // The planted cookie still names the LAN viewer alone, never the owner.
    let as_lan = me(&ts, &format!("clax_viewer={planted}")).await;
    assert_eq!(as_lan["public_id"], lan_id.as_str());
    assert_ne!(as_lan["public_id"], owner["public_id"]);
}

#[tokio::test]
async fn a_planted_lan_cookie_never_merges_its_comments_into_the_owner() {
    let Some((ts, ip)) = lan_daemon().await else {
        return;
    };
    let a = ts
        .publish("T", &[("index.html", "<main><h2>x</h2></main>")])
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    // The owner exists, adopted from a browser of this machine.
    let chrome = ts.viewer(Some("Alex")).await;
    shell_token(&ts, &format!("clax_viewer={}", chrome.cookie)).await;
    let (planted, lan_id) = lan_viewer(&ts, ip).await;
    let t = ts.thread_as(&aid, &planted, "spam").await;
    shell_token(&ts, &format!("clax_viewer={planted}")).await;
    let v: Value = ts
        .get(&format!(
            "/api/artifacts/{aid}/threads/{}",
            t["id"].as_str().unwrap()
        ))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["thread"]["comments"][0]["author_public_id"],
        lan_id.as_str()
    );
    assert_eq!(
        me(&ts, &format!("clax_viewer={planted}")).await["public_id"],
        lan_id.as_str()
    );
}

#[tokio::test]
async fn a_shadowing_viewer_cookie_claims_nothing() {
    let ts = TestServer::spawn().await;
    let mine = ts.viewer(Some("Alex")).await;
    let other = ts.viewer(Some("Other")).await;
    // A cookie scoped to /api/token is sent before the browser's own.
    let res = shell_token(
        &ts,
        &format!("clax_viewer={}; clax_viewer={}", other.cookie, mine.cookie),
    )
    .await;
    assert!(
        !res.headers()
            .get_all("set-cookie")
            .iter()
            .any(|v| v.to_str().unwrap().starts_with("clax_viewer=")),
        "nothing claimed, nothing cleared"
    );
    let owner = me(&ts, &owner_browser(&ts, None)).await;
    assert_ne!(owner["public_id"], mine.public_id.as_str());
    assert_ne!(owner["public_id"], other.public_id.as_str());
}

#[tokio::test]
async fn an_adopted_viewers_old_cookie_names_no_one() {
    let ts = TestServer::spawn().await;
    let chrome = ts.viewer(Some("Alex")).await;
    shell_token(&ts, &format!("clax_viewer={}", chrome.cookie)).await;
    let owner = me(&ts, &owner_browser(&ts, None)).await;
    assert_eq!(owner["public_id"], chrome.public_id.as_str());
    // Replaying the old cookie (it reached every localhost port) is not the owner.
    let replay = me(&ts, &format!("clax_viewer={}", chrome.cookie)).await;
    assert_ne!(replay["public_id"], owner["public_id"]);
    let res = put_name(
        &ts,
        Some(&format!("clax_viewer={}", chrome.cookie)),
        "Mallory",
    )
    .await;
    assert_eq!(res.status(), 200);
    assert_eq!(
        me(&ts, &owner_browser(&ts, None)).await["display_name"],
        "Alex"
    );
}

#[tokio::test]
async fn a_lan_peer_replaying_the_owner_cookie_is_not_the_owner() {
    let Some((ts, ip)) = lan_daemon().await else {
        return;
    };
    let owner = ts.owner_public_id().await;
    for site in [None, Some("same-origin")] {
        let mut r = ts
            .client
            .get(format!("http://{ip}:{}/api/viewers/me", ts.addr.port()))
            .header("host", format!("localhost:{}", ts.addr.port()))
            .header("cookie", ts.owner_cookie());
        if let Some(s) = site {
            r = r.header("sec-fetch-site", s);
        }
        let v: Value = r.send().await.unwrap().json().await.unwrap();
        assert_ne!(v["viewer"]["public_id"], owner.as_str(), "{site:?}");
    }
}

#[tokio::test]
async fn the_cli_reads_the_owner_without_making_one_and_a_browser_claimed_later_keeps_its_id() {
    let ts = TestServer::spawn().await;
    let a = ts
        .publish("T", &[("index.html", "<main><h2>x</h2></main>")])
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let v: Value = ts.get_authed("/api/viewers/me").await.json().await.unwrap();
    assert_eq!(v["viewer"], Value::Null, "reading makes no owner");
    // The CLI names the owner and replies before any browser was claimed.
    assert_eq!(put_name(&ts, None, "Alex").await.status(), 200);
    let t = ts.thread(&aid, 1, "first").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let from_cli = reply(ts.authed(ts.client.post(comments_url(&ts, &aid, &tid)))).await;
    let cli_id = from_cli["author_public_id"].as_str().unwrap().to_string();
    // The browser the person used before keeps its user ID; the CLI's
    // history and name move to it.
    let chrome = ts.viewer(None).await;
    shell_token(&ts, &format!("clax_viewer={}", chrome.cookie)).await;
    let owner = me(&ts, &owner_browser(&ts, None)).await;
    assert_eq!(owner["public_id"], chrome.public_id.as_str());
    assert_eq!(owner["display_name"], "Alex");
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["thread"]["comments"][1]["author_public_id"],
        chrome.public_id.as_str()
    );
    let gone: Value = ts
        .get(&format!("/api/viewers?ids={cli_id}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(gone["viewers"], json!([]));
}

#[tokio::test]
async fn a_merged_viewer_leaves_presence_at_once() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let chrome = ts.viewer(Some("Alex")).await;
    shell_token(&ts, &format!("clax_viewer={}", chrome.cookie)).await;
    let safari = ts.viewer(Some("Alex S")).await;
    let res = ts
        .client
        .put(format!("{}/api/viewers/me/presence", ts.base))
        .header("cookie", format!("clax_viewer={}", safari.cookie))
        .json(&json!({"artifact_id": aid, "state": "here"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut ev = ts.events(&format!("?artifact={aid}&types=presence")).await;
    shell_token(&ts, &format!("clax_viewer={}", safari.cookie)).await;
    let e = tokio::time::timeout(std::time::Duration::from_secs(5), ev.next_named("presence"))
        .await
        .expect("the merged viewer's presence is dropped at once");
    assert_eq!(e["people"], json!([]), "{e}");
}
