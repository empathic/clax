//! Counting page views on a hosted Clax: `POST /_clax/api/analytics` on an
//! artifact's site host (`clax_server::routes::site_api`), the batch it
//! adds to and its write from `AppState::sweep`
//! (`clax_http::analytics`), and the `data-count` mark on a counted page's
//! bridge tag (`clax_server::routes::content`).

use clax_core::ArtifactId;
use clax_core::audit::AuditCtx;
use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{COOKIE, ETAG, HOST, IF_NONE_MATCH, ORIGIN, USER_AGENT};
use serde_json::{Value, json};
use std::sync::Arc;

const UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";
const DAY: &str = "2026-11-19";

/// A hosted daemon on a stopped clock; behind Cloudflare when `cf`, so a
/// request names its client by `CF-Connecting-IP` and its country by
/// `CF-IPCountry`.
async fn hosted(cf: bool) -> (Arc<ManualClock>, TestServer) {
    let clock = Arc::new(ManualClock::at("2026-11-19T10:00:00Z"));
    let ts = TestServer::spawn_hosted_with(clock.clone(), None, move |s| {
        if cf {
            s.hosted = s.hosted.take().map(|h| {
                Arc::new(
                    Arc::into_inner(h)
                        .expect("the only reference")
                        .behind_cloudflare(true),
                )
            });
        }
    })
    .await;
    (clock, ts)
}

/// A published artifact with two pages, its site open to the world unless
/// `private`.
async fn artifact(ts: &TestServer, private: bool) -> String {
    let aid = ts
        .publish(
            "Notes",
            &[("index.html", "<h1>Notes</h1>"), ("b.html", "<h1>B</h1>")],
        )
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    if !private {
        ts.store
            .set_site_open_to_world(&ArtifactId::parse(&aid).unwrap(), true)
            .unwrap();
    }
    aid
}

fn report(file: &str, version: u32) -> Value {
    json!({"file": file, "version": version, "framed": false,
           "referrer": "news.example.com", "load_ms": 840, "lcp_ms": 1210})
}

/// A page view request on `aid`'s site from its own origin, with [`UA`].
fn view(ts: &TestServer, aid: &str) -> reqwest::RequestBuilder {
    view_as(ts, aid, UA)
}

/// [`view`] with the User-Agent `ua` (a second `.header` would add one,
/// not replace it).
fn view_as(ts: &TestServer, aid: &str, ua: &str) -> reqwest::RequestBuilder {
    ts.client
        .post(format!("{}/_clax/api/analytics", ts.base))
        .header(HOST, ts.site_host(aid))
        .header(ORIGIN, format!("http://{}", ts.site_host(aid)))
        .header(USER_AGENT, ua)
}

async fn status(r: reqwest::RequestBuilder) -> u16 {
    r.send().await.unwrap().status().as_u16()
}

/// Writes what is pending: the clock past the batch's age, then a sweep.
async fn flush(clock: &ManualClock, ts: &TestServer) {
    clock.advance(60);
    ts.state.sweep().await;
}

/// `aid`'s stored `(views, visitors)` on `day`.
fn day(ts: &TestServer, aid: &str, day: &str) -> (u64, u64) {
    let d = clax_core::analytics::parse_day(day).unwrap();
    let r = ts
        .store
        .analytics_range(&ArtifactId::parse(aid).unwrap(), d, d)
        .unwrap();
    (r.days[0].views, r.days[0].visitors)
}

/// `aid`'s stored totals on [`DAY`].
fn range(ts: &TestServer, aid: &str) -> clax_core::analytics::RangeTotals {
    let d = clax_core::analytics::parse_day(DAY).unwrap();
    ts.store
        .analytics_range(&ArtifactId::parse(aid).unwrap(), d, d)
        .unwrap()
}

