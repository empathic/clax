//! `clax.browser.test` and `clax.browser.browse` in a script (spec
//! docs/superpowers/specs/2026-11-19-claxbot-browser-design.md §5, §8,
//! §9): what is loaded, the grant, URL rule, cap and credits checked
//! before a browser opens, what is charged and counted, the journal's
//! answer to a step run again, and the screenshots a script shows the
//! model.

use crate::common::*;
use crate::resume::crash_once;
use base64::Engine as _;
use clax_bot::Next;
use clax_bot::browser::{
    BrowserError, CODE_BROWSER_BUSY, CODE_BROWSER_UNAVAILABLE, ChromeLog, FakeChrome, PageModel,
};
use clax_bot::host::CommentMeta;
use clax_bot::run::CrashPoint;
use clax_bot::web::OwnHosts;
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::bot_settings::Setting;
use clax_store::credits::{CostLinePatch, LINE_BROWSER_TIME};
use clax_store::roles::OWNER;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// The person's browser time rows: each one's quantity in milliseconds.
fn browser_rows(env: &Env) -> Vec<i64> {
    env.usage()
        .into_iter()
        .filter(|r| r.line == LINE_BROWSER_TIME)
        .map(|r| r.qty)
        .collect()
}

/// The `tool_result` content of the `i`th script step of `run` (from 0).
fn script_result(env: &Env, run: &str, i: usize) -> Value {
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    let s = steps
        .iter()
        .filter(|s| s.kind == StepKind::Script)
        .nth(i)
        .unwrap_or_else(|| panic!("no script step {i}"));
    let p = s.payload.as_ref().expect("committed");
    serde_json::from_str(p["result"]["content"].as_str().unwrap()).unwrap()
}

/// The `calls` of the `i`th script step's payload.
fn script_calls(env: &Env, run: &str, i: usize) -> Vec<Value> {
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    let s = steps
        .iter()
        .filter(|s| s.kind == StepKind::Script)
        .nth(i)
        .unwrap();
    s.payload.as_ref().unwrap()["calls"]
        .as_array()
        .unwrap()
        .clone()
}

/// Runs `code` as the one script of a run of `shape` (`""` for none),
/// then finishes; the run and the script's value.
async fn one_script(env: &Env, shape: &str, code: &str) -> (String, Value) {
    env.api.push(env.script_turn("tu_1", "Test", code));
    env.api.push(finish_turn("done"));
    let run = if shape.is_empty() {
        env.run()
    } else {
        env.run_with_browser(shape)
    };
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let r = script_result(env, &run, 0);
    assert!(r.get("error").is_none(), "{r}");
    (run, r["value"].clone())
}

/// The bodies the browser was sent for its documents, as text, in order.
fn served(log: &ChromeLog) -> Vec<String> {
    log.sent("Fetch.fulfillRequest")
        .iter()
        .map(|c| {
            let b = c["params"]["body"].as_str().unwrap_or_default();
            let bytes = base64::engine::general_purpose::STANDARD.decode(b).unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        })
        .collect()
}

/// The bodies of the `bot.browser` audit records, in order.
fn browser_events(env: &Env) -> Vec<Value> {
    env.bot_events()
        .into_iter()
        .filter(|(k, _)| k == "bot.spend")
        .map(|(_, b)| b)
        .collect()
}

#[tokio::test]
async fn a_test_answers_and_is_charged_its_measured_time() {
    let env = Env::new().await;
    let clock = env.clock.clone();
    // The page takes 4.2 s to settle.
    env.browser.opens(move || {
        let clock = clock.clone();
        Ok(FakeChrome::page(PageModel::new().evaluate(move |_| {
            clock.advance_millis(4_200);
            json!({"result": {"type": "undefined"}})
        })))
    });
    let start = env.clock_now();
    let (run, v) = one_script(&env, "test", "return await clax.browser.test();").await;
    assert_eq!(env.browser.opened(), 1);
    assert_eq!(v["passed"], true, "{v}");
    assert_eq!(v["url"], format!("https://{}.clax-test.invalid/", env.aid));
    assert_eq!(v["steps"], json!([]));
    assert_eq!(v["screenshots"], json!([]));
    // Two evaluations (the settle, the collect) of 4.2 s each.
    assert_eq!(v["ms"], 8_400);
    assert_eq!(v["left"]["tests"], 9);
    assert_eq!(v["left"]["browses"], 0);
    assert!(v["cost"].as_f64().unwrap() > 0.0, "{v}");
    // Charged 9 s: the measured time rounded up to the second.
    assert_eq!(browser_rows(&env), [9_000]);
    let rows = env.store.usage_of_run(&run).unwrap();
    let row = rows.iter().find(|r| r.line == LINE_BROWSER_TIME).unwrap();
    assert_eq!(row.step_n, Some(2));
    assert_eq!(
        (row.credits_milli as f64) / 1000.0,
        v["cost"].as_f64().unwrap()
    );
    // The deadline is the test's time cap from when it began.
    assert_eq!(
        env.browser.deadlines(),
        [start + chrono::Duration::seconds(60)]
    );
    // The browser was closed.
    let log = &env.browser.logs()[0];
    assert_eq!(log.sent("Browser.close").len(), 1);
    // The step's call and the audit record say what was tested.
    let calls = script_calls(&env, &run, 0);
    let b = &calls[0]["browser"];
    assert_eq!(
        (&b["target"], &b["path"], &b["viewport"], &b["passed"]),
        (
            &json!("draft"),
            &json!("index.html"),
            &json!("desktop"),
            &json!(true)
        )
    );
    assert_eq!(b["ms"], 8_400);
    assert_eq!(calls[0]["cost_milli"], row.credits_milli);
    let audit = browser_events(&env);
    assert_eq!(audit.len(), 1);
    let a = &audit[0];
    assert_eq!(
        (&a["op"], &a["path"], &a["viewport"], &a["passed"], &a["ok"]),
        (
            &json!("browser.test"),
            &json!("index.html"),
            &json!("desktop"),
            &json!(true),
            &json!(true)
        )
    );
    assert_eq!(a["ms"], 8_400);
    assert_eq!(a["credits_milli"], row.credits_milli);
    assert!(a.get("version").is_none(), "{a}");
}

