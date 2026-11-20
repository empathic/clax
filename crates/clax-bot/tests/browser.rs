//! `clax.browser` in a script (spec
//! docs/superpowers/specs/2026-11-19-claxbot-browser-design.md §6, §8,
//! §10): `browser.test`'s one-shot run of the saved tests, and sessions
//! opened through the prelude's `Page` that live across steps: what is
//! loaded, the capability, URL rule, caps and credits checked before a
//! browser opens, what is charged (each millisecond once) and counted,
//! the journal's answer to a step run again, and what a session's end
//! answers.

use crate::common::*;
use crate::resume::crash_once;
use base64::Engine as _;
use clax_bot::Next;
use clax_bot::browser::{
    BrowserError, CODE_BROWSER_BUSY, CODE_BROWSER_UNAVAILABLE, ChromeLog, FakeChrome, PageModel,
};
use clax_bot::host::CommentMeta;
use clax_bot::run::CrashPoint;
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

/// Runs `code` as the one script of a run holding the capabilities
/// `caps`, then finishes; the run and the script's value.
async fn one_script_with(env: &Env, caps: &str, code: &str) -> (String, Value) {
    env.api.push(env.script_turn("tu_1", "Test", code));
    env.api.push(finish_turn("done"));
    let run = env.run_with_caps(caps);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let r = script_result(env, &run, 0);
    assert!(r.get("error").is_none(), "{r}");
    (run, r["value"].clone())
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

/// The bodies of the `bot.spend` audit records, in order.
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
    assert!(
        v.get("steps").is_none() && v.get("screenshots").is_none(),
        "{v}"
    );
    // Two evaluations (the settle, the collect) of 4.2 s each.
    assert_eq!(v["ms"], 8_400);
    assert_eq!(v["left"]["artifact"], 9);
    assert_eq!(v["left"]["url"], 0);
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
         return [a.passed, b.passed, a.left.artifact, b.left.artifact];",
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
         const a = await clax.browser.test({artifact: 1});
         let missing, nopage;
         try { await clax.browser.test({artifact: 9}); } catch (e) { missing = e.code; }
         try { await clax.browser.test({artifact: 'draft', path: 'nope.html'}); } catch (e) { nopage = e.code; }
         return [a.passed, missing, nopage, a.left.artifact];",
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
    assert_eq!(
        counted.get("browser.open.artifact"),
        Some(&1),
        "{counted:?}"
    );
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
         catch (e) { return [a.left.artifact, e.code]; }",
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
    assert_eq!(
        counted.get("browser.open.artifact"),
        Some(&1),
        "{counted:?}"
    );
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
        .set_cap_choice(&AuditCtx::DAEMON, OWNER, "browser.open.artifact", false)
        .unwrap();
    env.drive(&deps, &run, &d).await;
    assert_eq!(script_result(&env, &run, 0)["value"], "browser_disabled");
    assert_eq!(env.browser.opened(), 0);
    assert!(browser_rows(&env).is_empty());
}