#[tokio::test]
async fn a_world_visitors_view_counts_once_a_day_and_twice_from_two_addresses() {
    let (clock, ts) = hosted(true).await;
    let aid = artifact(&ts, false).await;
    let from = |ip: &str| {
        view(&ts, &aid)
            .header("cf-connecting-ip", ip)
            .header("cf-ipcountry", "de")
    };
    assert_eq!(
        status(from("192.0.2.1").json(&report("index.html", 1))).await,
        204
    );
    assert_eq!(
        status(from("192.0.2.1").json(&report("index.html", 1))).await,
        204
    );
    assert_eq!(
        status(from("192.0.2.1").json(&report("b.html", 1))).await,
        204
    );
    assert_eq!(
        status(
            view_as(&ts, &aid, "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
                .header("cf-connecting-ip", "2001:db8::1")
                .header("cf-ipcountry", "de")
                .header("sec-ch-ua-mobile", "?0")
                .json(&json!({"file": "index.html", "version": 1, "framed": true,
                              "referrer": ts.site_host(&aid).split(':').next().unwrap(),
                              "load_ms": null, "lcp_ms": null, "unknown": [1]}))
        )
        .await,
        204
    );
    // Not written until the batch is due; counted in memory at once.
    assert_eq!(day(&ts, &aid, DAY), (0, 0));
    let pending = ts.state.analytics.pending(&aid);
    assert_eq!(pending.len(), 1);
    assert_eq!((pending[0].1.views, pending[0].1.visitors), (4, 2));
    flush(&clock, &ts).await;
    assert_eq!(day(&ts, &aid, DAY), (4, 2));
    let r = range(&ts, &aid);
    assert_eq!(r.days[0].framed, 1);
    let dim = |d: clax_core::analytics::Dim, v: &str| {
        let c = r.dims[&d][v];
        (c.views, c.visitors)
    };
    use clax_core::analytics::Dim;
    assert_eq!(dim(Dim::Path, "index.html"), (3, 2));
    assert_eq!(dim(Dim::Path, "b.html"), (1, 1));
    assert_eq!(dim(Dim::Country, "DE"), (4, 2));
    assert_eq!(dim(Dim::Device, "phone"), (3, 1));
    assert_eq!(dim(Dim::Device, "desktop"), (1, 1));
    assert_eq!(dim(Dim::Referrer, "news.example.com"), (3, 1));
    // The site's own host is no referring site.
    assert_eq!(dim(Dim::Referrer, ""), (1, 1));
    assert_eq!(dim(Dim::Version, "1"), (4, 2));
    // The same visitor later that day is no new visitor.
    from("192.0.2.1")
        .json(&report("index.html", 1))
        .send()
        .await
        .unwrap();
    flush(&clock, &ts).await;
    assert_eq!(day(&ts, &aid, DAY), (5, 2));
}

#[tokio::test]
async fn a_private_sites_view_needs_a_site_session() {
    let (clock, ts) = hosted(false).await;
    let aid = artifact(&ts, true).await;
    let r = view(&ts, &aid)
        .json(&report("index.html", 1))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "site_session_required", "{v}");
    let cookie = ts.site_cookie(&aid).await;
    assert_eq!(
        status(
            view(&ts, &aid)
                .header(COOKIE, cookie)
                .json(&report("index.html", 1))
        )
        .await,
        204
    );
    flush(&clock, &ts).await;
    assert_eq!(day(&ts, &aid, DAY), (1, 1));
}

#[tokio::test]
async fn a_view_from_another_origin_or_none_is_refused() {
    let (clock, ts) = hosted(false).await;
    let aid = artifact(&ts, false).await;
    let other = artifact(&ts, false).await;
    let send = |origin: Option<String>| {
        let mut r = ts
            .client
            .post(format!("{}/_clax/api/analytics", ts.base))
            .header(HOST, ts.site_host(&aid))
            .header(USER_AGENT, UA)
            .json(&report("index.html", 1));
        if let Some(o) = origin {
            r = r.header(ORIGIN, o);
        }
        async move {
            let r = r.send().await.unwrap();
            let s = r.status().as_u16();
            let v: Value = r.json().await.unwrap_or(Value::Null);
            (
                s,
                v["error"]["code"].as_str().unwrap_or_default().to_string(),
            )
        }
    };
    for origin in [
        Some(format!("http://{}", ts.site_host(&other))),
        Some("null".to_string()),
        None,
    ] {
        assert_eq!(
            send(origin.clone()).await,
            (403, "forbidden_origin".to_string()),
            "{origin:?}"
        );
    }
    // Only POST.
    let r = ts
        .client
        .get(format!("{}/_clax/api/analytics", ts.base))
        .header(HOST, ts.site_host(&aid))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 405);
    flush(&clock, &ts).await;
    assert_eq!(day(&ts, &aid, DAY), (0, 0));
}