#[tokio::test]
async fn a_replayed_step_opens_no_browser() {
    let env = Env::new().await;
    env.api
        .push(env.script_turn("tu_1", "Test", "return (await clax.browser.test()).passed;"));
    env.api.push(finish_turn("done"));
    let run = env.run_with_browser("test");
    let mut a = env.deps();
    crash_once(
        &mut a,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&a, &run, &DriverId::mint()).await;
    assert_eq!(
        out.last().unwrap().next,
        Next::Waiting(clax_bot::Wait::Crashed)
    );
    assert_eq!(browser_rows(&env).len(), 1, "charged when it returned");
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(script_result(&env, &run, 0)["value"], true);
    assert_eq!(env.browser.opened(), 1);
    assert_eq!(browser_rows(&env).len(), 1);
    assert_eq!(browser_events(&env).len(), 1);
}

#[tokio::test]
async fn two_tests_in_one_script_run_one_after_the_other() {
    let env = Env::new().await;
    let logs: Arc<Mutex<Vec<ChromeLog>>> = Arc::default();
    let overlapped = Arc::new(AtomicBool::new(false));
    let (seen, over) = (logs.clone(), overlapped.clone());
    env.browser.opens(move || {
        let mut seen = seen.lock().unwrap();
        if seen.iter().any(|l| l.closes() == 0) {
            over.store(true, Ordering::SeqCst);
        }
        let chrome = FakeChrome::page(PageModel::new());
        seen.push(chrome.log());
        Ok(chrome)
    });
    let (run, v) = one_script(
        &env,
        "test",
        "const [a, b] = await Promise.all([clax.browser.test(), clax.browser.test(null, {viewport: 'phone'})]);
         return [a.passed, b.passed, a.left.tests, b.left.tests];",
    )
    .await;
    assert!(!overlapped.load(Ordering::SeqCst), "two browsers at once");
    assert_eq!(env.browser.opened(), 2);
    assert_eq!(v[0], true);
    assert_eq!(v[1], true);
    let mut left = [v[2].as_i64().unwrap(), v[3].as_i64().unwrap()];
    left.sort_unstable();
    assert_eq!(left, [8, 9]);
    // Each is charged its own time.
    assert_eq!(browser_rows(&env).len(), 2);
    let calls = script_calls(&env, &run, 0);
    let mut viewports: Vec<&str> = calls
        .iter()
        .map(|c| c["browser"]["viewport"].as_str().unwrap())
        .collect();
    viewports.sort_unstable();
    assert_eq!(viewports, ["desktop", "phone"]);
}

#[tokio::test]
async fn the_test_loads_the_drafts_uncommitted_edit() {
    let env = Env::new().await;
    let (_, v) = one_script(
        &env,
        "test",
        "await clax.files.write('index.html', '<!doctype html><title>Edited</title><h1>Edited in this script</h1>');
         return await clax.browser.test();",
    )
    .await;
    assert_eq!(v["passed"], true, "{v}");
    let bodies = served(&env.browser.logs()[0]);
    assert!(
        bodies[0].contains("<h1>Edited in this script</h1>"),
        "{bodies:?}"
    );
    // Wrapped as the site wraps it: the bridge names the draft's base.
    assert!(bodies[0].contains("/_clax/bridge.js"), "{bodies:?}");
}

#[tokio::test]
async fn a_version_target_loads_that_version() {
    let env = Env::new().await;
    let (run, v) = one_script(
        &env,
        "test",
        "await clax.files.write('index.html', '<h1>Draft only</h1>');
         const a = await clax.browser.test({version: 1});
         let missing, nopage;
         try { await clax.browser.test({version: 9}); } catch (e) { missing = e.code; }
         try { await clax.browser.test({path: 'nope.html'}); } catch (e) { nopage = e.code; }
         return [a.passed, missing, nopage, a.left.tests];",
    )
    .await;
    assert_eq!(
        v,
        json!([
            true,
            "browser_target_not_found",
            "browser_target_not_found",
            9
        ])
    );
    assert_eq!(env.browser.opened(), 1);
    let bodies = served(&env.browser.logs()[0]);
    assert!(bodies[0].contains("Quarterly Report"), "{bodies:?}");
    assert!(!bodies[0].contains("Draft only"), "{bodies:?}");
    let calls = script_calls(&env, &run, 0);
    assert_eq!(calls[1]["browser"]["target"], "v1");
    let audit = browser_events(&env);
    assert_eq!(audit[0]["version"], 1);
    // A refusal is audited, uncharged and not counted.
    assert_eq!(audit.len(), 3);
    assert_eq!(audit[1]["code"], "browser_target_not_found");
    assert_eq!(audit[1]["credits_milli"], 0);
    let counted = env.store.bot_spend_counts(&run, "browser").unwrap();
    assert_eq!(counted.get("browser.test"), Some(&1), "{counted:?}");
}

