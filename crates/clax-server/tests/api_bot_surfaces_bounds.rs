//! What a host of a ClaxBot surface is held to over HTTP
//! (`docs/contract.md`, "ClaxBot surfaces"): the bounds of a
//! registration's settings, how often a registration may change, how
//! many runs it may have that have not ended, and that a `serve` which
//! names no registration leaves nothing behind.

use clax_core::working::ManualClock;
use clax_testkit::remotes::json_of;
use clax_testkit::{ManualSleeper, TestServer};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const FIXTURE: &str = include_str!("../../../docs/claxbot-surfaces/contract.json");
const KEY: &str = "sk-ant-api03-TOPSECRETKEYVALUE-abcd";

fn desk_dts() -> String {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("the fixture is JSON");
    fixture["files"]["desk.d.ts"]
        .as_str()
        .expect("the fixture has desk.d.ts")
        .to_string()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// A driver that runs nothing: a run that is made stays queued.
#[derive(Default)]
struct Told(Mutex<Vec<String>>);

impl clax_http::bot::Runs for Told {
    fn started(&self, run: &str) {
        self.0.lock().unwrap().push(run.to_string());
    }
}

/// A daemon whose clock the test moves and whose driver runs nothing,
/// with its surfaces.
struct Daemon {
    ts: TestServer,
    clock: Arc<ManualClock>,
    surfaces: Arc<clax_http::bot_surfaces::Surfaces>,
}

async fn daemon() -> Daemon {
    let clock = Arc::new(ManualClock::at("2026-11-11T00:00:00Z"));
    let sleeper = Arc::new(ManualSleeper::default());
    let told = Arc::new(Told::default());
    let kept = Arc::new(Mutex::new(None));
    let keep = kept.clone();
    let ts = TestServer::spawn_with_clock_and(clock.clone(), move |state| {
        state.bot_surfaces.set_sleeper(sleeper);
        *keep.lock().unwrap() = Some(state.bot_surfaces.clone());
        state.bot = Arc::new(clax_http::bot::Bot {
            kek: Some(clax_store::secrets::Kek::from_bytes([7; 32])),
            runs: told,
            ..Default::default()
        });
    })
    .await;
    let surfaces = kept
        .lock()
        .unwrap()
        .take()
        .expect("the daemon has its surfaces");
    Daemon {
        ts,
        clock,
        surfaces,
    }
}

async fn put(ts: &TestServer, path: &str, body: &Value) -> (u16, Value) {
    let r = ts.authed(ts.client.put(format!("{}{path}", ts.base)).json(body));
    json_of(r.send().await.unwrap()).await
}

async fn post(ts: &TestServer, path: &str, body: &Value) -> (u16, Value) {
    let r = ts.authed(ts.client.post(format!("{}{path}", ts.base)).json(body));
    json_of(r.send().await.unwrap()).await
}

fn registering(definition: &str) -> Value {
    json!({"clax_surface": 1, "notation": "dts", "definition": definition})
}

/// Registers `desk` and answers its registration's ID.
async fn desk(ts: &TestServer) -> String {
    let (status, body) = put(ts, "/api/bot/surfaces/desk", &registering(&desk_dts())).await;
    assert_eq!(status, 200, "{body}");
    body["registration"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_setting_out_of_its_bounds_is_refused_and_nothing_is_kept() {
    let Daemon { ts, .. } = daemon().await;
    for (settings, needle) in [
        (
            json!({"allow": [format!("desk.{}", "a".repeat(7 * 1024 * 1024))]}),
            "too long",
        ),
        (json!({"call_timeout_s": 0}), "`call_timeout_s` is 0"),
        (json!({"wait_limit_s": u64::MAX}), "`wait_limit_s`"),
        (json!({"runs": {"max_steps": 0}}), "`runs.max_steps` is 0"),
        (
            json!({"runs": {"max_steps": 1_000_001}}),
            "`runs.max_steps` is 1000001",
        ),
        (
            json!({"runs": {"max_run_minutes": 0}}),
            "`runs.max_run_minutes` is 0",
        ),
        (
            json!({"runs": {"max_run_minutes": 525_601}}),
            "`runs.max_run_minutes` is 525601",
        ),
        (
            json!({"max_effects_per_run": 1_000_001}),
            "`max_effects_per_run` is 1000001",
        ),
        (
            json!({"call_timeout_s": 31_536_001}),
            "`call_timeout_s` is 31536001",
        ),
        // Nothing implements a reader view yet, whichever it is.
        (
            json!({"reader_view": "names"}),
            "`reader_view` (\"names\") is not supported yet",
        ),
        (
            json!({"reader_view": "none"}),
            "`reader_view` (\"none\") is not supported yet",
        ),
    ] {
        let mut asked = registering(&desk_dts());
        asked["settings"] = settings;
        let (status, body) = put(&ts, "/api/bot/surfaces/desk", &asked).await;
        assert_eq!((status, code(&body)), (400, "invalid_settings"), "{body}");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.contains(needle) && message.len() < 600, "{message}");
    }
    // A limit that is not a whole number of zero or more, or not
    // `"none"`, is no setting at all: the body is not the message
    // (`invalid_request`, as for a setting the daemon does not have).
    for settings in [
        json!({"max_effects_per_run": -1}),
        json!({"max_effects_per_run": 1.5}),
        json!({"max_effects_per_run": "5"}),
        json!({"runs": {"max_steps": true}}),
        json!({"runs": {"max_run_minutes": "forever"}}),
        json!({"call_timeout_s": 1e30}),
        json!({"max_effects_per_run": null}),
        json!({"runs": {"max_steps": null}}),
    ] {
        let mut asked = registering(&desk_dts());
        asked["settings"] = settings.clone();
        let (status, body) = put(&ts, "/api/bot/surfaces/desk", &asked).await;
        assert_eq!(
            (status, code(&body)),
            (400, "invalid_request"),
            "{settings}: {body}"
        );
        let message = body["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("a limit is a whole number, zero or more, or \"none\""),
            "{settings}: {message}"
        );
    }
    // `null` is no setting's value: not a limit, not `runs`.
    for settings in [json!({"runs": null}), json!({"allow": null})] {
        let mut asked = registering(&desk_dts());
        asked["settings"] = settings.clone();
        let (status, body) = put(&ts, "/api/bot/surfaces/desk", &asked).await;
        assert_eq!(
            (status, code(&body)),
            (400, "invalid_request"),
            "{settings}: {body}"
        );
    }
    let mut asked = registering(&desk_dts());
    asked["settings"] = json!({"max_effects": 5});
    let (status, body) = put(&ts, "/api/bot/surfaces/desk", &asked).await;
    assert_eq!((status, code(&body)), (400, "invalid_request"), "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown field `max_effects`"),
        "{body}"
    );
    assert!(
        ts.store
            .registrations_named("desk", true)
            .unwrap()
            .is_empty()
    );
    // At their bounds, a registration is taken: no effects at all, one
    // step, a year of minutes.
    let mut asked = registering(&desk_dts());
    asked["settings"] = json!({"max_effects_per_run": 0,
        "runs": {"max_steps": 1, "max_run_minutes": 525_600}});
    let (status, body) = put(&ts, "/api/bot/surfaces/desk", &asked).await;
    assert_eq!(status, 200, "{body}");
    // Within them, the settings are kept as they were sent.
    let mut asked = registering(&desk_dts());
    asked["settings"] =
        json!({"allow": ["desk.lamps"], "call_timeout_s": 30, "wait_limit_s": "none"});
    let (status, body) = put(&ts, "/api/bot/surfaces/desk", &asked).await;
    assert_eq!(status, 200, "{body}");
    // The listing is oldest first, in the order the registrations were
    // made, though both were made at one instant; the row read is the
    // one the answer names.
    let rows = ts.store.registrations_named("desk", true).unwrap();
    let last = rows.last().unwrap();
    assert_eq!(Some(last.id.as_str()), body["registration"].as_str());
    assert_eq!(last.settings, asked["settings"]);
}

