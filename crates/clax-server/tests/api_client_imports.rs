//! The app's one-time import of its JSON state over HTTP (spec
//! 2026-10-19-in-process-core-design §9): `POST
//! /api/client-imports/app-json-v1`, under the queue owner's token, once.

use clax_testkit::{EventReader, TestServer};
use reqwest::Method;
use serde_json::{Value, json};

const OWNER: &str = "x-clax-jobs-owner";
const PATH: &str = "/api/client-imports/app-json-v1";

async fn send(r: reqwest::RequestBuilder) -> (u16, Value) {
    let res = r.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

fn authed(ts: &TestServer, m: Method, path: &str) -> reqwest::RequestBuilder {
    ts.authed(ts.client.request(m, format!("{}{path}", ts.base)))
}

/// The app's queue owner: its stream (kept open) and token.
async fn owner(ts: &TestServer) -> (EventReader, String) {
    let res = authed(ts, Method::GET, "/api/stream").send().await.unwrap();
    let mut events = EventReader::from_response(res);
    let sid = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = send(
        authed(ts, Method::POST, "/api/jobs/owner").json(&json!({"client": "app", "stream": sid})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    (events, v["owner"].as_str().unwrap().to_string())
}

/// The files: two pending makes, as `PendingMake` writes them, and a read
/// consent between two apps, as `FileConsentMemory` writes it.
fn files(reader: &str, source: &str) -> Value {
    json!({
        "pending-make.json": json!({"requests": ["a budget", "a habit tracker"], "waiting": []}).to_string(),
        "agent-read-consent.json": json!({"always": [[reader, source]]}).to_string(),
        "plans.json": "{\"plans\": ",
    })
}

#[tokio::test]
async fn an_import_applies_its_files_once_under_the_queue_owner() {
    let ts = TestServer::spawn().await;
    let id = |v: Value| v["artifact"]["id"].as_str().unwrap().to_string();
    let reader = id(ts.publish("Notes", &[("index.html", "<p>notes")]).await);
    let source = id(ts.publish("Runs", &[("index.html", "<p>runs")]).await);
    let body = json!({"client": "app", "files": files(&reader, &source)});
    // Without the owner's token: refused, nothing written.
    let (s, v) = send(authed(&ts, Method::POST, PATH).json(&body)).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_owner")),
        "{v}"
    );
    let (_o, token) = owner(&ts).await;
    let import = || {
        authed(&ts, Method::POST, PATH)
            .header(OWNER, &token)
            .json(&body)
    };
    let (s, v) = send(import()).await;
    assert_eq!(s, 200, "{v}");
    let i = &v["import"];
    assert_eq!(
        (
            i["already"].as_bool(),
            i["jobs"].as_u64(),
            i["consents"].as_u64()
        ),
        (Some(false), Some(2), Some(1)),
        "{v}"
    );
    assert_eq!(i["skipped"], json!(["plans.json"]));
    let (_, open) = send(authed(&ts, Method::GET, "/api/jobs?open=1&client=app")).await;
    let jobs = open["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 2, "{open}");
    assert!(jobs.iter().all(|j| j["job"]["state"] == "queued"
        && j["job"]["kind"] == "make"
        && j["job"]["id"].as_str().unwrap().starts_with("json-")));
    // Again: the summary, with nothing new.
    let (s, again) = send(import()).await;
    assert_eq!(s, 200, "{again}");
    assert_eq!(again["import"]["already"], true);
    assert_eq!(again["import"]["jobs"], 2);
    let (_, open) = send(authed(&ts, Method::GET, "/api/jobs?open=1&client=app")).await;
    assert_eq!(open["jobs"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn an_import_of_another_key_client_or_file_is_refused() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    for (path, body) in [
        (
            "/api/client-imports/app-json-v2",
            json!({"client": "app", "files": {}}),
        ),
        (
            PATH,
            json!({"client": "app", "files": {"settings.json": "{}"}}),
        ),
        (PATH, json!({"client": "web", "files": {}})),
    ] {
        let (s, v) = send(
            authed(&ts, Method::POST, path)
                .header(OWNER, &token)
                .json(&body),
        )
        .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (400, Some("invalid_args")),
            "{path} {body}: {v}"
        );
    }
}