#[tokio::test]
async fn the_cap_and_credits_refuse_before_opening() {
    let env = Env::new().await;
    env.admin_sets(&[(Setting::BrowserOpenArtifactMaxPerRun, json!(1))]);
    let (_, v) = one_script(
        &env,
        "test",
        "const a = await clax.browser.test();
         try { await clax.browser.test(); return 'tested'; }
         catch (e) { return [a.left.tests, e.code]; }",
    )
    .await;
    assert_eq!(v, json!([0, "browser_cap"]));
    assert_eq!(env.browser.opened(), 1);
    assert_eq!(browser_rows(&env).len(), 1);

    // A test costs at most 60 s at 1,000,000 credits an hour, some
    // 16,667 credits; the person has 1 left when the script runs.
    let env = Env::new().await;
    env.store
        .put_cost_line(
            &AuditCtx::DAEMON,
            LINE_BROWSER_TIME,
            CostLinePatch {
                fixed_credits_milli: Some(Some(1_000_000_000)),
                ..CostLinePatch::default()
            },
        )
        .unwrap();
    env.api.push(env.script_turn(
        "tu_1",
        "Test",
        "try { await clax.browser.test(); return 'tested'; } catch (e) { return e.code; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_browser("test");
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    let spent = env.store.week_usage_now(OWNER).unwrap().spent_milli;
    env.admin_sets(&[(Setting::FreeWeeklyMilli, json!(spent + 1_000))]);
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(out[0].kind, Some(StepKind::Script));
    assert_eq!(script_result(&env, &run, 0)["value"], "credits_exhausted");
    assert_eq!(env.browser.opened(), 0);
    assert!(browser_rows(&env).is_empty());
}

#[tokio::test]
async fn a_failure_before_opening_is_not_charged_but_counts() {
    let env = Env::new().await;
    env.admin_sets(&[(Setting::BrowserOpenArtifactMaxPerRun, json!(1))]);
    // The instance's browsers are busy, then Browser Run refuses.
    let tries = Arc::new(AtomicU32::new(0));
    let n = tries.clone();
    env.browser.opens(move || {
        Err(match n.fetch_add(1, Ordering::SeqCst) {
            0 => BrowserError::new(CODE_BROWSER_BUSY, "all 2 browsers are in use", true),
            _ => BrowserError::new(
                CODE_BROWSER_UNAVAILABLE,
                "Browser Run answered 429 (Retry-After: 5)",
                true,
            ),
        })
    });
    let (run, v) = one_script(
        &env,
        "test",
        "const codes = [];
         for (let i = 0; i < 3; i++) {
           try { await clax.browser.test(); codes.push('tested'); }
           catch (e) { codes.push([e.code, e.retryable === true]); }
         }
         return codes;",
    )
    .await;
    assert_eq!(
        v,
        json!([
            ["browser_busy", true],
            ["browser_unavailable", true],
            ["browser_cap", false]
        ])
    );
    assert_eq!(tries.load(Ordering::SeqCst), 2);
    assert!(
        browser_rows(&env).is_empty(),
        "nothing opened, nothing charged"
    );
    // Busy is not counted; Browser Run's refusal is.
    let counted = env.store.bot_spend_counts(&run, "browser").unwrap();
    assert_eq!(counted.get("browser.test"), Some(&1), "{counted:?}");
    let calls = script_calls(&env, &run, 0);
    assert!(calls.iter().all(|c| c["cost_milli"] == 0), "{calls:?}");
}

#[tokio::test]
async fn a_mid_run_opt_out_answers_browser_disabled() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Test",
        "try { await clax.browser.test(); return 'tested'; } catch (e) { return e.code; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_browser("test");
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    env.store
        .set_browser_choice(&AuditCtx::DAEMON, OWNER, false)
        .unwrap();
    env.drive(&deps, &run, &d).await;
    assert_eq!(script_result(&env, &run, 0)["value"], "browser_disabled");
    assert_eq!(env.browser.opened(), 0);
    assert!(browser_rows(&env).is_empty());
}

#[tokio::test]
async fn returned_screenshots_are_shown_to_the_model() {
    let env = Env::new().await;
    env.settings.0.lock().unwrap().model_takes_images = true;
    let (run, v) = one_script(
        &env,
        "test",
        "const r = await clax.browser.test(null, {steps: [{screenshot: {}}]});
         return {shots: r.screenshots, step: r.steps[0]};",
    )
    .await;
    let shot = &v["shots"][0];
    assert_eq!(v["shots"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["step"]["ok"], true);
    assert_eq!(&v["step"]["value"], shot);
    assert_eq!(shot["media_type"], "image/png");
    assert_eq!((&shot["width"], &shot["height"]), (&json!(1), &json!(1)));
    let id = shot["id"].as_str().unwrap();
    let row = env.store.bot_content_of(&run, id).unwrap().unwrap();
    assert_eq!(row.surface, "clax");
    assert_eq!(row.op.as_deref(), Some("browser.test"));
    assert_eq!(row.step_n, Some(2));
    // The tool result says it was shown, and the next turn shows it.
    let r = script_result(&env, &run, 0);
    assert_eq!(r["images"], json!([{"id": id, "shown": true}]));
    let reqs = env.api.requests();
    assert!(reqs[1].body.to_string().contains("\"type\":\"image\""));
    // The step's call keeps the handle's ID, not its bytes.
    let calls = script_calls(&env, &run, 0);
    assert_eq!(calls[0]["browser"]["screenshots"], json!([id]));
}

#[tokio::test]
async fn an_undeclared_browser_call_is_refused() {
    let env = Env::new().await;
    // The admin turned testing on, but the run was made without it.
    env.admin_sets(&[(Setting::BrowserOpenArtifactEnabled, json!(true))]);
    let (_, v) = one_script(
        &env,
        "",
        "try { await clax.browser.test(); return 'tested'; } catch (e) { return [e.code, e.message]; }",
    )
    .await;
    assert_eq!(
        v,
        json!([
            "browser_disabled",
            "clax.browser.test is not available to this run"
        ])
    );
    assert_eq!(env.browser.opened(), 0);
    assert!(browser_events(&env).is_empty());
}

#[tokio::test]
async fn saved_tests_results_come_through() {
    let env = Env::new().await;
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = asked.clone();
    env.browser.opens(move || {
        let seen = seen.clone();
        Ok(FakeChrome::page(PageModel::new().evaluate(move |p| {
            let e = p["expression"].as_str().unwrap_or_default();
            if e.contains("claxTest") {
                seen.lock().unwrap().push(e.to_string());
                json!({"result": {"type": "object", "value": {"passed": 1, "skipped": 0,
                    "failed": [{"name": "submits", "error": "expected true"}]}}})
            } else {
                json!({"result": {"type": "undefined"}})
            }
        })))
    });
    let (run, v) = one_script(
        &env,
        "test",
        "await clax.files.write('tests/form.test.js', \"test('submits', () => expect(false).toBe(true));\");
         return await clax.browser.test(null, {tests: true});",
    )
    .await;
    assert_eq!(
        v["tests"],
        json!({"passed": 1, "failed": [{"name": "submits", "error": "expected true"}], "skipped": 0})
    );
    assert_eq!(v["passed"], false);
    // The draft's saved test, written in this script, is the one run.
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0].contains(r#""files":["tests/form.test.js"]"#),
        "{asked:?}"
    );
    let calls = script_calls(&env, &run, 0);
    assert_eq!(
        calls[1]["browser"]["tests"],
        json!({"passed": 1, "failed": 1})
    );
}

#[tokio::test]
async fn a_failure_after_opening_is_charged_at_most_the_time_cap_and_counts() {
    let env = Env::new().await;
    let clock = env.clock.clone();
    let source = env.browser.clone();
    // The page never loads: its time runs 5 s past the 60 s cap before
    // the deadline is seen.
    env.browser.opens(move || {
        let clock = clock.clone();
        let source = source.clone();
        Ok(FakeChrome::page(PageModel::new().on(move |cmd: &Value| {
            (cmd["method"] == "Page.navigate").then(|| {
                clock.advance_millis(65_000);
                source.fire_deadline();
                vec![]
            })
        })))
    });
    let (run, v) = one_script(
        &env,
        "test",
        "try { await clax.browser.test(); return 'tested'; } catch (e) { return e.code; }",
    )
    .await;
    assert_eq!(v, "browser_timeout");
    assert_eq!(env.browser.opened(), 1);
    assert_eq!(browser_rows(&env), [60_000]);
    let counted = env.store.bot_spend_counts(&run, "browser").unwrap();
    assert_eq!(counted.get("browser.test"), Some(&1), "{counted:?}");
    let rows = env.store.usage_of_run(&run).unwrap();
    let row = rows.iter().find(|r| r.line == LINE_BROWSER_TIME).unwrap();
    assert_eq!(
        script_calls(&env, &run, 0)[0]["cost_milli"],
        row.credits_milli
    );
    let audit = browser_events(&env);
    assert_eq!(
        (&audit[0]["ok"], &audit[0]["code"]),
        (&json!(false), &json!("browser_timeout"))
    );
}

/// An open that stalls is bounded by the call's time cap, not only by
/// the source's own timeout: it answers browser_timeout, counted and not
/// charged.
#[tokio::test]
async fn an_open_that_stalls_ends_at_the_calls_deadline() {
    let env = Env::new().await;
    env.browser.hang_on_open();
    env.browser.fire_deadline();
    let (run, v) = one_script(
        &env,
        "test",
        "try { await clax.browser.test(); return 'tested'; } catch (e) { return e.code; }",
    )
    .await;
    assert_eq!(v, "browser_timeout");
    assert_eq!(env.browser.opened(), 1);
    assert_eq!(browser_rows(&env), Vec::<i64>::new());
    let counted = env.store.bot_spend_counts(&run, "browser").unwrap();
    assert_eq!(counted.get("browser.test"), Some(&1), "{counted:?}");
}

/// The run is cancelled while its browser is open: the sandbox drops the
/// call, and the browser's time so far is still charged, counted,
/// journaled and audited, once.
#[tokio::test]
async fn a_run_cancelled_while_the_browser_is_open_is_charged_its_time() {
    let env = Arc::new(Env::new().await);
    let clock = env.clock.clone();
    let held = Arc::new(tokio::sync::Notify::new());
    let hold = held.clone();
    // The page navigates 2.5 s in and its load never comes.
    env.browser.opens(move || {
        let (clock, hold) = (clock.clone(), hold.clone());
        Ok(FakeChrome::page(PageModel::new().on(move |cmd: &Value| {
            (cmd["method"] == "Page.navigate").then(|| {
                clock.advance_millis(2_500);
                hold.notify_one();
                vec![]
            })
        })))
    });
    env.api
        .push(env.script_turn("tu_1", "Test", "return await clax.browser.test();"));
    let run = env.run_with_browser("test");
    let cancel = clax_bot_sandbox::CancelToken::new();
    let task = {
        let (env, run, cancel) = (env.clone(), run.clone(), cancel.clone());
        tokio::spawn(async move {
            let deps = env.deps();
            let d = DriverId::mint();
            // The turn, then the script.
            env.tick(&deps, &run, &d).await;
            clax_bot::tick(
                &deps,
                &run,
                &d,
                clax_bot::TickOpts {
                    budget: None,
                    cancel,
                    events: &*env.hub,
                },
            )
            .await
            .unwrap()
        })
    };
    within("the browser is open", held.notified()).await;
    env.store
        .cancel_run(&clax_bot::run::bot_ctx("*owner"), &run)
        .unwrap();
    cancel.cancel();
    within("the cancelled tick ends", task).await.unwrap();
    assert_eq!(env.browser.opened(), 1);
    assert_eq!(browser_rows(&env), [3_000]);
    let counted = env.store.bot_spend_counts(&run, "browser").unwrap();
    assert_eq!(counted.get("browser.test"), Some(&1), "{counted:?}");
    let audit = browser_events(&env);
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(
        (&audit[0]["ok"], &audit[0]["code"], &audit[0]["ms"]),
        (&json!(false), &json!("cancelled"), &json!(2_500))
    );
}

#[tokio::test]
async fn an_eval_value_shaped_like_a_screenshot_stays_a_value() {
    let env = Env::new().await;
    env.browser.opens(|| {
        Ok(FakeChrome::page(PageModel::new().evaluate(|p: &Value| {
            let e = p["expression"].as_str().unwrap_or_default();
            if e.contains("JSON.stringify(v)") {
                json!({"result": {"type": "object", "value": {"json": "{\"screenshot\":0}"}}})
            } else {
                json!({"result": {"type": "undefined"}})
            }
        })))
    });
    let (_, v) = one_script(
        &env,
        "test",
        "const r = await clax.browser.test(null, {steps: [{eval: 'x'}, {screenshot: {}}]});
         return {steps: r.steps, shots: r.screenshots};",
    )
    .await;
    assert_eq!(
        v["steps"][0],
        json!({"ok": true, "value": {"screenshot": 0}}),
        "{v}"
    );
    assert_eq!(&v["steps"][1]["value"], &v["shots"][0]);
}

#[tokio::test]
async fn screenshots_are_kept_under_the_hosts_content_limits() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Test",
        "const r = await clax.browser.test(null, {steps: [{screenshot: {}}]});
         return {passed: r.passed, shots: r.screenshots, step: r.steps[0]};",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_browser("test");
    // An item may hold fewer bytes than the page's one-pixel PNG.
    let deps = env
        .deps()
        .with_content_limits(clax_store::bot_content::ContentLimits {
            item_bytes: 16,
            run_bytes: 1 << 20,
            total_bytes: 1 << 30,
        });
    let out = env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let v = script_result(&env, &run, 0)["value"].clone();
    assert_eq!(v["shots"], json!([]), "{v}");
    assert_eq!(v["passed"], false, "{v}");
    assert_eq!(v["step"]["ok"], false, "{v}");
    let why = v["step"]["error"].as_str().unwrap_or_default();
    assert!(why.contains("could not be kept"), "{v}");
    // Charged all the same: the browser opened.
    assert_eq!(browser_rows(&env).len(), 1);
}