#[tokio::test]
async fn a_serve_that_names_no_registration_leaves_nothing_behind() {
    let Daemon { ts, surfaces, .. } = daemon().await;
    let never = "0".repeat(32);
    for registration in ["a".repeat(7 * 1024 * 1024), never, "nope".to_string()] {
        let asked = json!({"registration": registration, "wait_ms": 0});
        let (status, body) = post(&ts, "/api/bot/surfaces/desk/serve", &asked).await;
        assert_eq!(
            (status, code(&body)),
            (404, "unknown_registration"),
            "{body}"
        );
        assert_eq!(surfaces.wakes(), 0);
    }
    // A registration of another surface, too.
    let registration = desk(&ts).await;
    let asked = json!({"registration": registration, "wait_ms": 0});
    let (status, body) = post(&ts, "/api/bot/surfaces/shelf/serve", &asked).await;
    assert_eq!(
        (status, code(&body)),
        (404, "unknown_registration"),
        "{body}"
    );
    assert_eq!(surfaces.wakes(), 0);
    // And a serve that was answered keeps none once it is over.
    let (status, body) = post(&ts, "/api/bot/surfaces/desk/serve", &asked).await;
    assert_eq!((status, &body), (200, &json!({"calls": []})));
    assert_eq!(surfaces.wakes(), 0);
}