#[tokio::test]
async fn what_is_not_counted_is_answered_204_all_the_same() {
    let (clock, ts) = hosted(false).await;
    let aid = artifact(&ts, false).await;
    let cases: Vec<(&str, reqwest::RequestBuilder)> = vec![
        (
            "global privacy control",
            view(&ts, &aid)
                .header("sec-gpc", "1")
                .json(&report("index.html", 1)),
        ),
        (
            "a bot",
            view_as(&ts, &aid, "Mozilla/5.0 (compatible; Googlebot/2.1)")
                .json(&report("index.html", 1)),
        ),
        (
            "no User-Agent",
            view_as(&ts, &aid, "").json(&report("index.html", 1)),
        ),
        (
            "a version past current",
            view(&ts, &aid).json(&report("index.html", 2)),
        ),
        (
            "a path with a control character",
            view(&ts, &aid).json(&report("a\u{7}.html", 1)),
        ),
    ];
    for (what, r) in cases {
        assert_eq!(status(r).await, 204, "{what}");
    }
    // The setting off.
    let id = ArtifactId::parse(&aid).unwrap();
    ts.store
        .set_analytics(&AuditCtx::DAEMON, &id, false)
        .unwrap();
    assert_eq!(
        status(view(&ts, &aid).json(&report("index.html", 1))).await,
        204
    );
    // A draft (a hidden artifact) is never counted.
    let p = clax_core::publish::validate(
        serde_json::from_value(json!({
            "title": "Draft",
            "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
        }))
        .unwrap(),
    )
    .unwrap();
    let draft = ts
        .store
        .create_draft(&AuditCtx::DAEMON, None, p)
        .unwrap()
        .artifact
        .id
        .to_string();
    let draft_id = ArtifactId::parse(&draft).unwrap();
    ts.store.set_site_open_to_world(&draft_id, true).unwrap();
    assert_eq!(
        status(view(&ts, &draft).json(&report("index.html", 1))).await,
        204
    );
    assert!(ts.state.analytics.pending(&aid).is_empty());
    assert!(ts.state.analytics.pending(&draft).is_empty());
    flush(&clock, &ts).await;
    assert_eq!(day(&ts, &aid, DAY), (0, 0));
    assert_eq!(day(&ts, &draft, DAY), (0, 0));
}