/// Sets the admin's browse URL rule.
fn browse_rule(env: &Env, rule: &str) {
    env.admin_sets(&[(Setting::BrowserOpenUrlUrlRule, json!(rule))]);
}

/// Adds a comment of the sender's saying `text` to the run's request.
fn sender_says(env: &Env, text: &str) {
    env.host.lock().context.threads[0]
        .comments
        .push(CommentMeta {
            id: "c-sender".into(),
            author: "Owner".into(),
            from_sender: true,
            at: "2026-10-21T09:30:00Z".into(),
            text: text.into(),
        });
}

#[tokio::test]
async fn a_browse_opens_its_url_and_is_charged_as_a_browse() {
    let env = Env::new().await;
    browse_rule(&env, "any_public");
    let (run, v) = one_script(
        &env,
        "browse",
        "return await clax.browser.browse('https://public.example/', {viewport: 'phone', steps: [{goto: 'https://public.example/next'}]});",
    )
    .await;
    assert_eq!(env.browser.opened(), 1);
    assert_eq!(v["passed"], true, "{v}");
    // browser.open.url.max_per_run is 5 by default.
    assert_eq!(v["left"]["browses"], 4, "{v}");
    assert_eq!(v["left"]["tests"], 0, "{v}");
    let log = &env.browser.logs()[0];
    let navigated: Vec<Value> = log
        .sent("Page.navigate")
        .iter()
        .map(|c| c["params"]["url"].clone())
        .collect();
    assert_eq!(
        navigated,
        [
            json!("https://public.example/"),
            json!("https://public.example/next")
        ]
    );
    // Its requests go to the network; nothing of Clax's is served.
    assert_eq!(log.sent("Fetch.continueRequest").len(), 2);
    assert!(log.sent("Fetch.fulfillRequest").is_empty());
    assert_eq!(log.sent("Browser.close").len(), 1);
    assert_eq!(browser_rows(&env).len(), 1);
    let counted = env.store.bot_spend_counts(&run, "browser").unwrap();
    assert_eq!(counted.get("browser.browse"), Some(&1), "{counted:?}");
    assert_eq!(counted.get("browser.test"), None, "{counted:?}");
    let calls = script_calls(&env, &run, 0);
    let b = &calls[0]["browser"];
    assert_eq!(
        (&b["url"], &b["viewport"], &b["passed"]),
        (
            &json!("https://public.example/"),
            &json!("phone"),
            &json!(true)
        )
    );
    let audit = browser_events(&env);
    assert_eq!(
        (&audit[0]["op"], &audit[0]["url"], &audit[0]["ok"]),
        (
            &json!("browser.browse"),
            &json!("https://public.example/"),
            &json!(true)
        )
    );
}