#[tokio::test]
async fn a_registration_may_change_eight_times_a_minute() {
    let Daemon { ts, clock, .. } = daemon().await;
    let registration = desk(&ts).await;
    let changed = |n: u32| {
        let definition =
            desk_dts().replace("A desk and what is on it.", &format!("A desk, as of {n}."));
        let mut asked = registering(&definition);
        asked["registration"] = json!(registration);
        asked
    };
    for n in 1..=8 {
        let (status, body) = put(&ts, "/api/bot/surfaces/desk", &changed(n)).await;
        assert_eq!(status, 200, "{body}");
    }
    let (status, body) = put(&ts, "/api/bot/surfaces/desk", &changed(9)).await;
    assert_eq!((status, code(&body)), (429, "too_many_changes"), "{body}");
    // Unchanged, it is still gone on with; and a minute later it may
    // change again.
    let (status, body) = put(&ts, "/api/bot/surfaces/desk", &changed(8)).await;
    assert_eq!(status, 200, "{body}");
    clock.advance(61);
    let (status, body) = put(&ts, "/api/bot/surfaces/desk", &changed(9)).await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn a_registration_may_have_sixteen_runs_that_have_not_ended() {
    let Daemon { ts, .. } = daemon().await;
    let r = ts
        .client
        .put(format!("{}/api/admin/provider/key", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"key": KEY}))
        .send()
        .await
        .unwrap();
    let (status, body) = json_of(r).await;
    assert_eq!(status, 200, "{body}");
    let registration = desk(&ts).await;
    let start = json!({
        "registration": registration, "kind": "tidy", "request": {"ask": "Tidy the desk."}
    });
    let mut runs = Vec::new();
    for _ in 0..16 {
        let (status, body) = post(&ts, "/api/bot/surfaces/desk/runs", &start).await;
        assert_eq!(status, 200, "{body}");
        runs.push(body["run"].as_str().unwrap().to_string());
    }
    let (status, body) = post(&ts, "/api/bot/surfaces/desk/runs", &start).await;
    assert_eq!((status, code(&body)), (429, "too_many_runs"), "{body}");
    assert_eq!(ts.store.live_runs_started_by(&registration).unwrap(), 16);
    // Another registration has its own count.
    let other = desk(&ts).await;
    let mut theirs = start.clone();
    theirs["registration"] = json!(other);
    let (status, body) = post(&ts, "/api/bot/surfaces/desk/runs", &theirs).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(runs.len(), 16);
}

#[tokio::test]
async fn going_on_as_a_registration_with_something_changed_is_recorded_and_kept() {
    let Daemon { ts, .. } = daemon().await;
    let mut first = registering(&desk_dts());
    first["settings"] = json!({"allow": ["desk.*"]});
    let (status, body) = put(&ts, "/api/bot/surfaces/desk", &first).await;
    assert_eq!(status, 200, "{body}");
    let registration = body["registration"].as_str().unwrap().to_string();
    let records = || -> Vec<Value> {
        ts.store
            .events_after(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|row| row.kind == "bot.surface.registered")
            .map(|row| serde_json::from_str(&row.body).unwrap())
            .collect()
    };
    assert_eq!(records().len(), 1);
    assert_eq!(records()[0].get("changed"), None);
    // Gone on with as it is: no record.
    let mut same = first.clone();
    same["registration"] = json!(registration);
    let (status, body) = put(&ts, "/api/bot/surfaces/desk", &same).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(records().len(), 1);
    // With another definition and other settings: the row has them,
    // and a record says what it was changed to.
    let definition = desk_dts().replace("A desk and what is on it.", "A desk, changed.");
    let mut changed = registering(&definition);
    changed["registration"] = json!(registration);
    changed["settings"] = json!({"call_timeout_s": 5});
    let (status, body) = put(&ts, "/api/bot/surfaces/desk", &changed).await;
    assert_eq!(status, 200, "{body}");
    let row = ts.store.registration(&registration).unwrap().unwrap();
    assert_eq!(row.def_hash, body["hash"].as_str().unwrap());
    assert_eq!(row.prompt, None);
    assert_eq!(row.settings, json!({"call_timeout_s": 5}));
    let records = records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1]["changed"], true);
    assert_eq!(records[1]["registration"], registration.as_str());
    assert_eq!(records[1]["def_hash"], body["hash"]);
    assert_eq!(records[1].get("prompt_sha256"), None);
}