/// The prelude's `Page` layer and its `expect` are in the sandbox of a
/// run holding a `browser.open` capability, and in no other's (spec
/// §17.5); the plain operation a run without one calls is refused as
/// before.
#[tokio::test]
async fn a_run_without_a_browser_capability_has_no_page_layer() {
    let env = Env::new().await;
    let probe = "let code = null;
try { await clax.browser.open({ html: '' }); } catch (e) { code = e.code; }
return [typeof expect, code];";
    let (_, v) = one_script_with(&env, "", probe).await;
    assert_eq!(v, json!(["undefined", "browser_disabled"]));
    let (_, v) = one_script_with(
        &env,
        "",
        "return [typeof expect, typeof clax.browser.page];",
    )
    .await;
    assert_eq!(v, json!(["undefined", "function"]));
    let env = Env::new().await;
    let (_, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "return [typeof expect, typeof clax.browser.page];",
    )
    .await;
    assert_eq!(v, json!(["function", "undefined"]));
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
async fn test_runs_the_saved_tests_in_one_call() {
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
         return await clax.browser.test();",
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
    assert_eq!(
        counted.get("browser.open.artifact"),
        Some(&1),
        "{counted:?}"
    );
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
    assert_eq!(
        counted.get("browser.open.artifact"),
        Some(&1),
        "{counted:?}"
    );
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
    assert_eq!(
        counted.get("browser.open.artifact"),
        Some(&1),
        "{counted:?}"
    );
    let audit = browser_events(&env);
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(
        (&audit[0]["ok"], &audit[0]["code"], &audit[0]["ms"]),
        (&json!(false), &json!("cancelled"), &json!(2_500))
    );
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
            good: "clax.browser.test(null, {timeout: 10000})",
            subject: [
                "clax.browser.test(null, {timeout: 10000, viewport: {width: 9000, height: 800}})",
                "clax.browser.test({artifact: 'draft', path: 'nope.html'}, {timeout: 10000})",
                "clax.browser.test(null, {timeout: 10000})",
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
            good: "clax.browser.open({url: 'https://public.example/'}).then((p) => p.close())",
            subject: [
                "clax.browser.open({url: 'https://public.example/'}, {viewport: {width: 9000, height: 800}})",
                "clax.browser.open({url: 'http://localhost/'})",
                "clax.browser.open({url: 'https://public.example/'})",
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
                env.admin_sets(&[(Setting::BrowserCallSeconds, json!(10))]);
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
         try { await clax.browser.test({artifact: 'draft', path: 'nope.html'}); } catch (e) { codes.push(e.code); }
         try { await clax.browser.open({url: 'https://public.example/'}); } catch (e) { codes.push(e.code + ' ' + e.capability); }
         try { await clax.web.fetch('https://a.example/'); } catch (e) { codes.push(e.code + ' ' + e.capability); }
         return codes;",
    )
    .await;
    assert_eq!(
        v,
        json!([
            "ok",
            "browser_target_not_found",
            "browser_disabled browser.open.url",
            "web_disabled web.fetch"
        ])
    );
    let records = browser_events(&env);
    assert_eq!(records.len(), 3, "{records:?}");
    for r in &records[..2] {
        assert_eq!(r["capability"], "browser.open.artifact", "{r}");
        assert_eq!(r["op"], "browser.test", "{r}");
    }
    assert_eq!(records[0]["ok"], true);
    assert_eq!(records[1]["code"], "browser_target_not_found");
    assert_eq!(
        (
            &records[2]["capability"],
            &records[2]["op"],
            &records[2]["code"]
        ),
        (
            &json!("browser.open.url"),
            &json!("browser.open"),
            &json!("browser_disabled")
        )
    );
    assert!(!env.bot_events().iter().any(|(k, _)| k == "bot.web"));
}

/// A failure on the page is an answer: a test whose saved test fails is
/// journaled as a value (`passed: false`) and charged its time, and a
/// step run again answers it from the journal (spec §4.7).
#[tokio::test]
async fn an_ok_false_answer_is_journaled_and_charged() {
    let env = Env::new().await;
    env.browser.opens(|| {
        Ok(FakeChrome::page(PageModel::new().evaluate(|p| {
            if p["expression"]
                .as_str()
                .unwrap_or_default()
                .contains("claxTest")
            {
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
         return await clax.browser.test();",
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

// ------------------------------------------------------------ sessions

/// A page whose title is "Hello", whose document is `dom`.
fn page_of(dom: clax_bot::browser::fake::Dom) -> PageModel {
    PageModel::new().dom(dom).evaluate(|p| {
        if p["expression"] == "document.title" {
            clax_bot::browser::fake::value(json!("Hello"))
        } else {
            clax_bot::browser::fake::value(json!(true))
        }
    })
}

fn button(name: &str, x: f64) -> clax_bot::browser::fake::El {
    clax_bot::browser::fake::el("button")
        .text(name)
        .rect(x, 0.0, 100.0, 40.0)
        .role("button", name)
}

/// A page with one button, Book, and two named Save.
fn buttons() -> clax_bot::browser::fake::Dom {
    clax_bot::browser::fake::Dom::new(vec![
        button("Book", 0.0),
        button("Save", 200.0),
        button("Save", 400.0),
    ])
}

/// Every browser the source opens shows `model()`.
fn opens(env: &Env, model: impl Fn() -> PageModel + Send + 'static) {
    env.browser.opens(move || Ok(FakeChrome::page(model())));
}

/// The next script of `run`, `code`, as the model's turn `id`: its turn
/// and its script ticked; the script's result.
async fn next_script(
    env: &Env,
    deps: &clax_bot::run::Deps,
    run: &str,
    d: &DriverId,
    id: &'static str,
    code: &str,
) -> Value {
    env.api.push(env.script_turn(id, "Step", code));
    assert_eq!(env.tick(deps, run, d).await.kind, Some(StepKind::Turn));
    assert_eq!(env.tick(deps, run, d).await.kind, Some(StepKind::Script));
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    let n = steps.iter().filter(|s| s.kind == StepKind::Script).count();
    script_result(env, run, n - 1)
}

/// The ID of the run's one browser session, as the store keeps it.
fn only_session(env: &Env, run: &str) -> String {
    let rows = env.store.bot_browser_sessions(run).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    rows[0].session_id.clone()
}

/// A session's handle reaches the prelude alone: the open's answer in the
/// journal holds it, and no event, record, step payload field the run
/// panel reads, log or result names it; they name the session's ID.
#[tokio::test]
async fn an_open_answers_a_handle_no_record_names() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let (run, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "state.page = await clax.browser.open({ artifact: 'draft' });
         console.log(state.page);
         await state.page.title();
         return [state.page, String(state.page.toJSON())];",
    )
    .await;
    assert_eq!(v, json!(["<browser page>", "<browser page>"]));
    let session = only_session(&env, &run);
    let journal = env.store.bot_calls(&run, 2).unwrap();
    let open = journal
        .iter()
        .find(|c| c.op() == "browser.open")
        .expect("the open's entry");
    let handle = open.result()["value"]["page"]
        .as_str()
        .unwrap_or_else(|| panic!("no handle in {}", open.result()))
        .to_string();
    assert!(handle.starts_with("cxk_"), "{handle}");
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    let script = steps.iter().find(|s| s.kind == StepKind::Script).unwrap();
    let p = script.payload.as_ref().unwrap();
    for (what, text) in [
        ("events", serde_json::to_string(&env.bot_events()).unwrap()),
        ("calls", p["calls"].to_string()),
        ("result", p["result"].to_string()),
        ("outcome", p["outcome"].to_string()),
    ] {
        assert!(!text.contains(&handle), "{what} names the handle: {text}");
    }
    assert!(p["calls"].to_string().contains(&session), "{}", p["calls"]);
    // The state the step kept holds it, for the next script.
    assert_eq!(p["state"]["handles"][0]["page"], handle);
}

/// A page only in the returned value, held by no `state`, closes
/// `dropped` once its step commits (spec §17.4), and its time is charged
/// once: the open's segment and the tail.
#[tokio::test]
async fn a_page_only_returned_closes_dropped_when_its_step_commits() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let (run, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "const page = await clax.browser.open({ artifact: 'draft' }); return page;",
    )
    .await;
    assert_eq!(v, "<browser page>");
    let row = session_row(&env, &run, &only_session(&env, &run));
    assert_eq!(row.reason, Some(clax_store::bot_web::SessionEnd::Dropped));
    assert_eq!(env.browser.logs()[0].sent("Browser.close").len(), 1);
    assert_eq!(browser_rows(&env).iter().sum::<i64>(), row.charged_ms);
}

/// A page `state` holds lives into the next script; once a script lets
/// it go, it closes `dropped` when that script's step commits.
#[tokio::test]
async fn a_page_in_state_lives_into_the_next_script() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let run = env.run_with_caps("browser.open.artifact");
    let deps = env.deps();
    let d = DriverId::mint();
    next_script(
        &env,
        &deps,
        &run,
        &d,
        "tu_1",
        "state.page = await clax.browser.open({ artifact: 'draft' }); return 1;",
    )
    .await;
    let session = only_session(&env, &run);
    assert!(session_row(&env, &run, &session).is_open());
    let r = next_script(
        &env,
        &deps,
        &run,
        &d,
        "tu_2",
        "const t = await state.page.title(); state.n = 1; return t;",
    )
    .await;
    assert_eq!(r["value"], "Hello", "{r}");
    assert!(session_row(&env, &run, &session).is_open());
    next_script(
        &env,
        &deps,
        &run,
        &d,
        "tu_3",
        "delete state.page; return 1;",
    )
    .await;
    let row = session_row(&env, &run, &session);
    assert_eq!(row.reason, Some(clax_store::bot_web::SessionEnd::Dropped));
    assert_eq!(browser_rows(&env).iter().sum::<i64>(), row.charged_ms);
}

/// A step executed again after a crash before its commit answers its
/// open from the journal and, its state the same, closes the same
/// session `dropped` when it commits: one browser, charged once.
#[tokio::test]
async fn a_replayed_step_drops_the_same_sessions() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    env.api.push(env.script_turn(
        "tu_1",
        "Look",
        "const page = await clax.browser.open({ artifact: 'draft' }); return await page.title();",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_caps("browser.open.artifact");
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
    let session = only_session(&env, &run);
    assert!(
        session_row(&env, &run, &session).is_open(),
        "nothing committed yet"
    );
    env.clock.advance(61);
    let out = env.drive(&a, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(env.browser.opened(), 1);
    let row = session_row(&env, &run, &session);
    assert_eq!(row.reason, Some(clax_store::bot_web::SessionEnd::Dropped));
    assert_eq!(browser_rows(&env).iter().sum::<i64>(), row.charged_ms);
}

/// A page shared read-only reads and cannot click, navigate or close;
/// revoked, it allows nothing, and the page it was shared from still
/// works (spec §18.4). The share and the revoke are `bot.spend` records
/// naming the capabilities by their IDs, and nothing the run shows or
/// records holds a capability.
#[tokio::test]
async fn a_shared_read_only_page_reads_and_cannot_click() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let (run, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "const page = await clax.browser.open({ artifact: 'draft' });
         const ro = await page.share({ ops: ['read'] });
         const out = [await ro.title(), ro, JSON.stringify({ ro })];
         try { await ro.getByRole('button', { name: 'Book' }).click(); out.push('clicked'); } catch (e) { out.push(e.code); }
         try { await ro.goto('/x'); out.push('went'); } catch (e) { out.push(e.code); }
         try { await ro.close(); out.push('closed'); } catch (e) { out.push(e.code); }
         try { await page.share({ ops: ['write'] }); } catch (e) { out.push(e.code); }
         await page.getByRole('button', { name: 'Book' }).click();
         await ro.revoke();
         try { await ro.title(); } catch (e) { out.push(e.code + ': ' + e.message); }
         try { await page.revoke(); } catch (e) { out.push(e.code); }
         out.push(await page.title());
         state.page = page;
         return out;",
    )
    .await;
    assert_eq!(
        v,
        json!([
            "Hello",
            "<browser page: read>",
            "{\"ro\":\"<browser page: read>\"}",
            "browser_forbidden",
            "browser_forbidden",
            "browser_forbidden",
            "invalid_args",
            "browser_forbidden: this browser page's capability was revoked",
            "invalid_args",
            "Hello"
        ])
    );
    let records = browser_events(&env);
    let of = |action: &str| -> Vec<&Value> {
        records
            .iter()
            .filter(|r| r["action"] == action && r["ok"] == true)
            .collect()
    };
    let (shares, revokes) = (of("share"), of("revoke"));
    assert_eq!((shares.len(), revokes.len()), (1, 1), "{records:?}");
    let id = shares[0]["capability"].as_str().unwrap();
    assert_eq!(id.len(), 22);
    assert_eq!(shares[0]["parent"].as_str().map(str::len), Some(22));
    assert_eq!(shares[0]["ops"], json!(["read"]));
    assert_eq!(revokes[0]["capability"], id);
    assert_eq!(shares[0]["credits_milli"], 0, "{}", shares[0]);
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    let script = steps.iter().find(|s| s.kind == StepKind::Script).unwrap();
    let p = script.payload.as_ref().unwrap();
    for (what, text) in [
        ("events", serde_json::to_string(&env.bot_events()).unwrap()),
        ("calls", p["calls"].to_string()),
        ("result", p["result"].to_string()),
    ] {
        assert!(!text.contains("cxk_"), "{what} holds a capability: {text}");
    }
    let row = session_row(&env, &run, &only_session(&env, &run));
    assert_eq!(row.reason, Some(clax_store::bot_web::SessionEnd::RunEnded));
}

/// A step executed again after a crash before its commit answers its
/// share from the journal: the state it commits holds the capability the
/// journal answered, which alone keeps the session open (spec §18.7), and
/// the session ends with the run, not `dropped`.
#[tokio::test]
async fn a_replayed_share_answers_the_same_capability() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    env.api.push(env.script_turn(
        "tu_1",
        "Share",
        "const page = await clax.browser.open({ artifact: 'draft' });
         state.ro = await page.share({ ops: ['read'], expires: 600 });
         return String(state.ro.toJSON());",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_caps("browser.open.artifact");
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
    let shared = |env: &Env| -> Vec<String> {
        env.store
            .bot_calls(&run, 2)
            .unwrap()
            .iter()
            .filter(|c| c.op() == "browser.page")
            .map(|c| {
                c.result()["value"]["value"]["page"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    };
    let first = shared(&env);
    assert_eq!(first.len(), 1, "{first:?}");
    env.clock.advance(61);
    let out = env.drive(&a, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(
        shared(&env),
        first,
        "the share is answered from the journal"
    );
    assert_eq!(env.browser.opened(), 1);
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    let script = steps.iter().find(|s| s.kind == StepKind::Script).unwrap();
    let p = script.payload.as_ref().unwrap();
    assert_eq!(p["state"]["handles"][0]["page"], first[0]);
    assert!(
        script_result(&env, &run, 0)["value"]
            .as_str()
            .is_some_and(|s| s.starts_with("<browser page: read; expires ")),
        "{}",
        script_result(&env, &run, 0)
    );
    let row = session_row(&env, &run, &only_session(&env, &run));
    assert_eq!(row.reason, Some(clax_store::bot_web::SessionEnd::RunEnded));
}

/// The run's session row of `id`.
fn session_row(env: &Env, run: &str, id: &str) -> clax_store::bot_web::SessionRow {
    env.store.bot_browser_session(run, id).unwrap().unwrap()
}

/// A session opened in one step and clicked in a later one, a model turn
/// longer than Browser Run's keep-alive between them: the heartbeat kept
/// the browser, and each millisecond of the session is charged once, the
/// click's segment from the open's end to its own.
#[tokio::test]
async fn a_session_lives_across_steps_and_its_time_is_charged_once() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let run = env.run_with_caps("browser.open.artifact");
    let deps = env.deps();
    let d = DriverId::mint();
    let r = next_script(
        &env,
        &deps,
        &run,
        &d,
        "tu_1",
        "state.page = await clax.browser.open({ artifact: 'draft' }); return state.page;",
    )
    .await;
    assert_eq!(r["value"], "<browser page>");
    let session = only_session(&env, &run);
    let opened = session_row(&env, &run, &session);
    assert!(opened.is_open());
    // The model thinks for 90 s: the heartbeat goes out.
    env.clock.advance(90);
    env.browser.fire_until(env.clock_now());
    let log = env.browser.logs()[0].clone();
    within("the heartbeat", log.wait_sent("Browser.getVersion", 1)).await;
    let r = next_script(
        &env,
        &deps,
        &run,
        &d,
        "tu_2",
        "const page = state.page;
         await page.getByRole('button', { name: 'Book' }).click();
         return await page.close();",
    )
    .await;
    assert!(r.get("error").is_none(), "{r}");
    let closed = session_row(&env, &run, &session);
    assert!(!closed.is_open());
    assert_eq!(closed.reason, Some(clax_store::bot_web::SessionEnd::Closed));
    let rows = browser_rows(&env);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0], opened.charged_ms);
    assert!(
        rows[1] >= 90_000,
        "the click's segment holds the turn: {rows:?}"
    );
    assert_eq!(rows.iter().sum::<i64>(), closed.charged_ms);
    assert_eq!(r["value"]["ms"], closed.charged_ms);
    assert_eq!(log.sent("Input.dispatchMouseEvent").len(), 3);
    assert_eq!(env.browser.opened(), 1);
}

/// A step cut while a call of its session was in flight, run again by a
/// new process (a new registry): the calls its journal holds answer from
/// it, and the first it does not hold answers `browser_session_lost`.
#[tokio::test]
async fn a_replay_after_a_restart_answers_lost() {
    let env = Arc::new(Env::new().await);
    // The click waits for an element that never comes.
    opens(&env, || page_of(buttons()).frame_limit(2));
    env.api.push(env.script_turn(
        "tu_1",
        "Step",
        "const page = await clax.browser.open({ artifact: 'draft' });
         const t = await page.title();
         try { await page.getByText('Never').click(); return 'clicked'; }
         catch (e) { return [t, e.code]; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_caps("browser.open.artifact");
    let first = {
        let (env, run) = (env.clone(), run.clone());
        tokio::spawn(async move {
            let deps = env.deps();
            let d = DriverId::mint();
            env.drive(&deps, &run, &d).await
        })
    };
    let log = loop {
        if let Some(l) = env.browser.logs().first() {
            break l.clone();
        }
        tokio::task::yield_now().await;
    };
    within("the click waits", log.wait_sent("DOM.getDocument", 3)).await;
    first.abort();
    let _ = first.await;
    assert_eq!(browser_rows(&env).len(), 2, "the open and the title");
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(
        script_result(&env, &run, 0)["value"],
        json!(["Hello", "browser_session_lost"])
    );
    assert_eq!(env.browser.opened(), 1, "the replay opened no browser");
    assert_eq!(browser_rows(&env).len(), 2);
}

/// An `expect` that waited 3 s live answers, in a step run again, from
/// the journal at once with the same pass, and charges nothing more.
#[tokio::test]
async fn a_replayed_expect_answers_at_once() {
    let env = Env::new().await;
    let clock = env.clock.clone();
    opens(&env, move || {
        let clock = clock.clone();
        let later = clax_bot::browser::fake::Dom::states(vec![
            vec![button("Book", 0.0)],
            vec![button("Book", 0.0)],
            vec![button("Book", 0.0)],
            vec![
                button("Book", 0.0),
                clax_bot::browser::fake::el("p")
                    .text("Booked")
                    .rect(0.0, 100.0, 100.0, 20.0),
            ],
        ]);
        // Each wait in the page takes a second.
        PageModel::new().dom(later).on(move |cmd: &Value| {
            let e = cmd["params"]["expression"].as_str().unwrap_or_default();
            if cmd["method"] == "Runtime.evaluate" && e.contains("clax:frame") {
                clock.advance_millis(1_000);
            }
            None
        })
    });
    env.api.push(env.script_turn(
        "tu_1",
        "Step",
        "const page = await clax.browser.open({ artifact: 'draft' });
         await expect(page.getByText('Booked')).toBeVisible();
         return 'seen';",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_caps("browser.open.artifact");
    let mut a = env.deps();
    crash_once(
        &mut a,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    env.drive(&a, &run, &DriverId::mint()).await;
    let rows = browser_rows(&env);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[1] >= 3_000, "the expect waited 3 s: {rows:?}");
    let sent = env.browser.logs()[0].commands().len();
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(script_result(&env, &run, 0)["value"], "seen");
    assert_eq!(browser_rows(&env), rows, "nothing more charged");
    assert_eq!(
        env.browser.logs()[0].commands().len(),
        sent,
        "nothing more sent"
    );
    assert_eq!(env.browser.opened(), 1);
}

/// A session no call reached for its idle time ends `idle`, its tail
/// charged once; a later call answers `browser_session_closed`.
#[tokio::test]
async fn an_idle_session_answers_closed_idle() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let run = env.run_with_caps("browser.open.artifact");
    let deps = env.deps();
    let d = DriverId::mint();
    next_script(
        &env,
        &deps,
        &run,
        &d,
        "tu_1",
        "state.page = await clax.browser.open({ artifact: 'draft' }); return 1;",
    )
    .await;
    let session = only_session(&env, &run);
    env.clock.advance(301);
    env.browser.fire_until(env.clock_now());
    let log = env.browser.logs()[0].clone();
    within("the idle end", log.wait_sent("Browser.close", 1)).await;
    let r = next_script(
        &env,
        &deps,
        &run,
        &d,
        "tu_2",
        "try { await state.page.title(); return 'answered'; }
         catch (e) { return [e.name, e.code, e.message]; }",
    )
    .await;
    assert_eq!(r["value"][0], "HostError");
    assert_eq!(r["value"][1], "browser_session_closed");
    assert!(r["value"][2].as_str().unwrap().contains("idle"), "{r}");
    let row = session_row(&env, &run, &session);
    assert_eq!(row.reason, Some(clax_store::bot_web::SessionEnd::Idle));
    assert_eq!(browser_rows(&env).iter().sum::<i64>(), row.charged_ms);
    assert_eq!(browser_rows(&env).len(), 2, "the open, then the tail");
}

/// A locator matching two buttons fails `click` at once with Playwright's
/// strict-mode message, as an answer journaled and charged, thrown by
/// the prelude as an `Error`; the session stays open.
#[tokio::test]
async fn two_matches_are_a_strict_mode_violation() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let (run, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "const page = await clax.browser.open({ artifact: 'draft' });
         let caught;
         try { await page.getByRole('button', { name: 'Save' }).click(); }
         catch (e) { caught = [e.name, e.message]; }
         return [caught, await page.title()];",
    )
    .await;
    assert_eq!(v[0][0], "Error");
    let msg = v[0][1].as_str().unwrap();
    assert!(msg.contains("strict mode violation"), "{msg}");
    assert!(msg.contains("resolved to 2 elements"), "{msg}");
    assert_eq!(v[1], "Hello");
    let calls = script_calls(&env, &run, 0);
    let click = calls.iter().find(|c| c["op"] == "browser.act").unwrap();
    assert_eq!(click["ok"], true, "an answer: {click}");
    assert_eq!(
        browser_rows(&env).len(),
        4,
        "the open, the click, the title, the tail the run's end charged"
    );
}

/// A failure on the page that ran its time is journaled and thrown as a
/// `TimeoutError`, charged its time.
#[tokio::test]
async fn a_failed_click_is_journaled_and_thrown() {
    let env = Arc::new(Env::new().await);
    opens(&env, || page_of(buttons()).frame_limit(2));
    env.api.push(env.script_turn(
        "tu_1",
        "Step",
        "const page = await clax.browser.open({ artifact: 'draft' });
         try { await page.getByText('Never').click({ timeout: 2000 }); return 'clicked'; }
         catch (e) { return [e.name, e.message.split('\\n')[0]]; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_caps("browser.open.artifact");
    let task = {
        let (env, run) = (env.clone(), run.clone());
        tokio::spawn(async move { env.drive(&env.deps(), &run, &DriverId::mint()).await })
    };
    let log = loop {
        if let Some(l) = env.browser.logs().first() {
            break l.clone();
        }
        tokio::task::yield_now().await;
    };
    within("the click waits", log.wait_sent("DOM.getDocument", 3)).await;
    env.clock.advance(2);
    env.browser.fire_until(env.clock_now());
    within("the run ends", task).await.unwrap();
    assert_eq!(
        script_result(&env, &run, 0)["value"],
        json!(["TimeoutError", "locator.click: Timeout 2000ms exceeded."])
    );
    let calls = script_calls(&env, &run, 0);
    let click = calls.iter().find(|c| c["op"] == "browser.act").unwrap();
    assert_eq!(click["ok"], true, "{click}");
    // The open, the click, and the tail the run's end charged.
    let rows = browser_rows(&env);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows[1] >= 2_000, "{rows:?}");
}

/// Supplied content opens with neither web capability nor
/// `browser.open.url`, and its page reaches nothing else.
#[tokio::test]
async fn content_opens_without_any_web_capability() {
    let env = Env::new().await;
    opens(&env, || {
        page_of(buttons()).requests(&[("https://evil.example/x.js", "Script")])
    });
    let (_, v) = one_script_with(
        &env,
        "browser.open.content",
        "const page = await clax.browser.open({ html: '<p>hi</p><script src=\"https://evil.example/x.js\"></script>' });
         const r = await page.failedRequests();
         await page.close();
         return [page.url(), r.blocked];",
    )
    .await;
    let url = v[0].as_str().unwrap();
    assert!(url.ends_with(".clax-content.invalid/"), "{v}");
    assert_eq!(
        v[1],
        json!([{"url": "https://evil.example/x.js", "why": "origin"}])
    );
    let served = served(&env.browser.logs()[0]);
    assert!(served[0].contains("<p>hi</p>"), "{served:?}");
}

/// An open of a kind the run does not hold is `browser_disabled`.
#[tokio::test]
async fn an_open_of_a_kind_the_run_lacks_is_disabled() {
    let env = Env::new().await;
    let (_, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "const codes = [];
         for (const s of [{ url: 'https://public.example/' }, { html: '<p>x</p>' }]) {
           try { await clax.browser.open(s); codes.push('opened'); }
           catch (e) { codes.push([e.code, e.capability]); }
         }
         return codes;",
    )
    .await;
    assert_eq!(
        v,
        json!([
            ["browser_disabled", "browser.open.url"],
            ["browser_disabled", "browser.open.content"]
        ])
    );
    assert_eq!(env.browser.opened(), 0);
}

/// A browser capability granted after the run was made is not added to
/// it (spec §4.3, §4.4): with `browser.open.url` live for the person and
/// not declared by the run, an open on a URL and a `goto` out of an
/// artifact session are both `browser_disabled`.
#[tokio::test]
async fn a_kind_granted_mid_run_is_not_added_to_it() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    browse_rule(&env, "any_public");
    env.admin_sets(&[(Setting::BrowserOpenUrlEnabled, json!(true))]);
    let (_, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "const codes = [];
         try { await clax.browser.open({ url: 'https://public.example/' }); codes.push('opened'); }
         catch (e) { codes.push(e.code); }
         const p = await clax.browser.open({ artifact: 'draft' });
         try { await p.goto('https://public.example/'); codes.push('went'); }
         catch (e) { codes.push(e.code); }
         await p.close();
         return codes;",
    )
    .await;
    assert_eq!(v, json!(["browser_disabled", "browser_disabled"]));
    assert_eq!(env.browser.opened(), 1);
}

/// Each `goto` of a URL session passes `browser.open.url`'s URL rule, as
/// its open did: a URL the sender did not write is refused with the
/// rule's code, and the page does not go there.
#[tokio::test]
async fn a_goto_in_a_url_session_passes_the_url_rule() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    sender_says(&env, "See https://a.example/one");
    let (_, v) = one_script_with(
        &env,
        "browser.open.url",
        "const p = await clax.browser.open({ url: 'https://a.example/one' });
         const codes = [];
         for (const u of ['https://a.example/one', 'https://b.example/two']) {
           try { await p.goto(u); codes.push('went'); } catch (e) { codes.push(e.code); }
         }
         await p.close();
         return codes;",
    )
    .await;
    assert_eq!(v, json!(["went", "url_not_in_prior_context"]));
    let log = &env.browser.logs()[0];
    let navigated: Vec<Value> = log
        .sent("Page.navigate")
        .iter()
        .map(|c| c["params"]["url"].clone())
        .collect();
    assert!(
        !navigated.contains(&json!("https://b.example/two")),
        "{navigated:?}"
    );
}

/// A run holds at most `browser.max_open_per_run` sessions at once (1 by
/// default): another open answers `browser_cap` until one is closed.
#[tokio::test]
async fn a_second_open_past_the_runs_limit_is_capped() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let (_, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "const a = await clax.browser.open({ artifact: 'draft' });
         let capped;
         try { await clax.browser.open({ artifact: 'draft' }); } catch (e) { capped = e.code; }
         await a.close();
         const b = await clax.browser.open({ artifact: 'draft' });
         await b.close();
         return capped;",
    )
    .await;
    assert_eq!(v, "browser_cap");
    assert_eq!(env.browser.opened(), 2);
}

/// A call refused `credits_exhausted` closes its session, `credits`.
#[tokio::test]
async fn credits_running_out_close_the_session() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let run = env.run_with_caps("browser.open.artifact");
    let deps = env.deps();
    let d = DriverId::mint();
    next_script(
        &env,
        &deps,
        &run,
        &d,
        "tu_1",
        "state.page = await clax.browser.open({ artifact: 'draft' }); return 1;",
    )
    .await;
    let session = only_session(&env, &run);
    // A browser call now costs more than the person has left.
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
    let r = next_script(
        &env,
        &deps,
        &run,
        &d,
        "tu_2",
        "const page = state.page;
         const codes = [];
         for (let i = 0; i < 2; i++) {
           try { await page.title(); codes.push('ok'); } catch (e) { codes.push(e.code); }
         }
         return codes;",
    )
    .await;
    assert_eq!(
        r["value"],
        json!(["credits_exhausted", "browser_session_closed"])
    );
    let row = session_row(&env, &run, &session);
    assert_eq!(row.reason, Some(clax_store::bot_web::SessionEnd::Credits));
}

/// A run's end closes the sessions it left open, `run_ended`, and
/// charges their tails.
#[tokio::test]
async fn the_run_end_closes_its_sessions() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let (run, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "state.page = await clax.browser.open({ artifact: 'draft' }); return state.page;",
    )
    .await;
    assert_eq!(v, "<browser page>");
    let session = only_session(&env, &run);
    let row = session_row(&env, &run, &session);
    assert_eq!(row.reason, Some(clax_store::bot_web::SessionEnd::RunEnded));
    assert_eq!(env.browser.logs()[0].sent("Browser.close").len(), 1);
    assert_eq!(browser_rows(&env).iter().sum::<i64>(), row.charged_ms);
}

/// An option a method does not take is `invalid_args`, naming it, before
/// any browser is asked.
#[tokio::test]
async fn an_unknown_action_is_invalid_args() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let (_, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "const page = await clax.browser.open({ artifact: 'draft' });
         const codes = [];
         try { await page.getByRole('button', { name: 'Book' }).click({ force: true }); }
         catch (e) { codes.push([e.code, e.message]); }
         try { await page.goto(); } catch (e) { codes.push([e.code, e.message]); }
         await page.close();
         return codes;",
    )
    .await;
    assert_eq!(v[0][0], "invalid_args");
    assert!(v[0][1].as_str().unwrap().contains("args.force"), "{v}");
    assert_eq!(v[1][0], "invalid_args");
    assert!(v[1][1].as_str().unwrap().contains("args.url"), "{v}");
}

/// Two calls a script starts together are performed one after the other,
/// in the order it made them, each charged its own segment.
#[tokio::test]
async fn two_calls_in_one_promise_all_run_in_order() {
    let env = Env::new().await;
    opens(&env, || page_of(buttons()));
    let (run, v) = one_script_with(
        &env,
        "browser.open.artifact",
        "const page = await clax.browser.open({ artifact: 'draft' });
         const [t, n] = await Promise.all([page.title(), page.getByRole('button').count()]);
         await page.close();
         return [t, n];",
    )
    .await;
    assert_eq!(v, json!(["Hello", 3]));
    let calls = script_calls(&env, &run, 0);
    let ops: Vec<&str> = calls.iter().map(|c| c["op"].as_str().unwrap()).collect();
    assert_eq!(
        ops,
        [
            "browser.open",
            "browser.page",
            "browser.act",
            "browser.close"
        ]
    );
    let rows = browser_rows(&env);
    assert_eq!(rows.len(), 4, "{rows:?}");
}

/// A recording keeps each frame Chrome sent as an item of the run's
/// content and the clip as a manifest naming them, and answers four
/// evenly spaced frames to look at.
#[tokio::test]
async fn a_recording_keeps_its_frames_and_a_manifest() {
    let env = Env::new().await;
    let clock = env.clock.clone();
    opens(&env, move || {
        let clock = clock.clone();
        let mut n = 0;
        page_of(buttons()).on(move |cmd: &Value| {
            let method = cmd["method"].as_str().unwrap_or_default();
            if method != "Page.startScreencast" && method != "Page.screencastFrameAck" {
                return None;
            }
            if method == "Page.screencastFrameAck" {
                clock.advance_millis(200);
            }
            n += 1;
            let data = base64::engine::general_purpose::STANDARD.encode([0xff, 0xd8, 0xff, n]);
            Some(vec![
                clax_bot::browser::fake::answer(cmd, json!({})),
                clax_bot::browser::fake::event(
                    "Page.screencastFrame",
                    json!({"data": data, "sessionId": n, "metadata": {}}),
                    Some("S"),
                ),
            ])
        })
    });
    let (run, v) = one_script_with(
        &env,
        "browser.open.content",
        "const page = await clax.browser.open({ html: '<p>moving</p>' });
         const clip = await page.record({ seconds: 2 });
         let refused;
         try { await page.record({ seconds: 11 }); } catch (e) { refused = e.code; }
         await page.close();
         return { clip, refused };",
    )
    .await;
    let clip = &v["clip"];
    assert_eq!(v["refused"], "invalid_args");
    assert_eq!(clip["count"], 10, "{clip}");
    assert_eq!(clip["ms"], 2000);
    assert_eq!(clip["frames"].as_array().unwrap().len(), 4);
    let row = env
        .store
        .bot_content_of(&run, clip["clip"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(row.media_type, "application/vnd.clax.frames+json");
    let bytes = env.store.bot_content_bytes(&row).unwrap().unwrap();
    let m: Value = serde_json::from_slice(&bytes).unwrap();
    let frames = m["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 10);
    assert_eq!(frames[1]["at_ms"], 200);
    assert_eq!(frames[0]["item"], clip["frames"][0]["id"]);
    assert_eq!(frames[9]["item"], clip["frames"][3]["id"]);
    // The run panel's summary of the call: its session, kind, flat
    // operation and action, how it went, its time and its clip.
    let calls = script_calls(&env, &run, 0);
    let b = &calls[1]["browser"];
    assert_eq!(
        (&b["kind"], &b["op"], &b["action"], &b["ok"], &b["clip"]),
        (
            &json!("content"),
            &json!("page"),
            &json!("record"),
            &json!(true),
            &clip["clip"]
        ),
        "{b}"
    );
    assert_eq!(b["session"], calls[0]["browser"]["session"]);
    assert!(b["ms"].as_i64().unwrap() >= 2000, "{b}");
}