#[tokio::test]
async fn sender_rule_admits_only_the_senders_urls() {
    let env = Env::new().await;
    sender_says(&env, "Compare with https://news.example/story please.");
    let (run, v) = one_script(
        &env,
        "browse",
        "const codes = [];
         for (const [url, opts] of [
           ['https://evil.example/'],
           ['https://news.example/story', {steps: [{goto: 'https://evil.example/?d=1'}]}],
           ['https://news.example/story'],
         ]) {
           try { codes.push((await clax.browser.browse(url, opts)).passed); }
           catch (e) { codes.push(e.code); }
         }
         return codes;",
    )
    .await;
    // A URL the sender did not write is in no source the rule takes.
    assert_eq!(
        v,
        json!(["url_not_in_prior_context", "url_not_in_prior_context", true])
    );
    assert_eq!(env.browser.opened(), 1);
    assert_eq!(browser_rows(&env).len(), 1);
    // Refusals are audited, uncharged and not counted.
    let counted = env.store.bot_spend_counts(&run, "browser").unwrap();
    assert_eq!(counted.get("browser.browse"), Some(&1), "{counted:?}");
    let audit = browser_events(&env);
    assert_eq!(audit.len(), 3);
    assert_eq!(audit[1]["code"], "url_not_in_prior_context");
    assert_eq!(audit[1]["credits_milli"], 0);
}

