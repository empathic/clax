//! One browser test against real Cloudflare Browser Run (spec
//! docs/superpowers/specs/2026-11-19-claxbot-browser-design.md §12, "Live,
//! opt-in"): opt-in, never in CI. It runs only with both
//! `CLAX_LIVE_BROWSER_RUN_ACCOUNT` (a Cloudflare account ID) and
//! `CLAX_LIVE_BROWSER_RUN_TOKEN` (an API token with Browser Rendering –
//! Edit) set, as `just browser-live` passes them; without them it returns
//! at once and passes, so CI runs it as a no-op.
//!
//! With them, a scratch daemon (the testkit's own home) whose model is a
//! scripted Messages API opens one browser session on a two-file
//! artifact through the daemon's own `DaemonBrowser`, sending to
//! Cloudflare's API, in one script step, and uses it through a `Page` in
//! the next, after a model turn longer than the socket's `keep_alive`
//! (60 s): the heartbeat kept the browser, so the second step's calls
//! reach the same page over the same socket, its title, one screenshot
//! and the stylesheet served, and `Browser.close` answered on it. This is
//! where the path's spelling (`/browser-run/devtools/browser`) and the
//! keep-alive are checked against the real service. No assertion is about
//! how long anything took.

use clax_bot::browser::{BrowserError, BrowserSource, CdpTransport, Deadline, Served};
use clax_bot_sandbox::CancelToken;
use clax_core::audit::AuditCtx;
use clax_store::bot_settings::Setting;
use clax_testkit::TestServer;
use clax_testkit::fake_messages::{FakeMessages, Turn};
use futures::StreamExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const KEY: &str = "sk-ant-api03-FAKEKEYFORTESTS-live";

/// The live credentials, or `None` when either is unset or empty.
fn credentials() -> Option<(String, String)> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    Some((
        var("CLAX_LIVE_BROWSER_RUN_ACCOUNT")?,
        var("CLAX_LIVE_BROWSER_RUN_TOKEN")?,
    ))
}

/// Every frame each way, in order: (`true` for one sent).
type Frames = Arc<Mutex<Vec<(bool, String)>>>;

/// A source that is `inner` and keeps every CDP frame of its sockets.
struct Recording {
    inner: Arc<dyn BrowserSource>,
    frames: Frames,
}

struct RecordingTransport {
    inner: Box<dyn CdpTransport>,
    frames: Frames,
}

#[async_trait::async_trait]
impl CdpTransport for RecordingTransport {
    async fn send(&mut self, frame: String) -> Result<(), BrowserError> {
        self.frames.lock().unwrap().push((true, frame.clone()));
        self.inner.send(frame).await
    }
    async fn recv(&mut self) -> Result<Option<String>, BrowserError> {
        let f = self.inner.recv().await?;
        if let Some(f) = &f {
            self.frames.lock().unwrap().push((false, f.clone()));
        }
        Ok(f)
    }
    async fn close(&mut self) {
        self.inner.close().await;
    }
}

#[async_trait::async_trait]
impl BrowserSource for Recording {
    async fn open(&self, cancel: &CancelToken) -> Result<Box<dyn CdpTransport>, BrowserError> {
        let inner = self.inner.open(cancel).await?;
        Ok(Box::new(RecordingTransport {
            inner,
            frames: self.frames.clone(),
        }))
    }
    fn configured(&self) -> bool {
        self.inner.configured()
    }
    fn deadline(&self, at: chrono::DateTime<chrono::Utc>) -> Deadline {
        self.inner.deadline(at)
    }
    fn clax_file(&self, path: &str) -> Option<Served> {
        self.inner.clax_file(path)
    }
    fn site_headers(&self, artifact: &str) -> Vec<(String, String)> {
        self.inner.site_headers(artifact)
    }
    async fn site_api(&self, person: &str, artifact: &str, path_and_query: &str) -> Served {
        self.inner.site_api(person, artifact, path_and_query).await
    }
}

