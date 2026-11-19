//! An artifact's site analytics setting (`PATCH /api/artifacts/<id>
//! {analytics}`, `artifact.analytics` of `GET /api/artifacts/<id>`) and its
//! read (`GET /api/artifacts/<id>/analytics`,
//! `clax_server::routes::analytics`): who administers it on a hosted Clax
//! turns counting on and off without approval and reads the totals; no one
//! else, no page and no agent does, and a local Clax has neither.

use clax_core::analytics::{Batch, DaySums, Device, Seen, View};
use clax_core::working::ManualClock;
use clax_testkit::{TestServer, error_code as code};
use reqwest::Method;
use reqwest::header::{CACHE_CONTROL, COOKIE, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

const SECRET: &str = "the setup secret";

fn id(s: &str) -> clax_core::ArtifactId {
    clax_core::ArtifactId::parse(s).unwrap()
}

/// A hosted daemon on a stopped clock, its owner's browser's session
/// cookie, and a raw client that follows no redirect.
struct Hosted {
    ts: TestServer,
    clock: Arc<ManualClock>,
    raw: reqwest::Client,
}

impl Hosted {
    async fn new() -> Hosted {
        Hosted::at("2026-11-19T10:00:00Z").await
    }

    /// [`Hosted::new`] whose clock starts at `start`.
    async fn at(start: &str) -> Hosted {
        let clock = Arc::new(ManualClock::at(start));
        let ts = TestServer::spawn_hosted(clock.clone(), Some(SECRET)).await;
        let raw = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Hosted { ts, clock, raw }
    }

    fn public_host(&self) -> String {
        format!("localhost:{}", self.ts.addr.port())
    }

    /// `method path` on the public host from the browser holding session
    /// `cookie` (none: signed out), as the shell sends it.
    async fn send(
        &self,
        cookie: Option<&str>,
        method: Method,
        path: &str,
        body: Value,
    ) -> (u16, Value) {
        let r = self.request(cookie, method, path, body).await;
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    async fn request(
        &self,
        cookie: Option<&str>,
        method: Method,
        path: &str,
        body: Value,
    ) -> reqwest::Response {
        let mut r = self
            .raw
            .request(method.clone(), format!("{}{path}", self.ts.base))
            .header(HOST, self.public_host());
        if method != Method::GET {
            r = r
                .header(ORIGIN, format!("http://{}", self.public_host()))
                .json(&body);
        }
        if let Some(c) = cookie {
            r = r.header(COOKIE, format!("clax_session={c}"));
        }
        r.send().await.unwrap()
    }

    /// Signs a browser in through `path`: its session cookie.
    async fn sign_in(&self, path: &str, body: Value) -> String {
        let r = self.request(None, Method::POST, path, body).await;
        assert_eq!(r.status(), 200, "{path}");
        let c = r.headers()[SET_COOKIE].to_str().unwrap();
        c.strip_prefix("clax_session=")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    /// The owner's browser.
    async fn owner(&self) -> String {
        self.sign_in(
            "/api/hosted/setup",
            json!({"secret": SECRET, "name": "Ada"}),
        )
        .await
    }

    /// A person invited by `owner`, who administers nothing of the owner's:
    /// their browser's session cookie and their public ID.
    async fn member(&self, owner: &str, name: &str) -> (String, String) {
        let (s, v) = self
            .send(
                Some(owner),
                Method::POST,
                "/api/people/invites",
                json!({"name": name}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let code = v["invite"]["url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let b = self
            .sign_in(&format!("/api/hosted/invites/{code}/accept"), json!({}))
            .await;
        let (_, me) = self
            .send(Some(&b), Method::GET, "/api/hosted/me", Value::Null)
            .await;
        let pid = me["person"]["public_id"].as_str().unwrap().to_string();
        (b, pid)
    }

    async fn artifact(&self, title: &str) -> String {
        self.ts
            .publish(title, &[("index.html", "<h1>hi</h1>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

/// A view of `path` on version `version`.
fn view(path: &str, version: u32) -> View {
    View {
        path: path.to_string(),
        referrer: String::new(),
        country: "DE".into(),
        device: Device::Phone,
        version,
        framed: false,
        load_ms: Some(800),
        lcp_ms: None,
    }
}

/// Writes `views` of `aid` on `day`, each its visitor's first of the day.
fn record(ts: &TestServer, aid: &str, day: &str, views: &[View]) {
    let mut sums = DaySums::default();
    for v in views {
        sums.add(
            v,
            Seen {
                first_of_day: true,
                first_on_page: true,
            },
        );
    }
    let mut batch = Batch::default();
    batch.sums.insert((aid.to_string(), day.to_string()), sums);
    ts.store.analytics_record(&batch).unwrap();
}

/// The `fields` of every `artifact.update` of `aid` in the audit journal.
fn updates(ts: &TestServer, aid: &str) -> Vec<Value> {
    ts.store
        .events_after(0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == "artifact.update" && r.ids.artifact.as_deref() == Some(aid))
        .map(|r| serde_json::from_str::<Value>(&r.body).unwrap()["fields"].clone())
        .collect()
}

#[tokio::test]
async fn who_administers_it_turns_counting_off_and_on_without_approval_and_it_is_recorded() {
    let h = Hosted::new().await;
    let owner = h.owner().await;
    let aid = h.artifact("Counted").await;
    let path = format!("/api/artifacts/{aid}");
    let (s, v) = h.send(Some(&owner), Method::GET, &path, Value::Null).await;
    assert_eq!((s, &v["artifact"]["analytics"]), (200, &json!("on")), "{v}");
    // The browser sends it alone, without the token.
    let (s, v) = h
        .send(
            Some(&owner),
            Method::PATCH,
            &path,
            json!({"analytics": "off"}),
        )
        .await;
    assert_eq!(
        (s, &v["artifact"]["analytics"]),
        (200, &json!("off")),
        "{v}"
    );
    assert!(!h.ts.store.analytics_on(&id(&aid)).unwrap());
    let (_, v) = h.send(Some(&owner), Method::GET, &path, Value::Null).await;
    assert_eq!(v["artifact"]["analytics"], "off", "{v}");
    // The same value again changes and records nothing.
    let (s, _) = h
        .send(
            Some(&owner),
            Method::PATCH,
            &path,
            json!({"analytics": "off"}),
        )
        .await;
    assert_eq!(s, 200);
    // An agent's change asks no one, even while a session is live.
    let sid = h.ts.register_session("claude", "an-agent").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res =
        h.ts.authed(h.ts.client.patch(format!("{}{path}", h.ts.base)))
            .header("x-clax-session", &sid)
            .json(&json!({"analytics": "on"}))
            .send()
            .await
            .unwrap();
    let s = res.status().as_u16();
    let v: Value = res.json().await.unwrap();
    assert_eq!((s, &v["artifact"]["analytics"]), (200, &json!("on")), "{v}");
    // Nor does the token alone while that session is live.
    let (s, v) =
        h.ts.token_call(Method::PATCH, &path, json!({"analytics": "off"}), None)
            .await;
    assert_eq!(
        (s, &v["artifact"]["analytics"]),
        (200, &json!("off")),
        "{v}"
    );
    assert_eq!(
        updates(&h.ts, &aid),
        [
            json!({"analytics": "off"}),
            json!({"analytics": "on"}),
            json!({"analytics": "off"})
        ]
    );
    // Anything but on or off, and with another setting, is refused.
    let (s, v) =
        h.ts.token_call(Method::PATCH, &path, json!({"analytics": "yes"}), None)
            .await;
    assert_eq!((s, code(&v)), (400, "invalid_argument"), "{v}");
    let (s, v) =
        h.ts.token_call(
            Method::PATCH,
            &path,
            json!({"analytics": "on", "content_security_policy": "self"}),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("site_access, content_security_policy and analytics"),
        "{v}"
    );
}

#[tokio::test]
async fn a_page_never_sets_it() {
    let h = Hosted::new().await;
    let aid = h.artifact("Mine").await;
    let path = format!("/api/artifacts/{aid}");
    for value in ["off", "on"] {
        let res =
            h.ts.authed(h.ts.client.patch(format!("{}{path}", h.ts.base)))
                .header("x-clax-via", "page")
                .json(&json!({"analytics": value}))
                .send()
                .await
                .unwrap();
        let s = res.status().as_u16();
        let body: Value = res.json().await.unwrap();
        assert_eq!((s, code(&body)), (403, "not_for_pages"), "{value}");
    }
    assert!(h.ts.store.analytics_on(&id(&aid)).unwrap());
}

#[tokio::test]
async fn a_person_who_does_not_administer_it_is_admin_only() {
    let h = Hosted::new().await;
    let owner = h.owner().await;
    let aid = h.artifact("Shared").await;
    let (sam, pid) = h.member(&owner, "Sam").await;
    // Before it is shared with Sam, it does not exist for them.
    let (s, _) = h
        .send(
            Some(&sam),
            Method::GET,
            &format!("/api/artifacts/{aid}/analytics"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 404);
    let (s, v) = h
        .send(
            Some(&owner),
            Method::PUT,
            &format!("/api/hosted/artifacts/{aid}/shares"),
            json!({"person": pid, "access": "viewer"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // Sam reads the artifact, without its setting.
    let (s, v) = h
        .send(
            Some(&sam),
            Method::GET,
            &format!("/api/artifacts/{aid}"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["artifact"].get("analytics").is_none(), "{v}");
    let (s, v) = h
        .send(
            Some(&owner),
            Method::GET,
            &format!("/api/artifacts/{aid}/analytics"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = h
        .send(
            Some(&sam),
            Method::GET,
            &format!("/api/artifacts/{aid}/analytics"),
            Value::Null,
        )
        .await;
    assert_eq!((s, code(&v)), (403, "admin_only"), "{v}");
    let patch = |who: String| {
        let h = &h;
        let path = format!("/api/artifacts/{aid}");
        async move {
            h.send(
                Some(&who),
                Method::PATCH,
                &path,
                json!({"analytics": "off"}),
            )
            .await
        }
    };
    // A viewer may update nothing of it.
    let (s, v) = patch(sam.clone()).await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    // An editor updates it, but does not administer it.
    let (s, v) = h
        .send(
            Some(&owner),
            Method::PUT,
            &format!("/api/hosted/artifacts/{aid}/shares"),
            json!({"person": pid, "access": "editor"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = patch(sam.clone()).await;
    assert_eq!((s, code(&v)), (403, "admin_only"), "{v}");
    let (s, v) = h
        .send(
            Some(&sam),
            Method::GET,
            &format!("/api/artifacts/{aid}/analytics"),
            Value::Null,
        )
        .await;
    assert_eq!((s, code(&v)), (403, "admin_only"), "{v}");
    assert!(h.ts.store.analytics_on(&id(&aid)).unwrap());
}

/// The `artifact` of the shell's bootstrap of `/a/<aid>` in the browser
/// holding session `cookie`.
async fn boot_artifact(h: &Hosted, cookie: &str, aid: &str) -> Value {
    const OPEN: &str = r#"<script type="application/json" id="clax-boot">"#;
    let html = h
        .request(Some(cookie), Method::GET, &format!("/a/{aid}"), Value::Null)
        .await
        .text()
        .await
        .unwrap();
    let start = html.find(OPEN).expect("a bootstrap block") + OPEN.len();
    let end = start + html[start..].find("</script>").unwrap();
    let boot: Value = serde_json::from_str(&html[start..end]).unwrap();
    boot["artifact"]["artifact"].clone()
}

#[tokio::test]
async fn the_shells_bootstrap_carries_the_setting_for_who_administers_it_alone() {
    let h = Hosted::new().await;
    let owner = h.owner().await;
    let aid = h.artifact("Booted").await;
    let (sam, pid) = h.member(&owner, "Sam").await;
    let (s, v) = h
        .send(
            Some(&owner),
            Method::PUT,
            &format!("/api/hosted/artifacts/{aid}/shares"),
            json!({"person": pid, "access": "editor"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let a = boot_artifact(&h, &owner, &aid).await;
    assert_eq!(a["analytics"], "on", "{a}");
    let (s, v) = h
        .send(
            Some(&owner),
            Method::PATCH,
            &format!("/api/artifacts/{aid}"),
            json!({"analytics": "off"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let a = boot_artifact(&h, &owner, &aid).await;
    assert_eq!(a["analytics"], "off", "{a}");
    // An editor reads the page, not its setting.
    let a = boot_artifact(&h, &sam, &aid).await;
    assert_eq!(a["id"], json!(aid), "{a}");
    assert!(a.get("analytics").is_none(), "{a}");
}

#[tokio::test]
async fn a_local_clax_takes_no_analytics_and_has_no_read() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Local", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert!(v["artifact"].get("analytics").is_none(), "{v}");
    let (s, v) = ts
        .token_call(
            Method::PATCH,
            &format!("/api/artifacts/{aid}"),
            json!({"analytics": "off"}),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_argument"), "{v}");
    assert_eq!(v["error"]["message"], "a local Clax counts no visits");
    let res = ts
        .get_authed(&format!("/api/artifacts/{aid}/analytics"))
        .await;
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn a_draft_takes_no_analytics() {
    let h = Hosted::new().await;
    let res =
        h.ts.authed(h.ts.client.post(format!("{}/api/drafts", h.ts.base)))
            .json(&json!({"title": "Draft", "files": {"index.html": "<p>draft</p>"}}))
            .send()
            .await
            .unwrap();
    assert_eq!(res.status(), 201);
    let draft = res.json::<Value>().await.unwrap()["draft"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/artifacts/{draft}");
    let (s, v) =
        h.ts.token_call(Method::PATCH, &path, json!({"analytics": "off"}), None)
            .await;
    assert_eq!((s, code(&v)), (403, "draft"), "{v}");
    let v: Value = h.ts.get_authed(&path).await.json().await.unwrap();
    assert!(v["artifact"].get("analytics").is_none(), "{v}");
}

#[tokio::test]
async fn the_read_refuses_agents_and_pages() {
    let h = Hosted::new().await;
    let aid = h.artifact("Read").await;
    let path = format!("/api/artifacts/{aid}/analytics");
    // The owner's token by hand reads it.
    let res = h.ts.get_authed(&path).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()[CACHE_CONTROL], "private, no-store");
    // The owner's agent does not.
    let sid = h.ts.register_session("claude", "a-reader").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res =
        h.ts.authed(h.ts.client.get(format!("{}{path}", h.ts.base)))
            .header("x-clax-session", &sid)
            .send()
            .await
            .unwrap();
    let s = res.status().as_u16();
    let v: Value = res.json().await.unwrap();
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
    // Nor a page.
    let res =
        h.ts.authed(h.ts.client.get(format!("{}{path}", h.ts.base)))
            .header("x-clax-via", "page")
            .send()
            .await
            .unwrap();
    let s = res.status().as_u16();
    let v: Value = res.json().await.unwrap();
    assert_eq!((s, code(&v)), (403, "not_for_pages"), "{v}");
    // A deleted artifact is 404.
    let res =
        h.ts.authed(
            h.ts.client
                .delete(format!("{}/api/artifacts/{aid}", h.ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let res = h.ts.get_authed(&path).await;
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn the_read_answers_every_day_of_its_range_with_totals_tables_and_version_marks() {
    // Version 1 on 2026-11-10, version 2 on 2026-11-19 (today).
    let h = Hosted::at("2026-11-10T10:00:00Z").await;
    let aid = h.artifact("Totals").await;
    h.clock.advance(9 * 86_400);
    let res =
        h.ts.post_json(
            &format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 1, "files": {"index.html": {"content": "<h1>two</h1>"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    record(
        &h.ts,
        &aid,
        "2026-11-18",
        &[view("index.html", 1), view("index.html", 1), {
            let mut v = view("b.html", 1);
            v.framed = true;
            v
        }],
    );
    record(&h.ts, &aid, "2026-11-19", &[view("index.html", 2)]);
    // Outside the default range.
    record(&h.ts, &aid, "2026-10-01", &[view("old.html", 1)]);
    let res =
        h.ts.get_authed(&format!("/api/artifacts/{aid}/analytics"))
            .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["analytics"], "on");
    assert_eq!(
        (&v["from"], &v["to"]),
        (&json!("2026-10-21"), &json!("2026-11-19"))
    );
    let days = v["days"].as_array().unwrap();
    assert_eq!(days.len(), 30);
    assert_eq!(
        days[0],
        json!({"day": "2026-10-21", "visitors": 0, "views": 0, "framed": 0})
    );
    assert_eq!(
        days[28],
        json!({"day": "2026-11-18", "visitors": 3, "views": 3, "framed": 1})
    );
    assert_eq!(
        days[29],
        json!({"day": "2026-11-19", "visitors": 1, "views": 1, "framed": 0})
    );
    assert_eq!(v["totals"]["visitors"], 4);
    assert_eq!(v["totals"]["views"], 4);
    assert_eq!(v["totals"]["framed"], 1);
    let load = v["totals"]["load_ms_median"].as_u64().unwrap();
    assert!((720..=880).contains(&load), "{load}");
    assert_eq!(v["totals"]["lcp_ms_median"], Value::Null);
    let marks: Vec<u64> = v["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["n"].as_u64().unwrap())
        .collect();
    assert_eq!(marks, [1, 2], "{v}");
    assert!(
        v["versions"][0]["published_at"]
            .as_str()
            .unwrap()
            .starts_with("2026-11-10"),
        "{v}"
    );
    assert_eq!(
        v["pages"],
        json!([
            {"value": "index.html", "views": 3, "visitors": 3},
            {"value": "b.html", "views": 1, "visitors": 1}
        ])
    );
    assert_eq!(
        v["referrers"],
        json!([{"value": "", "views": 4, "visitors": 4}])
    );
    assert_eq!(
        v["countries"],
        json!([{"value": "DE", "views": 4, "visitors": 4}])
    );
    assert_eq!(
        v["devices"],
        json!([{"value": "phone", "views": 4, "visitors": 4}])
    );
    assert_eq!(
        v["versions_viewed"],
        json!([
            {"value": "1", "views": 3, "visitors": 3},
            {"value": "2", "views": 1, "visitors": 1}
        ])
    );
    // A range of its own: the one old day, version 1 not published in it.
    let v: Value =
        h.ts.get_authed(&format!(
            "/api/artifacts/{aid}/analytics?from=2026-10-01&to=2026-10-01"
        ))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["days"].as_array().unwrap().len(), 1);
    assert_eq!(v["totals"]["views"], 1);
    assert_eq!(v["versions"], json!([]));
    assert!(v["totals"]["load_ms_median"].is_u64(), "{v}");
    // Turned off, what was counted is still read.
    h.ts.token_call(
        Method::PATCH,
        &format!("/api/artifacts/{aid}"),
        json!({"analytics": "off"}),
        None,
    )
    .await;
    let v: Value =
        h.ts.get_authed(&format!("/api/artifacts/{aid}/analytics"))
            .await
            .json()
            .await
            .unwrap();
    assert_eq!(
        (&v["analytics"], &v["totals"]["views"]),
        (&json!("off"), &json!(4))
    );
}

#[tokio::test]
async fn a_table_lists_its_top_fifty_and_sums_the_rest_as_other() {
    let h = Hosted::new().await;
    let aid = h.artifact("Many").await;
    let mut views = vec![view("p00.html", 1), view("p00.html", 1)];
    for i in 0..55 {
        views.push(view(&format!("p{i:02}.html"), 1));
    }
    record(&h.ts, &aid, "2026-11-19", &views);
    let v: Value =
        h.ts.get_authed(&format!("/api/artifacts/{aid}/analytics"))
            .await
            .json()
            .await
            .unwrap();
    let pages = v["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 51, "{v}");
    assert_eq!(
        pages[0],
        json!({"value": "p00.html", "views": 3, "visitors": 3})
    );
    assert_eq!(pages[49]["value"], "p49.html");
    assert_eq!(
        pages[50],
        json!({"value": "(other)", "views": 5, "visitors": 5})
    );
}

#[tokio::test]
async fn the_read_takes_ordered_ranges_within_four_hundred_days_and_retention() {
    let h = Hosted::new().await;
    let aid = h.artifact("Ranges").await;
    let at = |q: &str| format!("/api/artifacts/{aid}/analytics?{q}");
    for (q, want) in [
        ("from=2026-11-01&to=2026-11-19", 200),
        ("from=2025-10-19", 200),
        ("from=2025-10-18", 400),
        ("from=2026-11-20&to=2026-11-19", 400),
        ("from=2026-11-19&to=2027-12-24", 400),
        ("from=yesterday", 400),
        ("since=2026-11-01", 400),
    ] {
        let res = h.ts.get_authed(&at(q)).await;
        let s = res.status().as_u16();
        let v: Value = res.json().await.unwrap_or_default();
        assert_eq!(s, want, "{q}: {v}");
        if want == 400 {
            assert_eq!(code(&v), "invalid_argument", "{q}");
        }
    }
}

#[tokio::test]
async fn a_view_not_yet_written_is_read_at_once() {
    let h = Hosted::new().await;
    let aid = h.artifact("Fresh").await;
    record(&h.ts, &aid, "2026-11-19", &[view("index.html", 1)]);
    let counted =
        h.ts.state
            .analytics
            .count(
                &h.ts.store,
                &id(&aid),
                "203.0.113.7".parse().unwrap(),
                "Mozilla/5.0 (iPhone)",
                view("index.html", 1),
            )
            .unwrap();
    assert!(counted);
    assert!(
        h.ts.state.analytics.pending(&aid).len() == 1,
        "the view is still in memory"
    );
    let v: Value =
        h.ts.get_authed(&format!("/api/artifacts/{aid}/analytics"))
            .await
            .json()
            .await
            .unwrap();
    assert_eq!(v["days"][29]["views"], 2, "{v}");
    assert_eq!(v["totals"]["views"], 2, "{v}");
    assert_eq!(
        v["pages"],
        json!([{"value": "index.html", "views": 2, "visitors": 2}])
    );
}