#[tokio::test]
async fn own_hosts_and_private_addresses_are_never_opened() {
    let env = Env::new().await;
    browse_rule(&env, "any_public");
    env.browser.set_own_hosts(Ok(OwnHosts::new(
        Some("https://clax.example"),
        Some("sites.example"),
    )
    .unwrap()));
    let (_, v) = one_script(
        &env,
        "browse",
        "const codes = [];
         for (const url of ['https://clax.example/a/x', 'https://x.sites.example/',
                            'http://127.0.0.1:9222/json', 'http://localhost/',
                            'http://169.254.169.254/latest/meta-data/', 'file:///etc/passwd']) {
           try { await clax.browser.browse(url); codes.push('opened'); }
           catch (e) { codes.push(e.code); }
         }
         return codes;",
    )
    .await;
    assert_eq!(v, json!(vec!["url_not_allowed"; 6]));
    assert_eq!(env.browser.opened(), 0);
    assert!(browser_rows(&env).is_empty());

    // An instance that cannot name its own hosts browses nothing.
    let env = Env::new().await;
    browse_rule(&env, "any_public");
    env.browser
        .set_own_hosts(Err("the public origin is not a URL".into()));
    let (_, v) = one_script(
        &env,
        "browse",
        "try { await clax.browser.browse('https://public.example/'); return 'opened'; }
         catch (e) { return e.code; }",
    )
    .await;
    assert_eq!(v, "browser_unavailable");
    assert_eq!(env.browser.opened(), 0);
}

#[tokio::test]
async fn a_browse_page_reaches_only_what_the_rule_admits() {
    let env = Env::new().await;
    browse_rule(&env, "allowlist");
    env.admin_sets(&[(
        Setting::BrowserOpenUrlAllowlist,
        json!(["news.example", "*.news.example"]),
    )]);
    // Under allowlist a source must hold the URL too.
    sender_says(&env, "Start from https://news.example/");
    env.browser.resolves_private("intranet.news.example");
    env.browser.opens(|| {
        Ok(FakeChrome::page(PageModel::new().requests(&[
            ("https://cdn.news.example/app.js", "Script"),
            ("https://intranet.news.example/", "Fetch"),
            ("http://10.1.2.3/", "Image"),
            ("https://evil.example/?d=x", "Image"),
        ])))
    });
    let (_, v) = one_script(
        &env,
        "browse",
        "return (await clax.browser.browse('https://news.example/')).requests.blocked;",
    )
    .await;
    assert_eq!(
        v,
        json!([
            {"url": "https://intranet.news.example/", "why": "private"},
            {"url": "http://10.1.2.3/", "why": "private"},
            {"url": "https://evil.example/?d=x", "why": "rule"},
        ])
    );
    let log = &env.browser.logs()[0];
    assert_eq!(log.sent("Fetch.continueRequest").len(), 2);
}