/// PUTs `body` to `path` as the owner's browser, expecting 200.
async fn put(ts: &TestServer, path: &str, body: Value) {
    let res = ts
        .client
        .put(format!("{}{path}", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{path}");
}

/// Reads run `rid`'s events until `run_end`.
async fn until_the_end(ts: &TestServer, rid: &str) {
    let res = ts
        .client
        .get(format!("{}/api/bot/runs/{rid}/events", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut body = res.bytes_stream();
    let mut seen = String::new();
    clax_testkit::within("the run's end", async {
        while let Some(chunk) = body.next().await {
            seen.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            if seen.contains("event: run_end") {
                return;
            }
        }
    })
    .await;
}

/// What a script returned between `<sid>` and `</sid>` in the requests
/// the model was sent (its code writes neither whole): the session an
/// earlier step opened.
fn session_in(requests: &[Value]) -> String {
    let sent = serde_json::to_string(requests).unwrap();
    let from = sent.find("<sid>").expect("a step answered its session") + 5;
    let to = from + sent[from..].find("</sid>").unwrap();
    sent[from..to].to_string()
}

/// The model thinking for longer than Browser Run's `keep_alive`, between
/// the step that opened the session and the one that uses it.
#[expect(
    clippy::disallowed_methods,
    reason = "the live test's subject is a real pause longer than Browser Run's keep_alive"
)]
fn think_past_keep_alive() {
    std::thread::sleep(std::time::Duration::from_secs(
        u64::from(clax_server::browser_source::KEEP_ALIVE_MS) / 1000 + 10,
    ));
}

#[tokio::test]
async fn a_session_outlives_keep_alive_in_real_browser_run() {
    let Some((account, token)) = credentials() else {
        eprintln!(
            "live_browser_run: skipped (set CLAX_LIVE_BROWSER_RUN_ACCOUNT and \
             CLAX_LIVE_BROWSER_RUN_TOKEN, or run `just browser-live`)"
        );
        return;
    };
    let fake = FakeMessages::start(vec![
        Turn::script(
            "tu_1",
            "Open the page in a browser",
            "const page = await clax.browser.open({ artifact: 'draft' }, { viewport: 'phone' });
             await expect(page.getByText('Hello from Browser Run')).toBeVisible();
             return '<' + 'sid>' + JSON.parse(JSON.stringify(page)).session + '</' + 'sid>';",
        ),
        Turn::script_from("tu_2", "Look at it again", |requests| {
            think_past_keep_alive();
            format!(
                "const page = clax.browser.session('{}');
                 const title = await page.title();
                 const shot = await page.screenshot();
                 const requests = await page.failedRequests();
                 const closed = await page.close();
                 return {{ title, shot: typeof shot, failed: requests.failed, closed: closed.ms > 0 }};",
                session_in(requests)
            )
        }),
        Turn::finish("The page loads in a browser."),
    ])
    .await;
    let frames: Frames = Arc::default();
    let kept = frames.clone();
    let ts = TestServer::spawn_with(move |s| {
        let browser = Arc::new(Recording {
            inner: Arc::new(clax_server::browser_source::DaemonBrowser::of(s)),
            frames: kept,
        });
        let web = Arc::new(clax_bot::web::FakeWebSource::new());
        let d = clax_server::bot::driver_with_sources(s, web, browser);
        clax_server::bot::start_driver(&d);
    })
    .await;
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[
                (Setting::BaseUrl, Some(json!(fake.base))),
                (Setting::ScriptCpuMs, Some(json!(600_000))),
            ],
        )
        .unwrap();
    for cap in clax_store::bot_grants::CAPABILITIES {
        ts.store
            .issue_cap_grant(
                &AuditCtx::DAEMON,
                cap,
                clax_store::bot_grants::GRANTER_INSTANCE,
                clax_store::bot_grants::EVERYONE,
                None,
            )
            .unwrap();
    }
    put(&ts, "/api/admin/provider/key", json!({"key": KEY})).await;
    put(
        &ts,
        "/api/admin/web",
        json!({"cloudflare": {"account_id": account}}),
    )
    .await;
    put(&ts, "/api/admin/web/token", json!({"token": token})).await;
    put(
        &ts,
        "/api/admin/browser",
        json!({"artifact": {"enabled": true}}),
    )
    .await;
    let aid = ts
        .publish(
            "Live browser test",
            &[
                (
                    "index.html",
                    "<!doctype html><html><head><title>Live browser test</title>\
                     <link rel=\"stylesheet\" href=\"style.css\"></head>\
                     <body><h1>Hello from Browser Run</h1></body></html>",
                ),
                ("style.css", "h1 { color: rebeccapurple; }"),
            ],
        )
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid = ts.thread(&aid, 1, "Does it load in a browser?").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .client
        .post(format!("{}/api/bot/runs", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"thread_ids": [tid], "artifact_id": aid}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let rid = res.json::<Value>().await.unwrap()["run"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    until_the_end(&ts, &rid).await;

    // What the second step returned is the third request's tool result.
    let sent = serde_json::to_string(&fake.requests()).unwrap();
    for want in [
        r#"\"title\":\"Live browser test\""#,
        r#"\"shot\":\"object\""#,
        r#"\"failed\":[]"#,
        r#"\"closed\":true"#,
    ] {
        assert!(sent.contains(want), "{want} in {sent}");
    }
    assert!(!sent.contains("browser_session_lost"), "{sent}");
    let frames = frames.lock().unwrap().clone();
    let parsed = |out: bool| {
        frames
            .iter()
            .filter(move |(o, _)| *o == out)
            .filter_map(|(_, f)| serde_json::from_str::<Value>(f).ok())
    };
    // The heartbeat went out while the model thought.
    assert!(
        parsed(true)
            .filter(|v| v["method"] == "Browser.getVersion")
            .count()
            >= 2,
        "the heartbeat was sent"
    );
    // The stylesheet was asked for and served from the synthetic origin:
    // the request Chrome paused for style.css was fulfilled with a 200.
    let paused = parsed(false)
        .find(|v| {
            v["method"] == "Fetch.requestPaused"
                && v["params"]["request"]["url"]
                    .as_str()
                    .is_some_and(|u| u.ends_with("/style.css"))
        })
        .and_then(|v| v["params"]["requestId"].as_str().map(str::to_string))
        .expect("the stylesheet's request was paused");
    let served = parsed(true).any(|v| {
        v["method"] == "Fetch.fulfillRequest"
            && v["params"]["requestId"] == paused.as_str()
            && v["params"]["responseCode"] == 200
    });
    assert!(served, "the stylesheet's request was fulfilled with a 200");
    // Browser.close was sent and answered, once.
    let closes: Vec<i64> = parsed(true)
        .filter(|v| v["method"] == "Browser.close")
        .filter_map(|v| v["id"].as_i64())
        .collect();
    assert_eq!(closes.len(), 1, "one browser, closed once");
    assert!(
        parsed(false).any(|v| v["id"].as_i64() == Some(closes[0])),
        "Browser.close was answered"
    );
    // Its time was charged, once, as the session's segments.
    let row = ts
        .store
        .bot_browser_session(&rid, &session_in(&fake.requests()))
        .unwrap()
        .expect("the session's row");
    let charged: i64 = ts
        .store
        .usage_of_run(&rid)
        .unwrap()
        .into_iter()
        .filter(|r| r.line == clax_store::credits::LINE_BROWSER_TIME)
        .map(|r| r.qty)
        .sum();
    assert_eq!(charged, row.charged_ms);
    assert!(row.charged_ms > 0);
}
