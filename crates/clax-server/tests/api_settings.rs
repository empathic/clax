//! The local daemon's settings (`clax_server::routes::settings`): `GET
//! /api/settings` lists every field with its value and source, `PUT
//! /api/settings` writes file-backed fields into the home's `config.toml`
//! keeping its comments, and only the owner reaches either.

use clax_core::config::settings::Change;
use clax_http::auth::Caller;
use clax_http::settings::{Settings, SettingsAudit};
use clax_store::Store;
use clax_testkit::TestServer;
use clax_testkit::remotes::{Public, json_of, local_req};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == key)
        .unwrap_or_else(|| panic!("no {key} in {v}"))
}

async fn put(ts: &TestServer, body: Value) -> (u16, Value) {
    local_req(ts, reqwest::Method::PUT, "/api/settings", body, None).await
}

#[tokio::test]
async fn the_owner_reads_and_writes_the_settings_keeping_the_files_comments() {
    let ts = TestServer::spawn().await;
    let file = ts.home.root().join("config.toml");
    std::fs::write(
        &file,
        "# Mine.\n[sample]\nprovider = \"stub\" # for tests\n",
    )
    .unwrap();
    let (s, v) = local_req(
        &ts,
        reqwest::Method::GET,
        "/api/settings",
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["file"], file.display().to_string());
    // The daemon started before the file had it: in effect is the
    // default, and the file's value is pending its next start.
    let p = field(&v, "sample.provider");
    assert_eq!(
        (&p["value"], &p["source"], &p["pending"]),
        (&json!("anthropic"), &json!("default"), &json!("stub"))
    );
    let steps = field(&v, "bot.max_steps");
    assert_eq!(
        (
            &steps["value"],
            &steps["source"],
            &steps["editable"],
            &steps["env"]
        ),
        (
            &json!(40),
            &json!("default"),
            &json!(true),
            &json!("CLAX_BOT__MAX_STEPS")
        )
    );
    assert_eq!(field(&v, "bin")["editable"], false);
    assert_eq!(v["problems"], json!([]));

    let (s, v) = put(
        &ts,
        json!({"set": {"bot.max_steps": 12, "sample.max_tokens": 9000}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["written"], json!(["bot.max_steps", "sample.max_tokens"]));
    assert_eq!(v["restart_required"], true);
    let steps = field(&v, "bot.max_steps");
    assert_eq!(
        (&steps["value"], &steps["source"], &steps["pending"]),
        (&json!(40), &json!("default"), &json!(12))
    );
    assert!(field(&v, "bot.concurrency").get("pending").is_none(), "{v}");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.starts_with("# Mine.\n[sample]\nprovider = \"stub\" # for tests\nmax_tokens = 9000\n"),
        "{text}"
    );
    assert!(text.contains("[bot]\nmax_steps = 12\n"), "{text}");

    // A refused change writes nothing.
    for (body, want) in [
        (json!({"set": {"nope": 1}}), "unknown_setting"),
        (json!({"set": {"bin": "/x"}}), "not_editable"),
        (
            json!({"set": {"bot.base_url": "http://evil.example"}}),
            "invalid_value",
        ),
        (
            json!({"set": {"serve.port": 0, "bot.max_steps": 3}}),
            "invalid_value",
        ),
        (json!({"sets": {}}), "invalid_json"),
    ] {
        let (s, v) = put(&ts, body.clone()).await;
        assert_eq!(s, 400, "{body}: {v}");
        if want != "invalid_json" {
            assert_eq!(code(&v), want, "{body}: {v}");
        }
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);

    // A table that does not check is a problem, and the rest still lists.
    std::fs::write(&file, "[bot]\nmax_steps = 0\n").unwrap();
    let (_, v) = local_req(
        &ts,
        reqwest::Method::GET,
        "/api/settings",
        json!(null),
        None,
    )
    .await;
    assert_eq!(v["problems"][0]["table"], "bot", "{v}");
    let steps = field(&v, "bot.max_steps");
    assert_eq!(
        (&steps["value"], &steps["pending"]),
        (&json!(40), &json!(0))
    );
}

#[tokio::test]
async fn only_the_owner_reaches_the_settings() {
    let ts = TestServer::spawn().await;
    let sid = ts.register_session("claude", "h-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let person = ts.person("Sam").await;
    for method in [reqwest::Method::GET, reqwest::Method::PUT] {
        let (s, v) = local_req(
            &ts,
            method.clone(),
            "/api/settings",
            json!({"set": {}}),
            Some(&sid),
        )
        .await;
        assert_eq!((s, code(&v)), (403, "not_for_agents"), "{method} {v}");
        let res = ts
            .authed(
                ts.client
                    .request(method.clone(), format!("{}/api/settings", ts.base)),
            )
            .header("x-clax-channel", "agent")
            .json(&json!({"set": {}}))
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(res).await;
        assert_eq!((s, code(&v)), (403, "not_for_agents"), "{method} {v}");
        let res = ts
            .client
            .request(method.clone(), format!("{}/api/settings", ts.base))
            .json(&json!({"set": {}}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401, "{method}");
        let res = ts
            .client
            .request(method.clone(), format!("{}/api/settings", ts.base))
            .header("cookie", format!("clax_viewer={}", person.cookie))
            .json(&json!({"set": {}}))
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(res).await;
        assert_eq!((s, code(&v)), (403, "forbidden"), "{method} {v}");
        // The owner's browser reaches both.
        let res = ts
            .client
            .request(method.clone(), format!("{}/api/settings", ts.base))
            .header("cookie", ts.owner_cookie())
            .json(&json!({"set": {}}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{method}");
    }
    // A page of another origin cannot change them.
    let res = ts
        .authed(ts.client.put(format!("{}/api/settings", ts.base)))
        .header("origin", "http://evil.example")
        .json(&json!({"set": {"bot.max_steps": 2}}))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{v}");
    assert!(!ts.home.root().join("config.toml").exists());
}

#[tokio::test]
async fn a_hosted_daemon_serves_no_settings() {
    let public = Public::new().await;
    let (s, _) = public
        .cli(reqwest::Method::GET, "/api/settings", None)
        .await;
    assert_eq!(s, 404);
    let (s, _) = public
        .browser(reqwest::Method::GET, "/api/settings", json!(null))
        .await;
    assert_eq!(s, 404);
}

/// A [`SettingsAudit`] that keeps what it is handed.
#[derive(Default)]
struct Recorder {
    viewed: Mutex<Vec<(String, Vec<String>)>>,
    changed: Mutex<Vec<(String, Vec<Change>)>>,
}

/// The caller as a recorder would name its actor.
fn actor(who: &Caller) -> String {
    match who {
        Caller::Owner { browser } => format!("owner(browser={browser})"),
        other => format!("{other:?}"),
    }
}

impl SettingsAudit for Recorder {
    fn viewed(&self, _st: &Store, who: &Caller, fields: &[String]) {
        self.viewed
            .lock()
            .unwrap()
            .push((actor(who), fields.to_vec()));
    }
    fn changed(&self, _st: &Store, who: &Caller, changes: &[Change]) {
        self.changed
            .lock()
            .unwrap()
            .push((actor(who), changes.to_vec()));
    }
}

#[tokio::test]
async fn views_and_changes_reach_the_audit_hook_and_refusals_do_not() {
    let rec = Arc::new(Recorder::default());
    let hook = rec.clone();
    let ts = TestServer::spawn_with(move |s| {
        s.settings = Arc::new(Settings {
            running: None,
            start_error: None,
            audit: hook,
        })
    })
    .await;
    let (s, _) = local_req(
        &ts,
        reqwest::Method::GET,
        "/api/settings",
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200);
    {
        let viewed = rec.viewed.lock().unwrap();
        assert_eq!(viewed.len(), 1);
        assert_eq!(viewed[0].0, "owner(browser=false)");
        assert!(
            viewed[0].1.iter().any(|k| k == "bot.max_steps"),
            "{viewed:?}"
        );
    }
    let (s, v) = put(&ts, json!({"set": {"bot.max_steps": 12}})).await;
    assert_eq!(s, 200, "{v}");
    {
        let changed = rec.changed.lock().unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, "owner(browser=false)");
        let c = &changed[0].1[0];
        assert_eq!(
            (c.key.as_str(), &c.old, &c.new),
            ("bot.max_steps", &json!(40), &json!(12))
        );
        assert_eq!(
            (c.old_source.label(), c.new_source.label()),
            ("default".to_string(), "file".to_string())
        );
    }
    // Without a running configuration the routes show what reads now.
    assert_eq!(field(&v, "bot.max_steps")["value"], 12);
    // A refused write, or a refused caller, records nothing.
    let (s, _) = put(&ts, json!({"set": {"bot.max_steps": 0}})).await;
    assert_eq!(s, 400);
    let sid = ts.register_session("claude", "h-2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for method in [reqwest::Method::GET, reqwest::Method::PUT] {
        let (s, _) = local_req(&ts, method, "/api/settings", json!({"set": {}}), Some(&sid)).await;
        assert_eq!(s, 403);
    }
    assert_eq!(rec.changed.lock().unwrap().len(), 1);
    assert_eq!(rec.viewed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_port_in_effect_is_the_one_the_daemon_listens_on() {
    let ts = TestServer::spawn().await;
    let (_, v) = local_req(
        &ts,
        reqwest::Method::GET,
        "/api/settings",
        json!(null),
        None,
    )
    .await;
    let port = field(&v, "serve.port");
    assert_eq!(port["value"], ts.addr.port(), "{v}");
    assert_eq!(port["pending"], 7480, "{v}");
    assert!(port["note"].as_str().unwrap().contains("--port"), "{v}");
    assert_eq!(v["started_with_invalid_config"], false);
}

#[tokio::test]
async fn a_daemon_started_on_a_config_that_did_not_read_says_so() {
    let ts = TestServer::spawn_with(|s| {
        std::fs::write(s.home.root().join("config.toml"), "[serve\n").unwrap();
        s.settings = Arc::new(Settings::at_start(&s.home, false));
    })
    .await;
    std::fs::write(ts.home.root().join("config.toml"), "[bot]\nmax_steps = 9\n").unwrap();
    let (s, v) = local_req(
        &ts,
        reqwest::Method::GET,
        "/api/settings",
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["started_with_invalid_config"], true, "{v}");
    assert!(
        v["start_error"].as_str().unwrap().contains("config.toml"),
        "{v}"
    );
    let steps = field(&v, "bot.max_steps");
    assert_eq!(
        (&steps["value"], &steps["source"], &steps["pending"]),
        (&json!(40), &json!("default"), &json!(9))
    );
}

#[tokio::test]
async fn a_dangling_config_symlink_is_a_conflict() {
    let ts = TestServer::spawn().await;
    let link = ts.home.root().join("config.toml");
    std::os::unix::fs::symlink(ts.home.root().join("gone.toml"), &link).unwrap();
    let (s, v) = put(&ts, json!({"set": {"bot.max_steps": 3}})).await;
    assert_eq!((s, code(&v)), (409, "config_symlink_dangling"), "{v}");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