#[tokio::test]
async fn a_replayed_browse_opens_no_browser() {
    let env = Env::new().await;
    browse_rule(&env, "any_public");
    env.api.push(env.script_turn(
        "tu_1",
        "Browse",
        "return (await clax.browser.browse('https://public.example/')).passed;",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_browser("browse");
    let mut a = env.deps();
    crash_once(
        &mut a,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&a, &run, &DriverId::mint()).await;
    assert_eq!(
        out.last().unwrap().next,
        Next::Waiting(clax_bot::Wait::Crashed)
    );
    assert_eq!(browser_rows(&env).len(), 1, "charged when it returned");
    // The admin narrows the rule meanwhile: the replay answers from the
    // journal, not from the rule.
    browse_rule(&env, "sender");
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(script_result(&env, &run, 0)["value"], true);
    assert_eq!(env.browser.opened(), 1);
    assert_eq!(browser_rows(&env).len(), 1);
    assert_eq!(browser_events(&env).len(), 1);
}

#[tokio::test]
async fn browse_counts_against_its_own_cap() {
    let env = Env::new().await;
    browse_rule(&env, "any_public");
    env.admin_sets(&[(Setting::BrowserOpenUrlMaxPerRun, json!(1))]);
    let (_, v) = one_script(
        &env,
        "test,browse",
        "const a = await clax.browser.browse('https://public.example/');
         let code;
         try { await clax.browser.browse('https://public.example/'); } catch (e) { code = e.code; }
         const t = await clax.browser.test();
         return [a.left.browses, code, t.left.tests, t.left.browses];",
    )
    .await;
    assert_eq!(v, json!([0, "browser_cap", 9, 0]));
    assert_eq!(env.browser.opened(), 2);
}

#[tokio::test]
async fn browse_needs_its_own_capability() {
    // A run made without browse cannot call it.
    let env = Env::new().await;
    browse_rule(&env, "any_public");
    let (_, v) = one_script(
        &env,
        "test",
        "try { await clax.browser.browse('https://public.example/'); return 'opened'; }
         catch (e) { return e.code; }",
    )
    .await;
    assert_eq!(v, "browser_disabled");
    assert_eq!(env.browser.opened(), 0);

    // The person revokes their grant of browser.open.url to their runs
    // mid-run: browsing stops, testing goes on.
    let env = Env::new().await;
    browse_rule(&env, "any_public");
    env.api.push(env.script_turn(
        "tu_1",
        "Browse",
        "const codes = [];
         try { await clax.browser.browse('https://public.example/'); codes.push('opened'); }
         catch (e) { codes.push(e.code); }
         codes.push((await clax.browser.test()).passed);
         return codes;",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_browser("test,browse");
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    let url = env.store.person_caps(OWNER).unwrap()["browser.open.url"]
        .runs_grant
        .clone()
        .unwrap();
    env.store.revoke_cap_grant(&AuditCtx::DAEMON, &url).unwrap();
    env.drive(&deps, &run, &d).await;
    assert_eq!(
        script_result(&env, &run, 0)["value"],
        json!(["browser_disabled", true])
    );
    assert_eq!(env.browser.opened(), 1);
}

#[tokio::test]
async fn tests_are_refused_under_browse() {
    let env = Env::new().await;
    browse_rule(&env, "any_public");
    let (_, v) = one_script(
        &env,
        "browse",
        "try { await clax.browser.browse('https://public.example/', {tests: true}); return 'opened'; }
         catch (e) { return e.code; }",
    )
    .await;
    assert_eq!(v, "invalid_args");
    assert_eq!(env.browser.opened(), 0);
}

/// One performer of the Spend sequence, for
/// [`the_check_order_is_one_for_every_performer`].
struct Steps {
    /// The run's web and browser shapes.
    web: &'static str,
    browser: &'static str,
    /// The admin's switch of the call's capability, and its per-run cap.
    switch: Setting,
    max: Setting,
    /// The call's line, and its fixed price per base unit.
    line: &'static str,
    fixed: i64,
    /// What the person can spend so that one good call leaves too little
    /// for another.
    budget: i64,
    /// A call that passes, made first.
    good: &'static str,
    /// The call checked: with arguments that fail, then a target that is
    /// not admitted, then one that passes.
    subject: [&'static str; 3],
    /// The subject's code at each stage, from 0, or `ok`.
    expect: [&'static str; 6],
}

/// Each performer meets the steps in one order (spec
/// 2026-11-19-claxbot-browser §4.6): at stage `k` every check from the
/// `k`th on would refuse the call (the capability turned off, the
/// arguments, the admission, the cap after one good call, the credits
/// one good call left), and the call is refused by the first.
#[tokio::test]
async fn the_check_order_is_one_for_every_performer() {
    let performers = [
        Steps {
            web: "search",
            browser: "",
            switch: Setting::WebSearchEnabled,
            max: Setting::WebMaxSearchesPerRun,
            line: "web.search.ceramic",
            fixed: 5_000_000,
            budget: 5_000,
            good: "clax.web.search('a')",
            subject: [
                "clax.web.search('')",
                "clax.web.search('b')",
                "clax.web.search('b')",
            ],
            expect: [
                "web_disabled",
                "invalid_args",
                "web_cap",
                "web_cap",
                "credits_exhausted",
                "ok",
            ],
        },
        Steps {
            web: "fetch",
            browser: "",
            switch: Setting::WebFetchEnabled,
            max: Setting::WebMaxFetchesPerRun,
            line: clax_store::credits::LINE_WEB_FETCH,
            fixed: 5_000_000,
            budget: 5_000,
            good: "clax.web.fetch('https://a.example/one')",
            subject: [
                "clax.web.fetch('https://a.example/two', {offset: -1})",
                "clax.web.fetch('https://b.example/')",
                "clax.web.fetch('https://a.example/two')",
            ],
            expect: [
                "web_disabled",
                "invalid_args",
                "url_not_in_prior_context",
                "web_cap",
                "credits_exhausted",
                "ok",
            ],
        },
        Steps {
            web: "",
            browser: "test",
            switch: Setting::BrowserOpenArtifactEnabled,
            max: Setting::BrowserOpenArtifactMaxPerRun,
            line: LINE_BROWSER_TIME,
            fixed: 1_000,
            budget: 10_000,
            good: "clax.browser.test(null, {timeout_s: 10})",
            subject: [
                "clax.browser.test(null, {timeout_s: 10, viewport: {width: 9000, height: 800}})",
                "clax.browser.test({path: 'nope.html'}, {timeout_s: 10})",
                "clax.browser.test(null, {timeout_s: 10})",
            ],
            expect: [
                "browser_disabled",
                "invalid_args",
                "browser_target_not_found",
                "browser_cap",
                "credits_exhausted",
                "ok",
            ],
        },
        Steps {
            web: "",
            browser: "browse",
            switch: Setting::BrowserOpenUrlEnabled,
            max: Setting::BrowserOpenUrlMaxPerRun,
            line: LINE_BROWSER_TIME,
            fixed: 1_000,
            budget: 10_000,
            good: "clax.browser.browse('https://public.example/', {timeout_s: 10})",
            subject: [
                "clax.browser.browse('https://public.example/', {timeout_s: 10, viewport: {width: 9000, height: 800}})",
                "clax.browser.browse('http://localhost/', {timeout_s: 10})",
                "clax.browser.browse('https://public.example/', {timeout_s: 10})",
            ],
            expect: [
                "browser_disabled",
                "invalid_args",
                "url_not_allowed",
                "browser_cap",
                "credits_exhausted",
                "ok",
            ],
        },
    ];
    for p in &performers {
        let mut seen = Vec::new();
        for k in 0..6 {
            let env = Env::new().await;
            env.store
                .put_cost_line(
                    &AuditCtx::DAEMON,
                    p.line,
                    CostLinePatch {
                        fixed_credits_milli: Some(Some(p.fixed)),
                        ..CostLinePatch::default()
                    },
                )
                .unwrap();
            if p.browser == "browse" {
                browse_rule(&env, "any_public");
            }
            sender_says(&env, "Read https://a.example/one and https://a.example/two");
            env.web().page(
                "https://a.example/one",
                Ok(clax_bot::web::FetchedPage {
                    final_url: String::new(),
                    status: 200,
                    content_type: "text/markdown".into(),
                    text: "one".into(),
                    links: Vec::new(),
                    request_id: None,
                }),
            );
            env.web().page(
                "https://a.example/two",
                Ok(clax_bot::web::FetchedPage {
                    final_url: String::new(),
                    status: 200,
                    content_type: "text/markdown".into(),
                    text: "two".into(),
                    links: Vec::new(),
                    request_id: None,
                }),
            );
            env.web().search_answers(
                "a",
                Ok(clax_bot::web::SearchAnswer {
                    hits: Vec::new(),
                    request_id: None,
                    raw: json!({}),
                }),
            );
            env.web().search_answers(
                "b",
                Ok(clax_bot::web::SearchAnswer {
                    hits: Vec::new(),
                    request_id: None,
                    raw: json!({}),
                }),
            );
            // A browser call takes 9 s: two evaluations of 4.5 s each.
            let clock = env.clock.clone();
            env.browser.opens(move || {
                let clock = clock.clone();
                Ok(FakeChrome::page(PageModel::new().evaluate(move |_| {
                    clock.advance_millis(4_500);
                    json!({"result": {"type": "undefined"}})
                })))
            });
            let subject = p.subject[match k {
                0 | 1 => 0,
                2 => 1,
                _ => 2,
            }];
            let code = format!(
                "const codes = [];
                 try {{ await {}; }} catch (e) {{ codes.push('good: ' + e.code); }}
                 try {{ await {subject}; codes.push('ok'); }} catch (e) {{ codes.push(e.code); }}
                 return codes;",
                p.good
            );
            env.api.push(env.script_turn("tu_1", "Order", &code));
            env.api.push(finish_turn("done"));
            let run = if p.web.is_empty() {
                env.run_with_browser(p.browser)
            } else {
                env.run_with_web(p.web)
            };
            env.admin_sets(&[(p.max, json!(if k <= 3 { 1 } else { 10 }))]);
            if k == 0 {
                env.admin_sets(&[(p.switch, json!(false))]);
            }
            let deps = env.deps();
            let d = DriverId::mint();
            assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
            let spent = env.store.week_usage_now(OWNER).unwrap().spent_milli;
            let budget = if k <= 4 { p.budget } else { 1_000_000_000 };
            env.admin_sets(&[(Setting::FreeWeeklyMilli, json!(spent + budget))]);
            env.drive(&deps, &run, &d).await;
            let codes = script_result(&env, &run, 0)["value"].clone();
            seen.push(codes.as_array().unwrap().last().unwrap().clone());
        }
        assert_eq!(seen, p.expect.map(|c| json!(c)), "{} {}", p.web, p.browser);
    }
}

/// Every call of any capability past its declaration check writes one
/// `bot.spend` record naming its capability, refusals included; a call
/// the run did not declare writes none.
#[tokio::test]
async fn every_call_writes_one_bot_spend() {
    let env = Env::new().await;
    let (_, v) = one_script(
        &env,
        "test",
        "const codes = [];
         try { await clax.browser.test(); codes.push('ok'); } catch (e) { codes.push(e.code); }
         try { await clax.browser.test({path: 'nope.html'}); } catch (e) { codes.push(e.code); }
         try { await clax.browser.browse('https://public.example/'); } catch (e) { codes.push(e.code); }
         try { await clax.web.fetch('https://a.example/'); } catch (e) { codes.push(e.code + ' ' + e.capability); }
         return codes;",
    )
    .await;
    assert_eq!(
        v,
        json!([
            "ok",
            "browser_target_not_found",
            "browser_disabled",
            "web_disabled web.fetch"
        ])
    );
    let records = browser_events(&env);
    assert_eq!(records.len(), 2, "{records:?}");
    for r in &records {
        assert_eq!(r["capability"], "browser.open.artifact", "{r}");
        assert_eq!(r["op"], "browser.test", "{r}");
    }
    assert_eq!(records[0]["ok"], true);
    assert_eq!(records[1]["code"], "browser_target_not_found");
    assert!(
        !env.bot_events()
            .iter()
            .any(|(k, _)| k == "bot.web" || k == "bot.browser")
    );
}

/// A failure on the page is an answer: a test whose saved test fails is
/// journaled as a value (`passed: false`) and charged its time, and a
/// step run again answers it from the journal (spec §4.7).
#[tokio::test]
async fn an_ok_false_answer_is_journaled_and_charged() {
    let env = Env::new().await;
    env.browser.opens(|| {
        Ok(FakeChrome::page(PageModel::new().evaluate(|p| {
            if p["expression"].as_str().unwrap_or_default().contains("claxTest") {
                json!({"result": {"type": "object", "value": {"passed": 0, "skipped": 0,
                    "failed": [{"name": "a", "error": "b"}]}}})
            } else {
                json!({"result": {"type": "undefined"}})
            }
        })))
    });
    let (run, v) = one_script(
        &env,
        "test",
        "await clax.files.write('tests/a.test.js', \"test('a', () => {});\");
         return await clax.browser.test(null, {tests: true});",
    )
    .await;
    assert_eq!(v["passed"], false, "{v}");
    assert_eq!(browser_rows(&env).len(), 1, "charged");
    let calls = script_calls(&env, &run, 0);
    let test = calls.iter().find(|c| c["op"] == "browser.test").unwrap();
    assert_eq!(test["ok"], true, "{test}");
    let records = browser_events(&env);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["ok"], true);
    assert_eq!(records[0]["passed"], false);
    assert!(records[0]["credits_milli"].as_i64().is_some());
}
