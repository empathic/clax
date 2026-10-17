use clax_testkit::TestServer;
use serde_json::Value;

#[tokio::test]
async fn lookups_name_viewers_by_public_id_and_never_return_cookies() {
    let ts = TestServer::spawn().await;
    let a = ts.viewer(Some("Alex")).await;
    let b = ts.viewer(None).await;
    let res = ts
        .get(&format!(
            "/api/viewers?ids={},{},u_ffffffffffffffffffffff",
            a.public_id, b.public_id
        ))
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    let text = v.to_string();
    assert!(!text.contains(&a.cookie) && !text.contains(&b.cookie));
    let mut got: Vec<(String, Value)> = v["viewers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["id"].as_str().unwrap().to_string(),
                x["display_name"].clone(),
            )
        })
        .collect();
    got.sort_by(|x, y| x.0.cmp(&y.0));
    let mut want = vec![
        (a.public_id.clone(), Value::from("Alex")),
        (b.public_id.clone(), Value::Null),
    ];
    want.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(got, want);
    let many = vec![a.public_id.as_str(); 65].join(",");
    assert_eq!(
        ts.get(&format!("/api/viewers?ids={many}")).await.status(),
        400
    );
    assert_eq!(
        ts.get(&format!("/api/viewers?ids={}", a.cookie))
            .await
            .status(),
        400,
        "cookie values are not public IDs"
    );
    assert_eq!(ts.get("/api/viewers").await.status(), 400);
    assert_eq!(
        ts.get_authed(&format!("/api/viewers?ids={}&q=al", a.public_id))
            .await
            .status(),
        400,
        "ids and q together"
    );
}

