//! One browser test against real Cloudflare Browser Run (spec
//! docs/superpowers/specs/2026-11-19-claxbot-browser-design.md §12, "Live,
//! opt-in"): opt-in, never in CI. It runs only with both
//! `CLAX_LIVE_BROWSER_RUN_ACCOUNT` (a Cloudflare account ID) and
//! `CLAX_LIVE_BROWSER_RUN_TOKEN` (an API token with Browser Rendering –
//! Edit) set, as `just browser-live` passes them; without them it returns
//! at once and passes, so CI runs it as a no-op.
//!
//! With them, a scratch daemon (the testkit's own home) whose model is a
//! scripted Messages API runs one `clax.browser.test()` of a two-file
//! artifact through the daemon's own `DaemonBrowser`, sending to
//! Cloudflare's API: the page's title, one screenshot and the stylesheet
//! served, and `Browser.close` answered on the socket. This is where the
//! path's spelling (`/browser-run/devtools/browser`) is checked against
//! the real service. No assertion is about how long anything took.

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

#[tokio::test]
async fn a_browser_test_runs_in_real_browser_run() {
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
            "Test the page in a browser",
            "const r = await clax.browser.test(null, {viewport: 'phone', steps: [
               {expect: {selector: 'h1', text: 'Hello from Browser Run'}},
               {screenshot: {}},
             ]});
             return {passed: r.passed, title: r.title, shots: r.screenshots.length,
                     failed: r.requests.failed, blocked: r.requests.blocked,
                     errors: r.errors, steps: r.steps};",
        ),
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
        json!({"test": {"enabled": true}}),
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

    // What the script returned is the second request's tool result.
    let sent = serde_json::to_string(&fake.requests()).unwrap();
    for want in [
        r#"\"title\":\"Live browser test\""#,
        r#"\"shots\":1"#,
        r#"\"passed\":true"#,
        r#"\"failed\":[]"#,
    ] {
        assert!(sent.contains(want), "{want} in {sent}");
    }
    // The stylesheet was asked for and served from the synthetic origin.
    let frames = frames.lock().unwrap().clone();
    let served = frames.iter().any(|(out, f)| {
        *out && f.contains("\"Fetch.fulfillRequest\"") && f.contains("\"responseCode\":200")
    });
    assert!(served, "a request was fulfilled");
    // Browser.close was sent and answered.
    let close_id = frames
        .iter()
        .filter(|(out, _)| *out)
        .filter_map(|(_, f)| serde_json::from_str::<Value>(f).ok())
        .find(|v| v["method"] == "Browser.close")
        .and_then(|v| v["id"].as_i64())
        .expect("Browser.close was sent");
    let answered = frames
        .iter()
        .filter(|(out, _)| !*out)
        .filter_map(|(_, f)| serde_json::from_str::<Value>(f).ok())
        .any(|v| v["id"].as_i64() == Some(close_id));
    assert!(answered, "Browser.close was answered");
    // Charged its browser time.
    let charged = ts
        .store
        .usage_of_run(&rid)
        .unwrap()
        .into_iter()
        .any(|r| r.line == clax_store::credits::LINE_BROWSER_TIME && r.credits_milli > 0);
    assert!(charged, "browser time was charged");
}