#[tokio::test]
async fn a_local_daemon_counts_nothing_and_marks_no_page() {
    let clock = Arc::new(ManualClock::at("2026-11-19T10:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let aid = ts
        .publish("Notes", &[("index.html", "<h1>Notes</h1>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let cookie = ts.site_cookie(&aid).await;
    assert_eq!(
        status(
            view(&ts, &aid)
                .header(COOKIE, cookie)
                .json(&report("index.html", 1))
        )
        .await,
        204
    );
    assert!(ts.state.analytics.pending(&aid).is_empty());
    flush(&clock, &ts).await;
    assert_eq!(day(&ts, &aid, DAY), (0, 0));
    let page = ts.site_get(&aid, "/").await.text().await.unwrap();
    assert!(page.contains("data-artifact="), "{page}");
    assert!(!page.contains("data-count"), "{page}");
}

#[tokio::test]
async fn a_body_past_2_kib_is_413_and_one_that_is_no_report_400() {
    let (_clock, ts) = hosted(false).await;
    let aid = artifact(&ts, false).await;
    let mut big = report("index.html", 1);
    let room = 2048 - big.to_string().len() - ",\"pad\":\"\"".len();
    big["pad"] = json!("x".repeat(room));
    assert_eq!(big.to_string().len(), 2048);
    assert_eq!(status(view(&ts, &aid).json(&big)).await, 204);
    big["pad"] = json!("x".repeat(room + 1));
    assert_eq!(status(view(&ts, &aid).json(&big)).await, 413);
    for bad in [
        "not json",
        "{\"file\": 3, \"version\": 1}",
        "{\"version\": 1}",
        "{\"file\": \"a\", \"version\": 1.5}",
    ] {
        let r = view(&ts, &aid)
            .header("content-type", "application/json")
            .body(bad)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400, "{bad}");
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["error"]["code"], "invalid_argument");
    }
}

#[tokio::test]
async fn past_the_rate_a_view_is_not_counted() {
    let (clock, ts) = hosted(false).await;
    let aid = artifact(&ts, false).await;
    for _ in 0..31 {
        assert_eq!(
            status(view(&ts, &aid).json(&report("index.html", 1))).await,
            204
        );
    }
    flush(&clock, &ts).await;
    assert_eq!(day(&ts, &aid, DAY), (30, 1), "30 at once");
    // One comes back every 2 s; the flush's 60 s gave back all 30.
    for _ in 0..31 {
        view(&ts, &aid)
            .json(&report("index.html", 1))
            .send()
            .await
            .unwrap();
    }
    flush(&clock, &ts).await;
    assert_eq!(day(&ts, &aid, DAY), (60, 1));
}

#[tokio::test]
async fn a_counted_pages_tag_is_marked_and_its_etag_follows_the_setting() {
    let (_clock, ts) = hosted(false).await;
    let aid = artifact(&ts, false).await;
    let get = |etag: Option<String>| {
        let mut r = ts
            .client
            .get(format!("{}/", ts.base))
            .header(HOST, ts.site_host(&aid));
        if let Some(t) = etag {
            r = r.header(IF_NONE_MATCH, t);
        }
        async move { r.send().await.unwrap() }
    };
    let r = get(None).await;
    assert_eq!(r.status(), 200);
    let on_tag = r.headers()[ETAG].to_str().unwrap().to_string();
    let page = r.text().await.unwrap();
    assert!(page.contains("data-count=\"1\"></script>"), "{page}");
    assert_eq!(page.matches("data-count").count(), 1);
    // Revalidated unchanged, 304.
    assert_eq!(get(Some(on_tag.clone())).await.status(), 304);
    // Turned off: the copy a browser keeps is answered 200, unmarked.
    let id = ArtifactId::parse(&aid).unwrap();
    ts.store
        .set_analytics(&AuditCtx::DAEMON, &id, false)
        .unwrap();
    let r = get(Some(on_tag.clone())).await;
    assert_eq!(r.status(), 200);
    let off_tag = r.headers()[ETAG].to_str().unwrap().to_string();
    assert_ne!(off_tag, on_tag);
    assert!(!r.text().await.unwrap().contains("data-count"));
    // A version's page under /_clax/v/<n>/ is marked as its root is.
    ts.store
        .set_analytics(&AuditCtx::DAEMON, &id, true)
        .unwrap();
    let r = ts
        .client
        .get(format!("{}/_clax/v/1/b.html", ts.base))
        .header(HOST, ts.site_host(&aid))
        .send()
        .await
        .unwrap();
    assert!(r.text().await.unwrap().contains("data-count=\"1\""));
    let r = get(Some(off_tag)).await;
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn cf_ipcountry_is_used_only_when_trusted() {
    for (cf, want) in [(true, "FR"), (false, "XX")] {
        let (clock, ts) = hosted(cf).await;
        let aid = artifact(&ts, false).await;
        view(&ts, &aid)
            .header("cf-ipcountry", "FR")
            .json(&report("index.html", 1))
            .send()
            .await
            .unwrap();
        flush(&clock, &ts).await;
        let r = range(&ts, &aid);
        let countries: Vec<&str> = r.dims[&clax_core::analytics::Dim::Country]
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(countries, vec![want], "behind Cloudflare: {cf}");
    }
}

#[tokio::test]
async fn the_rollover_writes_the_old_day_and_forgets_its_visitors() {
    use clax_core::working::Clock as _;
    let (clock, ts) = hosted(false).await;
    let aid = artifact(&ts, false).await;
    let due = || ts.state.analytics.next_due_in(clock.now());
    // At start the stored open day is unread: the first sweep reads it.
    assert_eq!(due(), Some(std::time::Duration::ZERO));
    ts.state.sweep().await;
    assert_eq!(due(), None);
    // 23:59:30.
    clock.advance(13 * 3600 + 59 * 60 + 30);
    assert_eq!(
        status(view(&ts, &aid).json(&report("index.html", 1))).await,
        204
    );
    assert!(ts.store.analytics_open_day().unwrap().is_some());
    // Midnight comes before the batch's age.
    assert_eq!(due(), Some(std::time::Duration::from_secs(30)));
    assert!(ts.state.next_sweep_in() <= Some(std::time::Duration::from_secs(30)));
    clock.advance(31);
    assert_eq!(due(), Some(std::time::Duration::ZERO));
    ts.state.sweep().await;
    assert_eq!(day(&ts, &aid, DAY), (1, 1));
    assert_eq!(
        ts.store.analytics_open_day().unwrap(),
        None,
        "secret and seen rows gone"
    );
    assert_eq!(due(), None);
    // The same client the next day is a new visitor.
    view(&ts, &aid)
        .json(&report("index.html", 1))
        .send()
        .await
        .unwrap();
    flush(&clock, &ts).await;
    assert_eq!(day(&ts, &aid, "2026-11-20"), (1, 1));
}