async fn names(ts: &TestServer, q: &str) -> Vec<String> {
    let v: Value = ts
        .get_authed(&format!("/api/viewers?q={q}"))
        .await
        .json()
        .await
        .unwrap();
    v["viewers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["display_name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn search_returns_at_most_eight_and_matches_literally() {
    let ts = TestServer::spawn().await;
    for i in 0..10 {
        ts.viewer(Some(&format!("Member {i}"))).await;
    }
    ts.viewer(Some("50% off")).await;
    ts.viewer(Some("a_b")).await;
    ts.viewer(Some("axb")).await;
    ts.viewer(Some("Ärger")).await;
    let members = names(&ts, "member").await;
    assert_eq!(members.len(), 8);
    assert_eq!(members[0], "Member 0");
    assert_eq!(names(&ts, "%25").await, ["50% off"]);
    assert_eq!(names(&ts, "a_b").await, ["a_b"]);
    assert_eq!(names(&ts, "%C3%A4rger").await, ["Ärger"]);
    assert!(names(&ts, &"m".repeat(61)).await.is_empty());
    assert!(names(&ts, "%20%20").await.is_empty());
}

#[tokio::test]
async fn search_needs_the_token() {
    let ts = TestServer::spawn().await;
    ts.viewer(Some("Alex")).await;
    assert_eq!(ts.get("/api/viewers?q=al").await.status(), 401);
    let v: Value = ts
        .get_authed("/api/viewers?q=al")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["viewers"][0]["display_name"], "Alex");
}

#[tokio::test]
async fn lookups_refuse_foreign_origins() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .get(format!(
            "{}/api/viewers?ids=u_ffffffffffffffffffffff",
            ts.base
        ))
        .header("origin", "http://evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

/// A page's `<img src=".../api/viewers/me">` is a no-cors GET, which carries
/// no `Origin`; the browser still says where it came from in `Sec-Fetch-Site`.
#[tokio::test]
async fn a_request_another_site_or_origin_made_without_an_origin_is_refused_and_creates_no_viewer()
{
    let ts = TestServer::spawn().await;
    for site in ["cross-site", "same-site"] {
        let res = ts
            .client
            .get(format!("{}/api/viewers/me", ts.base))
            .header("sec-fetch-site", site)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{site}");
        assert!(res.headers().get("set-cookie").is_none(), "{site}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "forbidden_origin", "{site}");
    }
    // The shell's own fetches, a typed URL, and scripts that send no such header.
    for site in [Some("same-origin"), Some("none"), None] {
        let mut req = ts.client.get(format!("{}/api/viewers/me", ts.base));
        if let Some(s) = site {
            req = req.header("sec-fetch-site", s);
        }
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 200, "{site:?}");
        assert!(res.headers().get("set-cookie").is_some(), "{site:?}");
    }
}

/// `PUT /api/viewers/me {display_name}` as the viewer whose cookie is `cookie`.
async fn set_name(ts: &TestServer, cookie: &str, name: &str) -> (u16, Value) {
    let res = ts
        .client
        .put(format!("{}/api/viewers/me", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .json(&serde_json::json!({"display_name": name}))
        .send()
        .await
        .unwrap();
    (res.status().as_u16(), res.json().await.unwrap())
}

#[tokio::test]
async fn a_lan_viewer_cannot_take_a_persons_or_the_owners_name() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let res = ts
        .client
        .put(format!("{}/api/viewers/me", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&serde_json::json!({"display_name": "Alex Novák"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    ts.connect_person("Mia", "clax:collaborator").await;
    let lan = ts.viewer(Some("Lan")).await;
    for taken in [
        "alex novak",
        "ALEX  NOVÁK",
        "Ａｌｅｘ Ｎｏｖáｋ",
        "mia",
        "MÍA",
        "Mi\u{200B}a",
    ] {
        let (st, v) = set_name(&ts, &lan.cookie, taken).await;
        assert_eq!(
            (st, v["error"]["code"].as_str()),
            (409, Some("name_taken")),
            "{taken:?}: {v}"
        );
    }
    let (st, v) = set_name(&ts, &lan.cookie, "Mia R.").await;
    assert_eq!(
        (st, &v["viewer"]["display_name"]),
        (200, &serde_json::json!("Mia R.")),
        "{v}"
    );
    // A live agent's name is protected too.
    ts.register_session("claude", "h1").await;
    let (st, _) = set_name(&ts, &lan.cookie, "Claude").await;
    assert_eq!(st, 409);
}

#[tokio::test]
async fn names_that_render_as_a_persons_or_the_owners_are_refused() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let res = ts
        .client
        .put(format!("{}/api/viewers/me", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&serde_json::json!({"display_name": "Alex"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    ts.connect_person("Sam", "clax:collaborator").await;
    ts.connect_person("Coco", "clax:collaborator").await;
    let lan = ts.viewer(Some("Lan")).await;
    let probes: &[(&str, u16)] = &[
        ("\u{202E}xelA", 400),        // a bidi override: renders "Alex"
        ("\u{2066}Sam\u{2069}", 400), // bidi isolates
        ("A\u{202A}lex", 400),        // a bidi embedding
        ("A\u{200F}lex", 409),        // a right-to-left mark is allowed, and ignored
        ("Alex\u{3164}", 409),        // Hangul fillers render blank
        ("Alex\u{FFA0}", 409),
        ("A\u{115F}lex", 409),
        ("Alex\u{2800}", 409),                     // the braille blank
        ("\u{0410}lex", 409),                      // Cyrillic А
        ("\u{0405}\u{0430}m", 409),                // Cyrillic Ѕа with Latin m
        ("\u{0391}lex", 409),                      // Greek Α
        ("AIex", 409),                             // a capital I for the l
        ("\u{0421}\u{043E}\u{0441}\u{043E}", 409), // all-Cyrillic Сосо for Coco
        ("Ａｌｅｘ", 409),                         // full-width
        ("Al\u{200B}ex", 409),                     // zero-width space
        ("Al\u{FE0F}ex", 409),                     // a variation selector
        ("Alex\u{E0041}", 409),                    // a tag character
        ("Al\u{034F}ex", 409),
        ("Al\u{200A}ex", 409), // a hair space
        ("Al\u{2009}ex", 409), // a thin space
        ("Al\u{202F}ex", 409), // a narrow no-break space
        ("A\u{2006}lex", 409), // a six-per-em space                     // the combining grapheme joiner
        ("Al\u{0}ex", 400),
        ("Al\nex", 400),
    ];
    for (name, want) in probes {
        let (st, v) = set_name(&ts, &lan.cookie, name).await;
        assert_eq!(st, *want, "{name:?}: {v}");
    }
    let (_, me) = {
        let res = ts
            .client
            .get(format!("{}/api/viewers/me", ts.base))
            .header("cookie", format!("clax_viewer={}", lan.cookie))
            .send()
            .await
            .unwrap();
        (res.status(), res.json::<Value>().await.unwrap())
    };
    assert_eq!(me["viewer"]["display_name"], "Lan", "no probe renamed it");
    // Any script, and any mix of scripts, is fine; so is a Hebrew name
    // with a right-to-left mark.
    for ok in [
        "Dmitri Дмитрий",
        "Ivan (Иван)",
        "محمد Ali",
        "\u{05D3}\u{05E0}\u{05D4}\u{200F} Dana",
        "Саша",
        "Αλέξης",
        "山田 Taro",
        "김 Kim",
        "АЛЕХ",
    ] {
        let (st, v) = set_name(&ts, &lan.cookie, ok).await;
        assert_eq!(st, 200, "{ok:?}: {v}");
    }
}

#[tokio::test]
async fn the_owner_cannot_take_a_live_agents_name() {
    let ts = TestServer::spawn().await;
    ts.register_session("claude", "h1").await;
    let put = |name: &'static str| {
        ts.client
            .put(format!("{}/api/viewers/me", ts.base))
            .header("cookie", ts.owner_cookie())
            .json(&serde_json::json!({"display_name": name}))
            .send()
    };
    let res = put("Claude").await.unwrap();
    assert_eq!(res.status(), 409);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "name_taken");
    assert_eq!(put("Alex").await.unwrap().status(), 200);
}
